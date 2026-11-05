//! Genuine admitted B allocates an independently owned C, binds Copying and
//! publishes its exact FORMAT inode. C remains Initializing with an empty DB;
//! this recipe does not claim content transfer, Ready, admission or crash resume.
use crate::projection::{
    ProjectionStore, ProjectionStoreError, SessionMode, format,
    provider::{
        database_io,
        managed_admission::read_admitted_import,
        managed_export::allocation::{
            self, ManagedAllocationStep, ManagedFormatState, ManagedFormatStep,
        },
        process_profile::tests::child,
        records::{
            Record,
            transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        },
        transition_sql::SourceIntentEffect,
    },
    shared::provider_access::ImportRegistrationEffect,
};
use layrs_core::scm::WorktreeId;
use layrs_vfs::Cancellation;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const LEAF: &str = "00000000000000000000000000000091";
const TX: &str = "00000000000000000000000000000092";

type Tree = BTreeMap<PathBuf, super::managed_ready_reopen_tests::Node>;

fn trees(
    root: &Path,
    family: &Path,
    destination: &Path,
    owner_lock: Option<&Path>,
) -> (Tree, Tree, Tree) {
    let family = match owner_lock {
        Some(lock) => super::managed_ready_reopen_tests::tree_with_owner_lock(family, lock),
        None => super::managed_ready_reopen_tests::tree(family),
    };
    (
        super::managed_ready_reopen_tests::tree(root),
        family,
        if destination.exists() {
            super::managed_ready_reopen_tests::tree(destination)
        } else {
            BTreeMap::new()
        },
    )
}

fn only_source_journal_changed(
    store: &ProjectionStore,
    before: &(Tree, Tree, Tree),
    after: &(Tree, Tree, Tree),
) {
    use super::managed_ready_reopen_tests::Node;
    assert_eq!(
        after.0, before.0,
        "Copying preserves the complete source checkout"
    );
    assert_eq!(
        after.2, before.2,
        "Copying does not publish destination bytes"
    );
    assert_eq!(
        after.1.keys().collect::<Vec<_>>(),
        before.1.keys().collect::<Vec<_>>()
    );
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
            assert_ne!(new_digest, old_digest, "Copying must be durable");
        } else {
            assert_eq!(current, previous, "Copying changed unrelated path {path:?}");
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
}

fn evidence(store: &ProjectionStore) -> Evidence {
    format::access(store, SessionMode::Shared)
        .unwrap()
        .run(|access| {
            access.with_content(|c, scope| {
                let tx = c.transaction()?;
                let admitted = read_admitted_import(&tx, &store.marker, None)?;
                let result = Evidence {
                    admission: admitted.record().digest()?,
                    ready: admitted.ready().encode()?,
                    outgoing: admitted
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
                };
                scope.verify(&tx, store)?;
                tx.rollback()?;
                Ok(result)
            })
        })
        .unwrap()
}

fn require_source(
    actual: &Evidence,
    original: &Evidence,
    outgoing: &ProviderTransitionIntentRecord,
) {
    assert_eq!(actual.admission, original.admission);
    assert_eq!(actual.ready, original.ready);
    assert_eq!(actual.generation, original.generation);
    assert_eq!(actual.captures, original.captures);
    assert_eq!(actual.outgoing, vec![outgoing.encode().unwrap()]);
}

#[derive(Debug, PartialEq, Eq)]
struct RegistryRow {
    metadata_path: String,
    database_identity: String,
    marker: Vec<u8>,
    state: String,
}

fn registry(family: &Path, owner: &WorktreeId) -> Option<RegistryRow> {
    let c = Connection::open_with_flags(
        family.join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let result = c
        .query_row(
            "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
            [owner.as_str()],
            |row| {
                Ok(RegistryRow {
                    metadata_path: row.get(0)?,
                    database_identity: row.get(1)?,
                    marker: row.get(2)?,
                    state: row.get(3)?,
                })
            },
        )
        .optional()
        .unwrap();
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM exact_installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
    ] {
        assert!(
            !c.query_row(sql, [owner.as_str()], |row| row.get::<_, bool>(0))
                .unwrap()
        );
    }
    c.execute_batch("ROLLBACK").unwrap();
    c.close().unwrap();
    result
}

fn live_refusal(error: ProjectionStoreError) {
    assert!(error.to_string().contains("live managed export"), "{error}");
}

