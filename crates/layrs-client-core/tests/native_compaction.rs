use layrs_client_core::native_scm::*;
use layrs_store_local::scm_v1::{self, compaction::*, NativeScmReader};
use std::{fs, path::PathBuf};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
#[test]
fn compaction_retains_historical_targets_operations_views_dirty_state_and_observations() {
    let f = Fixture(
        std::env::temp_dir().join(format!("layrs-native-compaction-{}", uuid::Uuid::new_v4())),
    );
    fs::create_dir(&f.0).unwrap();
    fs::write(f.0.join("file"), b"original").unwrap();
    let w = NativeWorkspace::initialize(&f.0, "Compaction").unwrap();
    let marker = w.create_marker("initial", "").unwrap();
    fs::write(f.0.join("file"), b"release content").unwrap();
    let release = w.create_release("v1", "").unwrap();
    w.hide_marker(marker.id()).unwrap();
    fs::write(f.0.join("file"), b"newest").unwrap();
    w.capture().unwrap();
    fs::write(f.0.join("file"), b"dirty unrecorded").unwrap();
    let state = w.state().unwrap();
    let state_bytes = serde_jcs::to_vec(&state).unwrap();
    let marker_files = w.read_step_files(marker.target_step_id()).unwrap();
    let release_files = w.read_step_files(release.target_step_id()).unwrap();
    // Create legal physical rowid gaps, while all immutable observation IDs and
    // their payloads remain untouched. Maintenance may renumber these rows.
    {
        let guard = scm_v1::open_exclusive(w.store_root()).unwrap();
        let db = rusqlite::Connection::open(guard.layout().sqlite_path()).unwrap();
        db.execute("UPDATE step_observations SET rowid=rowid+10000", [])
            .unwrap();
    }
    let reader = scm_v1::open(w.store_root()).unwrap();
    let before = NativeScmReader::new(&reader)
        .unwrap()
        .observation_projection(state.space.id(), None)
        .unwrap();
    drop(reader);
    let report = compact_metadata(w.store_root()).unwrap();
    assert!(!report.history_deleted);
    let reopened = NativeWorkspace::open(&f.0).unwrap();
    assert_eq!(
        serde_jcs::to_vec(&reopened.state().unwrap()).unwrap(),
        state_bytes
    );
    assert!(reopened.is_dirty().unwrap());
    assert_eq!(fs::read(f.0.join("file")).unwrap(), b"dirty unrecorded");
    assert_eq!(
        reopened.read_step_files(marker.target_step_id()).unwrap(),
        marker_files
    );
    assert_eq!(
        reopened.read_step_files(release.target_step_id()).unwrap(),
        release_files
    );
    assert_eq!(reopened.releases().unwrap()[0].id(), release.id());
    let reader = scm_v1::open(w.store_root()).unwrap();
    let typed = NativeScmReader::new(&reader).unwrap();
    let after = typed
        .observation_projection(state.space.id(), None)
        .unwrap();
    println!(
        "observation watermarks before/after VACUUM: {}/{}; count={}",
        before.snapshot.watermark, after.snapshot.watermark, after.snapshot.count
    );
    assert_eq!(
        serde_jcs::to_vec(&after.by_step).unwrap(),
        serde_jcs::to_vec(&before.by_step).unwrap()
    );
    // A reused rowid cursor either remains exactly valid or explicitly fails;
    // it can never silently return a changed observation set.
    if let Ok(reused) = typed.observation_projection(state.space.id(), Some(&before.snapshot)) {
        assert_eq!(reused.snapshot, before.snapshot);
        assert_eq!(
            serde_jcs::to_vec(&reused.by_step).unwrap(),
            serde_jcs::to_vec(&before.by_step).unwrap()
        );
    }
    let mut invalid = before.snapshot.clone();
    invalid.digest = "0".repeat(64);
    assert!(typed
        .observation_projection(state.space.id(), Some(&invalid))
        .is_err());
    drop(reader);
    // SQLite retained the explicit fixture gaps in this build. Also exercise
    // the existing cursor guard against a physical renumbering, without
    // changing any immutable observation or business identity.
    {
        let guard = scm_v1::open_exclusive(w.store_root()).unwrap();
        let db = rusqlite::Connection::open(guard.layout().sqlite_path()).unwrap();
        db.execute("UPDATE step_observations SET rowid=rowid+20000", [])
            .unwrap();
    }
    {
        let reader = scm_v1::open(w.store_root()).unwrap();
        let typed = NativeScmReader::new(&reader).unwrap();
        assert!(typed
            .observation_projection(state.space.id(), Some(&before.snapshot))
            .is_err());
        assert_eq!(
            serde_jcs::to_vec(
                &typed
                    .observation_projection(state.space.id(), None)
                    .unwrap()
                    .by_step
            )
            .unwrap(),
            serde_jcs::to_vec(&before.by_step).unwrap()
        );
    }
    assert!(
        scm_v1::fsck_existing(w.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn unfinished_recovery_in_store_refuses_compaction_without_changing_metadata() {
    let f = Fixture(std::env::temp_dir().join(format!(
        "layrs-compaction-recovery-{}",
        uuid::Uuid::new_v4()
    )));
    fs::create_dir(&f.0).unwrap();
    let w = NativeWorkspace::initialize(&f.0, "Recovery").unwrap();
    let path = {
        let guard = scm_v1::open_exclusive(w.store_root()).unwrap();
        let path = guard.layout().sqlite_path().to_path_buf();
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("INSERT INTO recovery_operations(recovery_operation_id,idempotency_key,request_digest,worktree_id,recovery_type,state,payload_json,evidence_json,attempt_count,created_at_ms,updated_at_ms) VALUES ('recovery:compaction','compaction-fixture','digest',?1,'materialize','pending','{}','{}',0,0,0)",[w.worktree_id().as_str()]).unwrap();
        path
    };
    let before = fs::read(&path).unwrap();
    let error = compact_metadata(w.store_root()).unwrap_err();
    assert!(error.to_string().contains("recovery"));
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(!w
        .store_root()
        .join("metadata/metadata-compaction.json")
        .exists());
}
