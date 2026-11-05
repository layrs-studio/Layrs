use super::*;
use crate::native_scm::{SnapshotBuilder, SnapshotLimits};

// Reproduce the old producer using actual Store acceptance and native
// registration, without adding a production representability bypass.
fn accept_test_image(
    client: &NativeClient,
    id: &WorktreeId,
    layer: &LayerId,
    step: &Step,
    mut objects: Vec<CanonicalObject>,
) -> OperationId {
    let store = client.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let worktree = reader.get_worktree(id).unwrap().unwrap();
    let active = reader
        .get_operation(worktree.active_operation_id())
        .unwrap()
        .unwrap();
    let old = reader
        .get_repository_view(active.resulting_view_id())
        .unwrap()
        .unwrap();
    let view = RepositoryView::new(
        worktree.space_id().clone(),
        old.layer_heads()
            .iter()
            .map(|head| {
                if head.layer_id() == layer {
                    LayerHeadSet::new(layer.clone(), vec![step.id().clone()]).unwrap()
                } else {
                    head.clone()
                }
            })
            .collect(),
        old.marker_ids().to_vec(),
        old.release_ids().to_vec(),
        old.worktrees()
            .iter()
            .map(|entry| {
                if entry.worktree_id() == id {
                    WorktreeViewState::new(
                        id.clone(),
                        Some(layer.clone()),
                        Some(step.id().clone()),
                        Some(step.root_tree_id().clone()),
                        WorktreeStatus::Ready,
                    )
                } else {
                    entry.clone()
                }
            })
            .collect(),
    )
    .unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let operation = prepare::operation(
        &stamp,
        worktree.space_id().clone(),
        vec![active.id().clone()],
        &view,
        OperationKind::Restore,
        "Legacy VFS fixture canonical image",
    )
    .unwrap();
    let current = reader
        .current_repository_view(worktree.space_id())
        .unwrap()
        .unwrap();
    let heads = reader.operation_heads(worktree.space_id()).unwrap();
    objects.push(CanonicalObject::RepositoryView(view.clone()));
    let batch = WriteBatch::new(
        stamp.transaction_id,
        objects.clone(),
        vec![EntityRecord::Operation(operation.clone())],
        prepare::admissions(worktree.space_id(), &objects),
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    worktree.space_id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    worktree.space_id().clone(),
                    heads.clone(),
                    operation.id().clone(),
                    worktree_operations::advance_heads(&heads, active.id(), operation.id()),
                )
                .unwrap(),
            ),
            StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(
                    id.clone(),
                    active.id().clone(),
                    operation.id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    drop(reader);
    drop(store);
    client
        .accept(&PreparedMutation {
            batch,
            step_id: step.id().clone(),
            chunks: None,
        })
        .unwrap();
    operation.id().clone()
}

struct LegacyMount {
    parent: PathBuf,
    workspace: NativeWorkspace,
    locator: VfsWorktreeLocator,
    supported_step: StepId,
    session: Session,
    journal: VfsRecovery,
}

fn append_legacy_ready_fixture(
    mut journal: VfsRecovery,
    locator: &VfsWorktreeLocator,
) -> VfsRecovery {
    assert!(journal
        .append(VfsEvent::Ready, &scm_v1::vfs_recovery::NoVfsJournalFaults)
        .is_err());
    let directory = journal.staging_root().unwrap();
    drop(journal);
    // Construct one old-producer record only in this fresh private fixture.
    // There is deliberately no production writer for legacy readiness.
    let previous = std::fs::read(directory.join("00000000005.json")).unwrap();
    let mut record: serde_json::Value = serde_json::from_slice(&previous).unwrap();
    assert_eq!(record["sequence"], 5);
    assert_eq!(record["event"]["type"], "locator_published");
    record["sequence"] = serde_json::json!(6);
    record["previousDigest"] = serde_json::json!(blake3::hash(&previous).to_hex().to_string());
    record["event"] = serde_json::json!({ "type": "ready" });
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("00000000006.json"))
        .unwrap();
    output
        .write_all(&serde_json::to_vec(&record).unwrap())
        .unwrap();
    output.sync_all().unwrap();
    drop(output);
    VfsRecovery::open(
        &locator.source.store_root,
        &locator.source.store_identity,
        &locator.source.store_metadata_identity,
        &locator.source.store_id,
        locator.worktree_id(),
    )
    .unwrap()
}