#[test]
#[ignore = "ROOT source release and audited owned native child900/outer960 required"]
fn managed_ordinary_allocation_binds_copying_and_initializing_without_admitting_c() {
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_allocation_binds_copying_and_initializing_without_admitting_c"
        ),
        900,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let family_database = family.join("metadata/catalog.sqlite3");
            let parent = root.parent().unwrap();
            let target = parent.join(LEAF);
            let original = evidence(&store);
            assert!(original.outgoing.is_empty());
            let mut destination = store.identity().clone();
            destination.worktree_id =
                WorktreeId::parse("worktree:ordinary-allocation-destination").unwrap();
            let cancel = Cancellation::default();
            let cancelled = Cancellation::default();
            cancelled.cancel();

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let (planned, effect) = owner.begin_plan(parent, LEAF, TX, &cancel).unwrap();
            assert_eq!(effect, SourceIntentEffect::Inserted);
            owner.close().unwrap();
            require_source(&evidence(&store), &original, &planned);
            let planned_trees = trees(&root, &family, &target, None);
            assert!(registry(&family, &destination.worktree_id).is_none());

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let refused = match allocation::allocate(&mut owner, &planned, &cancelled) {
                Ok(_) => panic!("pre-cancelled allocation created a destination"),
                Err(error) => error,
            };
            assert_eq!(refused.attempted_root, target);
            assert!(refused.effects.is_empty());
            assert_eq!(refused.retained_pin_count(), 0);
            drop(refused);
            owner.close().unwrap();
            assert_eq!(trees(&root, &family, &target, None), planned_trees);
            require_source(&evidence(&store), &original, &planned);

            let family_before = database_io::tests::support::witness(&family_database);
            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let mut allocated = allocation::allocate(&mut owner, &planned, &cancel)
                .unwrap_or_else(|error| panic!("ordinary allocation refused: {error:?}"));
            allocated.verify(&planned.plan).unwrap();
            assert_eq!(allocated.root(), target);
            assert_eq!(
                allocated
                    .allocation_effects()
                    .iter()
                    .map(|effect| effect.step)
                    .collect::<Vec<_>>(),
                vec![
                    ManagedAllocationStep::Root,
                    ManagedAllocationStep::Control,
                    ManagedAllocationStep::Member,
                    ManagedAllocationStep::Metadata,
                    ManagedAllocationStep::OwnerLock,
                    ManagedAllocationStep::Database,
                    ManagedAllocationStep::TemporaryFormat,
                ]
            );
            assert!(
                allocated
                    .allocation_effects()
                    .iter()
                    .all(|effect| effect.confirmed)
            );
            let metadata = allocated.family().member(&destination.worktree_id);
            let temporary = target.join(".layrs").join(format!("FORMAT.pending-{LEAF}"));
            let final_format = target.join(".layrs/FORMAT");
            let destination_database = metadata.join("projection.sqlite3");
            let destination_lock = metadata.join("layrs.lock");
            assert_eq!(
                allocated.destination_database().path(),
                destination_database
            );
            let expected_paths = vec![
                target.clone(),
                target.join(".layrs"),
                metadata.parent().unwrap().to_owned(),
                metadata.clone(),
                destination_lock.clone(),
                destination_database.clone(),
                temporary.clone(),
            ];
            assert_eq!(
                allocated
                    .allocation_effects()
                    .iter()
                    .map(|effect| effect.path.clone())
                    .collect::<Vec<_>>(),
                expected_paths
            );
            assert!(!final_format.exists());
            assert_eq!(fs::metadata(&temporary).unwrap().len(), 0);
            assert_eq!(fs::metadata(&destination_database).unwrap().len(), 0);
            assert_eq!(
                allocated.proof().format.initial_snapshot_digest,
                store.marker.initial_snapshot_digest,
                "C preserves the genuine generation-zero source commitment"
            );
            let premature = allocated
                .publish_format(&mut owner, &planned, &cancel)
                .unwrap_err();
            assert!(
                premature.effects.is_empty(),
                "Planned cannot publish FORMAT"
            );
            owner.close().unwrap();
            require_source(&evidence(&store), &original, &planned);
            assert!(registry(&family, &destination.worktree_id).is_none());
            assert_eq!(
                database_io::tests::support::witness(&family_database),
                family_before
            );
            let allocated_trees = trees(&root, &family, &target, None);
            assert_eq!(allocated_trees.0, planned_trees.0);
            for (path, node) in &planned_trees.1 {
                assert_eq!(
                    allocated_trees.1.get(path),
                    Some(node),
                    "allocation changed {path:?}"
                );
            }
            let added_family_paths = allocated_trees
                .1
                .keys()
                .filter(|path| !planned_trees.1.contains_key(*path))
                .cloned()
                .collect::<Vec<_>>();
            let mut expected_family_paths = vec![
                metadata.parent().unwrap().to_owned(),
                metadata.clone(),
                destination_lock.clone(),
                destination_database.clone(),
            ];
            expected_family_paths.sort();
            assert_eq!(added_family_paths, expected_family_paths);
            let mut expected_checkout_paths =
                vec![target.clone(), target.join(".layrs"), temporary.clone()];
            expected_checkout_paths.sort();
            assert_eq!(
                allocated_trees.2.keys().cloned().collect::<Vec<_>>(),
                expected_checkout_paths
            );
            let refused = store.cancel_managed_export_plan(TX, &cancel).unwrap_err();
            assert!(
                refused.to_string().contains("destination resources"),
                "Planned with physical artifacts is not resource-free: {refused}"
            );
            assert_eq!(trees(&root, &family, &target, None), allocated_trees);
            require_source(&evidence(&store), &original, &planned);

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let mut changed =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            changed.plan.destination_leaf_nonce = "00000000000000000000000000000093".into();
            changed.encode().unwrap();
            assert!(allocated.verify(&changed.plan).is_err());
            assert!(
                owner
                    .commit_allocation(&changed, &allocated, &cancel)
                    .is_err()
            );
            let mut changed_parent =
                ProviderTransitionIntentRecord::decode(&planned.encode().unwrap()).unwrap();
            let last = changed_parent
                .plan
                .destination_parent_identity
                .pop()
                .unwrap();
            changed_parent
                .plan
                .destination_parent_identity
                .push(if last == '0' { '1' } else { '0' });
            changed_parent.encode().unwrap();
            assert!(allocated.verify(&changed_parent.plan).is_err());
            assert!(
                owner
                    .commit_allocation(&changed_parent, &allocated, &cancel)
                    .is_err()
            );
            assert!(
                owner
                    .commit_allocation(&planned, &allocated, &cancelled)
                    .is_err()
            );
            owner.close().unwrap();
            assert_eq!(trees(&root, &family, &target, None), allocated_trees);
            require_source(&evidence(&store), &original, &planned);

            let temporary_witness = database_io::tests::support::witness(&temporary);
            assert_eq!(temporary_witness.0, allocated.proof().format_file_identity);
            let empty_database = database_io::tests::support::witness(&destination_database);
            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let (copying, effect) = owner
                .commit_allocation(&planned, &allocated, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Updated);
            assert_eq!(copying.role, TransitionRole::Source);
            assert_eq!(copying.state, TransitionState::Copying);
            assert_eq!(copying.destination.as_ref(), Some(allocated.proof()));
            assert!(copying.reservation.is_none() && copying.receipt_id.is_none());
            copying.require_transition(&planned).unwrap();
            owner.close().unwrap();
            require_source(&evidence(&store), &original, &copying);
            assert_eq!(
                database_io::tests::support::witness(&family_database),
                family_before
            );
            assert_eq!(
                database_io::tests::support::witness(&temporary),
                temporary_witness
            );
            assert_eq!(
                database_io::tests::support::witness(&destination_database),
                empty_database
            );
            assert!(!final_format.exists());
            let copying_trees = trees(&root, &family, &target, None);
            only_source_journal_changed(&store, &allocated_trees, &copying_trees);

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let (replayed, effect) = owner
                .commit_allocation(&planned, &allocated, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::AlreadyPresent);
            assert_eq!(replayed, copying);
            let mut substituted =
                ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
            let substituted_proof = substituted.destination.as_mut().unwrap();
            substituted_proof.format_file_identity =
                if substituted_proof.private_database_identity == "0:0:0" {
                    "0:0:1"
                } else {
                    "0:0:0"
                }
                .into();
            substituted.encode().unwrap();
            let refused = allocated
                .publish_format(&mut owner, &substituted, &cancel)
                .unwrap_err();
            assert!(
                refused.effects.is_empty(),
                "foreign proof precedes every FORMAT effect"
            );
            owner.close().unwrap();
            assert_eq!(trees(&root, &family, &target, None), copying_trees);
            require_source(&evidence(&store), &original, &copying);

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let publication = allocated
                .publish_format(&mut owner, &copying, &cancel)
                .unwrap();
            assert_eq!(publication.state, ManagedFormatState::Published);
            assert!(publication.effects.iter().all(|effect| effect.confirmed));
            assert!(
                publication
                    .effects
                    .iter()
                    .any(|effect| effect.step == ManagedFormatStep::Rename)
            );
            allocated.require_published(&copying.plan).unwrap();
            owner.close().unwrap();
            assert!(!temporary.exists());
            assert_eq!(
                fs::read(&final_format).unwrap(),
                allocated.proof().format.bytes().unwrap()
            );
            assert_eq!(
                database_io::tests::support::witness(&final_format).0,
                temporary_witness.0
            );
            assert_eq!(
                database_io::tests::support::witness(&destination_database),
                empty_database
            );
            assert_eq!(
                database_io::tests::support::witness(&family_database),
                family_before
            );
            assert!(registry(&family, &destination.worktree_id).is_none());
            require_source(&evidence(&store), &original, &copying);
            let published_trees = trees(&root, &family, &target, Some(&destination_lock));

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let publication = allocated
                .publish_format(&mut owner, &copying, &cancel)
                .unwrap();
            assert_eq!(publication.state, ManagedFormatState::AlreadyPublished);
            assert!(!publication.effects.iter().any(|effect| matches!(
                effect.step,
                ManagedFormatStep::Append | ManagedFormatStep::Rename
            )));
            owner.close().unwrap();
            assert_eq!(
                trees(&root, &family, &target, Some(&destination_lock)),
                published_trees
            );

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            assert_eq!(
                owner
                    .register_allocation(&allocated, &copying, &cancel)
                    .unwrap(),
                ImportRegistrationEffect::Inserted
            );
            owner.close().unwrap();
            let expected_registry = RegistryRow {
                metadata_path: metadata.to_str().unwrap().into(),
                database_identity: allocated.proof().private_database_identity.clone(),
                marker: allocated.proof().format.bytes().unwrap(),
                state: "initializing".into(),
            };
            assert_eq!(
                registry(&family, &destination.worktree_id),
                Some(expected_registry)
            );
            assert_eq!(
                database_io::tests::support::witness(&destination_database),
                empty_database
            );
            require_source(&evidence(&store), &original, &copying);
            let initializing_trees = trees(&root, &family, &target, Some(&destination_lock));
            assert_eq!(initializing_trees.0, published_trees.0);
            assert_eq!(initializing_trees.2, published_trees.2);
            assert_eq!(
                initializing_trees.1.keys().collect::<Vec<_>>(),
                published_trees.1.keys().collect::<Vec<_>>()
            );
            for (path, node) in &published_trees.1 {
                if path != &family_database {
                    assert_eq!(
                        initializing_trees.1.get(path),
                        Some(node),
                        "registration changed {path:?}"
                    );
                }
            }
            let registered_catalog = database_io::tests::support::witness(&family_database);
            assert_eq!(registered_catalog.0, family_before.0);
            assert_ne!(
                registered_catalog.2, family_before.2,
                "Initializing registration is durable"
            );

            let mut owner = store
                .begin_managed_export(state.generation, destination, &cancel)
                .unwrap();
            assert_eq!(
                owner
                    .register_allocation(&allocated, &copying, &cancel)
                    .unwrap(),
                ImportRegistrationEffect::AlreadyPresent
            );
            let publication = allocated
                .publish_format(&mut owner, &copying, &cancel)
                .unwrap();
            assert_eq!(publication.state, ManagedFormatState::AlreadyPublished);
            owner.close().unwrap();
            assert_eq!(
                trees(&root, &family, &target, Some(&destination_lock)),
                initializing_trees
            );
            require_source(&evidence(&store), &original, &copying);

            live_refusal(
                ProjectionStore::open(&root)
                    .err()
                    .expect("ordinary open must refuse Copying"),
            );
            live_refusal(store.capture_patch(state.generation).unwrap_err());
            live_refusal(
                store
                    .compare_and_swap(state.generation, &state.snapshot)
                    .unwrap_err(),
            );
            assert!(
                store.cancel_managed_export_plan(TX, &cancel).is_err(),
                "resource-bearing Copying cannot use preallocation cancellation"
            );
            assert_eq!(
                trees(&root, &family, &target, Some(&destination_lock)),
                initializing_trees
            );
            require_source(&evidence(&store), &original, &copying);
            allocated.require_published(&copying.plan).unwrap();
            // C's empty private database and retained Initializing registry are
            // deliberately left as evidence. No ordinary C opening is attempted.
            drop(allocated);
            drop(store);
        },
    );
}
