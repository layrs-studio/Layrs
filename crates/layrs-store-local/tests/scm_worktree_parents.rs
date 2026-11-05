//! Retained-parent authority comes from durable Worktree state, never a claim
//! in the candidate batch. Fixtures are fresh isolated native Stores.
use layrs_core::scm::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1::{self, NativeScmRepository};
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    space: SpaceId,
    a: WorktreeId,
    b: WorktreeId,
    initial: Operation,
    view: RepositoryView,
}
fn time() -> Timestamp {
    Timestamp::parse("2026-07-20T00:00:00Z").unwrap()
}
fn operation(
    name: &str,
    space: &SpaceId,
    parent: Option<&OperationId>,
    view: &RepositoryView,
    kind: OperationKind,
) -> Operation {
    Operation::new(
        OperationId::parse(format!("operation:{name}")).unwrap(),
        space.clone(),
        parent.cloned().into_iter().collect(),
        view.id().clone(),
        kind,
        name,
        vec![],
        None,
        None,
        time(),
        Metadata::empty(),
    )
    .unwrap()
}
impl Fixture {
    fn new() -> Self {
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(format!("layrs-worktree-parent-{suffix}"));
        let store_id = StoreId::parse("store:worktree-parent").unwrap();
        drop(scm_v1::init(&root, store_id.clone()).unwrap());
        let space = Space::new(
            SpaceId::parse("space:parent").unwrap(),
            "Parent",
            time(),
            Metadata::empty(),
        )
        .unwrap();
        let a = WorktreeId::parse("worktree:a").unwrap();
        let b = WorktreeId::parse("worktree:b").unwrap();
        let view = RepositoryView::new(
            space.id().clone(),
            vec![],
            vec![],
            vec![],
            vec![
                WorktreeViewState::new(a.clone(), None, None, None, WorktreeStatus::Ready),
                WorktreeViewState::new(b.clone(), None, None, None, WorktreeStatus::Ready),
            ],
        )
        .unwrap();
        let initial = operation(
            "initial",
            space.id(),
            None,
            &view,
            OperationKind::Initialize,
        );
        let records = vec![
            EntityRecord::Space(space.clone()),
            EntityRecord::Operation(initial.clone()),
            EntityRecord::Worktree(
                Worktree::new(
                    a.clone(),
                    store_id.clone(),
                    space.id().clone(),
                    initial.id().clone(),
                    root.join("a").to_str().unwrap(),
                    "fs:a",
                    time(),
                    Metadata::empty(),
                )
                .unwrap(),
            ),
            EntityRecord::Worktree(
                Worktree::new(
                    b.clone(),
                    store_id,
                    space.id().clone(),
                    initial.id().clone(),
                    root.join("b").to_str().unwrap(),
                    "fs:b",
                    time(),
                    Metadata::empty(),
                )
                .unwrap(),
            ),
        ];
        let batch = WriteBatch::new(
            TransactionId::new("transaction:initial").unwrap(),
            vec![CanonicalObject::RepositoryView(view.clone())],
            records,
            vec![SpaceObjectAdmission::new(
                space.id().clone(),
                StoreObjectId::RepositoryView(view.id().clone()),
            )],
            vec![
                StoreMutation::RepositoryView(
                    RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
                ),
                StoreMutation::OperationHeads(
                    OperationHeadsCas::new(
                        space.id().clone(),
                        vec![],
                        initial.id().clone(),
                        vec![initial.id().clone()],
                    )
                    .unwrap(),
                ),
            ],
        )
        .unwrap();
        NativeScmRepository::new(&root).apply_batch(&batch).unwrap();
        Self {
            root,
            space: space.id().clone(),
            a,
            b,
            initial,
            view,
        }
    }
    fn repo(&self) -> NativeScmRepository {
        NativeScmRepository::new(&self.root)
    }
    fn batch(
        &self,
        op: &Operation,
        view: &RepositoryView,
        targets: &[(WorktreeId, OperationId)],
        source: Option<WorktreeId>,
        mut extras: Vec<EntityRecord>,
    ) -> layrs_store::scm::Result<WriteBatch> {
        let repo = self.repo();
        let current = repo.current_repository_view(&self.space)?.unwrap();
        let heads = repo.operation_heads(&self.space)?;
        let mut next = heads
            .iter()
            .filter(|id| !op.parent_operation_ids().contains(id))
            .cloned()
            .collect::<Vec<_>>();
        next.push(op.id().clone());
        next.sort();
        let mut heads = OperationHeadsCas::new(self.space.clone(), heads, op.id().clone(), next)?;
        if let Some(source) = source {
            heads = heads.with_worktree_registration_source(source);
        }
        let mut mutations = vec![
            StoreMutation::RepositoryView(RepositoryViewCas::new(
                self.space.clone(),
                current.generation(),
                current.view_id().cloned(),
                view.id().clone(),
            )?),
            StoreMutation::OperationHeads(heads),
        ];
        for (id, expected) in targets {
            mutations.push(StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(id.clone(), expected.clone(), op.id().clone())?,
            ));
        }
        extras.push(EntityRecord::Operation(op.clone()));
        WriteBatch::new(
            TransactionId::new(format!("transaction:{}", op.id()))?,
            vec![CanonicalObject::RepositoryView(view.clone())],
            extras,
            vec![SpaceObjectAdmission::new(
                self.space.clone(),
                StoreObjectId::RepositoryView(view.id().clone()),
            )],
            mutations,
        )
    }
    fn advance(&self, name: &str, id: &WorktreeId, parent: &OperationId) -> Operation {
        let op = operation(
            name,
            &self.space,
            Some(parent),
            &self.view,
            OperationKind::CaptureWorktree,
        );
        self.repo()
            .apply_batch(
                &self
                    .batch(
                        &op,
                        &self.view,
                        &[(id.clone(), parent.clone())],
                        None,
                        vec![],
                    )
                    .unwrap(),
            )
            .unwrap();
        op
    }
    fn creation(
        &self,
        name: &str,
        parent: &OperationId,
        status: WorktreeStatus,
    ) -> (Operation, RepositoryView, Worktree) {
        let id = WorktreeId::parse(format!("worktree:{name}")).unwrap();
        let mut states = self.view.worktrees().to_vec();
        states.push(WorktreeViewState::new(id.clone(), None, None, None, status));
        let view = RepositoryView::new(self.space.clone(), vec![], vec![], vec![], states).unwrap();
        let op = operation(
            name,
            &self.space,
            Some(parent),
            &view,
            OperationKind::Restore,
        );
        let worktree = Worktree::new(
            id,
            StoreId::parse("store:worktree-parent").unwrap(),
            self.space.clone(),
            op.id().clone(),
            self.root.join(name).to_str().unwrap(),
            format!("fs:{name}"),
            time(),
            Metadata::empty(),
        )
        .unwrap();
        (op, view, worktree)
    }
    fn reject(&self, op: &Operation, batch: &WriteBatch) {
        let mut repo = self.repo();
        let heads = repo.operation_heads(&self.space).unwrap();
        let a = repo.get_worktree(&self.a).unwrap();
        let b = repo.get_worktree(&self.b).unwrap();
        assert!(
            repo.apply_batch(batch).is_err(),
            "forged publication accepted: {}",
            op.id()
        );
        assert!(repo.get_operation(op.id()).unwrap().is_none());
        assert_eq!(repo.operation_heads(&self.space).unwrap(), heads);
        assert_eq!(repo.get_worktree(&self.a).unwrap(), a);
        assert_eq!(repo.get_worktree(&self.b).unwrap(), b);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() && self.root.parent() == Some(std::env::temp_dir().as_path()) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
}