fn legacy_mount(unsupported: bool) -> LegacyMount {
    let parent = PathBuf::from(
        std::env::var_os("LAYRS_VFS_TEST_PARENT")
            .expect("explicit disposable LAYRS_VFS_TEST_PARENT required"),
    );
    assert!(parent.is_absolute() && parent.is_dir());
    let parent = parent.join(format!("legacy-vfs-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&parent).unwrap();
    eprintln!("retained legacy VFS fixture {}", parent.display());
    let ordinary = parent.join("ordinary");
    let root = parent.join("virtual");
    std::fs::create_dir(&ordinary).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(ordinary.join("file"), b"immutable source").unwrap();
    let workspace = NativeWorkspace::initialize(&ordinary, "Legacy VFS fixture").unwrap();
    let state = workspace.state().unwrap();
    let own = state
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == workspace.worktree_id())
        .unwrap();
    let layer = own.active_layer_id().unwrap().clone();
    let supported_step = own.base_step_id().unwrap().clone();
    let source = Arc::new(
        VerifiedVfsSnapshot::prepare(&workspace, &supported_step, &Cancellation::default())
            .unwrap(),
    );
    let mut locator =
        VfsWorktreeLocator::reserve(&workspace, &root, &layer, &supported_step, &source).unwrap();
    let client = locator.open_client().unwrap();
    let mut step = {
        let store = workspace.open_store().unwrap();
        NativeScmReader::new(&store)
            .unwrap()
            .get_step(&supported_step)
            .unwrap()
            .unwrap()
    };
    if unsupported {
        let empty = Tree::new(vec![]).unwrap();
        let mut entries = {
            let store = workspace.open_store().unwrap();
            NativeScmReader::new(&store)
                .unwrap()
                .get_tree(step.root_tree_id())
                .unwrap()
                .unwrap()
                .entries()
                .to_vec()
        };
        entries.push(TreeEntry::new(
            RepoPathComponent::parse("empty").unwrap(),
            TreeEntryTarget::tree(empty.id().clone()),
            false,
        ));
        let tree = Tree::new(entries).unwrap();
        step = Step::new(vec![supported_step.clone()], tree.id().clone()).unwrap();
        let operation = accept_test_image(
            &client,
            workspace.worktree_id(),
            &layer,
            &step,
            vec![
                CanonicalObject::Tree(empty),
                CanonicalObject::Tree(tree),
                CanonicalObject::Step(step.clone()),
            ],
        );
        locator.source.operation_id = operation.clone();
        locator.source.step_id = step.id().clone();
        locator.creation.source_operation_id = operation;
        locator.creation.step_id = step.id().clone();
    }
    let lease = locator.creation_lease(&source, None).unwrap();
    let mut journal = VfsRecovery::create(
        &locator.source.store_root,
        &locator.source.store_identity,
        &locator.source.store_metadata_identity,
        &locator.source.store_id,
        locator.worktree_id(),
        locator.encode().unwrap(),
        &scm_v1::vfs_recovery::NoVfsJournalFaults,
    )
    .unwrap();
    locator.register(&client, &lease).unwrap();
    let faults = scm_v1::vfs_recovery::NoVfsJournalFaults;
    journal.append(VfsEvent::Registered, &faults).unwrap();
    // The supported source has exactly the historical flattened catalog.
    // It is used solely to construct this old on-disk/native fixture.
    let mut session =
        Session::create_with_registration_hook(&root, locator.root_id, source, |ticket| {
            journal
                .append(
                    VfsEvent::ProviderPrepared {
                        ticket: ticket.encode(),
                    },
                    &faults,
                )
                .map_err(layrs_vfs::Error::Io)
        })
        .unwrap();
    journal
        .append(VfsEvent::ProviderPopulated, &faults)
        .unwrap();
    let staging = journal.staging_root().unwrap();
    let control = session
        .publish_control_file_recoverable(
            &locator.encode().unwrap(),
            &staging,
            None,
            |control| {
                journal
                    .append(
                        VfsEvent::ControlPrepared {
                            control: control.encode(),
                        },
                        &faults,
                    )
                    .map_err(layrs_vfs::Error::Io)
            },
            &layrs_vfs::NoControlPublishFaults,
        )
        .unwrap();
    journal
        .append(
            VfsEvent::LocatorPublished {
                control: control.encode(),
            },
            &faults,
        )
        .unwrap();
    if unsupported {
        locator
            .mark_ready(&client, &mut journal, &NoVfsWorktreeFaults)
            .unwrap();
    } else {
        accept_test_image(&client, locator.worktree_id(), &layer, &step, vec![]);
        journal = append_legacy_ready_fixture(journal, &locator);
    }
    drop(lease);
    LegacyMount {
        parent,
        workspace,
        locator,
        supported_step,
        session,
        journal,
    }
}

#[test]
#[ignore = "real CFAPI retained export-parent guards; explicit disposable parent required"]
fn export_parent_retains_sharing_guards_until_drop() {
    let mount = legacy_mount(false);
    let outer = mount.parent.join("ordinary-exports");
    let parent = outer.join("parent");
    std::fs::create_dir(&outer).unwrap();
    std::fs::create_dir(&parent).unwrap();
    let moved_parent = outer.join("moved-parent");
    let moved_outer = mount.parent.join("moved-ordinary-exports");
    let pinned = VfsExportParent::open(&parent, &mount.session).unwrap();
    let expected = CheckoutReader::open(&parent, None).unwrap().identity();
    pinned.verify().unwrap();

    // Both directories are outside the provider root: these failures must
    // come from the retained export capability, not a provider leaf guard.
    for (operation, result) in [
        ("rename parent", std::fs::rename(&parent, &moved_parent)),
        ("rename ancestor", std::fs::rename(&outer, &moved_outer)),
        ("delete empty parent", std::fs::remove_dir(&parent)),
    ] {
        let error = result.expect_err(operation);
        assert_eq!(error.raw_os_error(), Some(32), "{operation}: {error:?}");
    }
    pinned.verify().unwrap();
    assert_eq!(
        CheckoutReader::open(&parent, None).unwrap().identity(),
        expected
    );
    assert!(std::fs::read_dir(&parent).unwrap().next().is_none());
    assert!(!moved_parent.exists() && !moved_outer.exists());

    drop(pinned);
    // Positive controls prove the retained capability was the sharing
    // restriction and releases every affected handle when dropped.
    std::fs::rename(&parent, &moved_parent).unwrap();
    std::fs::rename(&moved_parent, &parent).unwrap();
    std::fs::rename(&outer, &moved_outer).unwrap();
    std::fs::rename(&moved_outer, &outer).unwrap();
    std::fs::remove_dir(&parent).unwrap();
}

#[test]
#[ignore = "real CFAPI export-parent identity interleave; explicit disposable parent required"]
fn export_parent_rejects_replacement_between_ordinary_pin_and_guard() {
    let mount = legacy_mount(false);
    let parent = mount.parent.join("replaceable-export-parent");
    let retained = mount.parent.join("retained-original-parent");
    std::fs::create_dir(&parent).unwrap();
    let absolute = std::path::absolute(&parent).unwrap();
    let paths: Vec<_> = absolute.ancestors().collect();
    let ordinary_pins: Vec<_> = paths
        .into_iter()
        .rev()
        .map(|path| CheckoutReader::open(path, None).unwrap())
        .collect();
    let original_identity = ordinary_pins.last().unwrap().identity();

    // Deterministically interleave after ordinary acquisition. These old
    // share-all pins allow replacement, so the new provider guard alone
    // proves a different directory and must not validate the old pins.
    std::fs::rename(&parent, &retained).unwrap();
    std::fs::create_dir(&parent).unwrap();
    std::fs::write(parent.join("replacement-sentinel"), b"unrelated bytes").unwrap();
    let guard = mount.session.guard_export_parent(&absolute).unwrap();
    assert_eq!(guard.root(), absolute);
    assert_ne!(guard.filesystem_identity(), original_identity);
    let error = match VfsExportParent::bind_guard(ordinary_pins, guard) {
        Ok(_) => panic!("new retained guard must not validate an old ordinary identity"),
        Err(error) => error,
    };
    assert!(
        matches!(
            &error,
            NativeClientError::InvalidState(
                "VFS export parent identity changed during acquisition"
            )
        ),
        "{error:?}"
    );
    assert_eq!(
        CheckoutReader::open(&retained, None).unwrap().identity(),
        original_identity
    );
    assert!(std::fs::read_dir(&retained).unwrap().next().is_none());
    assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(parent.join("replacement-sentinel")).unwrap(),
        b"unrelated bytes"
    );
    // The failed pairing consumes/drops its guard instead of leaking a
    // sharing restriction over the unrelated replacement directory.
    let moved = mount.parent.join("replacement-after-refusal");
    std::fs::rename(&parent, &moved).unwrap();
    std::fs::rename(&moved, &parent).unwrap();
}

