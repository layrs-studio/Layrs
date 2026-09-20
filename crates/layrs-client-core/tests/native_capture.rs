use layrs_client_core::native_scm::*;
use layrs_core::scm::*;
use layrs_store::scm::{CanonicalObject, ScmReader, StoreObjectId, TransactionId};
use layrs_store_local::scm_v1::{self, FsckOptions, NativeScmRepository};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    client: NativeClient,
    worktree: WorktreeId,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "layrs-native-client-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("checkout")).unwrap();
        fs::create_dir_all(root.join("checkout/src")).unwrap();
        fs::write(root.join("checkout/src/main.rs"), b"first").unwrap();
        fs::write(root.join("checkout/empty"), b"").unwrap();
        let (identity, _) =
            capture_checkout(root.join("checkout"), None, SnapshotLimits::default()).unwrap();
        let store = StoreId::parse("store:test").unwrap();
        drop(scm_v1::init(root.join("store"), store.clone()).unwrap());
        let client = NativeClient::open(root.join("store")).unwrap();
        let space = Space::new(
            SpaceId::parse("space:test").unwrap(),
            "Test",
            time(),
            Metadata::empty(),
        )
        .unwrap();
        let layer = Layer::new(
            LayerId::parse("layer:main").unwrap(),
            space.id().clone(),
            "Main",
            LayerKind::Base,
            LayerStatus::Active,
            vec![],
            LayerSettings::new(true, 500).unwrap(),
            time(),
            Metadata::empty(),
        )
        .unwrap();
        let worktree = Worktree::new(
            WorktreeId::parse("worktree:test").unwrap(),
            store,
            space.id().clone(),
            stamp("init").operation_id,
            root.join("checkout").to_str().unwrap(),
            identity,
            time(),
            Metadata::empty(),
        )
        .unwrap();
        let first = prepare_initialize(
            space,
            layer,
            worktree.clone(),
            snapshot(b"first"),
            stamp("init"),
        )
        .unwrap();
        assert!(!client.accept(&first).unwrap().was_replay());
        assert!(client.accept(&first).unwrap().was_replay());
        Self {
            root,
            client,
            worktree: worktree.id().clone(),
        }
    }
    fn repo(&self) -> NativeScmRepository {
        NativeScmRepository::new(self.root.join("store"))
    }
    fn capture(&self, label: &str, bytes: &[u8]) -> PreparedMutation {
        let base = self.client.capture_base(&self.worktree).unwrap();
        match self
            .client
            .prepare_capture(&base, snapshot(bytes), stamp(label))
            .unwrap()
        {
            CapturePlan::Change(change) => change,
            CapturePlan::Unchanged { .. } => panic!("expected changed snapshot"),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("preserved native capture fixture: {}", self.root.display());
        } else {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}
fn time() -> Timestamp {
    Timestamp::parse("2026-09-19T10:00:00Z").unwrap()
}
fn stamp(label: &str) -> MutationStamp {
    MutationStamp {
        transaction_id: TransactionId::new(format!("transaction:{label}")).unwrap(),
        operation_id: OperationId::parse(format!("operation:{label}")).unwrap(),
        observation_id: ObservationId::parse(format!("observation:{label}")).unwrap(),
        timestamp: time(),
    }
}
fn snapshot(bytes: &[u8]) -> PreparedSnapshot {
    let mut builder = SnapshotBuilder::new(SnapshotLimits::default());
    builder
        .add_file(RepoPath::parse("src/main.rs").unwrap(), bytes, false)
        .unwrap();
    builder
        .add_file(RepoPath::parse("empty").unwrap(), b"", false)
        .unwrap();
    builder.finish().unwrap()
}

#[test]
fn disk_capture_preserves_binary_empty_and_deletion_and_excludes_metadata() {
    let f = Fixture::new();
    fs::create_dir_all(f.root.join("checkout/.layrs")).unwrap();
    fs::write(f.root.join("checkout/.layrs/secret"), b"ignored").unwrap();
    assert!(matches!(
        f.client
            .prepare_disk_capture(&f.worktree, SnapshotLimits::default(), stamp("disk-noop"))
            .unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    fs::write(f.root.join("checkout/src/main.rs"), [0, 255, 10, 0]).unwrap();
    fs::remove_file(f.root.join("checkout/empty")).unwrap();
    let CapturePlan::Change(change) = f
        .client
        .prepare_disk_capture(&f.worktree, SnapshotLimits::default(), stamp("disk-change"))
        .unwrap()
    else {
        panic!("expected changed disk")
    };
    f.client.accept(&change).unwrap();
    assert!(matches!(
        f.client
            .prepare_disk_capture(&f.worktree, SnapshotLimits::default(), stamp("disk-reopen"))
            .unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    assert_eq!(
        fs::read(f.root.join("checkout/src/main.rs")).unwrap(),
        [0, 255, 10, 0]
    );
}

#[test]
fn disk_capture_rejects_replaced_checkout_and_limits_without_metadata_mutation() {
    let f = Fixture::new();
    let before = f.client.capture_base(&f.worktree).unwrap();
    let limits = SnapshotLimits {
        max_file_bytes: 2,
        ..SnapshotLimits::default()
    };
    assert!(f
        .client
        .prepare_disk_capture(&f.worktree, limits, stamp("too-large"))
        .is_err());
    fs::rename(f.root.join("checkout"), f.root.join("original-checkout")).unwrap();
    fs::create_dir(f.root.join("checkout")).unwrap();
    fs::write(f.root.join("checkout/new"), b"unrelated").unwrap();
    assert!(f
        .client
        .prepare_disk_capture(&f.worktree, SnapshotLimits::default(), stamp("identity"))
        .is_err());
    assert_eq!(f.client.capture_base(&f.worktree).unwrap(), before);
    assert_eq!(
        fs::read(f.root.join("original-checkout/src/main.rs")).unwrap(),
        b"first"
    );
}

#[cfg(unix)]
#[test]
fn disk_capture_never_follows_symlinks_and_preserves_executable_mode() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = Fixture::new();
    fs::write(f.root.join("outside"), b"outside bytes").unwrap();
    symlink("../outside", f.root.join("checkout/link")).unwrap();
    fs::set_permissions(
        f.root.join("checkout/src/main.rs"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let (_, snapshot) =
        capture_checkout(f.root.join("checkout"), None, SnapshotLimits::default()).unwrap();
    assert!(snapshot
        .objects()
        .iter()
        .any(|o| matches!(o, CanonicalObject::Symlink(_))));
    assert!(!snapshot
        .objects()
        .iter()
        .any(|o| matches!(o, CanonicalObject::ChunkRaw(c) if c.bytes() == b"outside bytes")));
}

#[test]
fn native_capture_is_durable_idempotent_and_noop_creates_no_operation() {
    let f = Fixture::new();
    let before = f
        .repo()
        .operation_heads(&SpaceId::parse("space:test").unwrap())
        .unwrap();
    assert!(matches!(
        f.client
            .prepare_capture(
                &f.client.capture_base(&f.worktree).unwrap(),
                snapshot(b"first"),
                stamp("noop")
            )
            .unwrap(),
        CapturePlan::Unchanged { .. }
    ));
    assert_eq!(
        f.repo()
            .operation_heads(&SpaceId::parse("space:test").unwrap())
            .unwrap(),
        before
    );
    assert!(f
        .repo()
        .get_operation(&stamp("noop").operation_id)
        .unwrap()
        .is_none());
    let change = f.capture("next", &[0, 255, 1, 0]);
    assert!(!f.client.accept(&change).unwrap().was_replay());
    let reopened = NativeClient::open(f.root.join("store")).unwrap();
    assert!(reopened.accept(&change).unwrap().was_replay());
    let repo = f.repo();
    let step = repo.get_step(change.step_id()).unwrap().unwrap();
    assert_eq!(step.parent_step_ids().len(), 1);
    let observation = repo
        .get_step_observation(&stamp("next").observation_id)
        .unwrap()
        .unwrap();
    assert_eq!(observation.step_id(), change.step_id());
    assert_eq!(
        repo.get_worktree(&f.worktree)
            .unwrap()
            .unwrap()
            .active_operation_id(),
        &stamp("next").operation_id
    );
    let report = scm_v1::fsck_existing(f.root.join("store"), FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:?}", report.findings);
    assert!(!f.root.join("store/client_documents").exists());
}

#[test]
fn competing_prepared_captures_never_overwrite_the_winner() {
    let f = Fixture::new();
    let left = f.capture("left", b"left");
    let right = f.capture("right", b"right");
    f.client.accept(&left).unwrap();
    assert_eq!(f.client.accept(&right).unwrap_err().code(), "cas_mismatch");
    let repo = f.repo();
    assert!(repo
        .get_operation(&stamp("right").operation_id)
        .unwrap()
        .is_none());
    assert_eq!(
        repo.operation_heads(&SpaceId::parse("space:test").unwrap())
            .unwrap(),
        vec![stamp("left").operation_id]
    );
    assert!(!repo
        .object_admitted_to_space(
            &StoreObjectId::Step(right.step_id().clone()),
            &SpaceId::parse("space:test").unwrap()
        )
        .unwrap());
    assert!(repo.get_step(left.step_id()).unwrap().is_some());
    assert_eq!(
        right.batch().transaction_id(),
        &stamp("right").transaction_id
    );
}

#[test]
fn capture_from_one_divergent_head_preserves_every_other_head() {
    use layrs_store::scm::*;
    let f = Fixture::new();
    let left = f.capture("left-fork", b"left");
    f.client.accept(&left).unwrap();
    let mut repo = f.repo();
    let left_step = repo.get_step(left.step_id()).unwrap().unwrap();
    let right_snapshot = snapshot(b"right");
    let right = Step::new(
        left_step.parent_step_ids().to_vec(),
        right_snapshot.root_tree_id().clone(),
    )
    .unwrap();
    let worktree = repo.get_worktree(&f.worktree).unwrap().unwrap();
    let active = repo
        .get_operation(worktree.active_operation_id())
        .unwrap()
        .unwrap();
    let old = repo
        .get_repository_view(active.resulting_view_id())
        .unwrap()
        .unwrap();
    let current = repo
        .current_repository_view(old.space_id())
        .unwrap()
        .unwrap();
    let view = RepositoryView::new(
        old.space_id().clone(),
        vec![LayerHeadSet::new(
            LayerId::parse("layer:main").unwrap(),
            vec![left.step_id().clone(), right.id().clone()],
        )
        .unwrap()],
        vec![],
        vec![],
        old.worktrees().to_vec(),
    )
    .unwrap();
    let stamp = stamp("receive-fork");
    let op = Operation::new(
        stamp.operation_id.clone(),
        old.space_id().clone(),
        vec![active.id().clone()],
        view.id().clone(),
        OperationKind::ReceiveRemoteState,
        "Receive concurrent head",
        vec![],
        None,
        None,
        stamp.timestamp,
        Metadata::empty(),
    )
    .unwrap();
    let mut objects = right_snapshot.objects().to_vec();
    objects.push(CanonicalObject::Step(right.clone()));
    objects.push(CanonicalObject::RepositoryView(view.clone()));
    let admissions = objects
        .iter()
        .map(|o| SpaceObjectAdmission::new(old.space_id().clone(), o.object_id()))
        .collect();
    let batch = WriteBatch::new(
        stamp.transaction_id,
        objects,
        vec![EntityRecord::Operation(op.clone())],
        admissions,
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    old.space_id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    old.space_id().clone(),
                    vec![active.id().clone()],
                    op.id().clone(),
                    vec![op.id().clone()],
                )
                .unwrap(),
            ),
            StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(
                    f.worktree.clone(),
                    active.id().clone(),
                    op.id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    repo.apply_batch(&batch).unwrap();
    let next = f.capture("after-divergence", b"left with new work");
    f.client.accept(&next).unwrap();
    let active = repo.get_worktree(&f.worktree).unwrap().unwrap();
    let active = repo
        .get_operation(active.active_operation_id())
        .unwrap()
        .unwrap();
    let latest = repo
        .get_repository_view(active.resulting_view_id())
        .unwrap()
        .unwrap();
    assert_eq!(latest.layer_heads()[0].step_ids().len(), 2);
    assert!(latest.layer_heads()[0].step_ids().contains(right.id()));
    assert!(latest.layer_heads()[0].step_ids().contains(next.step_id()));
    assert_eq!(
        repo.get_step(next.step_id())
            .unwrap()
            .unwrap()
            .parent_step_ids(),
        &[left.step_id().clone()]
    );
    assert!(
        scm_v1::fsck_existing(f.root.join("store"), FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn snapshot_order_is_canonical_and_repeated_bytes_are_deduplicated() {
    let mut a = SnapshotBuilder::new(SnapshotLimits::default());
    let mut b = SnapshotBuilder::new(SnapshotLimits::default());
    for path in ["z", "a/b"] {
        a.add_file(RepoPath::parse(path).unwrap(), b"same", true)
            .unwrap();
    }
    for path in ["a/b", "z"] {
        b.add_file(RepoPath::parse(path).unwrap(), b"same", true)
            .unwrap();
    }
    let (a, b) = (a.finish().unwrap(), b.finish().unwrap());
    assert_eq!(a.root_tree_id(), b.root_tree_id());
    assert_eq!(a.objects(), b.objects());
    assert_eq!(
        a.objects()
            .iter()
            .filter(|o| matches!(o, CanonicalObject::ChunkRaw(_)))
            .count(),
        1
    );
}

#[test]
fn duplicate_and_prefix_collisions_fail_without_changing_snapshot() {
    let mut a = SnapshotBuilder::new(SnapshotLimits::default());
    a.add_file(RepoPath::parse("a/b").unwrap(), b"one", false)
        .unwrap();
    assert!(a
        .add_file(RepoPath::parse("a/b").unwrap(), b"two", false)
        .is_err());
    assert!(a
        .add_file(RepoPath::parse("a").unwrap(), b"two", false)
        .is_err());
    assert!(a
        .add_file(RepoPath::parse("a/b/c").unwrap(), b"two", false)
        .is_err());
    let mut expected = SnapshotBuilder::new(SnapshotLimits::default());
    expected
        .add_file(RepoPath::parse("a/b").unwrap(), b"one", false)
        .unwrap();
    assert_eq!(
        a.finish().unwrap().objects(),
        expected.finish().unwrap().objects()
    );
}

#[test]
fn snapshot_limits_reserved_paths_and_symlinks_are_explicit() {
    let limits = SnapshotLimits {
        max_entries: 2,
        max_depth: 2,
        max_file_bytes: 4,
        max_total_bytes: 5,
    };
    let mut b = SnapshotBuilder::new(limits);
    assert!(b
        .add_file(RepoPath::parse(".layrs/x").unwrap(), b"", false)
        .is_err());
    assert!(b
        .add_file(RepoPath::parse(".GIT/x").unwrap(), b"", false)
        .is_err());
    assert!(b
        .add_file(RepoPath::parse("a/b/c").unwrap(), b"", false)
        .is_err());
    assert!(b
        .add_file(RepoPath::parse("a").unwrap(), b"12345", false)
        .is_err());
    b.add_symlink(RepoPath::parse("link").unwrap(), "../x")
        .unwrap();
    b.add_file(RepoPath::parse("a").unwrap(), b"x", false)
        .unwrap();
    assert!(b
        .add_file(RepoPath::parse("third").unwrap(), b"", false)
        .is_err());
    let snapshot = b.finish().unwrap();
    assert!(snapshot
        .objects()
        .iter()
        .any(|o| matches!(o, CanonicalObject::Symlink(s) if s.target() == "../x")));
}

#[test]
fn native_selectors_read_admitted_history_from_the_worktree_operation() {
    let f = Fixture::new();
    let selector = StepSelector::Layer {
        reference: ReferenceSelector::Name("Main".into()),
        head: None,
    };
    let before = f.client.resolve_step(&f.worktree, &selector, 20).unwrap();
    let change = f.capture("selected", b"changed");
    f.client.accept(&change).unwrap();
    let after = f.client.resolve_step(&f.worktree, &selector, 20).unwrap();
    assert_ne!(before.step_id, after.step_id);
    assert_eq!(&after.step_id, change.step_id());
    assert_eq!(after.source_operation_id, stamp("selected").operation_id);
    let old = f
        .client
        .resolve_step(&f.worktree, &StepSelector::Step(before.step_id.clone()), 20)
        .unwrap();
    assert_eq!(old.step_id, before.step_id);
    assert_eq!(old.repository_view_id, after.repository_view_id);
    assert!(f.client.resolve_step(&f.worktree, &selector, 1).is_err());
}

#[test]
fn native_selector_rejects_objects_not_admitted_to_the_space() {
    let f = Fixture::new();
    let foreign = Step::new(vec![], Tree::new(vec![]).unwrap().id().clone()).unwrap();
    let error = f
        .client
        .resolve_step(&f.worktree, &StepSelector::Step(foreign.id().clone()), 20)
        .unwrap_err();
    assert!(matches!(
        error,
        NativeClientError::Selector(layrs_graph::scm::SelectorError::OutsideSpace { .. })
    ));
}

#[test]
fn bytes_captured_against_a_stale_base_are_rejected_before_preparation() {
    let f = Fixture::new();
    let base_before_scan = f.client.capture_base(&f.worktree).unwrap();
    let old_snapshot = snapshot(b"scanned while another writer is active");
    let winner = f.capture("during-scan", b"newer");
    f.client.accept(&winner).unwrap();
    let error = f
        .client
        .prepare_capture(&base_before_scan, old_snapshot, stamp("stale-scan"))
        .unwrap_err();
    assert_eq!(error.code(), "stale_capture_base");
    assert!(f
        .repo()
        .get_operation(&stamp("stale-scan").operation_id)
        .unwrap()
        .is_none());
}
