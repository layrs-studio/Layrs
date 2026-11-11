//! A small materialization holds the exclusive Store from its journal to its
//! metadata commit (council s10, Q1). A process killed in between leaves the
//! applying journal only in the WAL: the next process folds it, sees the
//! pending journal and restores the originals (council s11 r1).
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_core::scm::StepId;
use layrs_store_local::scm_v1::{
    self,
    materialize::{MaterializationFaults, MaterializationPoint},
};
use std::{fs, path::PathBuf, process::Command};

const CHILD: &str = "LAYRS_HELD_CRASH_CHILD";
const POINT: &str = "LAYRS_HELD_CRASH_POINT";
const TARGET: &str = "LAYRS_HELD_CRASH_TARGET";

fn files(root: &std::path::Path, old: bool) {
    for i in 0..12 {
        fs::write(
            root.join(format!("file-{i:02}")),
            if old { b"old bytes" } else { b"new bytes" },
        )
        .unwrap();
    }
}

fn assert_files(root: &std::path::Path, old: bool) {
    for i in 0..12 {
        assert_eq!(
            fs::read(root.join(format!("file-{i:02}"))).unwrap(),
            if old { b"old bytes" } else { b"new bytes" }
        );
    }
}

/// Kills the process at one point of the materialization.
struct Abort(MaterializationPoint);
impl MaterializationFaults for Abort {
    fn check(&self, point: MaterializationPoint) -> std::io::Result<()> {
        if point == self.0 {
            std::process::abort();
        }
        Ok(())
    }
}

fn point(name: &str) -> MaterializationPoint {
    match name {
        "AfterJournal" => MaterializationPoint::AfterJournal,
        "AfterBackup" => MaterializationPoint::AfterBackup,
        "AfterPath" => MaterializationPoint::AfterPath,
        "BeforeMetadata" => MaterializationPoint::BeforeMetadata,
        other => panic!("unknown point {other}"),
    }
}

/// In the child: restore the folder to the marked Step, killed at `POINT`.
fn child(root: PathBuf) {
    let workspace = NativeWorkspace::discover(&root).unwrap();
    let target = StepId::parse(std::env::var(TARGET).unwrap()).unwrap();
    let at = point(&std::env::var(POINT).unwrap());
    let _ = workspace.restore_step_with_faults(&target, None, &Abort(at));
    // Not reached: the fault aborts the process.
    std::process::exit(0);
}

#[test]
fn a_held_session_killed_mid_phase_is_recovered_from_its_wal() {
    if let Ok(root) = std::env::var(CHILD) {
        child(PathBuf::from(root));
        return;
    }
    for at in ["AfterJournal", "AfterBackup", "AfterPath", "BeforeMetadata"] {
        let root =
            std::env::temp_dir().join(format!("layrs-materialize-held-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        files(&root, true);
        let workspace = NativeWorkspace::initialize(&root, "Held fixture").unwrap();
        let target = workspace
            .create_marker("old", "")
            .unwrap()
            .target_step_id()
            .clone();
        files(&root, false);
        workspace.capture().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "a_held_session_killed_mid_phase_is_recovered_from_its_wal",
                "--nocapture",
            ])
            .env(CHILD, &root)
            .env(POINT, at)
            .env(TARGET, target.as_str())
            .status()
            .unwrap();
        assert!(!status.success(), "{at}: the child was not killed");
        // A real crash of a held session (Windows, one NTFS volume): the
        // journal is only in the WAL, never checkpointed. Elsewhere the
        // journal was checkpointed before any path moved.
        #[cfg(windows)]
        let wal = workspace
            .store_root()
            .join("metadata")
            .join("layrs.sqlite3-wal");
        #[cfg(windows)]
        assert!(
            fs::metadata(&wal).map(|m| m.len()).unwrap_or(0) > 0,
            "{at}: the journal is not left in the WAL"
        );
        // A new process: the journal is pending; recovery restores the
        // originals, and the Store opens with an empty WAL.
        assert!(workspace.needs_repair().unwrap(), "{at}");
        assert_eq!(
            scm_v1::materialize::recover_checkout(workspace.store_root(), workspace.worktree_id())
                .unwrap(),
            1,
            "{at}"
        );
        assert!(!workspace.needs_repair().unwrap(), "{at}");
        assert_files(&root, false);
        assert!(!workspace.is_dirty().unwrap(), "{at}");
        // The restore itself now succeeds.
        workspace.restore_step(&target, None).unwrap();
        assert_files(&root, true);
        drop(workspace);
        let _ = fs::remove_dir_all(&root);
    }
}
