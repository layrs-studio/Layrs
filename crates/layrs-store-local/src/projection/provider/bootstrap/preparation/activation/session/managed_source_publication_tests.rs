//! One owned retained-checkpoint fixture: durable SourceReady ACK refusal,
//! one-writer retry to Published and zero-writer replay. No BUSY/crash hooks,
//! autonomous admission, source content rehash or cleanup is exercised.
use super::*;
use crate::projection::provider::{
    checkout_journal,
    database_io::{self, WriterCloseObserver, WriterOpenPoint},
    process_profile::tests::child,
    records::{
        Record,
        import::{ProviderTransitionReceiptRecord, TransitionReceiptRowData},
        transition::{ProviderTransitionIntentRecord, TransitionRowData, TransitionState},
    },
    validation,
};
use rusqlite::{
    Connection, DatabaseName, OpenFlags, ffi,
    types::{Value, ValueRef},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
struct Close {
    opens: usize,
    closes: usize,
    refuse_first_ack: bool,
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
            "source publication fixture requires positive physical writer close"
        );
        self.closes += 1;
        (self.refuse_first_ack && self.closes == 1)
            .then(|| invalid("source Ready acknowledgement withheld"))
    }
}

fn read_only<T>(path: &Path, read: impl FnOnce(&Connection) -> T) -> T {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(c.is_readonly(DatabaseName::Main).unwrap());
    c.execute_batch("BEGIN DEFERRED").unwrap();
    let value = read(&c);
    c.close().unwrap();
    value
}

type Rows = BTreeMap<String, Vec<Vec<Value>>>;
type SqlWitness = (u32, Vec<(String, String, String)>, Rows);

/// Tiny fixture observer, bounded before allocating each cell. Source excludes
/// ONLY the two intentionally mutated relations; destination/family exclude none.
fn sql_witness(c: &Connection, exclude_source_publication: bool) -> SqlWitness {
    let (count, largest): (u64, u64) = c
        .query_row(
            "SELECT count(*),coalesce(max(length(CAST(name AS BLOB))),0) FROM sqlite_schema \
         WHERE type='table' AND substr(name,1,7) COLLATE NOCASE!='sqlite_'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(count <= 64 && largest <= 128);
    let tables = c.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND substr(name,1,7) COLLATE NOCASE!='sqlite_' ORDER BY name COLLATE BINARY LIMIT 65").unwrap()
        .query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert_eq!(tables.len() as u64, count);
    if exclude_source_publication {
        assert!(
            tables
                .iter()
                .any(|s| s == "projection_provider_transitions")
        );
        assert!(
            tables
                .iter()
                .any(|s| s == "projection_provider_transition_receipts")
        );
    }
    let mut all = BTreeMap::new();
    let mut aggregate = 0usize;
    for table in tables {
        assert!(
            table
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        if exclude_source_publication
            && matches!(
                table.as_str(),
                "projection_provider_transitions" | "projection_provider_transition_receipts"
            )
        {
            continue;
        }
        let count: u64 = c
            .query_row(
                &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT 33)"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(count <= 32, "tiny fixture table row bound: {table}");
        let mut statement = c
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid LIMIT 33"))
            .unwrap();
        let columns = statement.column_count();
        assert!(columns <= 32);
        let mut rows = statement.query([]).unwrap();
        let mut values = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            assert!(values.len() < 32);
            let mut cells = Vec::new();
            for ordinal in 0..columns {
                let bytes = match row.get_ref(ordinal).unwrap() {
                    ValueRef::Text(v) | ValueRef::Blob(v) => v.len(),
                    _ => 8,
                };
                assert!(bytes <= 524288, "fixture SQL cell bound: {table}");
                aggregate = aggregate.checked_add(bytes).unwrap();
                assert!(aggregate <= 8 * 1024 * 1024, "fixture SQL aggregate bound");
                cells.push(row.get::<_, Value>(ordinal).unwrap());
            }
            values.push(cells);
        }
        all.insert(table, values);
    }
    (
        c.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        database_io::schema_inventory(c).unwrap(),
        all,
    )
}

fn witnesses(paths: &[PathBuf]) -> Vec<(String, u64, String)> {
    paths
        .iter()
        .map(|path| database_io::tests::support::witness(path))
        .collect()
}
fn no_sidecars(paths: &[PathBuf]) {
    for path in paths {
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            assert!(
                !PathBuf::from(sidecar).exists(),
                "fixture sidecar persisted: {path:?}{suffix}"
            );
        }
    }
}

