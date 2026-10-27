//! One genuine admitted source exercises the preallocation intent lifecycle.
//! No destination resources, transfer, publication or new admission are created.
use crate::projection::{
    format,
    provider::{
        managed_admission::read_admitted_import,
        process_profile::tests::child,
        records::{
            transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
            Record,
        },
        transition_sql::SourceIntentEffect,
    },
    ProjectionStore, ProjectionStoreError, SessionMode,
};
use layrs_core::scm::{FileId, RepoPath, WorktreeId};
use layrs_vfs::Cancellation;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const LEAF: &str = "00000000000000000000000000000081";
const OTHER_LEAF: &str = "00000000000000000000000000000082";
const TX: &str = "00000000000000000000000000000083";
const NEXT_TX: &str = "00000000000000000000000000000084";
const DIRTY: &[u8] = b"external edit while ordinary export is planned";

type Tree = BTreeMap<PathBuf, super::managed_ready_reopen_tests::Node>;
fn trees(root: &Path, family: &Path) -> (Tree, Tree) {
    (
        super::managed_ready_reopen_tests::tree(root),
        super::managed_ready_reopen_tests::tree(family),
    )
}

fn only_source_journal_changed(
    store: &ProjectionStore,
    before: &(Tree, Tree),
    after: &(Tree, Tree),
) {
    use super::managed_ready_reopen_tests::Node;
    assert_eq!(after.0, before.0, "journal preserves the complete checkout");
    assert_eq!(
        after.1.keys().collect::<Vec<_>>(),
        before.1.keys().collect::<Vec<_>>(),
        "journal creates or removes no family paths"
    );
    // The source's private database lives under family/worktrees. Only this
    // exact bound inode may change at an intentional journal commit boundary.
    let database = store
        .marker
        .metadata_path(&store.root)
        .join("projection.sqlite3");
    let expected_identity = &store.marker.shared.as_ref().unwrap().database_identity;
    for (path, previous) in &before.1 {
        let current = &after.1[path];
        if path == &database {
            let (Node::File((old_id, _, old_digest)), Node::File((new_id, _, new_digest))) =
                (previous, current)
            else {
                panic!("bound source database must remain a regular file");
            };
            assert_eq!(old_id, expected_identity);
            assert_eq!(new_id, expected_identity);
            assert_ne!(new_digest, old_digest, "journal commit must be durable");
        } else {
            assert_eq!(current, previous, "journal changed unrelated path {path:?}");
        }
    }
    assert!(before.1.contains_key(&database));
}

#[derive(Debug, PartialEq, Eq)]
struct Evidence {
    admission: String,
    ready: Vec<u8>,
    outgoing: Vec<Vec<u8>>,
    generation: u64,
    captures: u64,
    committed_budget: bool,
}

fn evidence(store: &ProjectionStore) -> Evidence {
    format::access(store, SessionMode::Shared)
        .unwrap()
        .run(|access| {
            access.with_content(|c, scope| {
                let tx = c.transaction()?;
                let proof = read_admitted_import(&tx, &store.marker, None)?;
                let result = Evidence {
                    admission: proof.record().digest()?,
                    ready: proof.ready().encode()?,
                    outgoing: proof
                        .outgoing()
                        .iter()
                        .map(|row| row.encode())
                        .collect::<crate::projection::Result<Vec<_>>>()?,
                    generation: tx.query_row(
                        "SELECT generation FROM projection_head WHERE singleton=1",
                        [],
                        |row| row.get(0),
                    )?,
                    captures: tx.query_row(
                        "SELECT coalesce(max(sequence),0) FROM projection_captures",
                        [],
                        |row| row.get(0),
                    )?,
                    committed_budget: tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM projection_budgets WHERE state='committed')",
                        [],
                        |row| row.get(0),
                    )?,
                };
                scope.verify(&tx, store)?;
                tx.rollback()?;
                Ok(result)
            })
        })
        .unwrap()
}

fn same_ingress(actual: &Evidence, original: &Evidence) {
    assert_eq!(actual.admission, original.admission);
    assert_eq!(actual.ready, original.ready);
}

fn live_refusal(error: ProjectionStoreError) {
    assert!(error.to_string().contains("live managed export"), "{error}");
}

