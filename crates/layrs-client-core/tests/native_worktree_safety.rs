mod worktree_support;
use layrs_client_core::native_scm::*;
use layrs_store::scm::StoreObjectId;
use layrs_store_local::scm_v1::{self, materialize::NoMaterializationFaults};
use std::{fs, io, path::Path};

#[test]
fn metadata_directory_copy_is_rejected_by_open_workspace_and_locator() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let root = secondary.checkout_root().unwrap();
    fs::write(root.join("same.txt"), b"pending metadata swap capture").unwrap();
    let CapturePlan::Change(change) = secondary
        .client()
        .prepare_disk_capture(
            secondary.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected capture")
    };
    let metadata = primary.store_root().join("metadata");
    let replacement = fixture.0.join("replacement-metadata");
    copy_directory(&metadata, &replacement);
    fs::rename(&metadata, fixture.0.join("original-metadata")).unwrap();
    fs::rename(replacement, &metadata).unwrap();
    let before = fs::read(metadata.join("layrs.sqlite3")).unwrap();
    assert!(primary.state().is_err());
    assert!(secondary.client().accept(&change).is_err());
    assert!(secondary.recover().is_err());
    assert!(NativeWorkspace::open(&root).is_err());
    assert!(NativeWorkspace::audit_worktree_store(&root).is_err());
    assert_eq!(fs::read(metadata.join("layrs.sqlite3")).unwrap(), before);
    assert_eq!(
        fs::read(root.join("same.txt")).unwrap(),
        b"pending metadata swap capture"
    );
}

#[test]
fn audited_maintenance_token_refuses_root_or_metadata_replacement_before_mutation() {
    for metadata_only in [false, true] {
        let fixture = Fixture::new();
        let primary = fixture.primary();
        let secondary = fixture.secondary(&primary);
        let checkout_root = secondary.checkout_root().unwrap();
        let token = NativeWorkspace::audit_worktree_store(&checkout_root).unwrap();
        let target = if metadata_only {
            primary.store_root().join("metadata")
        } else {
            primary.store_root().to_path_buf()
        };
        let replacement = fixture.0.join("copy");
        copy_directory(&target, &replacement);
        let metadata = primary.store_root().join("metadata");
        let before = fs::read(metadata.join("layrs.sqlite3")).unwrap();
        let identity = scm_v1::checkout::CheckoutReader::open(&target, None)
            .unwrap()
            .identity();
        let original = fixture.0.join("original");
        match fs::rename(&target, &original) {
            Ok(()) => {
                fs::rename(&replacement, &target).unwrap();
                assert!(token.compact_metadata().is_err());
                assert!(token.resume_metadata_compaction().is_err());
                assert!(!metadata.join("metadata-compaction.json").exists());
                assert_eq!(fs::read(metadata.join("layrs.sqlite3")).unwrap(), before);
            }
            Err(error) => {
                assert!(
                    cfg!(windows) && error.kind() == io::ErrorKind::PermissionDenied,
                    "unexpected replacement error: {error:?}"
                );
                // Windows denies rename while the audit token pins the original
                // directory. This is protection before the mutation boundary.
                assert!(!original.exists());
                assert_eq!(fs::read(metadata.join("layrs.sqlite3")).unwrap(), before);
                assert_eq!(
                    scm_v1::checkout::CheckoutReader::open(&target, Some(&identity))
                        .unwrap()
                        .identity(),
                    identity
                );
                let copied_database = if metadata_only {
                    replacement.join("layrs.sqlite3")
                } else {
                    replacement.join("metadata/layrs.sqlite3")
                };
                let operation = primary.state().unwrap().operation.id().clone();
                let report = token.compact_metadata().unwrap();
                assert_eq!(
                    &report.store_id,
                    primary.state().unwrap().worktree.store_id()
                );
                assert_eq!(primary.state().unwrap().operation.id(), &operation);
                assert_eq!(fs::read(copied_database).unwrap(), before);
                scm_v1::checkout::CheckoutReader::open(&target, Some(&identity)).unwrap();
                assert_clean(&primary, &[]);
            }
        }
        assert_eq!(
            fs::read(checkout_root.join("same.txt")).unwrap(),
            b"base\0bytes"
        );
    }
}

