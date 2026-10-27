//! Private Ready under the same holders/checkpoint. The injected fault refuses
//! an acknowledgement after successful close; it is not BUSY or crash recovery.
use super::*;
use crate::projection::provider::{
    checkout_journal::{
        self, CheckoutEntry, CheckoutRow, DirectoryState, ExpectedCheckout, FileState, RowKey,
    },
    database_io::{self, WriterCloseObserver, WriterOpenPoint},
    process_profile::tests::child,
    records::{
        import::ProviderTransitionReceiptRecord,
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        Record,
    },
};
use rusqlite::{ffi, Connection, DatabaseName, OpenFlags};
use std::{
    collections::BTreeSet,
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
        assert!(succeeded, "private Ready writer must close physically");
        self.closes += 1;
        self.refuse_acknowledgement
            .then(|| invalid("private Ready acknowledgement withheld"))
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

type JournalRows = Vec<(String, String, String, Vec<u8>, String)>;
fn journal_rows(c: &Connection, staging: &ProviderTransitionIntentRecord) -> JournalRows {
    assert!(!c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_prejournal WHERE length(CAST(path AS BLOB))>512 OR length(CAST(operation AS BLOB))>128 OR length(CAST(digest AS BLOB))!=71 OR length(payload)>8192 OR length(CAST(state AS BLOB))>16)",
        [],|r| r.get::<_,bool>(0),
    ).unwrap());
    let rows = c.prepare("SELECT path,operation,digest,payload,state FROM projection_prejournal ORDER BY path COLLATE BINARY LIMIT 6").unwrap()
        .query_map([],|r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Vec<u8>>(3)?,r.get::<_,String>(4)?))).unwrap()
        .collect::<rusqlite::Result<JournalRows>>().unwrap();
    assert_eq!(
        rows.len(),
        5,
        "header, stage, nested directory and two files remain durable"
    );
    for (_, operation, digest, payload, state) in &rows {
        assert_eq!(
            operation,
            &format!("provider-checkout:{}", staging.transaction_id)
        );
        assert_eq!(state, "owned");
        let row = CheckoutRow::decode(payload).unwrap();
        assert_eq!(row.digest().unwrap(), *digest);
        match &row.entry {
            CheckoutEntry::Header { header } => {
                assert_eq!(header.transaction_id, staging.transaction_id);
                assert_eq!((header.files, header.directories), (2, 1));
            }
            CheckoutEntry::Stage { state } | CheckoutEntry::Directory { state, .. } => {
                assert!(matches!(state, DirectoryState::Owned { .. }))
            }
            CheckoutEntry::File { state, .. } => {
                assert!(matches!(state, FileState::Published { .. }))
            }
        }
    }
    rows
}

