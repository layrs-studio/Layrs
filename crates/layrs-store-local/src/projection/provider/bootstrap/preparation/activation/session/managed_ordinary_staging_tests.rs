//! Genuine B9/C8 Staging phase gaps with explicit destination writer observers.
//! Rejecting an open and withholding a successful close acknowledgement are
//! deliberate observation errors, not simulated OS failures or crash recovery.
//! No C reservation, content transfer, Ready or admission is qualified here.
use crate::projection::{
    ProjectionStore, ProjectionStoreError, SessionMode, format,
    provider::{
        database_io::{WriterCloseObserver, WriterOpenPoint},
        managed_admission::read_admitted_import,
        managed_export::allocation,
        ordinary_destination::{CopyEffect, staging::StageEffect},
        ordinary_transition,
        process_profile::tests::child,
        records::{
            Record,
            transition::{
                ProviderTransitionIntentRecord, TransitionRole, TransitionRowData, TransitionState,
            },
        },
        recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
        transition_sql::SourceIntentEffect,
    },
    shared::provider_access::ImportRegistrationEffect,
};
use layrs_core::scm::WorktreeId;
use layrs_vfs::Cancellation;
use rusqlite::{Connection, OpenFlags, ffi, types::ValueRef};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const LEAF: &str = "000000000000000000000000000000b1";
const TX: &str = "000000000000000000000000000000b2";
const REJECT_OPEN: &str = "destination Stage writer deliberately refused before raw open";
const WITHHELD_ACK: &str = "destination Stage successful close acknowledgement withheld";
const TABLES: [&str; 13] = [
    "projection_snapshots",
    "projection_selections",
    "projection_captures",
    "projection_publications",
    "projection_provider_catalogs",
    "projection_provider_descriptors",
    "projection_provider_activations",
    "projection_provider_transition_receipts",
    "projection_provider_companions",
    "projection_provider_companion_files",
    "projection_provider_companion_deletions",
    "projection_content",
    "projection_restore_history",
];

#[derive(Default)]
struct Close {
    before_raw: usize,
    opened: usize,
    successful_closes: usize,
    reject_before_raw: bool,
    withhold_close_ack: bool,
    withheld: usize,
}
impl Close {
    fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.before_raw,
            self.opened,
            self.successful_closes,
            self.withheld,
        )
    }
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, raw: *mut ffi::sqlite3) -> CauseResult<()> {
        match point {
            WriterOpenPoint::BeforeRaw => {
                assert!(raw.is_null());
                self.before_raw += 1;
                if std::mem::take(&mut self.reject_before_raw) {
                    return Err(invalid(REJECT_OPEN));
                }
            }
            WriterOpenPoint::AfterRaw => {
                assert!(!raw.is_null());
                self.opened += 1;
            }
            _ => {}
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(succeeded, "observer requires a physically successful close");
        self.successful_closes += 1;
        if std::mem::take(&mut self.withhold_close_ack) {
            self.withheld += 1;
            Some(invalid(WITHHELD_ACK))
        } else {
            None
        }
    }
}

type Tree = BTreeMap<PathBuf, super::managed_ready_reopen_tests::Node>;
fn trees(root: &Path, family: &Path, target: &Path, owner_lock: &Path) -> (Tree, Tree, Tree) {
    (
        super::managed_ready_reopen_tests::tree(root),
        super::managed_ready_reopen_tests::tree_with_owner_lock(family, owner_lock),
        super::managed_ready_reopen_tests::tree(target),
    )
}

fn only_bound_database_changed(
    before: &(Tree, Tree, Tree),
    after: &(Tree, Tree, Tree),
    database: &Path,
    expected_identity: &str,
) {
    use super::managed_ready_reopen_tests::Node;
    assert_eq!(after.0, before.0, "Staging preserves the source checkout");
    assert_eq!(
        after.2, before.2,
        "Staging preserves the destination checkout"
    );
    assert_eq!(
        after.1.keys().collect::<Vec<_>>(),
        before.1.keys().collect::<Vec<_>>()
    );
    for (path, previous) in &before.1 {
        let current = &after.1[path];
        if path == database {
            let (Node::File((old_id, _, old_digest)), Node::File((new_id, _, new_digest))) =
                (previous, current)
            else {
                panic!("bound Staging database must remain a regular file");
            };
            assert_eq!(old_id, expected_identity);
            assert_eq!(new_id, expected_identity);
            assert_ne!(old_digest, new_digest, "Staging commit must be durable");
        } else {
            assert_eq!(current, previous, "Staging changed unrelated path {path:?}");
        }
    }
    assert!(before.1.contains_key(database));
}