#[test]
fn bound_reads_wait_for_writer_release_and_preserve_own_operation() {
    use std::{sync::mpsc, time::Duration};
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let root = secondary.checkout_root().unwrap();
    let expected = secondary.state().unwrap().operation.id().clone();
    for mode in 0..3 {
        let writer = scm_v1::open_exclusive(primary.store_root()).unwrap();
        std::thread::scope(|scope| {
            let (started, started_rx) = mpsc::channel();
            let (finished, finished_rx) = mpsc::channel();
            let secondary = &secondary;
            let root = &root;
            let worker = scope.spawn(move || {
                started.send(()).unwrap();
                let state = match mode {
                    0 => secondary.state(),
                    1 => NativeWorkspace::open(&root).and_then(|w| w.state()),
                    _ => NativeWorkspace::discover(&root).and_then(|w| w.state()),
                };
                finished
                    .send(state.map(|s| s.operation.id().clone()))
                    .unwrap();
            });
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let blocked = finished_rx.recv_timeout(Duration::from_millis(50));
            drop(writer);
            assert!(
                matches!(blocked, Err(mpsc::RecvTimeoutError::Timeout)),
                "read escaped exclusive writer: {blocked:?}"
            );
            let operation = finished_rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap();
            assert_eq!(operation, expected);
            worker.join().unwrap();
        });
    }
    assert_clean(&primary, &[]);
}

#[test]
fn staged_reservation_survives_source_restoring_an_older_view() {
    use layrs_core::scm::OperationKind;
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let initial = primary.state().unwrap().operation.id().clone();
    fs::write(
        primary.checkout_root().unwrap().join("same.txt"),
        b"reserved exact bytes",
    )
    .unwrap();
    primary.capture().unwrap();
    let reserved = own(&primary);
    let source = primary.state().unwrap().operation.id().clone();
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_creation_faults(
            &root,
            reserved.active_layer_id().unwrap(),
            reserved.base_step_id().unwrap(),
            &NoMaterializationFaults,
            &BeforePublication
        )
        .is_err());
    let restore = primary
        .restore_operation(&initial, OperationKind::Restore)
        .unwrap();
    assert_eq!(
        primary.state().unwrap().operation.parent_operation_ids(),
        &[source]
    );
    assert_eq!(primary.state().unwrap().operation.id(), &restore);
    let secondary = NativeWorkspace::resume_worktree(&root).unwrap();
    assert_eq!(own(&secondary).base_step_id(), reserved.base_step_id());
    assert_eq!(
        fs::read(root.join("same.txt")).unwrap(),
        b"reserved exact bytes"
    );
    assert_eq!(
        fs::read(primary.checkout_root().unwrap().join("same.txt")).unwrap(),
        b"base\0bytes"
    );
    assert_clean(&primary, &[reserved.base_step_id().unwrap().clone()]);
}

#[cfg(windows)]
#[test]
fn primary_checkout_case_alias_uses_registered_filesystem_identity() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let root = primary.checkout_root().unwrap();
    let alias = std::path::PathBuf::from(root.to_string_lossy().to_uppercase());
    let reopened = NativeWorkspace::open(&alias).unwrap();
    assert_eq!(reopened.worktree_id(), primary.worktree_id());
    assert_eq!(reopened.store_identity(), primary.store_identity());
    NativeWorkspace::audit_worktree_store(&alias)
        .unwrap()
        .compact_metadata()
        .unwrap();
    assert_clean(&primary, &[]);
}