fn write_legacy_user_bytes(root: &Path) {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "native_scm::vfs_worktree::tests::legacy_child_write",
            "--nocapture",
        ])
        .env("LAYRS_VFS_LEGACY_WRITE_ROOT", root)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "legacy writer child exited with {status}");
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("legacy writer child timed out");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
#[ignore = "helper invoked only by exact legacy activation recipe"]
fn legacy_child_write() {
    let root = PathBuf::from(std::env::var_os("LAYRS_VFS_LEGACY_WRITE_ROOT").unwrap());
    // The actual placeholder is opened by an external process; no ordinary
    // replacement or pre-filled source substitutes for this private write.
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(root.join("file"))
        .unwrap();
    output.write_all(b"legacy unaccepted user bytes").unwrap();
    output.sync_all().unwrap();
}

#[test]
#[ignore = "real CFAPI legacy activation fixture; explicit disposable parent required"]
fn legacy_ready_without_activation_allows_reopen_export_but_refuses_capture() {
    let mount = legacy_mount(false);
    let root = mount.locator.checkout_root().to_path_buf();
    let operation = {
        let store = mount.workspace.open_store().unwrap();
        NativeScmReader::new(&store)
            .unwrap()
            .worktree_active_operation(mount.locator.worktree_id())
            .unwrap()
            .unwrap()
    };
    write_legacy_user_bytes(&root);
    drop(mount.session);
    drop(mount.journal);
    for _ in 0..2 {
        let mut reopened = VfsWorktree::open(&root).unwrap();
        let status = reopened.status().unwrap();
        assert_eq!(status.active_operation_id, operation);
        assert_eq!(
            status.capture_blocked_reason,
            Some(VfsCaptureBlockedReason::UnprovenActivation)
        );
        assert_eq!(
            inspect_vfs_checkout(&root)
                .unwrap()
                .unwrap()
                .capture_blocked_reason,
            Some(VfsCaptureBlockedReason::UnprovenActivation)
        );
        assert!(
            matches!(reopened.capture_quiesced(), Err(NativeClientError::InvalidState(cause))
            if cause.contains("NeedsRepair"))
        );
        assert!(reopened.journal.state().pending_capture.is_none());
        assert!(reopened.journal.state().last_accepted.is_none());
        let exported = reopened.export_quiesced(&mount.parent).unwrap();
        assert_eq!(
            std::fs::read(exported.tree_root.join("file")).unwrap(),
            b"legacy unaccepted user bytes"
        );
    }
    assert_eq!(
        std::fs::read(root.join("file")).unwrap(),
        b"legacy unaccepted user bytes"
    );
}

