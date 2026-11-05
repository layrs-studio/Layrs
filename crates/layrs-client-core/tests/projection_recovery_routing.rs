//! Native discovery used by a fresh Desktop session must preserve recovery state.
#[path = "projection_recovery_routing/binding.rs"]
mod binding;
#[path = "projection_recovery_routing/recovery.rs"]
mod recovery;

use layrs_client_core::native_scm::{
    checkout_kind, execute_projection, execute_projection_offline, ProjectionAction,
    ProjectionOfflineAction,
};
use layrs_core::{
    projection::{
        EntryState, PathSemantics, ProjectionEntry, ProjectionSelection, ProjectionSnapshot,
    },
    scm::{
        FileId, ObjectId, RepoPath, SpaceId, StepId, StoreId, TreeEntryTarget, TreeId, WorktreeId,
    },
};
use layrs_store_local::projection::{
    InstallFaults, InstallPoint, PartialStoreFamily, PartialStoreIdentity, ProjectionIdentity,
    ProjectionStore, RecoveryBinding, SharedQuota,
};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("layrs-routing-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir(&path).unwrap();
        path
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
        store_id: StoreId::parse("store:routing").unwrap(),
        worktree_id: WorktreeId::parse("worktree:routing").unwrap(),
        space_id: SpaceId::parse("space:routing").unwrap(),
        origin: "https://example.test".into(),
        principal_id: "principal:routing".into(),
    }
}
fn snapshot() -> ProjectionSnapshot {
    let id = format!("blake3:{}", "1".repeat(64));
    ProjectionSnapshot {
        schema_version: "layrs.projection-snapshot.v1".into(),
        base_step_id: StepId::parse(&id).unwrap(),
        base_root_tree_id: TreeId::parse(&id).unwrap(),
        observed_policy_set_id: ObjectId::parse(&id).unwrap(),
        observed_authorization_epoch: 1,
        path_semantics: PathSemantics::PortableWindows,
        entries: vec![ProjectionEntry {
            path: RepoPath::parse("a.txt").unwrap(),
            target: Some(TreeEntryTarget::file(FileId::from_bytes(b"alpha"))),
            executable: false,
            writable: true,
            state: EntryState::Unavailable,
        }],
    }
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                walk(&path, result);
            } else {
                result.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(root, &mut result);
    result
}
fn binding(reply: &serde_json::Value) -> RecoveryBinding {
    let binding: RecoveryBinding = serde_json::from_value(reply["binding"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&binding.identity).unwrap(),
        reply["identity"]
    );
    binding
}
struct StopAfterJournal;
impl InstallFaults for StopAfterJournal {
    fn check(&self, point: InstallPoint) -> io::Result<()> {
        if point == InstallPoint::JournalDurable {
            Err(io::Error::other(
                "injected durable installation interruption",
            ))
        } else {
            Ok(())
        }
    }
}

#[test]
fn interrupted_shared_checkout_is_classified_without_replay_then_explicitly_resumed() {
    let f = Fixture::new();
    let root = f.directory("checkout");
    let family_root = f.directory("family");
    let id = identity();
    let family = PartialStoreFamily::create(
        family_root,
        PartialStoreIdentity {
            store_id: id.store_id,
            space_id: id.space_id,
            origin: id.origin,
            principal_id: id.principal_id,
        },
        SharedQuota {
            bytes: 32 * 1024 * 1024,
        },
    )
    .unwrap();
    let store = family
        .create_worktree(&root, id.worktree_id, snapshot(), ProjectionSelection::All)
        .unwrap();
    let mut target = snapshot();
    target.entries[0].state = EntryState::Materialized;
    let error = store
        .install_projection_with_faults(
            0,
            &target,
            &mut |_, out| out.write_all(b"alpha"),
            &StopAfterJournal,
        )
        .unwrap_err();
    assert!(error.to_string().contains("injected durable"), "{error}");
    drop(store);
    assert!(ProjectionStore::open(&root).is_err());
    let before = files(&f.0);
    assert_eq!(checkout_kind(&root).unwrap(), "projection");
    let info =
        execute_projection_offline(&root, ProjectionOfflineAction::InspectConversionSource {})
            .unwrap();
    assert_eq!(info["shared"], true);
    assert_eq!(info["conversion"]["allowed"], false);
    assert_eq!(
        files(&f.0),
        before,
        "inspection must not settle or replay journals"
    );
    assert!(!root.join("a.txt").exists());
    execute_projection(
        &root,
        ProjectionAction::ResumeInstallation {
            expected: binding(&info),
        },
    )
    .unwrap();
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(
        ProjectionStore::open(&root)
            .unwrap()
            .state()
            .unwrap()
            .generation,
        1
    );
}