#[cfg(windows)]
#[test]
fn secondary_case_alias_opens_discovers_and_audits_without_accepting_a_copied_root() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let root = secondary.checkout_root().unwrap();
    let alias = std::path::PathBuf::from(root.to_string_lossy().to_uppercase());
    assert_ne!(root, alias);
    let locator_bytes = fs::read(root.join(".layrs")).unwrap();
    let reopened = NativeWorkspace::open(&alias).unwrap();
    let discovered = NativeWorkspace::discover(alias.join("same.txt")).unwrap();
    for workspace in [&reopened, &discovered] {
        assert_eq!(workspace.worktree_id(), secondary.worktree_id());
        assert_eq!(workspace.store_identity(), secondary.store_identity());
        assert_eq!(workspace.metadata_identity(), secondary.metadata_identity());
        assert_eq!(own(workspace), own(&secondary));
        assert_eq!(workspace.checkout_root().unwrap(), root);
    }
    let audit = NativeWorkspace::audit_worktree_store(&alias).unwrap();
    assert_eq!(audit.store_root(), primary.store_root());
    assert_eq!(
        NativeWorkspace::resolve_store_for_audit(&alias).unwrap(),
        primary.store_root()
    );
    assert_eq!(fs::read(root.join(".layrs")).unwrap(), locator_bytes);

    let copied = fixture.0.join("copied-secondary");
    copy_directory(&root, &copied);
    let copied_alias = std::path::PathBuf::from(copied.to_string_lossy().to_uppercase());
    assert!(NativeWorkspace::open(&copied_alias).is_err());
    assert!(NativeWorkspace::discover(copied_alias.join("same.txt")).is_err());
    assert!(NativeWorkspace::audit_worktree_store(&copied_alias).is_err());
    assert!(NativeWorkspace::resume_worktree(&copied_alias).is_err());
    assert_eq!(fs::read(copied.join(".layrs")).unwrap(), locator_bytes);
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_clean(&primary, &[own(&secondary).base_step_id().unwrap().clone()]);
}

#[cfg(windows)]
#[test]
fn secondary_staged_reservation_resumes_through_case_alias_with_exact_recorded_identity() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own(&primary);
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_creation_faults(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
            &NoMaterializationFaults,
            &BeforePublication,
        )
        .is_err());
    let bytes = fs::read(root.join(".layrs.reservation")).unwrap();
    let intent: scm_v1::worktrees::WorktreeLocator = serde_json::from_slice(&bytes).unwrap();
    let alias = std::path::PathBuf::from(root.to_string_lossy().to_uppercase());
    assert_ne!(root, alias);
    let resumed = NativeWorkspace::resume_worktree(&alias).unwrap();
    assert_eq!(resumed.worktree_id(), &intent.worktree_id);
    assert_eq!(resumed.checkout_root().unwrap(), root);
    assert_eq!(
        resumed.state().unwrap().worktree.filesystem_identity(),
        intent.filesystem_identity
    );
    assert_eq!(own(&resumed).base_step_id(), state.base_step_id());
    assert_eq!(
        own(&resumed).status(),
        layrs_core::scm::WorktreeStatus::Ready
    );
    assert_eq!(fs::read(root.join(".layrs")).unwrap(), bytes);
    assert!(!root.join(".layrs.reservation").exists());
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_eq!(
        NativeWorkspace::open(&alias).unwrap().worktree_id(),
        &intent.worktree_id
    );
    assert_clean(&primary, &[state.base_step_id().unwrap().clone()]);
}

#[cfg(windows)]
#[test]
fn replaced_registered_root_reserves_case_aliased_descendants() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let state = own(&primary);
    let root = secondary.checkout_root().unwrap();
    let identity = secondary
        .state()
        .unwrap()
        .worktree
        .filesystem_identity()
        .to_owned();
    let locator_bytes = fs::read(root.join(".layrs")).unwrap();
    let preserved = fixture.0.join("preserved-secondary");
    fs::rename(&root, &preserved).unwrap();
    let nested = root.join("sub");
    fs::create_dir_all(&nested).unwrap();
    assert_ne!(
        scm_v1::checkout::CheckoutReader::open(&root, None)
            .unwrap()
            .identity(),
        identity
    );
    let alias = std::path::PathBuf::from(nested.to_string_lossy().to_uppercase());
    assert_ne!(nested, alias);
    let error = primary
        .create_worktree(
            &alias,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .err()
        .expect("registered namespace must remain reserved after root replacement");
    assert!(error.to_string().contains("overlap"), "{error:?}");
    assert!(fs::read_dir(&nested).unwrap().next().is_none());
    assert_eq!(primary.list_worktrees().unwrap().len(), 2);
    assert_eq!(own(&primary), state);
    assert_eq!(fs::read(preserved.join(".layrs")).unwrap(), locator_bytes);
    assert_eq!(
        fs::read(preserved.join("same.txt")).unwrap(),
        b"base\0bytes"
    );
    assert_clean(&primary, &[state.base_step_id().unwrap().clone()]);
}