#[test]
#[ignore = "real CFAPI historical unsupported source; explicit disposable parent required"]
fn unsupported_legacy_source_keeps_identity_overlap_checks_without_poisoning_creation() {
    let mount = legacy_mount(true);
    let root = mount.locator.checkout_root().to_path_buf();
    assert!(
        matches!(VerifiedVfsSnapshot::reopen(&mount.locator.source, &Cancellation::default()),
        Err(NativeClientError::InvalidState(cause)) if cause.contains("named empty directories"))
    );
    let store = mount.workspace.open_store().unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let worktree = reader
        .get_worktree(mount.locator.worktree_id())
        .unwrap()
        .unwrap();
    assert_eq!(
        verify_registered_vfs_root(&store, &worktree).unwrap(),
        worktree.filesystem_identity()
    );
    let probe = mount.parent.join("unregistered-valid-probe");
    std::fs::create_dir(&probe).unwrap();
    let mut verified = 0;
    scm_v1::worktrees::validate_secondary_root_with_vfs(&store, &probe, |registered| {
        if registered.id() == worktree.id() {
            verified += 1;
        }
        verify_registered_vfs_root(&store, registered).map_err(io::Error::other)
    })
    .unwrap();
    assert_eq!(
        verified, 1,
        "unrelated root must reach the VFS identity verifier"
    );
    assert!(
        scm_v1::worktrees::validate_secondary_root_with_vfs(&store, &root, |worktree| {
            verify_registered_vfs_root(&store, worktree).map_err(io::Error::other)
        })
        .is_err()
    );
    drop(reader);
    drop(store);
    assert!(matches!(inspect_vfs_checkout(&root),
        Err(NativeClientError::InvalidState(cause)) if cause.contains("named empty directories")));
    let ordinary = mount.parent.join("unrelated-ordinary");
    std::fs::create_dir(&ordinary).unwrap();
    mount
        .workspace
        .create_worktree(
            &ordinary,
            &mount.locator.creation.layer_id,
            &mount.supported_step,
        )
        .unwrap();
    let virtual_root = mount.parent.join("unrelated-virtual");
    std::fs::create_dir(&virtual_root).unwrap();
    let other = VfsWorktree::create(
        &mount.workspace,
        &virtual_root,
        &mount.locator.creation.layer_id,
        &mount.supported_step,
    )
    .unwrap();
    assert_eq!(other.source_read_counts(), (0, 0));
    drop(other);
    drop(mount.session);
    drop(mount.journal);
    assert!(VfsWorktree::open(&root).is_err());
    // Matching bytes in a new native file are not the recorded capability.
    std::fs::rename(root.join(".layrs"), mount.parent.join("retained-control")).unwrap();
    std::fs::write(root.join(".layrs"), mount.locator.encode().unwrap()).unwrap();
    let store = mount.workspace.open_store().unwrap();
    assert!(matches!(
        verify_registered_vfs_root(&store, &worktree),
        Err(NativeClientError::InvalidState(
            "VFS registered control differs from private journal"
        ))
    ));
    let mut verified = 0;
    let error = scm_v1::worktrees::validate_secondary_root_with_vfs(&store, &probe, |registered| {
        if registered.id() == worktree.id() {
            verified += 1;
        }
        verify_registered_vfs_root(&store, registered).map_err(io::Error::other)
    })
    .unwrap_err();
    assert_eq!(verified, 1);
    assert!(
        error
            .to_string()
            .contains("VFS registered control differs from private journal"),
        "{error}"
    );
    drop(store);
    // Nor may a new directory at the recorded pathname inherit registration.
    std::fs::rename(&root, mount.parent.join("retained-provider-root")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join(".layrs"), mount.locator.encode().unwrap()).unwrap();
    let store = mount.workspace.open_store().unwrap();
    let mut verified = 0;
    let error = scm_v1::worktrees::validate_secondary_root_with_vfs(&store, &probe, |registered| {
        if registered.id() == worktree.id() {
            verified += 1;
        }
        verify_registered_vfs_root(&store, registered).map_err(io::Error::other)
    })
    .unwrap_err();
    assert_eq!(verified, 1);
    assert!(
        error
            .to_string()
            .contains("verify registered provider identity")
            && error.to_string().contains("ForeignRoot"),
        "{error}"
    );
}