#[test]
fn durable_own_active_nonhead_advances_without_consuming_another_worktree() {
    let f = Fixture::new();
    let a = f.advance("a1", &f.a, f.initial.id());
    let b = f.advance("b1", &f.b, f.initial.id());
    let repo = f.repo();
    let heads = repo.operation_heads(&f.space).unwrap();
    assert_eq!(heads.len(), 2);
    assert!(heads.contains(a.id()) && heads.contains(b.id()));
    assert_eq!(
        repo.get_worktree(&f.a)
            .unwrap()
            .unwrap()
            .active_operation_id(),
        a.id()
    );
    assert_eq!(
        repo.get_worktree(&f.b)
            .unwrap()
            .unwrap()
            .active_operation_id(),
        b.id()
    );
    assert!(
        scm_v1::fsck_existing(&f.root, scm_v1::FsckOptions::default())
            .unwrap()
            .is_clean()
    );
}

#[test]
fn retained_parent_rejects_missing_wrong_and_extra_worktree_cas() {
    let f = Fixture::new();
    let a = f.advance("a1", &f.a, f.initial.id());
    let op = operation(
        "missing-cas",
        &f.space,
        Some(f.initial.id()),
        &f.view,
        OperationKind::CaptureWorktree,
    );
    assert!(f.batch(&op, &f.view, &[], None, vec![]).is_err());
    let op = operation(
        "wrong-cas",
        &f.space,
        Some(f.initial.id()),
        &f.view,
        OperationKind::CaptureWorktree,
    );
    f.reject(
        &op,
        &f.batch(
            &op,
            &f.view,
            &[(f.a.clone(), f.initial.id().clone())],
            None,
            vec![],
        )
        .unwrap(),
    );
    let op = operation(
        "extra-cas",
        &f.space,
        Some(f.initial.id()),
        &f.view,
        OperationKind::CaptureWorktree,
    );
    assert!(matches!(
        f.batch(
            &op,
            &f.view,
            &[
                (f.b.clone(), f.initial.id().clone()),
                (f.a.clone(), a.id().clone()),
            ],
            None,
            vec![],
        ),
        Err(StoreError::InvalidOperationHeadsTransition { .. })
    ));
    let op = operation(
        "missing-worktree",
        &f.space,
        Some(f.initial.id()),
        &f.view,
        OperationKind::CaptureWorktree,
    );
    f.reject(
        &op,
        &f.batch(
            &op,
            &f.view,
            &[(
                WorktreeId::parse("worktree:forged").unwrap(),
                f.initial.id().clone(),
            )],
            None,
            vec![],
        )
        .unwrap(),
    );
}