fn assert_ready(
    path: &Path,
    staging: &ProviderTransitionIntentRecord,
    root_id: &str,
    receipt_count_before: u64,
    before_journal: &JournalRows,
) -> (Vec<u8>, Vec<u8>) {
    read_only(path, |c| {
        assert_eq!(
            journal_rows(c, staging),
            *before_journal,
            "Ready retains every checkout journal column byte-for-byte"
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM (SELECT 1 FROM projection_claims LIMIT 2)",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
        let claim: (String, String, String, String) = c
            .query_row(
                "SELECT root_id,digest,kind,owner FROM projection_claims",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            claim,
            (
                root_id.into(),
                root_id.into(),
                "provider-import".into(),
                staging.transaction_id.clone()
            )
        );
        assert_eq!(c.query_row("SELECT count(*) FROM (SELECT 1 FROM projection_provider_transition_receipts LIMIT 4097)",[],|r| r.get::<_,u64>(0)).unwrap(),receipt_count_before+1);
        assert_eq!(
            c.query_row(
                "SELECT generation FROM projection_head WHERE singleton=1",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            staging.plan.placement_generation
        );
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 2)",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            1
        );
        let (phase,receipt_id,payload,digest): (String,String,Vec<u8>,String) = c.query_row(
            "SELECT state,receipt_id,payload,digest FROM projection_provider_transitions WHERE id=?1 AND length(payload)<=262144",
            [staging.transaction_id.as_str()],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        ).unwrap();
        assert_eq!(phase, "ready");
        let ready = ProviderTransitionIntentRecord::decode(&payload).unwrap();
        let destination_staging =
            super::super::managed_destination::destination_intent(staging).unwrap();
        ready.require_transition(&destination_staging).unwrap();
        assert_eq!(ready.role, TransitionRole::Destination);
        assert_eq!(ready.state, TransitionState::Ready);
        assert_eq!(ready.plan, staging.plan);
        assert_eq!(ready.reservation, staging.reservation);
        assert_eq!(ready.digest().unwrap(), digest);
        assert_eq!(ready.receipt_id.as_deref(), Some(receipt_id.as_str()));
        let receipt_payload: Vec<u8> = c.query_row(
            "SELECT payload FROM projection_provider_transition_receipts WHERE id=?1 AND transaction_id=?2 AND length(payload)<=65536",
            rusqlite::params![receipt_id,staging.transaction_id],|r| r.get(0),
        ).unwrap();
        let receipt = ProviderTransitionReceiptRecord::decode(&receipt_payload).unwrap();
        assert_eq!(receipt.digest().unwrap(), receipt_id);
        ready.require_receipt(&receipt).unwrap();
        receipt.require_prefix(&staging.plan.source_prefix).unwrap();
        assert_eq!(
            receipt.destination_placement_generation,
            staging.plan.placement_generation
        );
        assert_eq!(
            receipt.destination_capture_sequence,
            staging.plan.capture_sequence
        );
        assert!(!c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        (payload, receipt_payload)
    })
}

fn witnesses(paths: &[PathBuf]) -> Vec<(String, u64, String)> {
    paths
        .iter()
        .map(|p| database_io::tests::support::witness(p))
        .collect()
}

/// Deliberate SQL corruption in this owned fixture, followed by fixture-only
/// restoration. Neither UPDATE is a product repair or a product writer probe.
fn substitute_inode_probe<T>(
    private_path: &Path,
    expected: &ExpectedCheckout,
    staging: &ProviderTransitionIntentRecord,
    original_journal: &JournalRows,
    cancel: &Cancellation,
    finalize: impl FnOnce(&mut Close) -> CauseResult<T>,
) {
    let key = expected.sql_key(RowKey::File(0)).unwrap();
    let (original_payload, original_digest): (Vec<u8>, String) = read_only(private_path, |c| {
        c.query_row("SELECT payload,digest FROM projection_prejournal WHERE path=?1 AND length(payload)<=8192 AND length(CAST(digest AS BLOB))=71",[&key],|r| Ok((r.get(0)?,r.get(1)?))).unwrap()
    });
    let original = CheckoutRow::decode(&original_payload).unwrap();
    let mut substituted = original.clone();
    let known_inodes: BTreeSet<_> = original_journal
        .iter()
        .filter_map(
            |(_, _, _, payload, _)| match CheckoutRow::decode(payload).unwrap().entry {
                CheckoutEntry::File { state, .. } => state.inode().map(str::to_owned),
                _ => None,
            },
        )
        .collect();
    let fake = (0..=known_inodes.len() as u32)
        .map(|n| format!("4294967295:4294967295:{}", u32::MAX - n))
        .find(|inode| !known_inodes.contains(inode))
        .unwrap();
    let CheckoutEntry::File {
        state: FileState::Published { inode, .. },
        ..
    } = &mut substituted.entry
    else {
        panic!("fixture substitution requires completed File(0)")
    };
    assert_ne!(*inode, fake);
    *inode = fake.clone();
    let corrupt_payload = substituted.encode().unwrap();
    let corrupt_digest = substituted.digest().unwrap();
    let replace_columns = |before_payload: &[u8],
                           before_digest: &str,
                           after_payload: &[u8],
                           after_digest: &str| {
        let c =
            Connection::open_with_flags(private_path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
            .unwrap();
        assert_eq!(c.execute(
            "UPDATE projection_prejournal SET payload=?1,digest=?2 WHERE path=?3 AND operation=?4 AND state='owned' AND payload=?5 AND digest=?6",
            rusqlite::params![after_payload,after_digest,key,expected.operation(),before_payload,before_digest],
        ).unwrap(),1,"fixture corruption/restoration must CAS the exact previous columns");
        c.execute_batch("COMMIT").unwrap();
        c.close().unwrap();
    };
    replace_columns(
        &original_payload,
        &original_digest,
        &corrupt_payload,
        &corrupt_digest,
    );
    read_only(private_path, |c| {
        let journal = checkout_journal::read(c, expected, Some(cancel)).unwrap();
        assert!(
            journal.is_complete(),
            "canonical SQL DATA alone still accepts the substituted inode"
        );
        assert_eq!(journal.row(RowKey::File(0)).unwrap(), &substituted);
    });
    let corrupt_witness = database_io::tests::support::witness(private_path);
    let mut close = Close::default();
    let error = finalize(&mut close)
        .err()
        .expect("retained physical proof must reject substituted SQL ownership");
    assert!(
        error
            .to_string()
            .contains("checkout SQL ownership differs from retained physical proof"),
        "wrong refusal: {error}"
    );
    assert_eq!(
        (close.opens, close.closes),
        (0, 0),
        "ownership mismatch refuses before any product writer"
    );
    assert_eq!(
        database_io::tests::support::witness(private_path),
        corrupt_witness,
        "product refusal neither repairs nor rewrites corrupt SQL"
    );
    replace_columns(
        &corrupt_payload,
        &corrupt_digest,
        &original_payload,
        &original_digest,
    );
    read_only(private_path, |c| {
        assert_eq!(
            journal_rows(c, staging),
            *original_journal,
            "only the owned fixture restores original journal columns"
        );
        let journal = checkout_journal::read(c, expected, Some(cancel)).unwrap();
        assert_eq!(journal.row(RowKey::File(0)).unwrap(), &original);
    });
}

fn case(refuse_acknowledgement: bool) {
    super::managed_staging_tests::with_content_planned_checkout(
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
            assert_eq!(image.files().len(), 2);
            let current: Vec<_> = image
                .files()
                .iter()
                .map(|f| f.manifest.manifest().id().clone())
                .collect();
            let required =
                super::managed_staging_tests::manifests_for(&active.active.source, &image);
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
            // Current image first establishes resident chunks for exact history.
            // Avoid duplicate Published replays in this runtime-bounded recipe.
            let mut installed = BTreeSet::new();
            for id in current.iter().chain(required.iter().map(|m| m.id())) {
                if installed.insert(id.clone()) {
                    lease.install_exact(id, &mut setup, &cancel).unwrap();
                }
            }
            let protected = lease.protect_exact(&mut setup, &cancel).unwrap();
            let checkout = lease
                .copy_checkout(&protected, &mut setup, &cancel)
                .unwrap();
            assert_eq!(setup.opens, setup.closes);
            let source_path = fixture.metadata.join("projection.sqlite3");
            let family_path = fixture.family.root().join("metadata/catalog.sqlite3");
            let private_path = destination.destination_database().path().to_owned();
            let stable_paths = [source_path.clone(), family_path.clone()];
            let source_family_before = witnesses(&stable_paths);
            let output_paths = [
                destination.root().join("README.md"),
                destination.root().join("dir/extra.txt"),
                destination.root().join(".layrs/FORMAT"),
            ];
            let output_before = witnesses(&output_paths);
            let (before_journal, receipt_count_before) = read_only(&private_path, |c| {
                assert_eq!(
                    c.query_row(
                        "SELECT state FROM projection_provider_transitions",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                    "staging"
                );
                assert_eq!(
                    c.query_row(
                        "SELECT count(*) FROM (SELECT 1 FROM projection_claims LIMIT 2)",
                        [],
                        |r| r.get::<_, u64>(0)
                    )
                    .unwrap(),
                    0
                );
                let count = c.query_row("SELECT count(*) FROM (SELECT 1 FROM projection_provider_transition_receipts LIMIT 4097)",[],|r| r.get::<_,u64>(0)).unwrap();
                assert_eq!(
                    count,
                    staging.plan.source_prefix.closure.transition_receipts.count
                );
                (journal_rows(c, &staging), count)
            });
            // Split the two corruption probes across the recipes to preserve
            // runtime headroom under their 240-second owned-child bound.
            if !refuse_acknowledgement {
                substitute_inode_probe(
                    &private_path,
                    checkout.expected(),
                    &staging,
                    &before_journal,
                    &cancel,
                    |close| lease.finalize_private_ready(&protected, &checkout, close, &cancel),
                );
            }
            let mut close = Close {
                refuse_acknowledgement,
                ..Close::default()
            };
            let result = lease.finalize_private_ready(&protected, &checkout, &mut close, &cancel);
            let acknowledged = if refuse_acknowledgement {
                let error = result
                    .err()
                    .expect("Ready acknowledgement refusal cannot return a capability");
                assert!(
                    error
                        .to_string()
                        .contains("private Ready acknowledgement withheld"),
                    "{error}"
                );
                None
            } else {
                let ready = result.unwrap();
                ready.require_physical(&lease, &cancel).unwrap();
                let data = (
                    ready.ready_intent().encode().unwrap(),
                    ready.receipt().encode().unwrap(),
                );
                drop(ready);
                Some(data)
            };
            assert_eq!(
                (close.opens, close.closes),
                (1, 1),
                "only one private Ready writer closes positively"
            );
            let durable = assert_ready(
                &private_path,
                &staging,
                protected.root().id(),
                receipt_count_before,
                &before_journal,
            );
            if let Some(acknowledged) = acknowledged {
                assert_eq!(acknowledged, durable);
            }
            assert_eq!(
                witnesses(&stable_paths),
                source_family_before,
                "private Ready cannot mutate source, reservations or family journals"
            );
            assert_eq!(witnesses(&output_paths), output_before);
            if refuse_acknowledgement {
                substitute_inode_probe(
                    &private_path,
                    checkout.expected(),
                    &staging,
                    &before_journal,
                    &cancel,
                    |close| lease.finalize_private_ready(&protected, &checkout, close, &cancel),
                );
            }
            let private_durable = database_io::tests::support::witness(&private_path);
            let mut replay_close = Close::default();
            // Keep exactly the same lease, checkout, protection and checkpoint.
            // Reopening through the Staging reader would not prove Ready replay.
            let ready = lease
                .finalize_private_ready(&protected, &checkout, &mut replay_close, &cancel)
                .unwrap();
            ready.require_physical(&lease, &cancel).unwrap();
            assert_eq!(
                (
                    ready.ready_intent().encode().unwrap(),
                    ready.receipt().encode().unwrap()
                ),
                durable
            );
            assert_eq!((replay_close.opens, replay_close.closes), (0, 0));
            drop(ready);
            assert_eq!(
                database_io::tests::support::witness(&private_path),
                private_durable
            );
            assert_eq!(witnesses(&stable_paths), source_family_before);
            assert_eq!(witnesses(&output_paths), output_before);
            assert_eq!(
                assert_ready(
                    &private_path,
                    &staging,
                    protected.root().id(),
                    receipt_count_before,
                    &before_journal
                ),
                durable
            );
            {
                let _locks = held
                    .family
                    .locks(
                        &held.artifacts.allocation.format.identity.worktree_id,
                        false,
                    )
                    .unwrap();
                read_only(&source_path, |c| {
                    let (phase,payload,receipt): (String,Vec<u8>,Option<String>) = c.query_row("SELECT state,payload,receipt_id FROM projection_provider_transitions WHERE id=?1 AND length(payload)<=262144",[staging.transaction_id.as_str()],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                    assert_eq!(phase, "staging");
                    assert_eq!(payload, staging.encode().unwrap());
                    assert!(receipt.is_none());
                });
                read_only(&family_path, |c| {
                    let owner = staging
                        .plan
                        .source_prefix
                        .destination_identity
                        .worktree_id
                        .as_str();
                    assert_eq!(
                        c.query_row("SELECT state FROM worktrees WHERE id=?1", [owner], |r| {
                            r.get::<_, String>(0)
                        })
                        .unwrap(),
                        "initializing"
                    );
                    let operation = staging.reservation_operation().unwrap();
                    assert_eq!(c.query_row("SELECT count(*) FROM (SELECT 1 FROM reservations WHERE owner=?1 AND id=?2 LIMIT 2)",rusqlite::params![owner,operation],|r| r.get::<_,u64>(0)).unwrap(),1);
                    assert_eq!(c.query_row("SELECT count(*) FROM (SELECT 1 FROM exact_installations WHERE reservation=?1 LIMIT ?2)",rusqlite::params![operation,required.len()+1],|r| r.get::<_,usize>(0)).unwrap(),required.len());
                });
            }
            let stage = destination
                .root()
                .join(format!(".layrs/checkout-{}", staging.transaction_id));
            assert!(fs::read_dir(stage).unwrap().next().is_none());
            drop(checkout);
            drop(protected);
            drop(lease);
            drop(image);
            drop(checkpoint);
        },
    );
}

macro_rules! owned {
    ($name:ident,$refuse:expr) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT releases this exact owned private Ready recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                240,
                || {
                    child::declare();
                    case($refuse);
                },
            );
        }
    };
}
owned!(
    managed_private_ready_retains_journals_and_replays_without_writer,
    false
);
owned!(
    managed_private_ready_committed_ack_refusal_replays_without_writer,
    true
);
