use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{self, NativeScmRepository};
use std::{fs, path::PathBuf, sync::Barrier};

struct Fixture(PathBuf, NativeWorkspace);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("layrs-concurrent-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), b"original").unwrap();
        let workspace = NativeWorkspace::initialize(&root, "Concurrent").unwrap();
        Self(root, workspace)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("preserved concurrency fixture {}", self.0.display());
        } else {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

fn prepare(client: &NativeClient, worktree: &WorktreeId, contents: &[u8]) -> PreparedMutation {
    let mut snapshot = SnapshotBuilder::new(SnapshotLimits::default());
    snapshot
        .add_file(RepoPath::parse("file").unwrap(), contents, false)
        .unwrap();
    let base = client.capture_base(worktree).unwrap();
    match client
        .prepare_capture(
            &base,
            snapshot.finish().unwrap(),
            MutationStamp::fresh().unwrap(),
        )
        .unwrap()
    {
        CapturePlan::Change(change) => change,
        _ => panic!("expected capture change"),
    }
}

fn race(root: &std::path::Path, changes: &[PreparedMutation; 2]) -> usize {
    let barrier = Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let jobs: Vec<_> = changes
            .iter()
            .map(|change| {
                let barrier = &barrier;
                scope.spawn(move || {
                    let client = NativeClient::open(root).unwrap();
                    barrier.wait();
                    client.accept(change)
                })
            })
            .collect();
        jobs.into_iter()
            .map(|job| job.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let winner = results.iter().position(|result| result.is_ok()).unwrap();
    assert_eq!(
        results[1 - winner].as_ref().unwrap_err().code(),
        "cas_mismatch"
    );
    winner
}

#[test]
fn simultaneous_writers_same_worktree_have_one_winner_and_preserve_both_snapshots() {
    let fixture = Fixture::new();
    let client = NativeClient::open(fixture.1.store_root()).unwrap();
    let changes = [
        prepare(&client, fixture.1.worktree_id(), b"left"),
        prepare(&client, fixture.1.worktree_id(), b"right"),
    ];
    let winner = race(fixture.1.store_root(), &changes);
    let loser = 1 - winner;
    let repo = NativeScmRepository::new(fixture.1.store_root());
    assert!(repo
        .object_admitted_to_space(
            &StoreObjectId::Step(changes[winner].step_id().clone()),
            fixture.1.state().unwrap().space.id()
        )
        .unwrap());
    assert!(!repo
        .object_admitted_to_space(
            &StoreObjectId::Step(changes[loser].step_id().clone()),
            fixture.1.state().unwrap().space.id()
        )
        .unwrap());
    assert_eq!(fixture.1.operations(20).unwrap().len(), 2);
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"original");
    // Caller retains the losing snapshot and must explicitly prepare against
    // the winner; silently applying the stale batch is never permitted.
    let retry = prepare(
        &client,
        fixture.1.worktree_id(),
        if loser == 0 { b"left" } else { b"right" },
    );
    client.accept(&retry).unwrap();
    let step = repo.get_step(retry.step_id()).unwrap().unwrap();
    assert_eq!(step.parent_step_ids(), &[changes[winner].step_id().clone()]);
    assert_eq!(fixture.1.operations(20).unwrap().len(), 3);
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"original");
    assert!(
        scm_v1::fsck_existing(fixture.1.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

fn attach_second_worktree(fixture: &Fixture) -> WorktreeId {
    let source = fixture.1.state().unwrap();
    let checkout = fixture.0.join("second-checkout");
    fs::create_dir(&checkout).unwrap();
    fs::write(checkout.join("file"), b"second local work").unwrap();
    let (identity, _) = capture_checkout(&checkout, None, SnapshotLimits::default()).unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let worktree = Worktree::new(
        WorktreeId::parse("worktree:second").unwrap(),
        source.worktree.store_id().clone(),
        source.space.id().clone(),
        stamp.operation_id.clone(),
        checkout.to_str().unwrap(),
        identity,
        stamp.timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let first = &source.view.worktrees()[0];
    let second = WorktreeViewState::new(
        worktree.id().clone(),
        first.active_layer_id().cloned(),
        first.base_step_id().cloned(),
        first.materialized_tree_id().cloned(),
        WorktreeStatus::Ready,
    );
    let view = RepositoryView::new(
        source.space.id().clone(),
        source.view.layer_heads().to_vec(),
        vec![],
        vec![],
        vec![first.clone(), second],
    )
    .unwrap();
    let operation = Operation::new(
        stamp.operation_id.clone(),
        source.space.id().clone(),
        vec![source.operation.id().clone()],
        view.id().clone(),
        OperationKind::Restore,
        "Attach test Worktree",
        vec![],
        None,
        None,
        stamp.timestamp,
        Metadata::empty(),
    )
    .unwrap();
    let mut repo = NativeScmRepository::new(fixture.1.store_root());
    let current = repo
        .current_repository_view(source.space.id())
        .unwrap()
        .unwrap();
    let batch = WriteBatch::new(
        stamp.transaction_id,
        vec![CanonicalObject::RepositoryView(view.clone())],
        vec![
            EntityRecord::Worktree(worktree.clone()),
            EntityRecord::Operation(operation.clone()),
        ],
        vec![SpaceObjectAdmission::new(
            source.space.id().clone(),
            StoreObjectId::RepositoryView(view.id().clone()),
        )],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    source.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    source.space.id().clone(),
                    vec![source.operation.id().clone()],
                    operation.id().clone(),
                    vec![operation.id().clone()],
                )
                .unwrap(),
            ),
            StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(
                    fixture.1.worktree_id().clone(),
                    source.operation.id().clone(),
                    operation.id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    repo.apply_batch(&batch).unwrap();
    worktree.id().clone()
}

#[test]
fn two_worktrees_same_space_never_overwrite_each_others_operation_or_files() {
    let fixture = Fixture::new();
    let initial = fixture.1.state().unwrap().operation.id().clone();
    let second = attach_second_worktree(&fixture);
    let attached = fixture.1.state().unwrap().operation.id().clone();
    assert!(fixture.1.preview_restore_operation(&initial).is_err());
    assert_eq!(fixture.1.state().unwrap().operation.id(), &attached);
    let client = NativeClient::open(fixture.1.store_root()).unwrap();
    let worktrees = [fixture.1.worktree_id().clone(), second];
    let changes = [
        prepare(&client, &worktrees[0], b"first prepared"),
        prepare(&client, &worktrees[1], b"second prepared"),
    ];
    let winner = race(fixture.1.store_root(), &changes);
    let loser = 1 - winner;
    let repo = NativeScmRepository::new(fixture.1.store_root());
    let winning_worktree = repo.get_worktree(&worktrees[winner]).unwrap().unwrap();
    let losing_worktree = repo.get_worktree(&worktrees[loser]).unwrap().unwrap();
    assert_ne!(
        winning_worktree.active_operation_id(),
        losing_worktree.active_operation_id()
    );
    assert_eq!(
        repo.operation_heads(winning_worktree.space_id()).unwrap(),
        vec![winning_worktree.active_operation_id().clone()]
    );
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"original");
    assert_eq!(
        fs::read(fixture.0.join("second-checkout/file")).unwrap(),
        b"second local work"
    );
    let base = client.capture_base(&worktrees[loser]).unwrap();
    let snapshot = SnapshotBuilder::new(SnapshotLimits::default())
        .finish()
        .unwrap();
    let CapturePlan::Change(retried) = client
        .prepare_capture(&base, snapshot, MutationStamp::fresh().unwrap())
        .unwrap()
    else {
        panic!("independent Worktree can prepare against its own active View");
    };
    client.accept(&retried).unwrap();
    let heads = repo.operation_heads(winning_worktree.space_id()).unwrap();
    assert_eq!(heads.len(), 2);
    assert!(heads.contains(winning_worktree.active_operation_id()));
    assert!(heads.contains(
        repo.get_worktree(&worktrees[loser])
            .unwrap()
            .unwrap()
            .active_operation_id()
    ));
    assert!(!repo
        .object_admitted_to_space(
            &StoreObjectId::Step(changes[loser].step_id().clone()),
            winning_worktree.space_id()
        )
        .unwrap());
    assert!(
        scm_v1::fsck_existing(fixture.1.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn guarded_restore_multihead_view_preserves_divergence_and_undo_recovers_dirty_work() {
    let fixture = Fixture::new();
    let workspace = &fixture.1;
    workspace.create_layer("topic").unwrap();
    workspace.switch_layer("topic").unwrap();
    fs::write(fixture.0.join("file"), b"right").unwrap();
    workspace.capture().unwrap();
    let right = workspace
        .create_marker("right", "")
        .unwrap()
        .target_step_id()
        .clone();
    workspace.switch_layer("main").unwrap();
    fs::write(fixture.0.join("file"), b"left").unwrap();
    workspace.capture().unwrap();
    let source = workspace.state().unwrap();
    let state = &source.view.worktrees()[0];
    let active = state.active_layer_id().unwrap();
    let left = state.base_step_id().unwrap().clone();
    let heads = source
        .view
        .layer_heads()
        .iter()
        .map(|heads| {
            if heads.layer_id() == active {
                LayerHeadSet::new(active.clone(), vec![left.clone(), right.clone()]).unwrap()
            } else {
                heads.clone()
            }
        })
        .collect();
    let view = RepositoryView::new(
        source.space.id().clone(),
        heads,
        source.view.marker_ids().to_vec(),
        source.view.release_ids().to_vec(),
        source.view.worktrees().to_vec(),
    )
    .unwrap();
    let stamp = MutationStamp::fresh().unwrap();
    let operation = Operation::new(
        stamp.operation_id,
        source.space.id().clone(),
        vec![source.operation.id().clone()],
        view.id().clone(),
        OperationKind::ReceiveRemoteState,
        "Receive divergent test head",
        vec![],
        None,
        None,
        stamp.timestamp,
        Metadata::empty(),
    )
    .unwrap();
    let mut repo = NativeScmRepository::new(workspace.store_root());
    let current = repo
        .current_repository_view(source.space.id())
        .unwrap()
        .unwrap();
    repo.apply_batch(
        &WriteBatch::new(
            stamp.transaction_id,
            vec![CanonicalObject::RepositoryView(view.clone())],
            vec![EntityRecord::Operation(operation.clone())],
            vec![SpaceObjectAdmission::new(
                source.space.id().clone(),
                StoreObjectId::RepositoryView(view.id().clone()),
            )],
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(
                        source.space.id().clone(),
                        current.generation(),
                        current.view_id().cloned(),
                        view.id().clone(),
                    )
                    .unwrap(),
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        source.space.id().clone(),
                        vec![source.operation.id().clone()],
                        operation.id().clone(),
                        vec![operation.id().clone()],
                    )
                    .unwrap(),
                ),
                StoreMutation::WorktreeActiveOperation(
                    WorktreeActiveOperationCas::new(
                        workspace.worktree_id().clone(),
                        source.operation.id().clone(),
                        operation.id().clone(),
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    fs::write(fixture.0.join("file"), b"left changed").unwrap();
    workspace.capture().unwrap();
    fs::write(fixture.0.join("uncommitted"), b"dirty bytes").unwrap();
    let before = workspace.state().unwrap().operation.id().clone();
    let preview = workspace.preview_restore_operation(operation.id()).unwrap();
    assert_eq!(workspace.state().unwrap().operation.id(), &before);
    workspace
        .restore_operation_guarded(operation.id(), &preview.expected)
        .unwrap();
    let restored = workspace.state().unwrap();
    assert_eq!(restored.view.layer_heads(), view.layer_heads());
    assert_eq!(restored.view.marker_ids(), view.marker_ids());
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"left");
    assert!(!fixture.0.join("uncommitted").exists());
    workspace.undo().unwrap();
    assert_eq!(fs::read(fixture.0.join("file")).unwrap(), b"left changed");
    assert_eq!(
        fs::read(fixture.0.join("uncommitted")).unwrap(),
        b"dirty bytes"
    );
    assert!(workspace
        .state()
        .unwrap()
        .view
        .layer_heads()
        .iter()
        .find(|heads| heads.layer_id() == active)
        .unwrap()
        .step_ids()
        .contains(&right));
    assert!(
        scm_v1::fsck_existing(workspace.store_root(), scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}