#[test]
fn registration_requires_durable_source_ancestry_and_new_stale_worktree() {
    let f = Fixture::new();
    let a1 = f.advance("a1", &f.a, f.initial.id());
    f.advance("b1", &f.b, f.initial.id());
    f.advance("a2", &f.a, a1.id());
    for (name, parent, source, status) in [
        (
            "missing-source",
            f.initial.id(),
            WorktreeId::parse("worktree:forged").unwrap(),
            WorktreeStatus::Stale,
        ),
        (
            "sibling-parent",
            a1.id(),
            f.b.clone(),
            WorktreeStatus::Stale,
        ),
        (
            "ready-forgery",
            f.initial.id(),
            f.a.clone(),
            WorktreeStatus::Ready,
        ),
    ] {
        let (op, view, created) = f.creation(name, parent, status);
        f.reject(
            &op,
            &f.batch(
                &op,
                &view,
                &[],
                Some(source),
                vec![EntityRecord::Worktree(created)],
            )
            .unwrap(),
        );
    }
    let (op, view, created) = f.creation("valid-resumed", f.initial.id(), WorktreeStatus::Stale);
    let batch = f
        .batch(
            &op,
            &view,
            &[],
            Some(f.a.clone()),
            vec![EntityRecord::Worktree(created.clone())],
        )
        .unwrap();
    f.repo().apply_batch(&batch).unwrap();
    assert_eq!(
        f.repo()
            .get_worktree(created.id())
            .unwrap()
            .unwrap()
            .active_operation_id(),
        op.id()
    );
    assert_eq!(f.repo().operation_heads(&f.space).unwrap().len(), 3);
}

