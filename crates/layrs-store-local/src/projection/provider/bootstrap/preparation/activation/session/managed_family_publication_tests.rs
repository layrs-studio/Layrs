//! One retained native fixture covers the family CAS, post-close ACK refusal
//! and no-writer replay. No BUSY, process restart or ordinary admission here.
use super::*;
use crate::projection::provider::{
    checkout_journal,
    database_io::{self, WriterCloseObserver, WriterOpenPoint},
    process_profile::tests::child,
    records::{
        transition::{ProviderTransitionIntentRecord, TransitionState},
        Record,
    },
};
use rusqlite::{
    ffi,
    types::{Value, ValueRef},
    Connection, DatabaseName, OpenFlags,
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
    refuse_acknowledgement: bool,
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
            "family publication fixture requires positive physical close"
        );
        self.closes += 1;
        self.refuse_acknowledgement
            .then(|| invalid("family Ready acknowledgement withheld"))
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
fn witnesses(paths: &[PathBuf]) -> Vec<(String, u64, String)> {
    paths
        .iter()
        .map(|p| database_io::tests::support::witness(p))
        .collect()
}

type FamilyRows = BTreeMap<String, Vec<Vec<Value>>>;
fn family_rows(c: &Connection) -> FamilyRows {
    // The complete family schema has thirteen tables. Bound both rows and
    // cell sizes before allocating observed blobs in this tiny owned fixture.
    let tables = [
        "family",
        "quota",
        "worktrees",
        "files",
        "chunks",
        "manifests",
        "manifest_chunks",
        "roots",
        "root_manifests",
        "reservations",
        "installations",
        "exact_installations",
        "sweep",
    ];
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get::<_, usize>(0)
        )
        .unwrap(),
        tables.len()
    );
    assert_eq!(
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
            .unwrap(),
        2
    );
    let mut out = BTreeMap::new();
    let mut aggregate = 0usize;
    for table in tables {
        let mut query = c
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid LIMIT 33"))
            .unwrap();
        let columns = query.column_count();
        assert!(columns <= 8);
        let mut rows = query.query([]).unwrap();
        let mut values = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            assert!(values.len() < 32, "fixture family row budget");
            let mut cells = Vec::new();
            for ordinal in 0..columns {
                let bytes = match row.get_ref(ordinal).unwrap() {
                    ValueRef::Text(v) | ValueRef::Blob(v) => v.len(),
                    _ => 8,
                };
                assert!(bytes <= 262144, "fixture family cell budget");
                aggregate = aggregate.checked_add(bytes).unwrap();
                assert!(
                    aggregate <= 2 * 1024 * 1024,
                    "fixture family aggregate budget"
                );
                cells.push(row.get::<_, Value>(ordinal).unwrap());
            }
            values.push(cells);
        }
        out.insert(table.to_owned(), values);
    }
    out
}