#[cfg(windows)]
#[test]
fn secondary_locator_rejects_relative_or_inconsistent_recorded_checkout_paths() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let root = secondary.checkout_root().unwrap();
    let alias = std::path::PathBuf::from(root.to_string_lossy().to_uppercase());
    let different = fixture.directory("different-root");
    let original_bytes = fs::read(root.join(".layrs")).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
    let recorded = root.to_str().unwrap();
    let different = different.to_str().unwrap();
    for (locator_path, creation_path) in [
        ("secondary", "secondary"),
        (recorded, "secondary"),
        (different, recorded),
        (different, different),
    ] {
        let mut tampered = original.clone();
        tampered["checkoutRoot"] = locator_path.into();
        tampered["creation"]["worktree"]["checkoutRoot"] = creation_path.into();
        let bytes = serde_json::to_vec(&tampered).unwrap();
        fs::write(root.join(".layrs"), &bytes).unwrap();
        assert!(
            scm_v1::worktrees::read_locator(&alias).is_err(),
            "{locator_path} / {creation_path}"
        );
        assert!(NativeWorkspace::open(&alias).is_err());
        assert!(NativeWorkspace::discover(alias.join("same.txt")).is_err());
        assert!(NativeWorkspace::audit_worktree_store(&alias).is_err());
        assert!(NativeWorkspace::resume_worktree(&alias).is_err());
        assert_eq!(fs::read(root.join(".layrs")).unwrap(), bytes);
        assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
        assert_eq!(primary.list_worktrees().unwrap().len(), 2);
    }
    fs::write(root.join(".layrs"), original_bytes).unwrap();
    assert_eq!(
        NativeWorkspace::open(&alias).unwrap().worktree_id(),
        secondary.worktree_id()
    );
    assert_clean(&primary, &[own(&secondary).base_step_id().unwrap().clone()]);
}

#[cfg(windows)]
#[test]
fn secondary_registered_stale_creation_recovers_and_resumes_through_case_alias() {
    use scm_v1::materialize::{MaterializationFaults, MaterializationPoint};
    struct StopAfterJournal;
    impl MaterializationFaults for StopAfterJournal {
        fn check(&self, point: MaterializationPoint) -> io::Result<()> {
            if point == MaterializationPoint::AfterJournal {
                Err(io::Error::other(
                    "stop after durable registration and journal",
                ))
            } else {
                Ok(())
            }
        }
    }
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own(&primary);
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_faults(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
            &StopAfterJournal,
        )
        .is_err());
    let locator_bytes = fs::read(root.join(".layrs")).unwrap();
    let locator = scm_v1::worktrees::read_locator(&root).unwrap();
    let alias = std::path::PathBuf::from(root.to_string_lossy().to_uppercase());
    let stale = NativeWorkspace::open(&alias).unwrap();
    assert_eq!(primary.list_worktrees().unwrap().len(), 2);
    assert_eq!(stale.worktree_id(), &locator.worktree_id);
    assert_eq!(own(&stale).status(), layrs_core::scm::WorktreeStatus::Stale);
    assert!(stale.needs_repair().unwrap());
    assert!(matches!(
        NativeWorkspace::resume_worktree(&alias),
        Err(NativeClientError::NeedsRepair)
    ));
    stale.recover().unwrap();
    let resumed = NativeWorkspace::resume_worktree(&alias).unwrap();
    assert_eq!(resumed.worktree_id(), &locator.worktree_id);
    assert_eq!(resumed.checkout_root().unwrap(), root);
    assert_eq!(
        own(&resumed).status(),
        layrs_core::scm::WorktreeStatus::Ready
    );
    assert_eq!(own(&resumed).base_step_id(), state.base_step_id());
    assert_eq!(
        resumed.state().unwrap().operation.parent_operation_ids(),
        &[locator.creation.worktree.active_operation_id().clone()]
    );
    assert_eq!(fs::read(root.join(".layrs")).unwrap(), locator_bytes);
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_eq!(own(&primary), state);
    assert_clean(&primary, &[state.base_step_id().unwrap().clone()]);
}
use worktree_support::*;

struct ShortWrite;
impl WorktreeCreationFaults for ShortWrite {
    fn after_locator(&self) -> io::Result<()> {
        Ok(())
    }
    fn write_staged_locator(&self, output: &mut dyn io::Write, bytes: &[u8]) -> io::Result<()> {
        output.write_all(&bytes[..37])?;
        Err(io::Error::other("interrupted short locator write"))
    }
}
struct BeforePublication;
impl WorktreeCreationFaults for BeforePublication {
    fn after_locator(&self) -> io::Result<()> {
        Ok(())
    }
    fn before_locator_publication(&self) -> io::Result<()> {
        Err(io::Error::other("interrupted before publication"))
    }
}

