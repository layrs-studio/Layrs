use layrs_client_core::native_scm::*;
use layrs_core::scm::{OperationKind, StoreId};
use layrs_store_local::scm_v1;
use std::{fs, path::PathBuf};

struct Checkout(PathBuf);
impl Checkout {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("layrs-workspace-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("hello"), b"hello\0world").unwrap();
        Self(root)
    }
    fn init(&self) -> NativeWorkspace {
        NativeWorkspace::initialize(&self.0, "My project").unwrap()
    }
}
impl Drop for Checkout {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("preserved fixture {}", self.0.display());
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

#[test]
fn ignore_rules_preserve_untracked_bytes_and_keep_tracked_files_visible() {
    let checkout = Checkout::new();
    fs::write(
        checkout.0.join(".layrsignore"),
        "node_modules/\n*.tmp\n!important.tmp\n",
    )
    .unwrap();
    fs::create_dir(checkout.0.join("node_modules")).unwrap();
    fs::write(checkout.0.join("node_modules/big"), b"ignored dependency").unwrap();
    fs::write(checkout.0.join("cache.tmp"), b"ignored cache").unwrap();
    fs::write(checkout.0.join("important.tmp"), b"included exception").unwrap();
    let workspace = checkout.init();
    let baseline = workspace.create_marker("baseline", "").unwrap();
    let files = workspace
        .read_step_files(baseline.target_step_id())
        .unwrap();
    assert_eq!(
        files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        [".layrsignore", "hello", "important.tmp"]
    );
    fs::write(checkout.0.join("cache.tmp"), b"preserve me").unwrap();
    assert!(!workspace.is_dirty().unwrap());
    fs::write(
        checkout.0.join(".layrsignore"),
        "node_modules/\n*.tmp\nhello\n",
    )
    .unwrap();
    fs::write(checkout.0.join("hello"), b"tracked despite ignore").unwrap();
    workspace.capture().unwrap();
    assert!(!workspace.is_dirty().unwrap());
    workspace
        .restore_step(baseline.target_step_id(), Some("hello"))
        .unwrap();
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"hello\0world");
    assert_eq!(
        fs::read(checkout.0.join("cache.tmp")).unwrap(),
        b"preserve me"
    );
    assert!(checkout.0.join("node_modules/big").exists());
}

#[test]
fn repeated_file_content_keeps_each_path_and_is_revalidated_on_later_reads() {
    use layrs_store_local::scm_v1::checkout::CheckoutContent;
    let checkout = Checkout::new();
    fs::write(checkout.0.join("copy"), b"hello\0world").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(checkout.0.join("copy"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let workspace = checkout.init();
    let marker = workspace.create_marker("same-content", "").unwrap();
    let files = workspace.read_step_files(marker.target_step_id()).unwrap();
    assert_eq!(files.len(), 2);
    let projection = workspace
        .read_step_projection(marker.target_step_id())
        .unwrap();
    assert_eq!(
        projection,
        files
            .iter()
            .map(scm_v1::checkout::CheckoutProjectionEntry::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        scm_v1::checkout::projection_tree_id(&projection).unwrap(),
        scm_v1::checkout::entries_tree_id(&files).unwrap()
    );
    for entry in files {
        let CheckoutContent::File { bytes, executable } = entry.content else {
            panic!("expected file");
        };
        assert_eq!(bytes, b"hello\0world");
        #[cfg(unix)]
        assert_eq!(executable, entry.path == "copy");
        #[cfg(not(unix))]
        assert!(!executable);
    }
    fs::write(checkout.0.join("copy"), b"changed").unwrap();
    assert_eq!(workspace.working_diff().unwrap().len(), 1);
    workspace.capture().unwrap();
    workspace
        .restore_step(marker.target_step_id(), None)
        .unwrap();
    assert_eq!(fs::read(checkout.0.join("copy")).unwrap(), b"hello\0world");
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"hello\0world");
    // A later read must not trust a process-global cache of previously valid CAS.
    let shared = layrs_store::scm::StoreObjectId::ChunkRaw(
        layrs_core::scm::Chunk::new(b"hello\0world".to_vec())
            .unwrap()
            .id()
            .clone(),
    );
    let compact = damage_compact_object(workspace.store_root(), &shared).is_some();
    let mut pending = if compact {
        Vec::new()
    } else {
        vec![workspace.store_root().join("objects")]
    };
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else if entry
                .path()
                .components()
                .any(|part| part.as_os_str() == "chunk_raw")
            {
                fs::write(entry.path(), b"tampered").unwrap();
            }
        }
    }
    assert!(workspace.read_step_files(marker.target_step_id()).is_err());
    // Projections carry metadata only; every use that writes bytes re-verifies
    // them, so a restore needing the tampered content refuses before writing.
    // Format 6: this Step's Trees share the damaged block (ADR 0028: one
    // damaged byte fails its whole block), so its projection fails as well.
    let projection = workspace.read_step_projection(marker.target_step_id());
    if !compact {
        projection.unwrap();
    }
    fs::write(checkout.0.join("copy"), b"edited after tamper").unwrap();
    assert!(workspace
        .restore_step(marker.target_step_id(), None)
        .is_err());
    assert_eq!(
        fs::read(checkout.0.join("copy")).unwrap(),
        b"edited after tamper"
    );
}