#[test]
#[ignore = "ROOT source release and audited owned native child900/outer960 required"]
fn managed_ordinary_source_plan_retries_cancels_and_preserves_incoming_admission() {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_source_plan_retries_cancels_and_preserves_incoming_admission"
        ),
        900,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let parent = root.parent().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            let checkout = root.join(path.as_str());
            let original = evidence(&store);
            assert!(original.outgoing.is_empty());
            let incoming = ProviderTransitionIntentRecord::decode(&original.ready).unwrap();
            let mut destination = store.identity().clone();
            destination.worktree_id =
                WorktreeId::parse("worktree:ordinary-intent-destination").unwrap();
            let cancel = Cancellation::default();
            let cancelled = Cancellation::default();
            cancelled.cancel();

            let initial = trees(&root, &family);
            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            assert!(owner.begin_plan(parent, LEAF, TX, &cancelled).is_err());
            owner.close().unwrap();
            assert_eq!(
                trees(&root, &family),
                initial,
                "pre-cancel precedes every effect"
            );
            assert_eq!(evidence(&store), original);

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let (planned, effect) = owner.begin_plan(parent, LEAF, TX, &cancel).unwrap();
            assert_eq!(effect, SourceIntentEffect::Inserted);
            assert_eq!(planned.role, TransitionRole::Source);
            assert_eq!(planned.state, TransitionState::Planned);
            assert!(
                planned.destination.is_none()
                    && planned.reservation.is_none()
                    && planned.receipt_id.is_none()
            );
            owner.close().unwrap();
            let planned_tree = trees(&root, &family);
            only_source_journal_changed(&store, &initial, &planned_tree);
            let observed = evidence(&store);
            same_ingress(&observed, &original);
            assert_eq!(observed.outgoing, vec![planned.encode().unwrap()]);
            assert_eq!(
                (observed.generation, observed.captures),
                (original.generation, original.captures)
            );
            assert_eq!(observed.committed_budget, original.committed_budget);

            live_refusal(
                ProjectionStore::open(&root)
                    .err()
                    .expect("ordinary open must refuse a live source intent"),
            );
            assert_eq!(trees(&root, &family), planned_tree);
            live_refusal(store.capture_patch(state.generation).unwrap_err());
            assert_eq!(trees(&root, &family), planned_tree);
            live_refusal(
                store
                    .compare_and_swap(state.generation, &state.snapshot)
                    .unwrap_err(),
            );
            assert_eq!(trees(&root, &family), planned_tree);
            drop(store);

            let managed = ProjectionStore::open_for_managed_export(&root).unwrap();
            assert_eq!(
                trees(&root, &family),
                planned_tree,
                "managed discovery is read-only"
            );
            let mut owner = managed
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let (retry, effect) = owner.begin_plan(parent, LEAF, TX, &cancel).unwrap();
            assert_eq!(retry, planned);
            assert_eq!(effect, SourceIntentEffect::AlreadyPresent);
            assert!(
                owner.begin_plan(parent, OTHER_LEAF, TX, &cancel).is_err(),
                "same ID cannot substitute its plan"
            );
            assert!(
                owner
                    .begin_plan(parent, LEAF, &incoming.transaction_id, &cancel)
                    .is_err(),
                "incoming ID cannot become outgoing"
            );
            assert!(owner
                .begin_plan(parent, LEAF, "invalid-id", &cancel)
                .is_err());
            assert!(
                owner
                    .begin_plan(parent, OTHER_LEAF, NEXT_TX, &cancel)
                    .is_err(),
                "second live intent must refuse"
            );
            owner.close().unwrap();
            assert_eq!(
                trees(&root, &family),
                planned_tree,
                "exact retry and all refusals preserve complete witnesses"
            );
            assert_eq!(evidence(&managed), observed);
            drop(managed);

            let managed = ProjectionStore::open_for_managed_export(&root).unwrap();
            assert_eq!(trees(&root, &family), planned_tree);
            let mut owner = managed
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let (retry, effect) = owner.begin_plan(parent, LEAF, TX, &cancel).unwrap();
            assert_eq!(retry, planned);
            assert_eq!(effect, SourceIntentEffect::AlreadyPresent);
            owner.close().unwrap();
            assert_eq!(
                trees(&root, &family),
                planned_tree,
                "reopened retry is idempotent"
            );

            fs::write(&checkout, DIRTY).unwrap();
            let dirty = trees(&root, &family);
            assert!(managed.cancel_managed_export_plan(TX, &cancelled).is_err());
            assert_eq!(trees(&root, &family), dirty);
            managed.cancel_managed_export_plan(TX, &cancel).unwrap();
            assert_eq!(
                fs::read(&checkout).unwrap(),
                DIRTY,
                "cancellation preserves external edits"
            );
            let terminal_tree = trees(&root, &family);
            only_source_journal_changed(&managed, &dirty, &terminal_tree);
            let terminal = evidence(&managed);
            same_ingress(&terminal, &original);
            let mut cancelled_record =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            cancelled_record.state = TransitionState::Cancelled;
            assert_eq!(terminal.outgoing, vec![cancelled_record.encode().unwrap()]);
            assert_eq!(
                (
                    terminal.generation,
                    terminal.captures,
                    terminal.committed_budget
                ),
                (
                    original.generation,
                    original.captures,
                    original.committed_budget
                )
            );
            managed.cancel_managed_export_plan(TX, &cancel).unwrap();
            assert_eq!(
                trees(&root, &family),
                terminal_tree,
                "terminal retry performs no write"
            );
            assert_eq!(evidence(&managed), terminal);
            drop(managed);

            let ordinary = ProjectionStore::open(&root).unwrap();
            assert_eq!(trees(&root, &family), terminal_tree);
            let advanced = ordinary
                .compare_and_swap(state.generation, &state.snapshot)
                .unwrap();
            assert_eq!(advanced.generation, state.generation + 1);
            let capture = ordinary.capture_patch(advanced.generation).unwrap();
            assert_eq!(capture.sequence, original.captures + 1);
            assert_eq!(
                capture
                    .changes
                    .iter()
                    .find(|change| change.path == path)
                    .unwrap()
                    .replacement
                    .as_ref()
                    .unwrap()
                    .file_id,
                FileId::from_bytes(DIRTY)
            );
            // Witness immediately, before any ordinary reopen or exclusive owner
            // can legitimately settle this newly committed capture budget.
            let advanced_tree = trees(&root, &family);
            let advanced_evidence = evidence(&ordinary);
            assert!(advanced_evidence.committed_budget);
            same_ingress(&advanced_evidence, &original);
            assert_eq!(
                advanced_evidence.outgoing, terminal.outgoing,
                "old cancellation remains valid after generation and capture advance"
            );
            assert_eq!(
                (advanced_evidence.generation, advanced_evidence.captures),
                (advanced.generation, capture.sequence)
            );
            assert_eq!(trees(&root, &family), advanced_tree);
            ordinary.cancel_managed_export_plan(TX, &cancel).unwrap();
            assert_eq!(
                trees(&root, &family),
                advanced_tree,
                "historical terminal retry performs no write"
            );
            let retried = evidence(&ordinary);
            assert!(
                retried.committed_budget,
                "terminal retry must not settle an unrelated capture budget"
            );
            assert_eq!(retried, advanced_evidence);

            destination.worktree_id =
                WorktreeId::parse("worktree:ordinary-intent-next-destination").unwrap();
            let mut owner = ordinary
                .begin_managed_export(advanced.generation, destination, &cancel)
                .unwrap();
            let (next, effect) = owner
                .begin_plan(parent, OTHER_LEAF, NEXT_TX, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Inserted);
            assert_eq!(next.state, TransitionState::Planned);
            owner.close().unwrap();
            ordinary
                .cancel_managed_export_plan(NEXT_TX, &cancel)
                .unwrap();
            let final_evidence = evidence(&ordinary);
            same_ingress(&final_evidence, &original);
            assert_eq!(final_evidence.outgoing.len(), 2);
            assert!(final_evidence.outgoing.iter().all(|body| {
                ProviderTransitionIntentRecord::decode(body).unwrap().state
                    == TransitionState::Cancelled
            }));
            drop(ordinary);
            ProjectionStore::open(&root).unwrap();
        },
    );
}
