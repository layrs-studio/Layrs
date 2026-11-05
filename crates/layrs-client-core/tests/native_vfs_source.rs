use layrs_client_core::native_scm::{
    MutationStamp, NativeWorkspace, VerifiedVfsSnapshot, VerifiedVfsSnapshotBinding,
};
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::NativeScmRepository;
use layrs_vfs::{Cancellation, SnapshotSource};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{mpsc, Arc},
    time::Duration,
};

fn current_step(workspace: &NativeWorkspace) -> StepId {
    workspace
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == workspace.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone()
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("layrs-vfs-source-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("retained {}", self.0.display());
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

#[derive(Default)]
struct CanonicalTrees(BTreeMap<StoreObjectId, CanonicalObject>);
impl CanonicalTrees {
    fn insert(&mut self, object: CanonicalObject) {
        self.0.insert(object.object_id(), object);
    }
    fn tree(&mut self, entries: Vec<(&str, TreeEntryTarget)>) -> TreeId {
        let tree = Tree::new(
            entries
                .into_iter()
                .map(|(name, target)| {
                    TreeEntry::new(RepoPathComponent::parse(name).unwrap(), target, false)
                })
                .collect(),
        )
        .unwrap();
        let id = tree.id().clone();
        self.insert(CanonicalObject::Tree(tree));
        id
    }
    fn file(&mut self) -> TreeEntryTarget {
        let manifest = FileManifest::empty(ChunkingDescriptor::single()).unwrap();
        let target = TreeEntryTarget::file(manifest.file_id().clone());
        self.insert(CanonicalObject::FileManifest(manifest));
        target
    }
    fn tombstone(&mut self) -> TreeEntryTarget {
        let tombstone = Tombstone::new(None).unwrap();
        let target = TreeEntryTarget::tombstone(tombstone.id().clone());
        self.insert(CanonicalObject::Tombstone(tombstone));
        target
    }
}

// SnapshotBuilder deliberately cannot express named empty directories. Publish
// canonical metadata through the real admission/CAS path to reproduce an exact
// retained Step that can arrive from another producer, without any checkout IO.
fn publish_canonical_trees(
    workspace: &NativeWorkspace,
    mut graph: CanonicalTrees,
    root: TreeId,
) -> VerifiedVfsSnapshotBinding {
    let state = workspace.state().unwrap();
    let old = state
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == workspace.worktree_id())
        .unwrap();
    let step = Step::new(vec![old.base_step_id().unwrap().clone()], root.clone()).unwrap();
    let layer = old.active_layer_id().unwrap();
    let view = RepositoryView::new(
        state.space.id().clone(),
        state
            .view
            .layer_heads()
            .iter()
            .map(|head| {
                if head.layer_id() == layer {
                    LayerHeadSet::new(layer.clone(), vec![step.id().clone()]).unwrap()
                } else {
                    head.clone()
                }
            })
            .collect(),
        state.view.marker_ids().to_vec(),
        state.view.release_ids().to_vec(),
        state
            .view
            .worktrees()
            .iter()
            .map(|entry| {
                if entry.worktree_id() == workspace.worktree_id() {
                    WorktreeViewState::new(
                        workspace.worktree_id().clone(),
                        Some(layer.clone()),
                        Some(step.id().clone()),
                        Some(root.clone()),
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
    let operation = Operation::new(
        stamp.operation_id.clone(),
        state.space.id().clone(),
        vec![state.operation.id().clone()],
        view.id().clone(),
        OperationKind::Restore,
        "Publish canonical VFS namespace fixture",
        vec![],
        None,
        None,
        stamp.timestamp,
        Metadata::empty(),
    )
    .unwrap();
    graph.insert(CanonicalObject::Step(step.clone()));
    graph.insert(CanonicalObject::RepositoryView(view.clone()));
    let objects: Vec<_> = graph.0.into_values().collect();
    let admissions = objects
        .iter()
        .map(|object| SpaceObjectAdmission::new(state.space.id().clone(), object.object_id()))
        .collect();
    let mut repository = NativeScmRepository::new(workspace.store_root())
        .with_store_identity(workspace.store_identity())
        .with_metadata_identity(workspace.metadata_identity())
        .with_store_id(state.worktree.store_id().clone());
    let current = repository
        .current_repository_view(state.space.id())
        .unwrap()
        .unwrap();
    let heads = repository.operation_heads(state.space.id()).unwrap();
    let batch = WriteBatch::new(
        stamp.transaction_id,
        objects,
        vec![EntityRecord::Operation(operation.clone())],
        admissions,
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    state.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    state.space.id().clone(),
                    heads,
                    operation.id().clone(),
                    vec![operation.id().clone()],
                )
                .unwrap(),
            ),
            StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(
                    workspace.worktree_id().clone(),
                    state.operation.id().clone(),
                    operation.id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    repository.apply_batch(&batch).unwrap();
    VerifiedVfsSnapshotBinding {
        store_root: workspace.store_root().to_path_buf(),
        store_id: state.worktree.store_id().clone(),
        store_identity: workspace.store_identity().into(),
        store_metadata_identity: workspace.metadata_identity().into(),
        space_id: state.space.id().clone(),
        operation_id: operation.id().clone(),
        step_id: step.id().clone(),
    }
}

#[test]
fn unrepresentable_trees_are_refused_before_vfs_creation_or_source_reopen() {
    for case in [
        "empty",
        "tombstones",
        "nested-with-sibling",
        "aliased-empty",
        "tombstone-root",
        "mixed-tombstone-root",
        "mixed-tombstone-directory",
        "executable-directory",
    ] {
        let fixture = Fixture::new();
        let workspace =
            NativeWorkspace::initialize(&fixture.0, "Exact unsupported VFS source").unwrap();
        let mut graph = CanonicalTrees::default();
        let empty = if case == "tombstones" {
            let deleted = graph.tombstone();
            graph.tree(vec![("deleted", deleted)])
        } else {
            graph.tree(vec![])
        };
        let root = match case {
            "executable-directory" => {
                let file = graph.file();
                let branch = graph.tree(vec![("file", file)]);
                let root = Tree::new(vec![TreeEntry::new(
                    RepoPathComponent::parse("named").unwrap(),
                    TreeEntryTarget::tree(branch),
                    true,
                )])
                .unwrap();
                let id = root.id().clone();
                graph.insert(CanonicalObject::Tree(root));
                id
            }
            "tombstone-root" => {
                let deleted = graph.tombstone();
                graph.tree(vec![("deleted", deleted)])
            }
            "mixed-tombstone-root" | "mixed-tombstone-directory" => {
                let deleted = graph.tombstone();
                let file = graph.file();
                let branch = graph.tree(vec![("deleted", deleted), ("file", file)]);
                if case == "mixed-tombstone-directory" {
                    graph.tree(vec![("named", TreeEntryTarget::tree(branch))])
                } else {
                    branch
                }
            }
            "nested-with-sibling" => {
                let file = graph.file();
                let branch = graph.tree(vec![
                    ("empty", TreeEntryTarget::tree(empty)),
                    ("file", file),
                ]);
                graph.tree(vec![("nested", TreeEntryTarget::tree(branch))])
            }
            "aliased-empty" => graph.tree(vec![
                ("a", TreeEntryTarget::tree(empty.clone())),
                ("b", TreeEntryTarget::tree(empty)),
            ]),
            _ => graph.tree(vec![("named", TreeEntryTarget::tree(empty))]),
        };
        let binding = publish_canonical_trees(&workspace, graph, root);
        let projection = workspace.read_step_projection(&binding.step_id).unwrap();
        assert_eq!(
            projection.len(),
            usize::from(matches!(
                case,
                "nested-with-sibling"
                    | "mixed-tombstone-root"
                    | "mixed-tombstone-directory"
                    | "executable-directory"
            ))
        );
        let unsupported_kind = if case.contains("tombstone") {
            "Tombstones"
        } else if case == "executable-directory" {
            "executable directory entries"
        } else {
            "named empty directories"
        };
        // Source inspection never scans, flattens or retargets this ordinary root.
        fs::write(fixture.0.join("unaccepted.txt"), b"retain these user bytes").unwrap();
        for outcome in [
            VerifiedVfsSnapshot::prepare(&workspace, &binding.step_id, &Cancellation::default()),
            VerifiedVfsSnapshot::reopen(&binding, &Cancellation::default()),
        ] {
            let error = match outcome {
                Err(error) => error,
                Ok(_) => panic!("accepted {case}"),
            };
            assert!(
                matches!(&error, layrs_client_core::native_scm::NativeClientError::InvalidState(message)
                    if message.starts_with("VFS snapshot unsupported:") && message.contains(unsupported_kind)),
                "{case}: {error}"
            );
        }
        #[cfg(windows)]
        {
            let destination = Fixture::new(); // A sibling, outside the ordinary Worktree.
            let store = workspace.open_store().unwrap();
            layrs_store_local::scm_v1::worktrees::validate_secondary_root(&store, &destination.0)
                .unwrap();
            drop(store);
            let registrations: Vec<_> = workspace
                .list_worktrees()
                .unwrap()
                .into_iter()
                .map(|worktree| worktree.id().clone())
                .collect();
            let journal = workspace.store_root().join("metadata/vfs-v1");
            assert!(!journal.exists());
            let state = workspace.state().unwrap();
            let layer = state
                .view
                .worktrees()
                .iter()
                .find(|entry| entry.worktree_id() == workspace.worktree_id())
                .unwrap()
                .active_layer_id()
                .unwrap();
            let outcome = layrs_client_core::native_scm::VfsWorktree::create(
                &workspace,
                &destination.0,
                layer,
                &binding.step_id,
            );
            let error = match outcome {
                Err(error) => error,
                Ok(_) => panic!("created {case}"),
            };
            assert!(
                matches!(&error, layrs_client_core::native_scm::NativeClientError::InvalidState(message)
                if message.starts_with("VFS snapshot unsupported:") && message.contains(unsupported_kind)),
                "{case}: {error}"
            );
            assert!(fs::read_dir(&destination.0).unwrap().next().is_none());
            assert!(!journal.exists());
            assert_eq!(
                workspace
                    .list_worktrees()
                    .unwrap()
                    .into_iter()
                    .map(|worktree| worktree.id().clone())
                    .collect::<Vec<_>>(),
                registrations
            );
        }
        assert_eq!(
            workspace.state().unwrap().operation.id(),
            &binding.operation_id
        );
        assert_eq!(
            fs::read(fixture.0.join("unaccepted.txt")).unwrap(),
            b"retain these user bytes"
        );
    }
}

#[test]
fn canonical_empty_root_and_nonempty_directory_aliases_remain_supported() {
    for case in ["empty-root", "nonempty-aliases"] {
        let fixture = Fixture::new();
        let workspace = NativeWorkspace::initialize(&fixture.0, "Supported VFS metadata").unwrap();
        let mut graph = CanonicalTrees::default();
        let root = match case {
            "nonempty-aliases" => {
                let file = graph.file();
                let branch = graph.tree(vec![("file", file)]);
                graph.tree(vec![
                    ("a", TreeEntryTarget::tree(branch.clone())),
                    ("b", TreeEntryTarget::tree(branch)),
                ])
            }
            _ => graph.tree(vec![]),
        };
        let binding = publish_canonical_trees(&workspace, graph, root);
        let source =
            VerifiedVfsSnapshot::prepare(&workspace, &binding.step_id, &Cancellation::default())
                .unwrap();
        let paths: Vec<_> = source
            .catalog()
            .entries()
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        if case == "nonempty-aliases" {
            assert_eq!(paths, ["a/file", "b/file"]);
        } else {
            assert!(paths.is_empty());
        }
        assert_eq!(source.read_counts(), (0, 0));
        assert_eq!(source.binding(), binding);
        VerifiedVfsSnapshot::reopen(&binding, &Cancellation::default()).unwrap();
    }
}

#[test]
fn exact_source_reads_only_intersecting_chunks_and_survives_worktree_advance() {
    let fixture = Fixture::new();
    let content: Vec<u8> = (0..2_300_000)
        .map(|n| ((n * 31 + n / 67) % 251) as u8)
        .collect();
    fs::write(fixture.0.join("asset.bin"), &content).unwrap();
    fs::write(fixture.0.join("empty"), []).unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "VFS source").unwrap();
    let step = current_step(&workspace); // Retention must not depend on an extra Marker.
    let source = VerifiedVfsSnapshot::prepare(&workspace, &step, &Cancellation::default()).unwrap();
    assert_eq!(source.catalog().entries().len(), 2);
    assert_eq!(source.read_counts(), (0, 0));
    let file = FileId::from_bytes(&content);
    let mut bytes = [0u8; 32];
    source
        .read_exact_at(&file, 1_048_570, &mut bytes, &Cancellation::default())
        .unwrap();
    assert_eq!(bytes, content[1_048_570..1_048_602]);
    let (chunks, size) = source.read_counts();
    assert_eq!(chunks, 2);
    assert_eq!(size, 2 * 1_048_576);
    fs::write(fixture.0.join("asset.bin"), b"private changed bytes").unwrap();
    workspace.capture().unwrap(); // A live source must not keep the Store shared lock.
    source
        .read_exact_at(&file, 200_000, &mut bytes, &Cancellation::default())
        .unwrap();
    assert_eq!(bytes, content[200_000..200_032]);
    source
        .read_exact_at(
            &FileId::from_bytes(&[]),
            0,
            &mut [],
            &Cancellation::default(),
        )
        .unwrap();
    assert_eq!(source.step_id(), &step);
    let report =
        layrs_store_local::scm_v1::fsck_existing(workspace.store_root(), Default::default())
            .unwrap();
    assert!(report.is_clean());
    assert!(!report
        .gc_reachability
        .unreachable_objects
        .contains(&step.to_string()));
    let reopened =
        VerifiedVfsSnapshot::reopen(&source.binding(), &Cancellation::default()).unwrap();
    reopened
        .read_exact_at(&file, 200_000, &mut bytes, &Cancellation::default())
        .unwrap();
    assert_eq!(bytes, content[200_000..200_032]);
}

#[test]
fn invalid_ranges_missing_files_and_cancellation_never_succeed() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("file"), b"base").unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "VFS bounds").unwrap();
    let marker = workspace.create_marker("base", "").unwrap();
    let cancel = Cancellation::default();
    let source =
        VerifiedVfsSnapshot::prepare(&workspace, marker.target_step_id(), &cancel).unwrap();
    let mut output = [77; 8];
    assert!(source
        .read_exact_at(&FileId::from_bytes(b"base"), 0, &mut output, &cancel)
        .is_err());
    assert!(source
        .read_exact_at(&FileId::from_bytes(b"base"), u64::MAX, &mut output, &cancel)
        .is_err());
    assert!(source
        .read_exact_at(&FileId::from_bytes(b"missing"), 0, &mut output, &cancel)
        .is_err());
    assert_eq!(output, [77; 8]);
    assert_eq!(source.read_counts(), (0, 0));
    cancel.cancel();
    assert!(source
        .read_exact_at(&FileId::from_bytes(b"base"), 0, &mut output[..1], &cancel)
        .is_err());
    assert!(VerifiedVfsSnapshot::prepare(&workspace, marker.target_step_id(), &cancel).is_err());
    assert_eq!(source.read_counts(), (0, 0));
}

#[test]
fn source_rejects_another_store_step_and_unsupported_namespace_before_mount() {
    let a = Fixture::new();
    let b = Fixture::new();
    fs::write(a.0.join("ascii"), b"a").unwrap();
    fs::write(b.0.join("accent-é"), b"b").unwrap();
    let first = NativeWorkspace::initialize(&a.0, "A").unwrap();
    let second = NativeWorkspace::initialize(&b.0, "B").unwrap();
    let marker = second.create_marker("B", "").unwrap();
    assert!(VerifiedVfsSnapshot::prepare(
        &first,
        marker.target_step_id(),
        &Cancellation::default()
    )
    .is_err());
    assert!(VerifiedVfsSnapshot::prepare(
        &second,
        marker.target_step_id(),
        &Cancellation::default()
    )
    .is_err());
}

#[test]
fn corruption_after_preparation_is_not_served_as_verified_content() {
    let fixture = Fixture::new();
    let content = b"original asset";
    fs::write(fixture.0.join("asset.bin"), content).unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "Corruption").unwrap();
    let marker = workspace.create_marker("source", "").unwrap();
    let source = VerifiedVfsSnapshot::prepare(
        &workspace,
        marker.target_step_id(),
        &Cancellation::default(),
    )
    .unwrap();
    let chunk = layrs_core::scm::Chunk::new(content.to_vec()).unwrap();
    let path = {
        let store = workspace.open_store().unwrap();
        layrs_store_local::scm_v1::cas::NativeCas::open(&store)
            .unwrap()
            .object_path(&layrs_store::scm::StoreObjectId::ChunkRaw(
                chunk.id().clone(),
            ))
            .unwrap()
    };
    // Same length corruption must fail canonical verification, not merely a size check.
    fs::write(&path, vec![b'!'; content.len()]).unwrap();
    assert!(source
        .read_exact_at(
            &FileId::from_bytes(content),
            0,
            &mut [0u8; 4],
            &Cancellation::default()
        )
        .is_err());
    // Preserve the corrupt object as evidence while making its canonical path missing.
    fs::rename(&path, path.with_extension("missing-test")).unwrap();
    assert!(source
        .read_exact_at(
            &FileId::from_bytes(content),
            0,
            &mut [0u8; 4],
            &Cancellation::default()
        )
        .is_err());
}

#[test]
fn range_read_waits_for_concurrent_writer_and_can_cancel_while_waiting() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("file"), b"base").unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "Contended source").unwrap();
    let source = Arc::new(
        VerifiedVfsSnapshot::prepare(
            &workspace,
            &current_step(&workspace),
            &Cancellation::default(),
        )
        .unwrap(),
    );
    for should_cancel in [false, true] {
        // Hold the exact exclusive capability used by repository publication.
        let writer = layrs_store_local::scm_v1::open_exclusive(workspace.store_root()).unwrap();
        let cancel = Cancellation::default();
        let worker_cancel = cancel.clone();
        let worker_source = Arc::clone(&source);
        let (started, ready) = mpsc::channel();
        let (finished, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            let mut bytes = [77; 4];
            let outcome = worker_source.read_exact_at(
                &FileId::from_bytes(b"base"),
                0,
                &mut bytes,
                &worker_cancel,
            );
            finished.send((outcome, bytes)).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        if should_cancel {
            cancel.cancel();
            let (outcome, bytes) = result.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(matches!(outcome, Err(layrs_vfs::Error::Cancelled)));
            assert_eq!(bytes, [77; 4]);
            drop(writer);
        } else {
            drop(writer);
            let (outcome, bytes) = result.recv_timeout(Duration::from_secs(5)).unwrap();
            outcome.unwrap();
            assert_eq!(&bytes, b"base");
        }
        worker.join().unwrap();
    }
}

#[test]
fn prepared_source_and_serialized_binding_reject_store_replacement() {
    fn copy_directory(source: &std::path::Path, destination: &std::path::Path) {
        fs::create_dir(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_directory(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let fixture = Fixture::new();
    fs::write(fixture.0.join("file"), b"base").unwrap();
    let workspace = NativeWorkspace::initialize(&fixture.0, "Bound source").unwrap();
    let source = VerifiedVfsSnapshot::prepare(
        &workspace,
        &current_step(&workspace),
        &Cancellation::default(),
    )
    .unwrap();
    let binding = serde_json::from_slice(&serde_json::to_vec(&source.binding()).unwrap()).unwrap();
    let replacement = fixture.0.join("replacement-store");
    copy_directory(workspace.store_root(), &replacement);
    fs::rename(workspace.store_root(), fixture.0.join("original-store")).unwrap();
    fs::rename(replacement, workspace.store_root()).unwrap();
    assert!(VerifiedVfsSnapshot::reopen(&binding, &Cancellation::default()).is_err());
    assert!(source
        .read_exact_at(
            &FileId::from_bytes(b"base"),
            0,
            &mut [0; 4],
            &Cancellation::default()
        )
        .is_err());
}