#[test]
fn restoring_an_ignored_untracked_collision_refuses_before_writing() {
    let checkout = Checkout::new();
    let workspace = checkout.init();
    let original = workspace.create_marker("original", "").unwrap();
    fs::remove_file(checkout.0.join("hello")).unwrap();
    fs::write(checkout.0.join(".layrsignore"), "hello\n").unwrap();
    workspace.capture().unwrap();
    fs::write(checkout.0.join("hello"), b"external ignored bytes").unwrap();
    assert!(!workspace.is_dirty().unwrap());
    let before = workspace.state().unwrap();
    assert!(workspace
        .restore_step(original.target_step_id(), Some("hello"))
        .is_err());
    assert_eq!(
        fs::read(checkout.0.join("hello")).unwrap(),
        b"external ignored bytes"
    );
    assert_eq!(workspace.state().unwrap().operation, before.operation);
    assert!(!workspace.needs_repair().unwrap());
}

#[test]
fn real_workspace_initialize_capture_reopen_and_discover() {
    let checkout = Checkout::new();
    let workspace = checkout.init();
    assert!(!workspace.is_dirty().unwrap());
    fs::write(checkout.0.join("hello"), b"changed").unwrap();
    assert!(workspace.is_dirty().unwrap());
    assert!(matches!(
        workspace.capture().unwrap(),
        CapturePlan::Change(_)
    ));
    assert!(matches!(
        workspace.capture().unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    fs::create_dir(checkout.0.join("nested")).unwrap();
    let reopened = NativeWorkspace::discover(checkout.0.join("nested")).unwrap();
    assert!(!reopened.is_dirty().unwrap());
    let history = reopened.operations(100).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].kind(), OperationKind::CaptureWorktree);
    assert_eq!(history[1].kind(), OperationKind::Initialize);
    assert!(
        scm_v1::fsck_existing(reopened.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn markers_releases_and_layers_are_durable_operations() {
    let checkout = Checkout::new();
    let workspace = checkout.init();
    let marker = workspace
        .create_marker("start", "initial milestone")
        .unwrap();
    workspace.create_layer("experiment").unwrap();
    assert!(workspace.create_layer("experiment").is_err());
    fs::write(checkout.0.join("hello"), b"release contents").unwrap();
    let release = workspace.create_release("1.0.0", "First version").unwrap();
    assert_ne!(release.target_step_id(), marker.target_step_id());
    workspace.hide_marker(marker.id()).unwrap();
    let reopened = NativeWorkspace::open(&checkout.0).unwrap();
    assert!(reopened.markers().unwrap().is_empty());
    assert_eq!(reopened.releases().unwrap(), vec![release]);
    assert_eq!(reopened.state().unwrap().layers.len(), 2);
    assert_eq!(reopened.operations(100).unwrap().len(), 6);
    assert!(reopened.create_release("1.0.0", "duplicate").is_err());
}

#[test]
fn initialize_refuses_existing_metadata_without_modification() {
    let checkout = Checkout::new();
    fs::create_dir(checkout.0.join(".layrs")).unwrap();
    fs::write(checkout.0.join(".layrs/legacy"), b"preserve me").unwrap();
    assert!(NativeWorkspace::initialize(&checkout.0, "No migration").is_err());
    assert_eq!(
        fs::read(checkout.0.join(".layrs/legacy")).unwrap(),
        b"preserve me"
    );
    assert!(!checkout.0.join(".layrs/FORMAT").exists());
    assert!(scm_v1::init(
        checkout.0.join(".layrs"),
        StoreId::parse("store:no").unwrap()
    )
    .is_err());
}

#[test]
fn switch_and_restore_keep_original_history_and_exact_bytes() {
    let checkout = Checkout::new();
    let workspace = checkout.init();
    let first = workspace.create_marker("original", "").unwrap();
    workspace.create_layer("experiment").unwrap();
    workspace.switch_layer("experiment").unwrap();
    fs::write(checkout.0.join("hello"), [0, 255, 99]).unwrap();
    fs::write(checkout.0.join("added"), b"").unwrap();
    workspace.capture().unwrap();
    workspace.switch_layer("main").unwrap();
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"hello\0world");
    assert!(!checkout.0.join("added").exists());
    workspace.switch_layer("experiment").unwrap();
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), [0, 255, 99]);
    assert!(checkout.0.join("added").exists());
    let restored = workspace
        .restore_step(first.target_step_id(), Some("hello"))
        .unwrap();
    assert_ne!(&restored, first.target_step_id());
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"hello\0world");
    assert!(checkout.0.join("added").exists());
    assert!(!NativeWorkspace::open(&checkout.0)
        .unwrap()
        .is_dirty()
        .unwrap());
    assert!(
        scm_v1::fsck_existing(workspace.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn interrupted_materialization_retains_backup_and_blocks_new_capture() {
    use scm_v1::materialize::{MaterializationFaults, MaterializationPoint};
    struct Stop;
    impl MaterializationFaults for Stop {
        fn check(&self, point: MaterializationPoint) -> std::io::Result<()> {
            if point == MaterializationPoint::AfterPath {
                Err(std::io::Error::other("injected process interruption"))
            } else {
                Ok(())
            }
        }
    }
    let checkout = Checkout::new();
    let workspace = checkout.init();
    let marker = workspace.create_marker("old", "").unwrap();
    fs::write(checkout.0.join("hello"), b"valuable new work").unwrap();
    workspace.capture().unwrap();
    let before = workspace.state().unwrap().operation;
    assert!(workspace
        .restore_step_with_faults(marker.target_step_id(), None, &Stop)
        .is_err());
    assert_eq!(workspace.state().unwrap().operation, before);
    let mut store = scm_v1::open_exclusive(workspace.store_root()).unwrap();
    let pending = scm_v1::NativeRecovery::new(&mut store)
        .unwrap()
        .list_pending()
        .unwrap();
    assert_eq!(pending.len(), 1);
    let backup = PathBuf::from(pending[0].payload["backupRoot"].as_str().unwrap()).join("old-0");
    assert_eq!(fs::read(backup).unwrap(), b"valuable new work");
    drop(store);
    assert!(workspace.capture().is_err());
    assert_eq!(workspace.recover().unwrap(), 1);
    assert_eq!(
        fs::read(checkout.0.join("hello")).unwrap(),
        b"valuable new work"
    );
    assert!(!workspace.is_dirty().unwrap());
    assert_eq!(workspace.recover().unwrap(), 0);
}

#[test]
fn recovery_distinguishes_precommit_and_committed_interruptions() {
    use scm_v1::materialize::{MaterializationFaults, MaterializationPoint};
    struct Stop(MaterializationPoint);
    impl MaterializationFaults for Stop {
        fn check(&self, point: MaterializationPoint) -> std::io::Result<()> {
            if point == self.0 {
                Err(std::io::Error::other("crash"))
            } else {
                Ok(())
            }
        }
    }
    for point in [
        MaterializationPoint::AfterJournal,
        MaterializationPoint::BeforeMetadata,
        MaterializationPoint::AfterMetadata,
    ] {
        let checkout = Checkout::new();
        let workspace = checkout.init();
        let marker = workspace.create_marker("initial", "").unwrap();
        fs::write(checkout.0.join("hello"), b"saved work").unwrap();
        workspace.capture().unwrap();
        assert!(workspace
            .restore_step_with_faults(marker.target_step_id(), None, &Stop(point))
            .is_err());
        let reopened = NativeWorkspace::open(&checkout.0).unwrap();
        // The metadata and the completed journal commit together (council
        // s2 r1, B2): stopping after that commit leaves nothing to recover.
        assert_eq!(
            reopened.recover().unwrap(),
            usize::from(point != MaterializationPoint::AfterMetadata),
            "point {point:?}"
        );
        assert_eq!(
            fs::read(checkout.0.join("hello")).unwrap(),
            if point == MaterializationPoint::AfterMetadata {
                b"hello\0world".as_slice()
            } else {
                b"saved work".as_slice()
            }
        );
        assert!(!reopened.is_dirty().unwrap());
    }
}

/// Files are staged before the journal (council s4, Q1). Stopping between
/// staging and the journal leaves no journal and an untouched checkout; the
/// next materialization of the Worktree sweeps the folder no journal names.
#[test]
fn a_stop_after_staging_leaves_no_journal_and_its_folder_is_swept() {
    use scm_v1::materialize::{MaterializationFaults, MaterializationPoint};
    struct Stop;
    impl MaterializationFaults for Stop {
        fn check(&self, point: MaterializationPoint) -> std::io::Result<()> {
            if point == MaterializationPoint::AfterStaging {
                Err(std::io::Error::other("crash"))
            } else {
                Ok(())
            }
        }
    }
    let checkout = Checkout::new();
    let workspace = checkout.init();
    let marker = workspace.create_marker("initial", "").unwrap();
    fs::write(checkout.0.join("hello"), b"saved work").unwrap();
    workspace.capture().unwrap();
    let attempts = |workspace: &NativeWorkspace| -> Vec<String> {
        fs::read_dir(workspace.store_root().join("metadata/repair-backups"))
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|name| name.contains("-attempt-"))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(workspace
        .restore_step_with_faults(marker.target_step_id(), None, &Stop)
        .is_err());
    // No journal, nothing to repair, the checkout as it was; the stopped
    // attempt's folder stays until the Worktree is recovered...
    let reopened = NativeWorkspace::open(&checkout.0).unwrap();
    assert!(!reopened.needs_repair().unwrap());
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"saved work");
    assert!(!reopened.is_dirty().unwrap());
    let left = attempts(&reopened);
    assert_eq!(left.len(), 1, "the stopped attempt's folder: {left:?}");
    assert_eq!(reopened.recover().unwrap(), 0);
    assert!(attempts(&reopened).is_empty(), "swept by the recovery");
    // ...or when its Room is dropped (`room drop` retires the Worktree)...
    assert!(reopened
        .restore_step_with_faults(marker.target_step_id(), None, &Stop)
        .is_err());
    assert_eq!(attempts(&reopened).len(), 1);
    // Not while another process writes this Worktree's checkout.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(reopened.store_root().join("metadata").join(format!(
            "checkout-{}.lock",
            blake3::hash(reopened.worktree_id().as_str().as_bytes()).to_hex()
        )))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&lock).unwrap();
    reopened
        .forget_staged_attempts(reopened.worktree_id().as_str())
        .unwrap();
    assert_eq!(attempts(&reopened).len(), 1, "kept while the lock is held");
    fs2::FileExt::unlock(&lock).unwrap();
    drop(lock);
    reopened
        .forget_staged_attempts(reopened.worktree_id().as_str())
        .unwrap();
    assert!(
        attempts(&reopened).is_empty(),
        "swept for a retired Worktree"
    );
    // ...or materialized again.
    assert!(reopened
        .restore_step_with_faults(marker.target_step_id(), None, &Stop)
        .is_err());
    let left = attempts(&reopened);
    assert_eq!(left.len(), 1, "the stopped attempt's folder: {left:?}");
    reopened
        .restore_step(marker.target_step_id(), None)
        .unwrap();
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"hello\0world");
    assert!(
        !attempts(&reopened).contains(&left[0]),
        "swept before staging"
    );
}