#[test]
fn metadata_catalog_boundary_counts_user_files_and_directories_only() {
    let root = std::env::temp_dir().join(format!("layrs-vfs-catalog-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let workspace = NativeWorkspace::initialize(&root, "Metadata catalog boundary").unwrap();
    // One shared empty FileManifest and two Trees describe the whole image;
    // no checkout leaf or provider placeholder is created by this test.
    for files in [99_999usize, 100_000] {
        let mut snapshot = SnapshotBuilder::new(SnapshotLimits::default());
        for index in 0..files {
            snapshot
                .add_file(
                    RepoPath::parse(format!("dir/f{index:06}")).unwrap(),
                    &[],
                    false,
                )
                .unwrap();
        }
        let client = workspace.client();
        let base = client.capture_base(workspace.worktree_id()).unwrap();
        let CapturePlan::Change(change) = client
            .prepare_capture(
                &base,
                snapshot.finish().unwrap(),
                MutationStamp::fresh().unwrap(),
            )
            .unwrap()
        else {
            panic!("new metadata image must change the Step");
        };
        client.accept(&change).unwrap();
        let state = workspace.state().unwrap();
        let binding = VerifiedVfsSnapshotBinding {
            store_root: workspace.store_root().to_path_buf(),
            store_id: state.worktree.store_id().clone(),
            store_identity: workspace.store_identity().into(),
            store_metadata_identity: workspace.metadata_identity().into(),
            space_id: state.space.id().clone(),
            operation_id: state.operation.id().clone(),
            step_id: change.step_id().clone(),
        };
        let store = workspace.open_store().unwrap();
        let catalog = metadata_catalog(&store, &binding);
        if files == 99_999 {
            assert_eq!(catalog.unwrap().entries().len(), files);
        } else {
            assert!(
                catalog.is_err(),
                "100,000 files plus their directory exceed the budget"
            );
        }
    }
    assert!(!root.join("dir").exists());
    drop(workspace);
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