#[test]
fn passive_and_same_batch_parents_or_worktrees_cannot_manufacture_authority() {
    let f = Fixture::new();
    f.advance("a1", &f.a, f.initial.id());
    let passive = operation(
        "passive",
        &f.space,
        Some(f.initial.id()),
        &f.view,
        OperationKind::Restore,
    );
    let passive_batch = WriteBatch::new(
        TransactionId::new("transaction:passive").unwrap(),
        vec![],
        vec![EntityRecord::Operation(passive.clone())],
        vec![],
        vec![],
    )
    .unwrap();
    f.repo().apply_batch(&passive_batch).unwrap();
    let (op, view, created) = f.creation("passive-child", passive.id(), WorktreeStatus::Stale);
    f.reject(
        &op,
        &f.batch(
            &op,
            &view,
            &[],
            Some(f.b.clone()),
            vec![EntityRecord::Worktree(created)],
        )
        .unwrap(),
    );
    let forged = operation(
        "same-batch-parent",
        &f.space,
        Some(f.initial.id()),
        &f.view,
        OperationKind::Restore,
    );
    let (op, view, created) = f.creation("same-batch-child", forged.id(), WorktreeStatus::Stale);
    f.reject(
        &op,
        &f.batch(
            &op,
            &view,
            &[],
            Some(f.b.clone()),
            vec![
                EntityRecord::Worktree(created),
                EntityRecord::Operation(forged),
            ],
        )
        .unwrap(),
    );
    let fake_id = WorktreeId::parse("worktree:same-batch-source").unwrap();
    let fake = Worktree::new(
        fake_id.clone(),
        StoreId::parse("store:worktree-parent").unwrap(),
        f.space.clone(),
        f.initial.id().clone(),
        "C:/fake",
        "fs:fake",
        time(),
        Metadata::empty(),
    )
    .unwrap();
    let (op, view, created) = f.creation(
        "same-batch-source-child",
        f.initial.id(),
        WorktreeStatus::Stale,
    );
    f.reject(
        &op,
        &f.batch(
            &op,
            &view,
            &[],
            Some(fake_id),
            vec![
                EntityRecord::Worktree(created),
                EntityRecord::Worktree(fake),
            ],
        )
        .unwrap(),
    );
}

