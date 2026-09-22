use super::*;
mod capture;
mod maintenance;
mod namespace;
mod publication;
mod restore;
mod refresh;
use layrs_core::{
    projection::{EntryState, PathSemantics, ProjectionEntry},
    scm::{FileId, ObjectId, RepoPath, StepId, TreeEntryTarget, TreeId},
};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "layrs-projection-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.0.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
fn identity() -> ProjectionIdentity {
    ProjectionIdentity {
        store_id: StoreId::parse("store:partial").unwrap(),
        worktree_id: WorktreeId::parse("worktree:partial").unwrap(),
        space_id: SpaceId::parse("space:partial").unwrap(),
        origin: "https://example.test".into(),
        principal_id: "principal:alice".into(),
    }
}
fn snapshot() -> ProjectionSnapshot {
    let id = format!("blake3:{}", "1".repeat(64));
    ProjectionSnapshot {
        schema_version: "layrs.projection-snapshot.v1".into(),
        base_step_id: StepId::parse(&id).unwrap(),
        base_root_tree_id: TreeId::parse(&id).unwrap(),
        observed_policy_set_id: ObjectId::parse(&id).unwrap(),
        observed_authorization_epoch: 7,
        path_semantics: PathSemantics::PortableWindows,
        entries: vec![ProjectionEntry {
            path: RepoPath::parse("private.txt").unwrap(),
            target: Some(TreeEntryTarget::file(FileId::parse(id).unwrap())),
            executable: false,
            writable: false,
            state: EntryState::Reserved,
        }],
    }
}
fn hydration() -> (ProjectionSnapshot, ProjectionSnapshot) {
    let mut before = snapshot();
    for (path, bytes) in [
        ("a.txt", b"alpha".as_slice()),
        ("nested/b.txt", b"beta".as_slice()),
    ] {
        before.entries.push(ProjectionEntry {
            path: RepoPath::parse(path).unwrap(),
            target: Some(TreeEntryTarget::file(FileId::from_bytes(bytes))),
            executable: false,
            writable: true,
            state: EntryState::Unavailable,
        });
    }
    before.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut after = before.clone();
    for e in &mut after.entries {
        if e.state == EntryState::Unavailable {
            e.state = EntryState::Materialized;
        }
    }
    (before, after)
}
fn provide(path: &str, out: &mut dyn io::Write) -> io::Result<()> {
    match path {
        "a.txt" => out.write_all(b"alpha"),
        "nested/b.txt" => out.write_all(b"beta"),
        _ => panic!("reserved content requested"),
    }
}
struct StopInstall(InstallPoint);
impl InstallFaults for StopInstall {
    fn check(&self, p: InstallPoint) -> io::Result<()> {
        if p == self.0 {
            Err(io::Error::other("installation fault"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn hydration_recovery_is_exact_across_all_durable_boundaries() {
    for point in [
        InstallPoint::Staged,
        InstallPoint::JournalDurable,
        InstallPoint::PathInstalled,
        InstallPoint::BeforeCommit,
        InstallPoint::AfterCommit,
    ] {
        let f = Fixture::new();
        fs::write(f.0.join("private.txt"), b"private external edits").unwrap();
        let (before, after) = hydration();
        let store = ProjectionStore::create(&f.0, identity(), before).unwrap();
        assert!(
            store
                .install_projection_with_faults(0, &after, &mut provide, &StopInstall(point))
                .is_err()
        );
        match point {
            InstallPoint::Staged => {
                assert_eq!(store.state().unwrap().generation, 0);
                store.install_projection(0, &after, &mut provide).unwrap();
            }
            InstallPoint::AfterCommit => {
                assert_eq!(store.state().unwrap().generation, 1);
                store
                    .install_projection(0, &after, &mut |_, _| panic!("replay must not fetch"))
                    .unwrap();
            }
            _ => {
                assert!(store.state().is_err());
                assert!(ProjectionStore::open(&f.0).is_err());
                ProjectionStore::resume_installation(&f.0).unwrap();
            }
        }
        let reopened = ProjectionStore::open(&f.0).unwrap();
        assert_eq!(reopened.state().unwrap().snapshot, after);
        assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(f.0.join("nested/b.txt")).unwrap(), b"beta");
        assert_eq!(
            fs::read(f.0.join("private.txt")).unwrap(),
            b"private external edits"
        );
    }
}
#[test]
fn hydration_rejects_wrong_bytes_collisions_reserved_targets_and_unjournaled_ready() {
    let f = Fixture::new();
    let (before, after) = hydration();
    let store = ProjectionStore::create(&f.0, identity(), before).unwrap();
    assert!(store.compare_and_swap(0, &after).is_err());
    assert!(
        store
            .install_projection(0, &after, &mut |_, out| out.write_all(b"corrupt"))
            .is_err()
    );
    assert!(!f.0.join("a.txt").exists());
    assert_eq!(store.state().unwrap().generation, 0);
    fs::write(f.0.join("a.txt"), b"external work").unwrap();
    assert!(
        store
            .install_projection(0, &after, &mut |_, _| panic!(
                "collision must preflight before reading content"
            ))
            .is_err()
    );
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"external work");
    let g = Fixture::new();
    assert!(ProjectionStore::create(&g.0, identity(), after.clone()).is_err());
    assert!(!g.0.join(".layrs").exists());
    let mut forged = after.clone();
    forged
        .entries
        .iter_mut()
        .find(|e| e.path.as_str() == "private.txt")
        .unwrap()
        .state = EntryState::Materialized;
    assert!(
        store
            .install_projection(0, &forged, &mut |_, _| panic!(
                "reserved target must be rejected"
            ))
            .is_err()
    );
}
#[test]
fn recovery_refuses_an_external_replacement_even_with_identical_bytes() {
    let f = Fixture::new();
    let (before, after) = hydration();
    let store = ProjectionStore::create(&f.0, identity(), before).unwrap();
    assert!(
        store
            .install_projection_with_faults(
                0,
                &after,
                &mut provide,
                &StopInstall(InstallPoint::PathInstalled)
            )
            .is_err()
    );
    fs::rename(f.0.join("a.txt"), f.0.join("original-installed.txt")).unwrap();
    fs::write(f.0.join("a.txt"), b"alpha").unwrap();
    assert!(ProjectionStore::resume_installation(&f.0).is_err());
    assert!(store.state().is_err());
    assert_eq!(fs::read(f.0.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(
        fs::read(f.0.join("original-installed.txt")).unwrap(),
        b"alpha"
    );
}
#[test]
fn partial_format_isolated_and_generation_cas_preserves_physical_reserved_files() {
    let f = Fixture::new();
    fs::write(f.0.join("private.txt"), b"externally edited private bytes").unwrap();
    let store = ProjectionStore::create(&f.0, identity(), snapshot()).unwrap();
    assert!(matches!(
        crate::scm_v1::inspect(f.0.join(".layrs")).unwrap(),
        crate::scm_v1::NativeStoreInspection::FutureFormat {
            found_version: 3,
            ..
        }
    ));
    assert!(crate::scm_v1::open(f.0.join(".layrs")).is_err());
    let original = store.state().unwrap();
    assert_eq!(original.generation, 0);
    let mut next = original.snapshot.clone();
    next.observed_authorization_epoch += 1;
    let applied = store.compare_and_swap(0, &next).unwrap();
    assert_eq!(applied.generation, 1);
    assert!(matches!(
        store.compare_and_swap(0, &next),
        Err(ProjectionStoreError::Stale {
            expected: 0,
            actual: 1
        })
    ));
    drop(store);
    assert_eq!(
        ProjectionStore::open(&f.0).unwrap().state().unwrap(),
        applied
    );
    assert_eq!(
        fs::read(f.0.join("private.txt")).unwrap(),
        b"externally edited private bytes"
    );
    assert!(!f.0.join(".layrs/metadata/layrs.sqlite3").exists());
}
#[test]
fn unknown_state_absence_aliases_and_epoch_rollback_fail_before_metadata_changes() {
    let f = Fixture::new();
    let store = ProjectionStore::create(&f.0, identity(), snapshot()).unwrap();
    let before = store.state().unwrap();
    let mut bad = snapshot();
    bad.entries[0].state = EntryState::Absent;
    assert!(store.compare_and_swap(0, &bad).is_err());
    bad = snapshot();
    bad.entries[0].writable = true;
    assert!(store.compare_and_swap(0, &bad).is_err());
    bad = snapshot();
    bad.observed_authorization_epoch = 6;
    assert!(store.compare_and_swap(0, &bad).is_err());
    bad = snapshot();
    bad.entries[0].path = RepoPath::parse("private.txt.").unwrap();
    assert!(store.compare_and_swap(0, &bad).is_err());
    assert_eq!(store.state().unwrap(), before);
    let mut doc = serde_json::to_value(snapshot()).unwrap();
    doc["entries"][0]["state"] = "complete".into();
    assert!(serde_json::from_value::<ProjectionSnapshot>(doc).is_err());
}
struct Stop(CreationPoint);
impl CreationFaults for Stop {
    fn check(&self, p: CreationPoint) -> io::Result<()> {
        if p == self.0 {
            Err(io::Error::other("injected bootstrap interruption"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn durable_bootstrap_interruption_requires_explicit_exact_resume() {
    for point in [CreationPoint::MarkerDurable, CreationPoint::DatabaseDurable] {
        let f = Fixture::new();
        assert!(
            ProjectionStore::create_with_faults(&f.0, identity(), snapshot(), &Stop(point))
                .is_err()
        );
        assert!(crate::scm_v1::open(f.0.join(".layrs")).is_err());
        let mut other = identity();
        other.principal_id = "principal:bob".into();
        assert!(ProjectionStore::resume_create(&f.0, &other, &snapshot()).is_err());
        let resumed = ProjectionStore::resume_create(&f.0, &identity(), &snapshot()).unwrap();
        assert_eq!(resumed.state().unwrap().generation, 0);
        assert!(ProjectionStore::create(&f.0, identity(), snapshot()).is_err());
    }
}
#[test]
fn replaced_root_schema_and_snapshot_corruption_are_not_accepted() {
    let f = Fixture::new();
    let store = ProjectionStore::create(&f.0, identity(), snapshot()).unwrap();
    let db = rusqlite::Connection::open(f.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute(
        "UPDATE projection_snapshots SET payload=?1 WHERE generation=0",
        [b"{}".as_slice()],
    )
    .unwrap();
    drop(db);
    assert!(store.state().is_err());
    assert!(ProjectionStore::open(&f.0).is_err());
    let g = Fixture::new();
    let other = ProjectionStore::create(&g.0, identity(), snapshot()).unwrap();
    let db = rusqlite::Connection::open(g.0.join(".layrs/metadata/projection.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE injected(value TEXT)")
        .unwrap();
    drop(db);
    assert!(other.state().is_err());
}
#[test]
fn real_store_lock_refuses_other_writers_without_mutation() {
    let f = Fixture::new();
    let store = ProjectionStore::create(&f.0, identity(), snapshot()).unwrap();
    let before = store.state().unwrap();
    let guard = crate::lock::StoreLock::acquire_existing(
        &f.0.join(".layrs/metadata/layrs.lock"),
        SessionMode::Shared,
    )
    .unwrap();
    assert!(store.compare_and_swap(0, &snapshot()).is_err());
    drop(guard);
    assert_eq!(store.state().unwrap(), before);
}
#[cfg(unix)]
#[test]
fn symlink_database_is_refused_without_following_external_file() {
    let f = Fixture::new();
    let store = ProjectionStore::create(&f.0, identity(), snapshot()).unwrap();
    let db = f.0.join(".layrs/metadata/projection.sqlite3");
    let parked = f.0.join("original.sqlite3");
    fs::rename(&db, &parked).unwrap();
    std::os::unix::fs::symlink(&parked, &db).unwrap();
    let before = fs::read(&parked).unwrap();
    assert!(store.state().is_err());
    assert_eq!(fs::read(&parked).unwrap(), before);
}
