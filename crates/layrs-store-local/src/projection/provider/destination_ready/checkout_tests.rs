//! Owned DATA-only SQL recipes for retained checkout provenance in Ready.
//! The shared complete Ready fixture supplies a real interpreted source and
//! copied Staging prefix. Journal inode strings remain synthetic DATA: these
//! recipes do not qualify physical copying or ordinary destination admission.
use super::*;
use crate::projection::ProjectionStoreError;
use crate::projection::provider::{
    checkout_journal::{self, CheckoutEntry, CheckoutFile, ExpectedCheckout, FileState, RowKey},
    database_io::BareConnection,
    process_profile::tests::child,
    records::Receipt,
    validation::tests::fixture::Fixture as SourceFixture,
};
#[path = "tests/fixture.rs"]
mod fixture;
#[path = "tests/source.rs"]
mod source;
use fixture::Fixture;

#[path = "publication_tests.rs"]
mod publication_tests;

fn with_fixture(
    body: impl FnOnce(
        &Connection,
        &Connection,
        &Connection,
        &Fixture,
        &VerifiedImportClosure,
        &ExpectedCheckout,
    ),
) {
    let source = BareConnection::reference().unwrap();
    let destination = BareConnection::reference().unwrap();
    let family = BareConnection::reference().unwrap();
    let (mut f, closure) = Fixture::new(source.connection());
    // Build a distinct valid transition before publishing either Staging row.
    // Its new image has a non-executable replacement; the inherited source
    // capture remains untouched, including its historical representation.
    f.capture.changes[0]
        .replacement
        .as_mut()
        .unwrap()
        .executable = false;
    f.capture.digest = f.capture.computed_digest().unwrap();
    let overlay =
        serde_json::json!({"change":&f.capture.changes[0],"manifestId":&f.overlay[0].manifest_id});
    let encoded = canonical_json(&overlay).unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-overlay-v1\0");
    hash.update(&1u64.to_be_bytes());
    hash.update(&(encoded.len() as u64).to_be_bytes());
    hash.update(&encoded);
    f.copying.plan.capture_digest = f.capture.digest.clone();
    f.copying.plan.working_overlay.digest = format!("blake3:{}", hash.finalize());
    f.staging = ProviderTransitionIntentRecord::decode(&f.copying.encode().unwrap()).unwrap();
    f.staging.state = TransitionState::Staging;
    f.staging.reservation = Some(f.staging.reservation_operation().unwrap());
    f.receipt.transition_plan_digest = f.staging.plan.digest().unwrap();
    let proof = f.staging.destination.as_ref().unwrap();
    f.receipt.owner_identity_digest = proof.owner_digest(&f.staging.plan).unwrap();
    f.receipt.destination_proof_digest = proof.digest(&f.staging.plan).unwrap();
    f.receipt.destination_capture_digest = f.capture.digest.clone();
    f.ready = ProviderTransitionIntentRecord::decode(&f.staging.encode().unwrap()).unwrap();
    f.ready.state = TransitionState::Ready;
    f.ready.receipt_id = Some(f.receipt.digest().unwrap());
    f.ready.require_receipt(&f.receipt).unwrap();
    f.stage(source.connection(), destination.connection(), &closure);
    f.populate_family(family.connection());
    destination
        .connection()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    let replacement = f.capture.changes[0].replacement.as_ref().unwrap();
    let files = [CheckoutFile {
        path: f.capture.changes[0].path.clone(),
        manifest_id: f.current.id().clone(),
        file_id: replacement.file_id.clone(),
        bytes: replacement.bytes,
        executable: replacement.executable,
    }];
    let image = match &f.prefix.source {
        ImportSource::Provider { version, .. } => &version.effective_image_digest,
        _ => unreachable!(),
    };
    let expected = ExpectedCheckout::new(&f.staging, &files, &[], image, None).unwrap();
    body(
        source.connection(),
        destination.connection(),
        family.connection(),
        &f,
        &closure,
        &expected,
    );
    for c in [
        source.connection(),
        destination.connection(),
        family.connection(),
    ] {
        if !c.is_autocommit() {
            c.execute_batch("ROLLBACK").unwrap();
        }
    }
    source.finish(Ok(())).unwrap();
    destination.finish(Ok(())).unwrap();
    family.finish(Ok(())).unwrap();
}
fn complete(c: &Connection, expected: &ExpectedCheckout) {
    checkout_journal::declare(c, expected, None).unwrap();
    let stage = checkout_journal::read(c, expected, None)
        .unwrap()
        .row(RowKey::Stage)
        .unwrap()
        .clone();
    checkout_journal::advance(
        c,
        expected,
        &stage,
        &stage
            .owned_directory("fs:v1:0000000000000001:0000000000001000")
            .unwrap(),
        None,
    )
    .unwrap();
    assert!(expected.directories().is_empty());
    for ordinal in 0..expected.files().len() {
        let mut previous = checkout_journal::read(c, expected, None)
            .unwrap()
            .row(RowKey::File(ordinal as u32))
            .unwrap()
            .clone();
        for next in [previous
            .owned_file(&format!("1:0:{}", 2001 + ordinal))
            .unwrap()]
        {
            checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
            previous = next;
        }
        let next = previous.verified_file().unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
        previous = next;
        let next = previous
            .publishing_file(&expected.header().checkout_identity)
            .unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
        previous = next;
        let next = previous.published_file().unwrap();
        checkout_journal::advance(c, expected, &previous, &next, None).unwrap();
    }
    assert!(
        checkout_journal::read(c, expected, None)
            .unwrap()
            .is_complete()
    );
}
fn refuses<T>(result: Result<T>, expected: &str) {
    let error = match result {
        Ok(_) => panic!("expected refusal {expected}"),
        Err(error) => error,
    };
    assert!(
        matches!(&error,super::super::recovery::error::ProviderBootstrapCause::Validation(e) if matches!(e.as_ref(),ProjectionStoreError::Invalid(message) if message==expected)),
        "expected {expected:?}, got {error:?}"
    );
}
fn count(c: &Connection) -> u64 {
    c.query_row("SELECT count(*) FROM projection_prejournal", [], |r| {
        r.get(0)
    })
    .unwrap()
}
fn witness(c: &Connection) -> Vec<(String, String, String, Vec<u8>, String)> {
    let mut statement=c.prepare("SELECT path,operation,digest,payload,state FROM projection_prejournal ORDER BY path COLLATE BINARY").unwrap();
    statement
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
macro_rules! owned {
    ($name:ident,$body:block)=>{
        #[test]
        #[ignore="UNQUALIFIED until ROOT reviews/releases the exact owned checkout-to-Ready SQL recipe"]
        fn $name(){child::run(concat!(module_path!(),"::",stringify!($name)),120,||{child::declare();$body});}
    }
}

owned!(
    complete_checkout_retains_exact_journal_and_replays_ready_without_writes,
    {
        with_fixture(|s, c, family, f, closure, expected| {
            complete(c, expected);
            let data = f.data();
            let inputs = f.inputs(closure, &data);
            let retained = witness(c);
            assert_eq!(
                finalize_checkout_or_replay(s, c, family, &inputs, expected, None).unwrap(),
                ReadyEffect::Finalized
            );
            assert_eq!(witness(c), retained);
            read_checkout_ready(s, c, family, &inputs, expected, None).unwrap();
            refuses(
                read_ready(s, c, family, &inputs, None),
                "destination Ready table has missing or extra rows",
            );
            refuses(
                finalize_staged_or_replay(s, c, family, &inputs, None),
                "destination Ready table has missing or extra rows",
            );
            c.execute_batch(
                "COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED",
            )
            .unwrap();
            let before = c.total_changes();
            assert_eq!(
                finalize_checkout_or_replay(s, c, family, &inputs, expected, None).unwrap(),
                ReadyEffect::AlreadyReady
            );
            assert_eq!(c.total_changes(), before);
            assert_eq!(witness(c), retained);
            c.execute_batch("ROLLBACK; PRAGMA query_only=OFF; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE; SAVEPOINT forged_ready_tail").unwrap();
            c.execute("INSERT INTO projection_prejournal VALUES('foreign','foreign','foreign',x'7b7d','owned')",[]).unwrap();
            let before = c.total_changes();
            let rows = count(c);
            refuses(
                finalize_checkout_or_replay(s, c, family, &inputs, expected, None),
                "checkout journal is incomplete or has foreign rows",
            );
            assert_eq!(count(c), rows);
            assert_eq!(
                c.total_changes(),
                before,
                "Ready replay never removes a journal"
            );
            c.execute_batch("ROLLBACK TO forged_ready_tail; RELEASE forged_ready_tail")
                .unwrap();
            read_checkout_ready(s, c, family, &inputs, expected, None).unwrap();
            assert_eq!(witness(c), retained);
        });
    }
);

owned!(
    checkout_ready_rollback_restores_complete_journal_and_staging,
    {
        with_fixture(|s, c, family, f, closure, expected| {
            complete(c, expected);
            c.execute_batch("COMMIT; BEGIN IMMEDIATE").unwrap();
            let rows = count(c);
            let retained = witness(c);
            let data = f.data();
            let inputs = f.inputs(closure, &data);
            finalize_checkout_or_replay(s, c, family, &inputs, expected, None).unwrap();
            assert_eq!(witness(c), retained);
            c.execute_batch("ROLLBACK; BEGIN DEFERRED").unwrap();
            assert_eq!(count(c), rows);
            let journal = destination_validation::read_checkout_staging(
                c,
                &f.marker,
                &f.source.source,
                &f.source.allocation,
                &f.source.prepared,
                &f.activation,
                &f.prefix,
                &f.staging,
                closure,
                expected,
                None,
            )
            .unwrap();
            assert!(journal.is_complete());
            assert_eq!(witness(c), retained);
            assert_eq!(
                c.query_row("SELECT generation FROM projection_head", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                1
            );
            c.execute_batch("ROLLBACK; PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
                .unwrap();
            let before = c.total_changes();
            refuses(
                finalize_checkout_or_replay(s, c, family, &inputs, expected, None),
                "destination Ready writer requires FK ON",
            );
            assert_eq!(count(c), rows);
            assert_eq!(c.total_changes(), before);
        });
    }
);

owned!(
    checkout_ready_refuses_incomplete_foreign_and_aliased_identity_without_writes,
    {
        with_fixture(|s, c, family, f, closure, expected| {
            checkout_journal::declare(c, expected, None).unwrap();
            let data = f.data();
            let inputs = f.inputs(closure, &data);
            let before = c.total_changes();
            let rows = count(c);
            refuses(
                finalize_checkout_or_replay(s, c, family, &inputs, expected, None),
                "destination Ready requires complete checkout journal",
            );
            assert_eq!(c.total_changes(), before);
            assert_eq!(count(c), rows);
            complete(c, expected);
            c.execute_batch("SAVEPOINT foreign_tail").unwrap();
            c.execute("INSERT INTO projection_prejournal VALUES('foreign','foreign','foreign',x'7b7d','owned')",[]).unwrap();
            let before = c.total_changes();
            let rows = count(c);
            refuses(
                finalize_checkout_or_replay(s, c, family, &inputs, expected, None),
                "checkout journal is incomplete or has foreign rows",
            );
            assert_eq!(c.total_changes(), before);
            assert_eq!(count(c), rows);
            c.execute_batch(
                "ROLLBACK TO foreign_tail; RELEASE foreign_tail; SAVEPOINT aliased_inode",
            )
            .unwrap();
            let mut row = checkout_journal::read(c, expected, None)
                .unwrap()
                .row(RowKey::File(0))
                .unwrap()
                .clone();
            if let CheckoutEntry::File { state, .. } = &mut row.entry {
                *state = FileState::Published {
                    inode: expected.header().database_identity.clone(),
                    parent_identity: expected.header().checkout_identity.clone(),
                };
            }
            c.execute(
                "UPDATE projection_prejournal SET payload=?1,digest=?2 WHERE path=?3",
                params![
                    row.encode().unwrap(),
                    row.digest().unwrap(),
                    expected.sql_key(RowKey::File(0)).unwrap()
                ],
            )
            .unwrap();
            let before = c.total_changes();
            let rows = count(c);
            refuses(
                finalize_checkout_or_replay(s, c, family, &inputs, expected, None),
                "checkout file inode aliases another owner",
            );
            assert_eq!(c.total_changes(), before);
            assert_eq!(count(c), rows);
            c.execute_batch("ROLLBACK TO aliased_inode; RELEASE aliased_inode")
                .unwrap();
            assert!(
                checkout_journal::read(c, expected, None)
                    .unwrap()
                    .is_complete()
            );
        });
    }
);

owned!(
    checkout_ready_refuses_a_canonical_inventory_unrelated_to_ready_patch,
    {
        with_fixture(|s, c, family, f, closure, expected| {
            let mut files = expected.files().to_vec();
            files[0].path = RepoPath::parse("unrelated.txt").unwrap();
            let unrelated = ExpectedCheckout::new(
                &f.staging,
                &files,
                &[],
                &expected.header().image_digest,
                None,
            )
            .unwrap();
            complete(c, &unrelated);
            let data = f.data();
            let inputs = f.inputs(closure, &data);
            let before = c.total_changes();
            let rows = count(c);
            refuses(
                finalize_checkout_or_replay(s, c, family, &inputs, &unrelated, None),
                "checkout Ready file inventory differs",
            );
            assert_eq!(count(c), rows);
            assert_eq!(c.total_changes(), before);
        });
    }
);