#[test]
fn foreign_space_source_and_cas_cannot_authorize_retained_parent() {
    let f = Fixture::new();
    f.advance("a1", &f.a, f.initial.id());
    let space = Space::new(
        SpaceId::parse("space:foreign").unwrap(),
        "Foreign",
        time(),
        Metadata::empty(),
    )
    .unwrap();
    let id = WorktreeId::parse("worktree:foreign").unwrap();
    let view = RepositoryView::new(
        space.id().clone(),
        vec![],
        vec![],
        vec![],
        vec![WorktreeViewState::new(
            id.clone(),
            None,
            None,
            None,
            WorktreeStatus::Ready,
        )],
    )
    .unwrap();
    let init = operation(
        "foreign-init",
        space.id(),
        None,
        &view,
        OperationKind::Initialize,
    );
    let worktree = Worktree::new(
        id.clone(),
        StoreId::parse("store:worktree-parent").unwrap(),
        space.id().clone(),
        init.id().clone(),
        "C:/foreign",
        "fs:foreign",
        time(),
        Metadata::empty(),
    )
    .unwrap();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:foreign-init").unwrap(),
        vec![CanonicalObject::RepositoryView(view.clone())],
        vec![
            EntityRecord::Space(space.clone()),
            EntityRecord::Worktree(worktree),
            EntityRecord::Operation(init.clone()),
        ],
        vec![SpaceObjectAdmission::new(
            space.id().clone(),
            StoreObjectId::RepositoryView(view.id().clone()),
        )],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![],
                    init.id().clone(),
                    vec![init.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    f.repo().apply_batch(&batch).unwrap();
    let (op, view, created) = f.creation(
        "foreign-source-child",
        f.initial.id(),
        WorktreeStatus::Stale,
    );
    f.reject(
        &op,
        &f.batch(
            &op,
            &view,
            &[],
            Some(id.clone()),
            vec![EntityRecord::Worktree(created)],
        )
        .unwrap(),
    );
    let op = operation(
        "foreign-cas",
        &f.space,
        Some(f.initial.id()),
        &f.view,
        OperationKind::CaptureWorktree,
    );
    assert!(matches!(
        f.batch(
            &op,
            &f.view,
            &[
                (f.b.clone(), f.initial.id().clone()),
                (id, init.id().clone()),
            ],
            None,
            vec![],
        ),
        Err(StoreError::InvalidOperationHeadsTransition { .. })
    ));
}

#[test]
fn in_frontier_parent_cannot_consume_an_independent_worktree_history() {
    let f = Fixture::new();
    let a = f.advance("a1", &f.a, f.initial.id());
    let b = f.advance("b1", &f.b, f.initial.id());
    for (name, targets) in [
        ("wrong-own-parent", vec![(f.b.clone(), b.id().clone())]),
        (
            "one-matching-plus-independent",
            vec![(f.a.clone(), a.id().clone()), (f.b.clone(), b.id().clone())],
        ),
    ] {
        let op = operation(
            name,
            &f.space,
            Some(a.id()),
            &f.view,
            OperationKind::CaptureWorktree,
        );
        assert!(matches!(
            f.batch(&op, &f.view, &targets, None, vec![]),
            Err(StoreError::InvalidOperationHeadsTransition { .. })
        ));
        assert!(f.repo().get_operation(op.id()).unwrap().is_none());
    }
    assert_eq!(
        f.repo().worktree_active_operation(&f.a).unwrap().as_ref(),
        Some(a.id())
    );
    assert_eq!(
        f.repo().worktree_active_operation(&f.b).unwrap().as_ref(),
        Some(b.id())
    );
    let next = f.advance("valid-own-parent", &f.a, a.id());
    let heads = f.repo().operation_heads(&f.space).unwrap();
    assert_eq!(heads.len(), 2);
    assert!(heads.contains(next.id()) && heads.contains(b.id()));
}

#[test]
fn explicit_complete_integration_may_advance_both_bound_parent_worktrees() {
    let f = Fixture::new();
    let a = f.advance("a1", &f.a, f.initial.id());
    let b = f.advance("b1", &f.b, f.initial.id());
    let mut parents = vec![a.id().clone(), b.id().clone()];
    parents.sort();
    let operation = Operation::new(
        OperationId::parse("operation:integrate").unwrap(),
        f.space.clone(),
        parents,
        f.view.id().clone(),
        OperationKind::IntegrateOperations,
        "Explicit integration",
        vec![],
        None,
        None,
        time(),
        Metadata::empty(),
    )
    .unwrap();
    let batch = f
        .batch(
            &operation,
            &f.view,
            &[(f.a.clone(), a.id().clone()), (f.b.clone(), b.id().clone())],
            None,
            vec![],
        )
        .unwrap();
    f.repo().apply_batch(&batch).unwrap();
    assert_eq!(
        f.repo().operation_heads(&f.space).unwrap(),
        vec![operation.id().clone()]
    );
    assert_eq!(
        f.repo().worktree_active_operation(&f.a).unwrap().as_ref(),
        Some(operation.id())
    );
    assert_eq!(
        f.repo().worktree_active_operation(&f.b).unwrap().as_ref(),
        Some(operation.id())
    );
}