#[derive(Debug, PartialEq, Eq)]
struct Evidence {
    admission: Vec<u8>,
    ready: Vec<u8>,
    outgoing: Vec<Vec<u8>>,
    generation: u64,
    captures: u64,
    budgets: u64,
}
fn evidence(store: &ProjectionStore) -> Evidence {
    format::access(store, SessionMode::Shared)
        .unwrap()
        .run(|access| {
            access.with_content(|c, scope| {
                let tx = c.transaction()?;
                let admitted = read_admitted_import(&tx, &store.marker, None)?;
                let result = Evidence {
                    admission: admitted.record().encode()?,
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
                    budgets: tx.query_row(
                        "SELECT count(*) FROM projection_budgets",
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
fn require_evidence(
    actual: &Evidence,
    original: &Evidence,
    expected: &ProviderTransitionIntentRecord,
) {
    assert_eq!(actual.admission, original.admission);
    assert_eq!(actual.ready, original.ready);
    assert_eq!(actual.generation, original.generation);
    assert_eq!(actual.captures, original.captures);
    assert_eq!(actual.budgets, original.budgets);
    assert_eq!(actual.outgoing, vec![expected.encode().unwrap()]);
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}
type Rows = Vec<Vec<Cell>>;

fn copied_rows(c: &Connection) -> BTreeMap<&'static str, Rows> {
    let mut result = BTreeMap::new();
    for table in TABLES {
        let mut statement = c
            .prepare(&format!("SELECT * FROM \"{table}\" LIMIT 129"))
            .unwrap();
        let columns = statement.column_count();
        assert!(columns <= 32);
        let mut query = statement.query([]).unwrap();
        let mut rows = Vec::new();
        let mut bytes = 0usize;
        while let Some(row) = query.next().unwrap() {
            assert!(rows.len() < 128, "tiny Staging fixture row bound");
            let mut cells = Vec::with_capacity(columns);
            for column in 0..columns {
                cells.push(match row.get_ref(column).unwrap() {
                    ValueRef::Null => Cell::Null,
                    ValueRef::Integer(value) => Cell::Integer(value),
                    ValueRef::Real(value) => Cell::Real(value.to_bits()),
                    ValueRef::Text(value) => {
                        bytes = bytes.checked_add(value.len()).unwrap();
                        assert!(bytes <= 8 * 1024 * 1024);
                        Cell::Text(value.to_vec())
                    }
                    ValueRef::Blob(value) => {
                        bytes = bytes.checked_add(value.len()).unwrap();
                        assert!(bytes <= 8 * 1024 * 1024);
                        Cell::Blob(value.to_vec())
                    }
                });
            }
            rows.push(cells);
        }
        rows.sort();
        result.insert(table, rows);
    }
    result
}

#[derive(Debug, PartialEq, Eq)]
struct Destination {
    intent: Vec<u8>,
    copied: BTreeMap<&'static str, Rows>,
}
fn destination(
    path: &Path,
    marker: &format::Marker,
    expected: &ProviderTransitionIntentRecord,
) -> Destination {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        8
    );
    assert_eq!(
        c.query_row(
            "SELECT identity FROM projection_meta WHERE singleton=1",
            [],
            |row| row.get::<_, Vec<u8>>(0)
        )
        .unwrap(),
        marker.bytes().unwrap()
    );
    assert_eq!(
        c.query_row(
            "SELECT generation FROM projection_head WHERE singleton=1",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        expected.local_generation
    );
    assert_eq!(
        c.query_row(
            "SELECT digest,payload FROM projection_imports WHERE singleton=1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        )
        .unwrap(),
        (
            expected.plan.source_prefix_id.clone(),
            expected.plan.source_prefix.encode().unwrap()
        )
    );
    assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='projection_managed_admission')", [], |row| row.get::<_, bool>(0)).unwrap());
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM projection_provider_transitions",
            [],
            |row| row.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    let (id, role, state, generation, reservation, receipt, payload, digest):
        (String, String, String, u64, Option<String>, Option<String>, Vec<u8>, String) = c.query_row(
            "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)),
        ).unwrap();
    let record = ProviderTransitionIntentRecord::decode(&payload).unwrap();
    record
        .require_row(TransitionRowData {
            id: &id,
            role: &role,
            state: &state,
            local_generation: generation,
            reservation: reservation.as_deref(),
            receipt_id: receipt.as_deref(),
            payload: &payload,
            digest: &digest,
        })
        .unwrap();
    let mut incoming = ProviderTransitionIntentRecord::decode(&expected.encode().unwrap()).unwrap();
    incoming.role = TransitionRole::Destination;
    incoming.local_generation = 0;
    incoming.encode().unwrap();
    assert_eq!(record, incoming);
    assert!(record.receipt_id.is_none());
    let result = Destination {
        intent: payload,
        copied: copied_rows(&c),
    };
    c.execute_batch("ROLLBACK").unwrap();
    c.close().unwrap();
    result
}

fn require_initializing_without_reservation(
    family: &Path,
    source: &ProviderTransitionIntentRecord,
) {
    let proof = source.destination.as_ref().unwrap();
    let owner = proof.format.identity.worktree_id.as_str();
    let c = Connection::open_with_flags(
        family.join("metadata/catalog.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let actual: (String, String, Vec<u8>, String) = c
        .query_row(
            "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
            [owner],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        actual,
        (
            proof
                .format
                .shared
                .as_ref()
                .unwrap()
                .metadata_path
                .to_str()
                .unwrap()
                .into(),
            proof.private_database_identity.clone(),
            proof.format.bytes().unwrap(),
            "initializing".into()
        )
    );
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM exact_installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
    ] {
        assert!(
            !c.query_row(sql, [owner], |row| row.get::<_, bool>(0))
                .unwrap()
        );
    }
    c.execute_batch("ROLLBACK").unwrap();
    c.close().unwrap();
}

fn live_refusal(error: ProjectionStoreError) {
    assert!(error.to_string().contains("live managed export"), "{error}");
}

#[test]
#[ignore = "ROOT source release and audited owned native child1500/outer1560 required"]
fn managed_ordinary_staging_resumes_source_gap_and_withheld_destination_close_ack() {
    // One genuine admission, protected allocation/metadata copy and three
    // separately reacquired Staging attempts justify child1500/outer1560.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_staging_resumes_source_gap_and_withheld_destination_close_ack"
        ),
        1500,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let state = store.state().unwrap();
            let original = evidence(&store);
            assert!(original.outgoing.is_empty());
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let source_database = store.marker.metadata_path(&root).join("projection.sqlite3");
            let source_identity = store
                .marker
                .shared
                .as_ref()
                .unwrap()
                .database_identity
                .clone();
            let mut identity = store.identity().clone();
            identity.worktree_id =
                WorktreeId::parse("worktree:ordinary-staging-destination").unwrap();
            let cancel = Cancellation::default();

            // First owner: real durable Copying and positive physical C copy.
            let mut owner = store
                .begin_managed_export(state.generation, identity.clone(), &cancel)
                .unwrap();
            let (planned, effect) = owner
                .begin_plan(root.parent().unwrap(), LEAF, TX, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Inserted);
            let mut allocated = allocation::allocate(&mut owner, &planned, &cancel)
                .unwrap_or_else(|error| panic!("ordinary Staging allocation: {error:?}"));
            let (copying, effect) = owner
                .commit_allocation(&planned, &allocated, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Updated);
            allocated
                .publish_format(&mut owner, &copying, &cancel)
                .unwrap();
            assert_eq!(
                owner
                    .register_allocation(&allocated, &copying, &cancel)
                    .unwrap(),
                ImportRegistrationEffect::Inserted
            );
            let mut copy_close = Close::default();
            assert_eq!(
                owner
                    .copy_metadata(&allocated, &copying, &mut copy_close, &cancel)
                    .unwrap(),
                CopyEffect::Copied
            );
            owner.close().unwrap();
            assert_eq!(copy_close.counts(), (1, 1, 1, 0));
            let destination_database = allocated.destination_database().path().to_owned();
            let destination_identity = allocated.proof().private_database_identity.clone();
            let destination_root = allocated.root().to_owned();
            let destination_lock = destination_database.parent().unwrap().join("layrs.lock");
            let copied = destination(&destination_database, &allocated.proof().format, &copying);
            require_evidence(&evidence(&store), &original, &copying);
            require_initializing_without_reservation(&family, &copying);
            let before = trees(&root, &family, &destination_root, &destination_lock);
            let staging = ordinary_transition::proposed_staging(&copying).unwrap();
            assert_eq!(staging.state, TransitionState::Staging);
            assert_eq!(
                staging.reservation.as_deref(),
                Some(staging.reservation_operation().unwrap().as_str())
            );

            // Second owner: SourceStaging commits, then the C writer is denied
            // before SQLite opens. Source progress must remain observable.
            let mut owner = store
                .begin_managed_export(state.generation, identity.clone(), &cancel)
                .unwrap();
            let mut reject = Close {
                reject_before_raw: true,
                ..Close::default()
            };
            let refusal = match owner.stage_metadata(&allocated, &copying, &mut reject, &cancel) {
                Ok(_) => panic!("destination writer open was not refused"),
                Err(error) => error,
            };
            assert!(refusal.to_string().contains(REJECT_OPEN), "{refusal}");
            owner.close().unwrap();
            assert_eq!(reject.counts(), (1, 0, 0, 0));
            let gap = trees(&root, &family, &destination_root, &destination_lock);
            only_bound_database_changed(&before, &gap, &source_database, &source_identity);
            require_evidence(&evidence(&store), &original, &staging);
            assert_eq!(
                destination(&destination_database, &allocated.proof().format, &copying),
                copied
            );
            require_initializing_without_reservation(&family, &staging);
            drop(store);

            // Third owner: the source is already Staging. C now commits its
            // own Staging, but its successful close acknowledgement is withheld.
            let reopened = ProjectionStore::open_for_managed_export(&root).unwrap();
            let mut owner = reopened
                .begin_managed_export(state.generation, identity.clone(), &cancel)
                .unwrap();
            let mut withheld = Close {
                withhold_close_ack: true,
                ..Close::default()
            };
            let refusal = match owner.stage_metadata(&allocated, &copying, &mut withheld, &cancel) {
                Ok(_) => panic!("successful destination close acknowledgement was not withheld"),
                Err(error) => error,
            };
            assert!(refusal.to_string().contains(WITHHELD_ACK), "{refusal}");
            owner.close().unwrap();
            assert_eq!(withheld.counts(), (1, 1, 1, 1));
            assert!(!withheld.withhold_close_ack);
            let staged = trees(&root, &family, &destination_root, &destination_lock);
            only_bound_database_changed(
                &gap,
                &staged,
                &destination_database,
                &destination_identity,
            );
            require_evidence(&evidence(&reopened), &original, &staging);
            let staged_destination =
                destination(&destination_database, &allocated.proof().format, &staging);
            assert_ne!(staged_destination.intent, copied.intent);
            assert_eq!(
                staged_destination.copied, copied.copied,
                "all thirteen copied rowsets survive both Staging commits"
            );
            require_initializing_without_reservation(&family, &staging);
            drop(reopened);

            // Fourth owner: acknowledge the complete persisted state through
            // exact read-only replay, with no new destination writer callback.
            let reopened = ProjectionStore::open_for_managed_export(&root).unwrap();
            let mut owner = reopened
                .begin_managed_export(state.generation, identity, &cancel)
                .unwrap();
            let outcome = owner
                .stage_metadata(&allocated, &copying, &mut withheld, &cancel)
                .unwrap();
            assert_eq!(outcome.intent, staging);
            assert_eq!(outcome.source, SourceIntentEffect::AlreadyPresent);
            assert_eq!(outcome.destination, StageEffect::AlreadyPresent);
            owner.close().unwrap();
            assert_eq!(withheld.counts(), (1, 1, 1, 1));
            assert_eq!(
                trees(&root, &family, &destination_root, &destination_lock),
                staged
            );
            require_evidence(&evidence(&reopened), &original, &staging);
            assert_eq!(
                destination(&destination_database, &allocated.proof().format, &staging),
                staged_destination
            );
            require_initializing_without_reservation(&family, &staging);
            live_refusal(
                ProjectionStore::open(&root)
                    .err()
                    .expect("ordinary B open must refuse Staging"),
            );
            live_refusal(reopened.capture_patch(state.generation).unwrap_err());
            live_refusal(
                reopened
                    .compare_and_swap(state.generation, &state.snapshot)
                    .unwrap_err(),
            );
            assert!(reopened.cancel_managed_export_plan(TX, &cancel).is_err());
            assert_eq!(
                trees(&root, &family, &destination_root, &destination_lock),
                staged
            );
            allocated.require_published(&staging.plan).unwrap();
            drop(allocated);
            // Release C's coordination lock before checking its unready state.
            assert!(ProjectionStore::open(&destination_root).is_err());
            assert_eq!(
                trees(&root, &family, &destination_root, &destination_lock),
                staged
            );
            drop(reopened);
        },
    );
}