#[test]
fn short_locator_write_preserves_staging_and_leaves_empty_root_retryable() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own(&primary);
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_creation_faults(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
            &NoMaterializationFaults,
            &ShortWrite
        )
        .is_err());
    assert!(fs::read_dir(&root).unwrap().next().is_none());
    let staged: Vec<_> = fs::read_dir(primary.store_root().join("metadata/worktree-intents"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(staged.len(), 1);
    let evidence = fs::read(&staged[0]).unwrap();
    assert_eq!(evidence.len(), 37);
    assert_eq!(primary.list_worktrees().unwrap().len(), 1);
    let secondary = primary
        .create_worktree(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .unwrap();
    assert_eq!(fs::read(&staged[0]).unwrap(), evidence);
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_eq!(own(&secondary).base_step_id(), state.base_step_id());
    assert_clean(&primary, &[state.base_step_id().unwrap().clone()]);
}

#[test]
fn flushed_staged_reservation_resumes_exact_identity_after_source_advance() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own(&primary);
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_creation_faults(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
            &NoMaterializationFaults,
            &BeforePublication
        )
        .is_err());
    assert!(!root.join(".layrs").exists());
    assert!(NativeWorkspace::open(&root).is_err());
    let bytes = fs::read(root.join(".layrs.reservation")).unwrap();
    let intent: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    fs::write(
        primary.checkout_root().unwrap().join("same.txt"),
        b"source advanced",
    )
    .unwrap();
    primary.capture().unwrap();
    let secondary = NativeWorkspace::resume_worktree(&root).unwrap();
    assert_eq!(
        secondary.worktree_id().as_str(),
        intent["worktreeId"].as_str().unwrap()
    );
    assert_eq!(fs::read(root.join(".layrs")).unwrap(), bytes);
    assert!(!root.join(".layrs.reservation").exists());
    assert_eq!(own(&secondary).base_step_id(), state.base_step_id());
    assert_eq!(fs::read(root.join("same.txt")).unwrap(), b"base\0bytes");
    assert_clean(&primary, &[state.base_step_id().unwrap().clone()]);
}

struct Collide<'a>(&'a Path);
impl WorktreeCreationFaults for Collide<'_> {
    fn after_locator(&self) -> io::Result<()> {
        Ok(())
    }
    fn before_locator_publication(&self) -> io::Result<()> {
        fs::write(self.0.join(".layrs"), b"external locator collision")
    }
}
#[test]
fn atomic_publication_and_resume_preserve_final_marker_collision() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let state = own(&primary);
    let root = fixture.directory("secondary");
    assert!(primary
        .create_worktree_with_creation_faults(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
            &NoMaterializationFaults,
            &Collide(&root)
        )
        .is_err());
    let staged = fs::read(root.join(".layrs.reservation")).unwrap();
    assert!(NativeWorkspace::resume_worktree(&root).is_err());
    assert_eq!(
        fs::read(root.join(".layrs")).unwrap(),
        b"external locator collision"
    );
    assert_eq!(fs::read(root.join(".layrs.reservation")).unwrap(), staged);
    assert_eq!(primary.list_worktrees().unwrap().len(), 1);
}