#[test]
fn conversion_inspection_refuses_legacy_schema_without_migrating_source_bytes() {
    let f = Fixture::new();
    let root = f.directory("legacy");
    drop(ProjectionStore::create(&root, identity(), snapshot()).unwrap());
    let inspected =
        execute_projection_offline(&root, ProjectionOfflineAction::InspectRecovery {}).unwrap();
    let db = root.join(".layrs/metadata/projection.sqlite3");
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute_batch("DROP TABLE projection_selections; PRAGMA user_version=5;")
        .unwrap();
    drop(connection);
    let before = files(&root);
    assert_eq!(checkout_kind(&root).unwrap(), "projection");
    assert!(
        execute_projection_offline(&root, ProjectionOfflineAction::InspectConversionSource {})
            .is_err()
    );
    assert!(execute_projection(&root, ProjectionAction::Fsck).is_err());
    binding::assert_all_recovery_refused(&root, &binding(&inspected), &f.0, "schema");
    assert_eq!(files(&root), before);
    let connection =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        5
    );
}

#[test]
fn conversion_inspection_marks_pending_legacy_source_ineligible_without_writes() {
    let f = Fixture::new();
    let root = f.directory("pending-source");
    let store = ProjectionStore::create(&root, identity(), snapshot()).unwrap();
    let mut target = snapshot();
    target.entries[0].state = EntryState::Materialized;
    assert!(store
        .install_projection_with_faults(
            0,
            &target,
            &mut |_, out| out.write_all(b"alpha"),
            &StopAfterJournal,
        )
        .is_err());
    drop(store);
    let before = files(&root);
    let info =
        execute_projection_offline(&root, ProjectionOfflineAction::InspectConversionSource {})
            .unwrap();
    assert_eq!(info["shared"], false);
    assert_eq!(info["conversion"]["allowed"], false);
    assert_eq!(info["conversion"]["reason"], "pending_filesystem_operation");
    assert_eq!(info["recovery"], serde_json::json!(["filesystemOperation"]));
    assert_eq!(files(&root), before);
    assert!(!root.join("a.txt").exists());
    execute_projection(
        &root,
        ProjectionAction::ResumeInstallation {
            expected: binding(&info),
        },
    )
    .unwrap();
    let info =
        execute_projection_offline(&root, ProjectionOfflineAction::InspectConversionSource {})
            .unwrap();
    assert_eq!(info["conversion"]["allowed"], true);
}

