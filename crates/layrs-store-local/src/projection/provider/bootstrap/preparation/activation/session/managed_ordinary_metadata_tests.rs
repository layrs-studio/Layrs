//! Real B9 -> C8 physical metadata copy, with a nonempty restore-only history.
//! One successful writer close deliberately withholds acknowledgement; the
//! durable complete C8 is then recognized by an exact read-only retry.
//! C remains Copying/Initializing: no content reservation, Ready, admission,
//! inherited inverse, process-crash recovery or autonomous C is qualified here.
use crate::projection::{
    ProjectionStore, RestoreSource, SessionMode, format,
    provider::{
        database_io::{self, WriterCloseObserver, WriterOpenPoint},
        managed_admission::read_admitted_import,
        managed_export::allocation,
        ordinary_destination::CopyEffect,
        process_profile::tests::child,
        records::{
            Record,
            import::ImportSource,
            transition::{
                ProviderTransitionIntentRecord, TransitionRole, TransitionRowData, TransitionState,
            },
        },
        recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult, invalid},
        transition_sql::SourceIntentEffect,
    },
    selection,
    shared::provider_access::ImportRegistrationEffect,
};
use layrs_core::scm::{FileId, RepoPath, WorktreeId};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, OpenFlags, ffi, types::ValueRef};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const LEAF: &str = "000000000000000000000000000000a1";
const TX: &str = "000000000000000000000000000000a2";
const TARGET: &[u8] = b"ordinary metadata source captured target";
const PREIMAGE: &[u8] = b"ordinary metadata restore-only uncaptured preimage";
const WITHHELD_ACK: &str = "metadata close acknowledgement deliberately withheld";
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
    opens: usize,
    successful_closes: usize,
    withhold_next_success: bool,
    withheld_acknowledgements: usize,
}
impl WriterCloseObserver for Close {
    fn open_point(&mut self, point: WriterOpenPoint, _: *mut ffi::sqlite3) -> CauseResult<()> {
        if point == WriterOpenPoint::BeforeRaw {
            self.opens += 1;
        }
        Ok(())
    }
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        assert!(
            succeeded,
            "metadata copy requires a positive physical writer close"
        );
        self.successful_closes += 1;
        if self.withhold_next_success {
            self.withhold_next_success = false;
            self.withheld_acknowledgements += 1;
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

fn ingress(store: &ProjectionStore) -> (Vec<u8>, Vec<u8>) {
    format::access(store, SessionMode::Shared)
        .unwrap()
        .run(|access| {
            access.with_content(|c, scope| {
                let tx = c.transaction()?;
                let admitted = read_admitted_import(&tx, &store.marker, None)?;
                let result = (admitted.record().encode()?, admitted.ready().encode()?);
                scope.verify(&tx, store)?;
                tx.rollback()?;
                Ok(result)
            })
        })
        .unwrap()
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

fn rows(c: &Connection, table: &str) -> Rows {
    assert!(TABLES.contains(&table));
    let mut statement = c
        .prepare(&format!("SELECT * FROM \"{table}\" LIMIT 129"))
        .unwrap();
    let columns = statement.column_count();
    assert!(columns <= 16);
    let mut query = statement.query([]).unwrap();
    let mut result = Vec::new();
    let mut bytes = 0usize;
    while let Some(row) = query.next().unwrap() {
        assert!(result.len() < 128, "tiny copied rowset bound");
        let mut cells = Vec::with_capacity(columns);
        for column in 0..columns {
            let cell = match row.get_ref(column).unwrap() {
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
            };
            cells.push(cell);
        }
        result.push(cells);
    }
    result.sort();
    result
}

#[derive(Debug, PartialEq, Eq)]
struct Metadata {
    schema: u32,
    head: u64,
    initial: String,
    prefix: (String, Vec<u8>),
    transitions: Vec<Vec<u8>>,
    tables: BTreeMap<String, Rows>,
    preimage_mapping: (String, u64),
    has_admission_table: bool,
}

// These disposable-file checks run only after the ordinary source owner closes.
// Production authority still comes from the genuine owner/copy driver.
fn metadata(path: &Path, marker: &format::Marker, preimage: &FileId) -> Metadata {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let bytes: Vec<u8> = c
        .query_row(
            "SELECT identity FROM projection_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bytes, marker.bytes().unwrap());
    marker
        .validate_initial_selection(&selection::at(&c, 0).unwrap())
        .unwrap();
    let mut statement = c.prepare(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions ORDER BY id COLLATE BINARY",
    ).unwrap();
    let mut query = statement.query([]).unwrap();
    let mut transitions = Vec::new();
    while let Some(row) = query.next().unwrap() {
        assert!(transitions.len() < 8);
        let (id, role, state, generation, reservation, receipt, payload, digest): (
            String,
            String,
            String,
            u64,
            Option<String>,
            Option<String>,
            Vec<u8>,
            String,
        ) = (
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
            row.get(3).unwrap(),
            row.get(4).unwrap(),
            row.get(5).unwrap(),
            row.get(6).unwrap(),
            row.get(7).unwrap(),
        );
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
        transitions.push(payload);
    }
    drop(query);
    drop(statement);
    let result = Metadata {
        schema: c.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap(),
        head: c.query_row("SELECT generation FROM projection_head WHERE singleton=1", [], |row| row.get(0)).unwrap(),
        initial: c.query_row("SELECT digest FROM projection_snapshots WHERE generation=0 AND parent_digest IS NULL", [], |row| row.get(0)).unwrap(),
        prefix: c.query_row("SELECT digest,payload FROM projection_imports WHERE singleton=1", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap(),
        transitions,
        tables: TABLES.into_iter().map(|table| (table.into(), rows(&c, table))).collect(),
        preimage_mapping: c.query_row("SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1", [preimage.as_str()], |row| Ok((row.get(0)?, row.get(1)?))).unwrap(),
        has_admission_table: c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='projection_managed_admission')", [], |row| row.get(0)).unwrap(),
    };
    c.execute_batch("ROLLBACK").unwrap();
    c.close().unwrap();
    result
}

/// Independently interpret the exact outgoing content rowset in a fresh source
/// RO snapshot. B can retain other mappings (for example an unchanged snapshot
/// base) outside that historical rowset. C must copy exactly the committed
/// mappings; the later content installer separately handles the complete union.
fn copied_mapping_rows(
    path: &Path,
    marker: &format::Marker,
    copying: &ProviderTransitionIntentRecord,
) -> Rows {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let prefix = &copying.plan.source_prefix;
    let ImportSource::Ordinary { version, .. } = &prefix.source else {
        panic!("actual ordinary source required");
    };
    let (fresh, closure) = crate::projection::provider::import_validation::plan_admitted_import(
        &c,
        marker,
        prefix.destination_identity.clone(),
        &version.effective_image_digest,
        None,
    )
    .unwrap();
    assert_eq!(&fresh, prefix);
    let mappings = closure.copied_content_mappings(&c, &fresh, None).unwrap();
    assert_eq!(mappings.len() as u64, prefix.closure.content_mappings.count);
    let all_source_rows = rows(&c, "projection_content");
    let mut expected: Rows = mappings
        .into_iter()
        .map(|(file, (manifest, bytes))| {
            vec![
                Cell::Text(file.as_str().as_bytes().to_vec()),
                Cell::Text(manifest.as_str().as_bytes().to_vec()),
                Cell::Integer(i64::try_from(bytes).unwrap()),
            ]
        })
        .collect();
    expected.sort();
    for row in &expected {
        assert!(
            all_source_rows.contains(row),
            "committed mapping absent from source"
        );
    }
    assert!(
        all_source_rows.len() > expected.len(),
        "fixture must retain a source base outside the committed history mapping"
    );
    c.execute_batch("ROLLBACK").unwrap();
    c.close().unwrap();
    expected
}

fn require_initializing_without_reservation(
    family: &Path,
    copying: &ProviderTransitionIntentRecord,
) {
    let proof = copying.destination.as_ref().unwrap();
    let shared = proof.format.shared.as_ref().unwrap();
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
            shared.metadata_path.to_str().unwrap().into(),
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

#[test]
#[ignore = "ROOT source release and audited owned native child1200/outer1260 required"]
fn managed_ordinary_metadata_copies_restore_only_history_and_retries_without_writes() {
    // One genuine native admission, one local capture/restore and the full
    // protected allocation path justify a separate child1200/outer1260 bound.
    child::run(
        concat!(
            module_path!(),
            "::managed_ordinary_metadata_copies_restore_only_history_and_retries_without_writes"
        ),
        1200,
        || {
            child::declare();
            let root = super::managed_restore_recovery_tests::admitted_destination();
            let store = ProjectionStore::open(&root).unwrap();
            let original_ingress = ingress(&store);
            let state = store.state().unwrap();
            let path = RepoPath::parse("README.md").unwrap();
            let checkout = root.join(path.as_str());
            fs::write(&checkout, TARGET).unwrap();
            let target = store.capture_patch(state.generation).unwrap();
            fs::write(&checkout, PREIMAGE).unwrap();
            let preview = store
                .preview_restore(
                    state.generation,
                    &RestoreSource::Capture {
                        sequence: target.sequence,
                        paths: vec![path.clone()],
                    },
                )
                .unwrap();
            let preimage = preview.before[&path].as_ref().unwrap().clone();
            assert_eq!(preimage.file_id, FileId::from_bytes(PREIMAGE));
            let restored = store.apply_restore(&preview).unwrap();
            assert_eq!(restored.capture.sequence, target.sequence + 1);
            assert_eq!(fs::read(&checkout).unwrap(), TARGET);
            for sequence in 1..=restored.capture.sequence {
                assert!(
                    store
                        .captured_patch(sequence)
                        .unwrap()
                        .changes
                        .iter()
                        .filter_map(|change| change.replacement.as_ref())
                        .all(|file| file.file_id != preimage.file_id)
                );
            }
            drop(store);
            // Settle the completed restore budget before any byte witness is
            // attributed to export; later source reopen is the managed route.
            let store = ProjectionStore::open(&root).unwrap();
            assert_eq!(store.state().unwrap(), state);
            assert_eq!(ingress(&store), original_ingress);
            let family = store.marker.shared.as_ref().unwrap().root.clone();
            let source_database = store.marker.metadata_path(&root).join("projection.sqlite3");
            let mut destination = store.identity().clone();
            destination.worktree_id =
                WorktreeId::parse("worktree:ordinary-metadata-destination").unwrap();
            let cancel = Cancellation::default();
            let cancelled = Cancellation::default();
            cancelled.cancel();
            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            let (planned, effect) = owner
                .begin_plan(root.parent().unwrap(), LEAF, TX, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Inserted);
            assert_eq!(
                planned
                    .plan
                    .source_prefix
                    .closure
                    .restore_history
                    .as_ref()
                    .unwrap()
                    .count,
                1
            );
            assert!(owner.retained_manifest(&preimage.file_id).is_some());
            let mut allocated = allocation::allocate(&mut owner, &planned, &cancel)
                .unwrap_or_else(|error| panic!("ordinary metadata allocation: {error:?}"));
            let (copying, effect) = owner
                .commit_allocation(&planned, &allocated, &cancel)
                .unwrap();
            assert_eq!(effect, SourceIntentEffect::Updated);
            assert_eq!(copying.role, TransitionRole::Source);
            assert_eq!(copying.state, TransitionState::Copying);
            allocated
                .publish_format(&mut owner, &copying, &cancel)
                .unwrap();
            assert_eq!(
                owner
                    .register_allocation(&allocated, &copying, &cancel)
                    .unwrap(),
                ImportRegistrationEffect::Inserted
            );
            owner.close().unwrap();
            let destination_database = allocated.destination_database().path().to_owned();
            let destination_lock = destination_database.parent().unwrap().join("layrs.lock");
            let destination_root = allocated.root().to_owned();
            assert_eq!(fs::metadata(&destination_database).unwrap().len(), 0);
            let before = trees(&root, &family, &destination_root, &destination_lock);
            let source_metadata = metadata(&source_database, &store.marker, &preimage.file_id);
            let expected_mappings = copied_mapping_rows(&source_database, &store.marker, &copying);
            assert_eq!(source_metadata.schema, 9);
            assert!(source_metadata.has_admission_table);
            assert_eq!(
                source_metadata.tables["projection_restore_history"].len(),
                1
            );
            assert_eq!(source_metadata.preimage_mapping.1, PREIMAGE.len() as u64);
            assert!(
                source_metadata
                    .transitions
                    .contains(&copying.encode().unwrap())
            );
            require_initializing_without_reservation(&family, &copying);

            let mut close = Close::default();
            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            assert!(
                owner
                    .copy_metadata(&allocated, &copying, &mut close, &cancelled)
                    .is_err()
            );
            let mut wrong =
                ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
            let proof = wrong.destination.as_mut().unwrap();
            proof.format_file_identity = if proof.private_database_identity == "0:0:0" {
                "0:0:1"
            } else {
                "0:0:0"
            }
            .into();
            wrong.encode().unwrap();
            assert!(
                owner
                    .copy_metadata(&allocated, &wrong, &mut close, &cancel)
                    .is_err()
            );
            owner.close().unwrap();
            assert_eq!((close.opens, close.successful_closes), (0, 0));
            assert_eq!(close.withheld_acknowledgements, 0);
            assert_eq!(
                trees(&root, &family, &destination_root, &destination_lock),
                before
            );

            let mut owner = store
                .begin_managed_export(state.generation, destination.clone(), &cancel)
                .unwrap();
            close.withhold_next_success = true;
            let refused = owner
                .copy_metadata(&allocated, &copying, &mut close, &cancel)
                .unwrap_err();
            assert!(refused.to_string().contains(WITHHELD_ACK), "{refused}");
            owner.close().unwrap();
            assert_eq!((close.opens, close.successful_closes), (1, 1));
            assert_eq!(close.withheld_acknowledgements, 1);
            assert!(!close.withhold_next_success);
            assert_eq!(ingress(&store), original_ingress);
            // The observer reported a physically successful close but withheld
            // its acknowledgement. Err therefore does not imply rollback: the
            // separately opened readonly snapshot must find complete durable C8.
            let copied = metadata(
                &destination_database,
                &allocated.proof().format,
                &preimage.file_id,
            );
            assert_eq!(copied.schema, 8);
            assert!(!copied.has_admission_table);
            assert_eq!(copied.initial, store.marker.initial_snapshot_digest);
            assert_eq!(copied.initial, source_metadata.initial);
            let ImportSource::Ordinary { version, .. } = &copying.plan.source_prefix.source else {
                panic!("actual source prefix is not ordinary");
            };
            assert_eq!(copied.head, version.generation);
            assert_eq!(copied.head, source_metadata.head);
            assert!(copied.head > 0, "the copied head is not generation zero");
            assert_eq!(
                copied.prefix,
                (
                    copying.plan.source_prefix_id.clone(),
                    copying.plan.source_prefix.encode().unwrap()
                )
            );
            for table in TABLES {
                let expected = if table == "projection_content" {
                    &expected_mappings
                } else {
                    &source_metadata.tables[table]
                };
                assert_eq!(
                    &copied.tables[table], expected,
                    "complete committed rowset differs: {table}"
                );
            }
            assert_eq!(copied.preimage_mapping, source_metadata.preimage_mapping);
            let mut incoming =
                ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
            incoming.role = TransitionRole::Destination;
            incoming.local_generation = 0;
            incoming.encode().unwrap();
            assert_eq!(incoming.state, TransitionState::Copying);
            assert!(incoming.reservation.is_none() && incoming.receipt_id.is_none());
            assert_ne!(incoming.encode().unwrap(), copying.encode().unwrap());
            assert_eq!(copied.transitions, vec![incoming.encode().unwrap()]);
            require_initializing_without_reservation(&family, &copying);
            let after = trees(&root, &family, &destination_root, &destination_lock);
            assert_eq!(after.0, before.0);
            assert_eq!(after.2, before.2);
            assert_eq!(
                after.1.keys().collect::<Vec<_>>(),
                before.1.keys().collect::<Vec<_>>()
            );
            for (path, node) in &before.1 {
                if path != &destination_database {
                    assert_eq!(
                        after.1.get(path),
                        Some(node),
                        "metadata copy changed unrelated path {path:?}"
                    );
                }
            }
            let populated_database = database_io::tests::support::witness(&destination_database);
            assert_eq!(
                populated_database.0,
                allocated.proof().private_database_identity
            );
            assert!(populated_database.1 > 0);
            drop(store);

            let reopened = ProjectionStore::open_for_managed_export(&root).unwrap();
            let mut owner = reopened
                .begin_managed_export(state.generation, destination, &cancel)
                .unwrap();
            assert_eq!(
                owner
                    .copy_metadata(&allocated, &copying, &mut close, &cancel)
                    .unwrap(),
                CopyEffect::AlreadyPresent
            );
            owner.close().unwrap();
            assert_eq!(
                (close.opens, close.successful_closes),
                (1, 1),
                "copied retry never opens a writer"
            );
            assert_eq!(close.withheld_acknowledgements, 1);
            assert_eq!(
                trees(&root, &family, &destination_root, &destination_lock),
                after
            );
            assert_eq!(
                metadata(
                    &destination_database,
                    &allocated.proof().format,
                    &preimage.file_id
                ),
                copied
            );
            assert_eq!(ingress(&reopened), original_ingress);
            allocated.require_published(&copying.plan).unwrap();
            drop(allocated);
            // Release C's exclusive coordination lock first so this refusal
            // exercises its unready persisted state, rather than lock contention.
            assert!(ProjectionStore::open(&destination_root).is_err());
            assert_eq!(
                trees(&root, &family, &destination_root, &destination_lock),
                after
            );
            drop(reopened);
        },
    );
}