fn copy_directory(source: &Path, target: &Path) {
    fs::create_dir(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink());
        if kind.is_dir() {
            copy_directory(&entry.path(), &target.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
fn already_open_workspace_rejects_copied_store_with_same_logical_ids() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let secondary_root = secondary.checkout_root().unwrap();
    fs::write(secondary_root.join("same.txt"), b"pending capture").unwrap();
    let CapturePlan::Change(prepared) = secondary
        .client()
        .prepare_disk_capture(
            secondary.worktree_id(),
            SnapshotLimits::streaming(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected changed capture")
    };
    let replacement = fixture.0.join("replacement");
    copy_directory(primary.store_root(), &replacement);
    fs::rename(primary.store_root(), fixture.0.join("original-store")).unwrap();
    fs::rename(&replacement, primary.store_root()).unwrap();
    let metadata = primary.store_root().join("metadata/layrs.sqlite3");
    let before = fs::read(&metadata).unwrap();
    assert!(primary.state().is_err());
    assert!(secondary.open_store().is_err());
    assert!(secondary.state().is_err());
    assert!(secondary.capture().is_err());
    assert!(secondary.client().accept(&prepared).is_err());
    assert!(secondary.recover().is_err());
    assert!(NativeWorkspace::open(&secondary_root).is_err());
    assert!(NativeWorkspace::resolve_store_for_audit(&secondary_root).is_err());
    assert_eq!(fs::read(&metadata).unwrap(), before);
    assert_eq!(
        fs::read(secondary_root.join("same.txt")).unwrap(),
        b"pending capture"
    );
}

#[test]
fn secondary_audit_resolves_registered_identity_when_selected_view_is_corrupt() {
    let fixture = Fixture::new();
    let primary = fixture.primary();
    let secondary = fixture.secondary(&primary);
    let root = secondary.checkout_root().unwrap();
    let view = secondary.state().unwrap().view.id().clone();
    let store = secondary.open_store().unwrap();
    let path = scm_v1::NativeCas::open(&store)
        .unwrap()
        .object_path(&StoreObjectId::RepositoryView(view))
        .unwrap();
    drop(store);
    fs::write(path, b"corrupt selected View").unwrap();
    assert!(NativeWorkspace::open(&root).is_err());
    assert_eq!(
        NativeWorkspace::resolve_store_for_audit(&root).unwrap(),
        primary.store_root()
    );
    let diagnosis = diagnose_checkout(&root).unwrap();
    assert_eq!(diagnosis["clean"], false);
    assert!(!diagnosis["findings"].as_array().unwrap().is_empty());
}

struct StopCompaction(scm_v1::compaction::CompactionFaultPoint);
impl scm_v1::compaction::CompactionFaults for StopCompaction {
    fn check(
        &self,
        point: scm_v1::compaction::CompactionFaultPoint,
    ) -> scm_v1::compaction::CompactionResult<()> {
        if point == self.0 {
            Err("interrupted compaction".into())
        } else {
            Ok(())
        }
    }
}
#[test]
fn secondary_audit_resolves_registered_identity_across_compaction_fence() {
    use scm_v1::compaction::*;
    for point in [
        CompactionFaultPoint::JournalDurable,
        CompactionFaultPoint::DatabasePublished,
    ] {
        let fixture = Fixture::new();
        let primary = fixture.primary();
        let secondary = fixture.secondary(&primary);
        let root = secondary.checkout_root().unwrap();
        assert!(
            compact_metadata_with_faults(primary.store_root(), &StopCompaction(point)).is_err()
        );
        assert!(NativeWorkspace::open(&root).is_err());
        let store_root = NativeWorkspace::resolve_store_for_audit(&root).unwrap();
        assert_eq!(store_root, primary.store_root());
        resume_metadata_compaction(&store_root).unwrap();
        assert_eq!(
            NativeWorkspace::open(root).unwrap().worktree_id(),
            secondary.worktree_id()
        );
        assert_clean(&primary, &[]);
    }
}

/// Set this to an existing writable directory on a different volume on Windows.
/// Linux /dev/shm supplies a separate filesystem in the normal test environment.
#[test]
#[cfg_attr(
    windows,
    ignore = "requires LAYRS_TEST_SECOND_VOLUME on a distinct writable volume"
)]
fn different_volume_is_rejected_before_locator_even_for_an_empty_step() {
    let second = std::env::var_os("LAYRS_TEST_SECOND_VOLUME")
        .map(std::path::PathBuf::from)
        .or_else(|| cfg!(target_os = "linux").then(|| "/dev/shm".into()));
    let second = second.expect("set LAYRS_TEST_SECOND_VOLUME before running this ignored test");
    let fixture = Fixture::new();
    let primary = NativeWorkspace::initialize(fixture.directory("primary"), "Empty").unwrap();
    let state = own(&primary);
    let root = second.join(format!("layrs-cross-volume-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let error = primary
        .create_worktree(
            &root,
            state.active_layer_id().unwrap(),
            state.base_step_id().unwrap(),
        )
        .err()
        .expect("cross-volume creation must fail");
    assert!(error.to_string().contains("volume"), "{error}");
    assert!(fs::read_dir(&root).unwrap().next().is_none());
    assert_eq!(primary.list_worktrees().unwrap().len(), 1);
    fs::remove_dir(&root).unwrap();
}
