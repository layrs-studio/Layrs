use layrs_client_core::native_scm::NativeWorkspace;
use layrs_store_local::scm_v1::compaction::*;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "layrs-cli-secondary-maintenance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let root = self.0.join(name);
        fs::create_dir(&root).unwrap();
        root
    }
    fn workspaces(&self) -> (NativeWorkspace, NativeWorkspace) {
        let primary_root = self.directory("primary");
        fs::write(primary_root.join("file"), b"base").unwrap();
        let primary = NativeWorkspace::initialize(&primary_root, "Maintenance").unwrap();
        let state = primary.state().unwrap();
        let own = state
            .view
            .worktrees()
            .iter()
            .find(|entry| entry.worktree_id() == primary.worktree_id())
            .unwrap();
        let secondary = primary
            .create_worktree(
                self.directory("secondary"),
                own.active_layer_id().unwrap(),
                own.base_step_id().unwrap(),
            )
            .unwrap();
        (primary, secondary)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn run(root: &Path, args: &[&str], success: bool) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_layrs"))
        .current_dir(root)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{args:?}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], success);
    value["data"].clone()
}
struct Interrupt;
impl CompactionFaults for Interrupt {
    fn check(&self, point: CompactionFaultPoint) -> CompactionResult<()> {
        if point == CompactionFaultPoint::JournalDurable {
            return Err("simulated interruption".into());
        }
        Ok(())
    }
}

struct InterruptAt(CompactionFaultPoint);
impl CompactionFaults for InterruptAt {
    fn check(&self, point: CompactionFaultPoint) -> CompactionResult<()> {
        if point == self.0 {
            Err("simulated interruption".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn secondary_compaction_and_resume_preserve_both_private_views_and_pending_edits() {
    let fixture = Fixture::new();
    let (primary, secondary) = fixture.workspaces();
    let primary_root = primary.checkout_root().unwrap();
    let secondary_root = secondary.checkout_root().unwrap();
    let nested = secondary_root.join("nested");
    fs::create_dir(&nested).unwrap();
    secondary.create_layer("private").unwrap();
    let before_primary = serde_json::to_value(primary.state().unwrap()).unwrap();
    let before_secondary = serde_json::to_value(secondary.state().unwrap()).unwrap();
    let locator = fs::read(secondary_root.join(".layrs")).unwrap();
    fs::write(primary_root.join("file"), b"primary edits").unwrap();
    fs::write(secondary_root.join("file"), b"secondary edits").unwrap();
    let compacted = run(&nested, &["compact"], true);
    assert_eq!(compacted["historyDeleted"], false);
    assert_eq!(compacted["storeId"], before_primary["worktree"]["storeId"]);
    for point in [
        CompactionFaultPoint::JournalDurable,
        CompactionFaultPoint::DatabasePublished,
    ] {
        assert!(compact_metadata_with_faults(primary.store_root(), &InterruptAt(point)).is_err());
        run(&secondary_root, &["status"], false);
        let resumed = run(&nested, &["compact", "--resume"], true);
        assert_eq!(resumed["storeId"], before_primary["worktree"]["storeId"]);
        assert_eq!(
            serde_json::to_value(primary.state().unwrap()).unwrap(),
            before_primary
        );
        assert_eq!(
            serde_json::to_value(secondary.state().unwrap()).unwrap(),
            before_secondary
        );
    }
    assert_eq!(
        fs::read(primary_root.join("file")).unwrap(),
        b"primary edits"
    );
    assert_eq!(
        fs::read(secondary_root.join("file")).unwrap(),
        b"secondary edits"
    );
    assert_eq!(fs::read(secondary_root.join(".layrs")).unwrap(), locator);
    run(&secondary_root, &["doctor"], true);
    run(&secondary_root, &["compact", "--resume"], false);
}

#[test]
fn secondary_maintenance_refuses_unknown_locators_without_touching_the_store() {
    let fixture = Fixture::new();
    let (primary, secondary) = fixture.workspaces();
    let secondary_root = secondary.checkout_root().unwrap();
    let copied = fixture.directory("copied");
    let unknown = fixture.directory("unknown");
    let original = fs::read(secondary_root.join(".layrs")).unwrap();
    fs::write(copied.join(".layrs"), &original).unwrap();
    fs::write(unknown.join(".layrs"), b"not a supported locator").unwrap();
    let metadata_path = primary.store_root().join("metadata/layrs.sqlite3");
    let before = fs::read(&metadata_path).unwrap();
    for root in [&copied, &unknown] {
        for args in [
            &["compact"][..],
            &["compact", "--resume"][..],
            &["recover"][..],
        ] {
            run(root, args, false);
            assert_eq!(fs::read(&metadata_path).unwrap(), before);
        }
        assert_eq!(fs::read_dir(root).unwrap().count(), 1);
    }
    assert_eq!(fs::read(secondary_root.join(".layrs")).unwrap(), original);
    assert_eq!(fs::read(secondary_root.join("file")).unwrap(), b"base");
    assert_eq!(
        fs::read(primary.checkout_root().unwrap().join("file")).unwrap(),
        b"base"
    );
}

#[test]
fn cli_compacts_from_nested_checkout_and_resumes_a_fenced_store_without_losing_history() {
    let fixture = Fixture(std::env::temp_dir().join(format!(
            "layrs-cli-maintenance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    fs::create_dir(&fixture.0).unwrap();
    let nested = fixture.0.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(fixture.0.join("file"), b"initial").unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "Maintenance").unwrap();
    let marker = workspace.create_marker("initial", "").unwrap();
    fs::write(fixture.0.join("file"), b"released").unwrap();
    let release = workspace.create_release("v1", "").unwrap();
    let before = serde_json::to_value(workspace.state().unwrap()).unwrap();
    let marker_files = workspace.read_step_files(marker.target_step_id()).unwrap();
    let release_files = workspace.read_step_files(release.target_step_id()).unwrap();
    fs::write(fixture.0.join("file"), b"unrecorded").unwrap();
    let report = run(&nested, &["compact"], true);
    assert_eq!(report["historyDeleted"], false);
    assert_eq!(report["restartHistoryTraversal"], true);
    assert!(report["retainedRows"].as_u64().unwrap() > 0);
    assert!(
        Path::new(report["backupDirectory"].as_str().unwrap())
            .join("before.sqlite3")
            .is_file()
    );
    assert!(compact_metadata_with_faults(workspace.store_root(), &Interrupt).is_err());
    run(&fixture.0, &["status"], false);
    run(&nested, &["compact", "--resume"], true);
    let reopened = NativeWorkspace::open(&fixture.0).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.state().unwrap()).unwrap(),
        before
    );
    assert_eq!(
        reopened.read_step_files(marker.target_step_id()).unwrap(),
        marker_files
    );
    assert_eq!(
        reopened.read_step_files(release.target_step_id()).unwrap(),
        release_files
    );
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"unrecorded");
    assert!(reopened.is_dirty().unwrap());
    run(&fixture.0, &["doctor"], true);
    run(&fixture.0, &["compact", "--resume"], false);
}