#[test]
fn recovery_preserves_edits_made_after_interruption() {
    use scm_v1::materialize::{MaterializationFaults, MaterializationPoint};
    struct Stop;
    impl MaterializationFaults for Stop {
        fn check(&self, point: MaterializationPoint) -> std::io::Result<()> {
            if point == MaterializationPoint::AfterPath {
                Err(std::io::Error::other("crash"))
            } else {
                Ok(())
            }
        }
    }
    let checkout = Checkout::new();
    let workspace = checkout.init();
    let marker = workspace.create_marker("initial", "").unwrap();
    fs::write(checkout.0.join("hello"), b"saved before restore").unwrap();
    workspace.capture().unwrap();
    assert!(workspace
        .restore_step_with_faults(marker.target_step_id(), None, &Stop)
        .is_err());
    fs::write(checkout.0.join("hello"), b"edited while repair pending").unwrap();
    assert!(workspace.recover().is_err());
    assert_eq!(
        fs::read(checkout.0.join("hello")).unwrap(),
        b"edited while repair pending"
    );
}

#[test]
fn undo_redo_append_operations_and_preserve_intervening_work() {
    let checkout = Checkout::new();
    let workspace = checkout.init();
    fs::write(checkout.0.join("hello"), b"second").unwrap();
    workspace.capture().unwrap();
    let saved = workspace.state().unwrap().operation.id().clone();
    workspace.undo().unwrap();
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"hello\0world");
    workspace.redo(None).unwrap();
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"second");
    workspace.undo().unwrap();
    fs::write(checkout.0.join("hello"), b"intervening work").unwrap();
    workspace.capture().unwrap();
    assert!(workspace.redo(None).is_err());
    workspace.redo(Some(&saved)).unwrap();
    assert_eq!(fs::read(checkout.0.join("hello")).unwrap(), b"second");
    let history = workspace.operations(100).unwrap();
    assert_eq!(history.len(), 7);
    assert_eq!(history[0].kind(), OperationKind::Redo);
    assert!(history.iter().any(|op| op.id() == &saved));
}