fn terminal_row(
    c: &Connection,
    expected: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) {
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM projection_provider_transitions",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM projection_provider_transition_receipts",
            [],
            |r| r.get::<_, u64>(0)
        )
        .unwrap(),
        1
    );
    assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE length(CAST(id AS BLOB))>128 OR length(CAST(role AS BLOB))>16 OR length(CAST(state AS BLOB))>16 OR length(CAST(reservation AS BLOB))>128 OR length(CAST(receipt_id AS BLOB))>71 OR length(payload)>262144 OR length(CAST(digest AS BLOB))>71)",[],|r|r.get::<_,bool>(0)).unwrap());
    let (id,role,state,generation,reservation,receipt_id,payload,digest):(String,String,String,u64,Option<String>,Option<String>,Vec<u8>,String) = c.query_row(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest FROM projection_provider_transitions",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).unwrap();
    let observed = ProviderTransitionIntentRecord::decode(&payload).unwrap();
    observed
        .require_row(TransitionRowData {
            id: &id,
            role: &role,
            state: &state,
            local_generation: generation,
            reservation: reservation.as_deref(),
            receipt_id: receipt_id.as_deref(),
            payload: &payload,
            digest: &digest,
        })
        .unwrap();
    observed.require_receipt(receipt).unwrap();
    assert_eq!(&observed, expected);
    assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM projection_provider_transition_receipts WHERE length(CAST(id AS BLOB))>71 OR length(CAST(owner_identity_digest AS BLOB))>71 OR length(CAST(transaction_id AS BLOB))>128 OR length(CAST(companion_id AS BLOB))>71 OR length(payload)>65536)",[],|r|r.get::<_,bool>(0)).unwrap());
    let (id,owner,tx,companion,payload):(String,String,String,Option<String>,Vec<u8>) = c.query_row(
        "SELECT id,owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
    let observed = ProviderTransitionReceiptRecord::decode(&payload).unwrap();
    observed
        .require_row(TransitionReceiptRowData {
            id: &id,
            owner_identity_digest: &owner,
            transaction_id: &tx,
            companion_id: companion.as_deref(),
            payload: &payload,
        })
        .unwrap();
    assert_eq!(&observed, receipt);
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned native source publication recipe"]
fn managed_source_ready_ack_refusal_retries_published_then_replays_without_writer() {
    child::run(
        concat!(
            module_path!(),
            "::managed_source_ready_ack_refusal_retries_published_then_replays_without_writer"
        ),
        240,
        || {
            child::declare();
            super::managed_staging_tests::with_content_planned_bytes_then(
                b"provider base content",
                |fixture, session, attempt, planned| {
                    let cancel = Cancellation::default();
                    let (mut destination, copying) =
                        super::managed_allocation_tests::bound_holder(session, attempt, planned);
                    destination
                        .publish_format(
                            &session.source.root,
                            &session.source.record,
                            &session.source.activation,
                            attempt,
                            &copying,
                            &cancel,
                        )
                        .unwrap();
                    let held = &session.source.held;
                    let mut setup = Close::default();
                    super::super::managed_destination::register_and_copy(
                        held,
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &copying,
                        &destination,
                        &mut setup,
                        &cancel,
                    )
                    .unwrap();
                    let active = {
                        let _locks = held
                            .family
                            .locks(
                                &held.artifacts.allocation.format.identity.worktree_id,
                                false,
                            )
                            .unwrap();
                        capture_state::read(held, Some(&session.source.root), Some(&cancel))
                            .unwrap_or_else(|error| panic!("active read: {}", error.cause))
                    };
                    let fresh = active.active.source.initial().clone();
                    let mut checkpoint = session.session.export_checkpoint().unwrap();
                    let mut image = super::super::managed_image::plan(
                        &mut checkpoint,
                        session.source.as_ref(),
                        &active.active.source,
                        &active.active.activated.snapshot,
                        &cancel,
                    )
                    .unwrap();
                    assert_eq!(image.files().len(), 1);
                    let current = image.files()[0].manifest.manifest().id().clone();
                    let required =
                        super::managed_staging_tests::manifests_for(&active.active.source, &image);
                    assert_eq!(required.len(), 2);
                    assert_eq!(required[0].file_id(), required[1].file_id());
                    assert_ne!(required[0].id(), required[1].id());
                    let mut staging =
                        ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
                    staging.state = TransitionState::Staging;
                    staging.reservation = Some(staging.reservation_operation().unwrap());
                    super::super::managed_staging::stage(
                        held,
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &copying,
                        &staging,
                        &destination,
                        &mut image,
                        &required,
                        &mut setup,
                        &cancel,
                    )
                    .unwrap();
                    let mut lease = super::super::managed_content::ManagedImportLease::open(
                        held,
                        &session.source.root,
                        &session.source.record,
                        &session.source.activation,
                        attempt,
                        &destination,
                        &mut image,
                        &staging,
                        &required,
                        &fresh,
                        &cancel,
                    )
                    .unwrap();
                    lease.reserve_plan(&mut setup, &cancel).unwrap();
                    let mut installed = BTreeSet::new();
                    for id in std::iter::once(&current)
                        .chain(required.iter().map(|manifest| manifest.id()))
                    {
                        if installed.insert(id.clone()) {
                            lease.install_exact(id, &mut setup, &cancel).unwrap();
                        }
                    }
                    assert_eq!(installed.len(), 2);
                    let protected = lease.protect_exact(&mut setup, &cancel).unwrap();
                    let checkout = lease
                        .copy_checkout(&protected, &mut setup, &cancel)
                        .unwrap();
                    // Exactly one private Ready finalization and one normal family
                    // publication. Every fault below belongs to SOURCE publication.
                    let private = lease
                        .finalize_private_ready(&protected, &checkout, &mut setup, &cancel)
                        .unwrap();
                    let family = lease
                        .publish_family_ready(&private, &mut setup, &cancel)
                        .unwrap();
                    assert_eq!(setup.opens, setup.closes);
                    let receipt_bytes = family.receipt().encode().unwrap();
                    let receipt_id = family.receipt().digest().unwrap();
                    let mut source_ready =
                        ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
                    source_ready.state = TransitionState::Ready;
                    source_ready.receipt_id = Some(receipt_id.clone());
                    source_ready.require_transition(&staging).unwrap();
                    source_ready.require_receipt(family.receipt()).unwrap();
                    let mut source_published =
                        ProviderTransitionIntentRecord::decode(&source_ready.encode().unwrap())
                            .unwrap();
                    source_published.state = TransitionState::Published;
                    source_published.require_transition(&source_ready).unwrap();
                    let source_path = fixture.metadata.join("projection.sqlite3");
                    let destination_path = destination.destination_database().path().to_owned();
                    let family_path = fixture.family.root().join("metadata/catalog.sqlite3");
                    let databases = [
                        source_path.clone(),
                        destination_path.clone(),
                        family_path.clone(),
                    ];
                    let stable_paths = [
                        destination_path.clone(),
                        family_path.clone(),
                        destination.root().join("README.md"),
                        destination.root().join(".layrs/FORMAT"),
                        fixture.family.root().join("FORMAT"),
                    ];
                    // Generic no-follow witnesses must not traverse the live
                    // provider root (a ProjFS reparse point). Source FORMAT is
                    // checked through the retained native owner by boundary().
                    let stable_fs = witnesses(&stable_paths);
                    let source_file_before = database_io::tests::support::witness(&source_path);
                    let (source_rest, destination_sql, family_sql) = {
                        let _locks = held
                            .family
                            .locks(
                                &held.artifacts.allocation.format.identity.worktree_id,
                                false,
                            )
                            .unwrap();
                        (
                            read_only(&source_path, |c| sql_witness(c, true)),
                            read_only(&destination_path, |c| sql_witness(c, false)),
                            read_only(&family_path, |c| sql_witness(c, false)),
                        )
                    };
                    let journal_before = read_only(&destination_path, |c| {
                        let journal =
                            checkout_journal::read(c, checkout.expected(), Some(&cancel)).unwrap();
                        assert!(journal.is_complete());
                        assert_eq!(journal.rows().len(), 3);
                        journal
                            .rows()
                            .iter()
                            .map(|row| row.encode().unwrap())
                            .collect::<Vec<_>>()
                    });
                    assert_eq!(
                        fs::read(destination.root().join("README.md")).unwrap(),
                        b"provider base content"
                    );
                    no_sidecars(&databases);
                    let assert_stable =
                        |lease: &super::super::managed_content::ManagedImportLease<
                            '_,
                            '_,
                            '_,
                            '_,
                            '_,
                        >,
                         expected: &ProviderTransitionIntentRecord| {
                            private.require_physical(lease, &cancel).unwrap();
                            assert_eq!(
                                witnesses(&stable_paths),
                                stable_fs,
                                "source publication cannot alter family/quota/private Ready/outputs/FORMAT"
                            );
                            no_sidecars(&databases);
                            super::managed_live_read_tests::assert_retained_terminal_reads(
                                session,
                                expected,
                                family.receipt(),
                            );
                            let _locks = held
                                .family
                                .locks(
                                    &held.artifacts.allocation.format.identity.worktree_id,
                                    false,
                                )
                                .unwrap();
                            read_only(&source_path, |c| {
                                assert_eq!(
                                    sql_witness(c, true),
                                    source_rest,
                                    "all other source tables/schema/capture/catalog remain exact"
                                );
                                terminal_row(c, expected, family.receipt());
                                let proof = validation::private_active_publication_with_source(
                                    c,
                                    &held.artifacts.allocation,
                                    held.artifacts.prepared.as_ref().unwrap(),
                                    &held.binding,
                                    Some(session.source.root.registration().root_id()),
                                    Some(&cancel),
                                )
                                .unwrap();
                                assert_eq!(proof.transitions.len(), 1);
                                assert_eq!(proof.receipts[0].encode().unwrap(), receipt_bytes);
                                assert_eq!(
                                    proof.transitions[0].plan.source_prefix.encode().unwrap(),
                                    staging.plan.source_prefix.encode().unwrap()
                                );
                                if expected.state == TransitionState::Ready {
                                    assert!(proof.require_no_pending_transition().is_err());
                                } else {
                                    proof.require_no_pending_transition().unwrap();
                                }
                            });
                            assert_eq!(
                                read_only(&family_path, |c| sql_witness(c, false)),
                                family_sql
                            );
                            read_only(&destination_path, |c| {
                                assert_eq!(sql_witness(c, false), destination_sql);
                                let journal =
                                    checkout_journal::read(c, checkout.expected(), Some(&cancel))
                                        .unwrap();
                                assert_eq!(
                                    journal
                                        .rows()
                                        .iter()
                                        .map(|row| row.encode().unwrap())
                                        .collect::<Vec<_>>(),
                                    journal_before
                                );
                            });
                        };
                    let mut refusal = Close {
                        refuse_first_ack: true,
                        ..Close::default()
                    };
                    let error = lease
                        .publish_source(&family, &mut refusal, &cancel)
                        .err()
                        .expect(
                            "durable SourceReady cannot be acknowledged after observer refusal",
                        );
                    assert!(
                        error
                            .to_string()
                            .contains("source Ready acknowledgement withheld"),
                        "{error}"
                    );
                    assert_eq!((refusal.opens, refusal.closes), (1, 1));
                    assert_stable(&lease, &source_ready);
                    let ready_file = database_io::tests::support::witness(&source_path);
                    assert_eq!(ready_file.0, source_file_before.0);
                    assert_ne!(ready_file.2, source_file_before.2);
                    let mut retry = Close::default();
                    let published = lease.publish_source(&family, &mut retry, &cancel).unwrap();
                    assert_eq!((retry.opens, retry.closes), (1, 1));
                    assert_eq!(published.source_published(), &source_published);
                    assert_eq!(published.receipt().encode().unwrap(), receipt_bytes);
                    assert_eq!(published.receipt().digest().unwrap(), receipt_id);
                    drop(published);
                    assert_stable(&lease, &source_published);
                    let published_file = database_io::tests::support::witness(&source_path);
                    assert_eq!(published_file.0, ready_file.0);
                    assert_ne!(published_file.2, ready_file.2);
                    let mut replay = Close::default();
                    let published = lease.publish_source(&family, &mut replay, &cancel).unwrap();
                    assert_eq!((replay.opens, replay.closes), (0, 0));
                    assert_eq!(published.source_published(), &source_published);
                    assert_eq!(published.receipt().encode().unwrap(), receipt_bytes);
                    drop(published);
                    assert_stable(&lease, &source_published);
                    assert_eq!(
                        database_io::tests::support::witness(&source_path),
                        published_file
                    );
                    let stage = destination
                        .root()
                        .join(format!(".layrs/checkout-{}", staging.transaction_id));
                    assert!(fs::read_dir(stage).unwrap().next().is_none());
                    drop(family);
                    drop(private);
                    drop(checkout);
                    drop(protected);
                    drop(lease);
                    drop(image);
                    drop(checkpoint);
                    (
                        source_published,
                        ProviderTransitionReceiptRecord::decode(&receipt_bytes).unwrap(),
                    )
                },
                |_, session, (published, receipt)| {
                    super::managed_post_publication_capture_tests::capture_after_published(
                        session, &published, &receipt,
                    );
                },
            );
        },
    );
}