#[test]
#[ignore = "UNQUALIFIED until ROOT releases this exact owned native family publication recipe"]
fn managed_family_ready_committed_ack_refusal_replays_without_writer() {
    child::run(
        concat!(
            module_path!(),
            "::managed_family_ready_committed_ack_refusal_replays_without_writer"
        ),
        240,
        || {
            child::declare();
            // Source Single and current fixed representation share these exact
            // bytes. Current installation supplies the historical resident chunk.
            super::managed_staging_tests::with_content_planned_bytes(
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
                            .unwrap_or_else(|e| panic!("active read: {}", e.cause))
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
                    for id in std::iter::once(&current).chain(required.iter().map(|m| m.id())) {
                        if installed.insert(id.clone()) {
                            lease.install_exact(id, &mut setup, &cancel).unwrap();
                        }
                    }
                    let protected = lease.protect_exact(&mut setup, &cancel).unwrap();
                    let checkout = lease
                        .copy_checkout(&protected, &mut setup, &cancel)
                        .unwrap();
                    let private = lease
                        .finalize_private_ready(&protected, &checkout, &mut setup, &cancel)
                        .unwrap();
                    private.require_physical(&lease, &cancel).unwrap();
                    assert_eq!(setup.opens, setup.closes);
                    let receipt = private.receipt().encode().unwrap();
                    let receipt_id = private.receipt().digest().unwrap();
                    let ready_intent = private.ready_intent().encode().unwrap();
                    let source_path = fixture.metadata.join("projection.sqlite3");
                    let private_path = destination.destination_database().path().to_owned();
                    let family_path = fixture.family.root().join("metadata/catalog.sqlite3");
                    let stable_paths = [
                        source_path.clone(),
                        private_path.clone(),
                        destination.root().join("README.md"),
                        destination.root().join(".layrs/FORMAT"),
                    ];
                    let stable_before = witnesses(&stable_paths);
                    assert_eq!(
                        fs::read(destination.root().join("README.md")).unwrap(),
                        b"provider base content"
                    );
                    let journal_before = read_only(&private_path, |c| {
                        let journal =
                            checkout_journal::read(c, checkout.expected(), Some(&cancel)).unwrap();
                        assert!(journal.is_complete());
                        assert_eq!(journal.rows().len(), 3);
                        journal
                            .rows()
                            .iter()
                            .map(|r| r.encode().unwrap())
                            .collect::<Vec<_>>()
                    });
                    let family_before = {
                        let _locks = held
                            .family
                            .locks(
                                &held.artifacts.allocation.format.identity.worktree_id,
                                false,
                            )
                            .unwrap();
                        read_only(&family_path, family_rows)
                    };
                    let family_file_before = database_io::tests::support::witness(&family_path);
                    let mut expected_family = family_before.clone();
                    let owner = staging
                        .plan
                        .source_prefix
                        .destination_identity
                        .worktree_id
                        .as_str();
                    let rows = expected_family.get_mut("worktrees").unwrap();
                    let target = rows
                        .iter_mut()
                        .find(|r| r[0] == Value::Text(owner.to_owned()))
                        .unwrap();
                    assert_eq!(target.len(), 5);
                    assert_eq!(target[4], Value::Text("initializing".into()));
                    target[4] = Value::Text("ready".into());
                    let mut refusal = Close {
                        refuse_acknowledgement: true,
                        ..Close::default()
                    };
                    let error = lease
                        .publish_family_ready(&private, &mut refusal, &cancel)
                        .err()
                        .expect("family COMMIT is not acknowledged after observer refusal");
                    assert!(
                        error
                            .to_string()
                            .contains("family Ready acknowledgement withheld"),
                        "{error}"
                    );
                    assert_eq!((refusal.opens, refusal.closes), (1, 1));
                    private.require_physical(&lease, &cancel).unwrap();
                    assert_eq!(
                        witnesses(&stable_paths),
                        stable_before,
                        "family publication cannot mutate source/private Ready/output bytes"
                    );
                    {
                        let _locks = held
                            .family
                            .locks(
                                &held.artifacts.allocation.format.identity.worktree_id,
                                false,
                            )
                            .unwrap();
                        assert_eq!(read_only(&family_path,family_rows),expected_family,"only the exact destination worktrees.state cell changes; reservations/quota/journals/protection remain retained");
                    }
                    let family_durable = database_io::tests::support::witness(&family_path);
                    assert_eq!(family_durable.0, family_file_before.0);
                    assert_ne!(family_durable.2, family_file_before.2);
                    let mut replay = Close::default();
                    let published = lease
                        .publish_family_ready(&private, &mut replay, &cancel)
                        .unwrap();
                    assert_eq!(published.receipt().encode().unwrap(), receipt);
                    assert_eq!(published.receipt().digest().unwrap(), receipt_id);
                    assert_eq!((replay.opens, replay.closes), (0, 0));
                    drop(published);
                    assert_eq!(
                        database_io::tests::support::witness(&family_path),
                        family_durable
                    );
                    assert_eq!(witnesses(&stable_paths), stable_before);
                    {
                        let _locks = held
                            .family
                            .locks(
                                &held.artifacts.allocation.format.identity.worktree_id,
                                false,
                            )
                            .unwrap();
                        assert_eq!(read_only(&family_path, family_rows), expected_family);
                        read_only(&source_path, |c| {
                            let (state,payload,receipt): (String,Vec<u8>,Option<String>) = c.query_row("SELECT state,payload,receipt_id FROM projection_provider_transitions WHERE id=?1 AND length(payload)<=262144",[staging.transaction_id.as_str()],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                            assert_eq!(state, "staging");
                            assert_eq!(payload, staging.encode().unwrap());
                            assert!(receipt.is_none());
                        });
                    }
                    read_only(&private_path, |c| {
                        let (state,payload,id): (String,Vec<u8>,String) = c.query_row("SELECT state,payload,receipt_id FROM projection_provider_transitions WHERE id=?1 AND length(payload)<=262144",[staging.transaction_id.as_str()],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                        assert_eq!(state, "ready");
                        assert_eq!(payload, ready_intent);
                        assert_eq!(id, receipt_id);
                        let payload: Vec<u8> = c.query_row("SELECT payload FROM projection_provider_transition_receipts WHERE id=?1 AND length(payload)<=65536",[&receipt_id],|r| r.get(0)).unwrap();
                        assert_eq!(payload, receipt);
                        let journal =
                            checkout_journal::read(c, checkout.expected(), Some(&cancel)).unwrap();
                        assert_eq!(
                            journal
                                .rows()
                                .iter()
                                .map(|r| r.encode().unwrap())
                                .collect::<Vec<_>>(),
                            journal_before
                        );
                        assert_eq!(
                            c.query_row(
                                "SELECT generation FROM projection_head WHERE singleton=1",
                                [],
                                |r| r.get::<_, u64>(0)
                            )
                            .unwrap(),
                            staging.plan.placement_generation
                        );
                    });
                    let stage = destination
                        .root()
                        .join(format!(".layrs/checkout-{}", staging.transaction_id));
                    assert!(fs::read_dir(stage).unwrap().next().is_none());
                    drop(private);
                    drop(checkout);
                    drop(protected);
                    drop(lease);
                    drop(image);
                    drop(checkpoint);
                },
            );
        },
    );
}