#[test]
fn native_diff_preserves_newlines_and_distinguishes_empty_from_absence() {
    let checkout = Checkout::new();
    fs::write(checkout.0.join("hello"), b"one\ntwo\n").unwrap();
    let workspace = checkout.init();
    fs::write(checkout.0.join("hello"), b"one\ninserted\ntwo").unwrap();
    fs::write(checkout.0.join("empty"), b"").unwrap();
    fs::write(checkout.0.join("binary"), [0, 255]).unwrap();
    let changes = workspace.working_diff().unwrap();
    let text = changes.iter().find(|c| c.path == "hello").unwrap();
    assert!(!text.binary);
    assert!(text
        .text_diff
        .as_ref()
        .unwrap()
        .iter()
        .any(|line| line.op == "insert" && line.text == "two"));
    let empty = changes.iter().find(|c| c.path == "empty").unwrap();
    assert_eq!(empty.status, "added");
    assert_eq!(empty.before_bytes, None);
    assert_eq!(empty.after_bytes, Some(0));
    assert!(changes.iter().find(|c| c.path == "binary").unwrap().binary);
    assert_eq!(workspace.operations(100).unwrap().len(), 1);
}

/// Format 6 (ADR 0028) stores objects in compressed pack v2 blocks rather
/// than one loose file each: this damages `object`'s stored bytes by
/// flipping one byte of the block that holds it (every record of that block
/// then fails verification) and returns that pack. `None` for other formats,
/// whose tests damage the loose file as before.
fn damage_compact_object(
    store_root: &std::path::Path,
    object: &layrs_store::scm::StoreObjectId,
) -> Option<std::path::PathBuf> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let store = layrs_store_local::scm_v1::open(store_root).unwrap();
    if store.format_version() != layrs_store_local::scm_v1::COMPACT_FORMAT_VERSION {
        return None;
    }
    let location = layrs_store_local::scm_v1::NativeCas::open(&store)
        .unwrap()
        .locate(object)
        .unwrap()
        .unwrap();
    drop(store);
    let layrs_store_local::scm_v1::CasLocation::Packed { pack, offset, .. } = location else {
        panic!("format 6 object {object} is not packed");
    };
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&pack)
        .unwrap();
    let mut byte = [0_u8; 1];
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&[byte[0] ^ 0x5a]).unwrap();
    Some(pack)
}