#[test]
fn conversion_inspection_validates_source_identity_and_refuses_copied_checkout() {
    let f = Fixture::new();
    let root = f.directory("source");
    drop(ProjectionStore::create(&root, identity(), snapshot()).unwrap());
    let before = files(&root);
    let info =
        execute_projection_offline(&root, ProjectionOfflineAction::InspectConversionSource {})
            .unwrap();
    assert_eq!(info["identity"], serde_json::to_value(identity()).unwrap());
    assert_eq!(info["shared"], false);
    assert_eq!(info["conversion"]["allowed"], true);
    assert!(info["conversion"]["reason"].is_null());
    assert_eq!(files(&root), before);
    let copy = f.directory("copy");
    for (path, bytes) in before {
        let target = copy.join(path.strip_prefix(&root).unwrap());
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
    assert!(
        execute_projection_offline(&copy, ProjectionOfflineAction::InspectConversionSource {})
            .is_err()
    );
}

fn installed_shared(writable: bool) -> (Fixture, ProjectionStore) {
    let f = Fixture::new();
    let root = f.directory("checkout");
    let id = identity();
    let family = PartialStoreFamily::create(
        f.directory("family"),
        PartialStoreIdentity {
            store_id: id.store_id,
            space_id: id.space_id,
            origin: id.origin,
            principal_id: id.principal_id,
        },
        SharedQuota {
            bytes: 32 * 1024 * 1024,
        },
    )
    .unwrap();
    let mut initial = snapshot();
    initial.entries[0].writable = writable;
    let store = family
        .create_worktree(
            &root,
            id.worktree_id,
            initial.clone(),
            ProjectionSelection::All,
        )
        .unwrap();
    initial.entries[0].state = EntryState::Materialized;
    store
        .install_projection(0, &initial, &mut |_, out| out.write_all(b"alpha"))
        .unwrap();
    (f, store)
}

#[test]
fn local_version_restore_rechecks_preview_and_preserves_overwritten_bytes() {
    let (_f, store) = installed_shared(true);
    let root = store.checkout_root();
    let version = store.current_version().unwrap();
    fs::write(root.join("a.txt"), b"first local edit").unwrap();
    let request = || ProjectionOfflineAction::PreviewRestore {
        expected_generation: 1,
        version: version.clone(),
        paths: vec![RepoPath::parse("a.txt").unwrap()],
    };
    let reply = execute_projection_offline(root, request()).unwrap();
    let preview: layrs_store_local::projection::RestorePreview =
        serde_json::from_value(reply["preview"].clone()).unwrap();
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"first local edit");
    fs::write(root.join("a.txt"), b"later local edit").unwrap();
    assert!(
        execute_projection_offline(root, ProjectionOfflineAction::ApplyRestore { preview })
            .is_err()
    );
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"later local edit");
    let reply = execute_projection_offline(root, request()).unwrap();
    let preview: layrs_store_local::projection::RestorePreview =
        serde_json::from_value(reply["preview"].clone()).unwrap();
    let operation = preview.operation_id.clone();
    let restored = execute_projection_offline(
        root,
        ProjectionOfflineAction::ApplyRestore {
            preview: preview.clone(),
        },
    )
    .unwrap();
    assert_eq!(restored["restoration"]["operationId"], operation);
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"alpha");
    // Exact replay returns the same receipt rather than creating another capture.
    assert_eq!(
        execute_projection_offline(root, ProjectionOfflineAction::ApplyRestore { preview })
            .unwrap(),
        restored
    );
    let inverse = store
        .preview_restore(
            1,
            &layrs_store_local::projection::RestoreSource::Inverse {
                operation_id: operation,
            },
        )
        .unwrap();
    store.apply_restore(&inverse).unwrap();
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"later local edit");
    store.fsck().unwrap();
}

#[test]
fn local_version_restore_refuses_stale_generation_permissions_and_forged_preview() {
    let (_f, store) = installed_shared(true);
    let root = store.checkout_root();
    let version = store.current_version().unwrap();
    fs::write(root.join("a.txt"), b"private edit").unwrap();
    assert!(execute_projection_offline(
        root,
        ProjectionOfflineAction::PreviewRestore {
            expected_generation: 0,
            version: version.clone(),
            paths: vec![RepoPath::parse("a.txt").unwrap()],
        }
    )
    .is_err());
    let reply = execute_projection_offline(
        root,
        ProjectionOfflineAction::PreviewRestore {
            expected_generation: 1,
            version,
            paths: vec![RepoPath::parse("a.txt").unwrap()],
        },
    )
    .unwrap();
    let mut preview: layrs_store_local::projection::RestorePreview =
        serde_json::from_value(reply["preview"].clone()).unwrap();
    preview.after.clear();
    assert!(
        execute_projection_offline(root, ProjectionOfflineAction::ApplyRestore { preview })
            .is_err()
    );
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"private edit");
    let (_readonly_f, readonly) = installed_shared(false);
    assert!(execute_projection_offline(
        readonly.checkout_root(),
        ProjectionOfflineAction::PreviewRestore {
            expected_generation: 1,
            version: readonly.current_version().unwrap(),
            paths: vec![RepoPath::parse("a.txt").unwrap()],
        }
    )
    .is_err());
    assert_eq!(
        fs::read(readonly.checkout_root().join("a.txt")).unwrap(),
        b"alpha"
    );
}
