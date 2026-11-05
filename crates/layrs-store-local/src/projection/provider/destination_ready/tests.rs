//! Owned, ignored memory-SQL qualification. ROOT alone releases and executes
//! these 120-second children. Synthetic identities prove DATA consistency;
//! no filesystem/native/content capability or ordinary admission is asserted.
use super::*;
use crate::projection::ProjectionStoreError;
use crate::projection::provider::{
    database_io::BareConnection, process_profile::tests::child, records::Receipt,
    validation::tests::fixture::Fixture as SourceFixture,
};
#[path = "tests/fixture.rs"]
mod fixture;
#[path = "tests/source.rs"]
mod source;
use fixture::Fixture;

macro_rules! owned {
    ($name:ident, $body:block) => {
        #[test]
        #[ignore = "UNQUALIFIED until ROOT reviews/releases this exact owned Ready SQL recipe"]
        fn $name() {
            child::run(
                concat!(module_path!(), "::", stringify!($name)),
                120,
                || {
                    child::declare();
                    $body
                },
            );
        }
    };
}

fn with_fixture(
    body: impl FnOnce(&Connection, &Connection, &Connection, &Fixture, &VerifiedImportClosure),
) {
    let source = BareConnection::reference().unwrap();
    let destination = BareConnection::reference().unwrap();
    let family = BareConnection::reference().unwrap();
    let (fixture, closure) = Fixture::new(source.connection());
    fixture.stage(source.connection(), destination.connection(), &closure);
    fixture.populate_family(family.connection());
    destination
        .connection()
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    body(
        source.connection(),
        destination.connection(),
        family.connection(),
        &fixture,
        &closure,
    );
    for connection in [
        source.connection(),
        destination.connection(),
        family.connection(),
    ] {
        if !connection.is_autocommit() {
            connection.execute_batch("ROLLBACK").unwrap();
        }
    }
    // Each audited connection must positively close before this recipe exits.
    source.finish(Ok(())).unwrap();
    destination.finish(Ok(())).unwrap();
    family.finish(Ok(())).unwrap();
}

fn strict_staging(c: &Connection, f: &Fixture, closure: &VerifiedImportClosure) {
    destination_validation::read_staging(
        c,
        &f.marker,
        &f.source.source,
        &f.source.allocation,
        &f.source.prepared,
        &f.activation,
        &f.prefix,
        &f.staging,
        closure,
        None,
    )
    .unwrap();
}
fn assert_invalid<T>(result: Result<T>, expected: &str) {
    let error = match result {
        Ok(_) => panic!("expected refusal: {expected}"),
        Err(error) => error,
    };
    let super::super::recovery::error::ProviderBootstrapCause::Validation(error) = error else {
        panic!("expected exact validation refusal {expected}, got {error:?}");
    };
    let ProjectionStoreError::Invalid(message) = error.as_ref() else {
        panic!("unexpected validation cause: {error:?}")
    };
    assert_eq!(message, expected);
}

owned!(
    staged_ready_appends_exact_suffix_and_replays_without_writes,
    {
        with_fixture(|s, c, family, f, closure| {
            strict_staging(c, f, closure);
            let data = f.data();
            let inputs = f.inputs(closure, &data);
            let source_changes = s.total_changes();
            let family_changes = family.total_changes();
            assert_eq!(
                finalize_staged_or_replay(s, c, family, &inputs, None).unwrap(),
                ReadyEffect::Finalized
            );
            read_ready(s, c, family, &inputs, None).unwrap();
            assert_eq!(s.total_changes(), source_changes);
            assert_eq!(family.total_changes(), family_changes);
            let (head,captures,selections,claims):(u64,u64,u64,u64)=c.query_row(
            "SELECT (SELECT generation FROM projection_head),(SELECT count(*) FROM projection_captures),(SELECT count(*) FROM projection_selections),(SELECT count(*) FROM projection_claims WHERE kind='provider-import')",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
            assert_eq!((head, captures, selections, claims), (3, 2, 3, 1));
            // Same FileId, different exact representations: the imported historical
            // PK choice survives, while both representations remain protected.
            let mapping: String = c
                .query_row(
                    "SELECT manifest_id FROM projection_content WHERE file_id=?1",
                    [f.historical.file_id().as_str()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(mapping, f.historical.id().as_str());
            assert_ne!(mapping, f.current.id().as_str());
            assert_eq!(
                c.query_row("SELECT count(*) FROM projection_content", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                2
            );
            assert_eq!(
                family
                    .query_row(
                        "SELECT count(*) FROM root_manifests WHERE root_id=?1",
                        [&f.destination_root.0],
                        |r| r.get::<_, u64>(0)
                    )
                    .unwrap(),
                3
            );
            assert!(
                destination_validation::read_staging(
                    c,
                    &f.marker,
                    &f.source.source,
                    &f.source.allocation,
                    &f.source.prepared,
                    &f.activation,
                    &f.prefix,
                    &f.staging,
                    closure,
                    None
                )
                .is_err()
            );
            c.execute_batch(
                "COMMIT; PRAGMA foreign_keys=OFF; PRAGMA query_only=ON; BEGIN DEFERRED",
            )
            .unwrap();
            let before = c.total_changes();
            assert_eq!(
                finalize_staged_or_replay(s, c, family, &inputs, None).unwrap(),
                ReadyEffect::AlreadyReady
            );
            read_ready(s, c, family, &inputs, None).unwrap();
            assert_eq!(
                c.total_changes(),
                before,
                "Ready replay must work with FK OFF/query_only and no mutations"
            );
        });
    }
);

owned!(
    ready_append_rolls_back_to_exact_staging_and_requires_fk_for_write,
    {
        with_fixture(|s, c, family, f, closure| {
            let data = f.data();
            let inputs = f.inputs(closure, &data);
            assert_eq!(
                finalize_staged_or_replay(s, c, family, &inputs, None).unwrap(),
                ReadyEffect::Finalized
            );
            c.execute_batch("ROLLBACK; BEGIN DEFERRED").unwrap();
            strict_staging(c, f, closure);
            assert_eq!(
                c.query_row("SELECT generation FROM projection_head", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                1
            );
            assert!(
                !c.query_row("SELECT EXISTS(SELECT 1 FROM projection_claims)", [], |r| {
                    r.get::<_, bool>(0)
                })
                .unwrap()
            );
            c.execute_batch("ROLLBACK; PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
                .unwrap();
            let before = c.total_changes();
            assert_invalid(
                finalize_staged_or_replay(s, c, family, &inputs, None),
                "destination Ready writer requires FK ON",
            );
            assert_eq!(c.total_changes(), before);
            strict_staging(c, f, closure);
        });
    }
);

owned!(
    staging_refuses_forged_claim_overlay_prefix_and_family_before_append,
    {
        with_fixture(|s, c, family, f, closure| {
            let mut data = f.data();
            let forged = [ManagedReadyClaim {
                root_id: f.claims[0].root_id.clone(),
                digest: f.claims[0].digest.clone(),
                kind: "generation".into(),
                owner: f.claims[0].owner.clone(),
            }];
            data.claims = &forged;
            let before = c.total_changes();
            assert_invalid(
                finalize_staged_or_replay(s, c, family, &f.inputs(closure, &data), None),
                "destination Ready durable import claim differs",
            );
            assert_eq!(c.total_changes(), before);
            let mut data = f.data();
            let wrong_overlay = [ManagedReadyOverlay {
                path: f.overlay[0].path.clone(),
                manifest_id: Some(f.historical.id().clone()),
            }];
            data.overlay = &wrong_overlay;
            assert_invalid(
                finalize_staged_or_replay(s, c, family, &f.inputs(closure, &data), None),
                "destination Ready overlay commitment differs",
            );
            assert_eq!(c.total_changes(), before);
            let data = f.data();
            let inputs = f.inputs(closure, &data);
            c.execute_batch("SAVEPOINT corrupt_prefix").unwrap();
            c.execute(
                "UPDATE projection_provider_descriptors SET executable=1-executable",
                [],
            )
            .unwrap();
            let before = c.total_changes();
            assert_invalid(
                finalize_staged_or_replay(s, c, family, &inputs, None),
                "import descriptor SQL row differs from source",
            );
            assert_eq!(c.total_changes(), before);
            c.execute_batch("ROLLBACK TO corrupt_prefix; RELEASE corrupt_prefix")
                .unwrap();
            strict_staging(c, f, closure);
            family
                .execute_batch("SAVEPOINT corrupt_protection")
                .unwrap();
            family
                .execute(
                    "DELETE FROM root_manifests WHERE root_id=?1 AND manifest_id=?2",
                    params![f.destination_root.0, f.current.id().as_str()],
                )
                .unwrap();
            let before = c.total_changes();
            assert_invalid(
                finalize_staged_or_replay(s, c, family, &inputs, None),
                "destination Ready protection membership differs",
            );
            assert_eq!(c.total_changes(), before);
            family
                .execute_batch("ROLLBACK TO corrupt_protection; RELEASE corrupt_protection")
                .unwrap();
            for (kind, id, expected) in [
                (
                    "manifests",
                    f.current.id().as_str(),
                    "destination Ready protected manifest is swept",
                ),
                (
                    "chunks",
                    f.current.chunks()[0].chunk_id().as_str(),
                    "destination Ready protected chunk is swept",
                ),
            ] {
                family.execute_batch("SAVEPOINT overlapping_sweep").unwrap();
                family
                    .execute("INSERT INTO sweep VALUES(?1,?2,0)", params![kind, id])
                    .unwrap();
                assert!(
                    !family
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                            [],
                            |r| r.get::<_, bool>(0)
                        )
                        .unwrap(),
                    "sweep overlap is independent of FK validity"
                );
                let before = c.total_changes();
                assert_invalid(
                    finalize_staged_or_replay(s, c, family, &inputs, None),
                    expected,
                );
                assert_eq!(c.total_changes(), before);
                family
                    .execute_batch("ROLLBACK TO overlapping_sweep; RELEASE overlapping_sweep")
                    .unwrap();
            }
            // Even a same-cardinality FK-valid hostile membership must fail its
            // scalar bound before the reader materializes observed String values.
            family
                .execute_batch("SAVEPOINT oversized_membership")
                .unwrap();
            let oversized = "x".repeat(4096);
            family
                .execute(
                    "INSERT INTO manifests VALUES(?1,?2,?3,?4)",
                    params![
                        oversized,
                        f.current.file_id().as_str(),
                        f.current.byte_length(),
                        canonical_json(&f.current).unwrap()
                    ],
                )
                .unwrap();
            family
                .execute(
                    "DELETE FROM root_manifests WHERE root_id=?1 AND manifest_id=?2",
                    params![f.destination_root.0, f.current.id().as_str()],
                )
                .unwrap();
            family
                .execute(
                    "INSERT INTO root_manifests VALUES(?1,?2)",
                    params![f.destination_root.0, oversized],
                )
                .unwrap();
            assert!(
                !family
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                        [],
                        |r| r.get::<_, bool>(0)
                    )
                    .unwrap()
            );
            assert_invalid(
                finalize_staged_or_replay(s, c, family, &inputs, None),
                "destination Ready protection membership identity exceeds bound",
            );
            family
                .execute_batch("ROLLBACK TO oversized_membership; RELEASE oversized_membership")
                .unwrap();
            // SQL-only Ready retains Initializing. Merely editing the family row to
            // Ready cannot grant or bypass the later physical publication protocol.
            family
                .execute_batch("SAVEPOINT premature_family_ready")
                .unwrap();
            family
                .execute(
                    "UPDATE worktrees SET state='ready' WHERE id=?1",
                    [f.marker.identity.worktree_id.as_str()],
                )
                .unwrap();
            assert_invalid(
                finalize_staged_or_replay(s, c, family, &inputs, None),
                "destination Ready family member differs",
            );
            family
                .execute_batch("ROLLBACK TO premature_family_ready; RELEASE premature_family_ready")
                .unwrap();
            strict_staging(c, f, closure);
            // Unrelated collection work does not block the exact protected union.
            family
                .execute(
                    "INSERT INTO sweep VALUES('chunks',?1,1)",
                    [format!("blake3:{}", "f".repeat(64))],
                )
                .unwrap();
            assert_eq!(
                finalize_staged_or_replay(s, c, family, &inputs, None).unwrap(),
                ReadyEffect::Finalized
            );
            read_ready(s, c, family, &inputs, None).unwrap();
        });
    }
);

owned!(
    ready_reader_refuses_prefix_suffix_mapping_claim_and_unrelated_tail,
    {
        with_fixture(|s, c, family, f, closure| {
            let data = f.data();
            let inputs = f.inputs(closure, &data);
            finalize_staged_or_replay(s, c, family, &inputs, None).unwrap();
            for (sql, expected) in [
                (
                    "UPDATE projection_snapshots SET payload=x'7b7d' WHERE generation=3",
                    "destination Ready local snapshot or selection differs",
                ),
                (
                    "UPDATE projection_claims SET owner='another-transition'",
                    "destination Ready private import claim differs",
                ),
                (
                    "INSERT INTO projection_observations VALUES('unrelated',x'7b7d')",
                    "destination Ready table has missing or extra rows",
                ),
            ] {
                c.execute_batch("SAVEPOINT corrupt_suffix").unwrap();
                c.execute_batch(sql).unwrap();
                let before = c.total_changes();
                assert_invalid(read_ready(s, c, family, &inputs, None), expected);
                assert_eq!(c.total_changes(), before);
                c.execute_batch("ROLLBACK TO corrupt_suffix; RELEASE corrupt_suffix")
                    .unwrap();
                read_ready(s, c, family, &inputs, None).unwrap();
            }
            c.execute_batch("SAVEPOINT corrupt_old_mapping").unwrap();
            c.execute(
                "UPDATE projection_content SET manifest_id=?1 WHERE file_id=?2",
                params![f.current.id().as_str(), f.historical.file_id().as_str()],
            )
            .unwrap();
            assert_invalid(
                read_ready(s, c, family, &inputs, None),
                "import immutable SQL closure differs from prefix",
            );
            c.execute_batch("ROLLBACK TO corrupt_old_mapping; RELEASE corrupt_old_mapping")
                .unwrap();
            c.execute_batch("SAVEPOINT corrupt_new_mapping").unwrap();
            let base = &f.source.source.descriptors()[0];
            c.execute(
                "UPDATE projection_content SET manifest_id=?1 WHERE file_id=?2",
                params![f.current.id().as_str(), base.file_id.as_str()],
            )
            .unwrap();
            assert_invalid(
                read_ready(s, c, family, &inputs, None),
                "destination Ready local content mapping differs",
            );
            c.execute_batch("ROLLBACK TO corrupt_new_mapping; RELEASE corrupt_new_mapping")
                .unwrap();
            read_ready(s, c, family, &inputs, None).unwrap();
        });
    }
);

owned!(
    ready_refuses_companion_without_closed_data_and_incomplete_union_before_append,
    {
        with_fixture(|s, c, family, f, closure| {
            let before = c.total_changes();
            let original = f.data();
            let subset = &f.required[..f.required.len() - 1];
            let bad_union = ManagedReadyData {
                required: subset,
                ..f.data()
            };
            let inputs = f.inputs(closure, &bad_union);
            assert!(finalize_staged_or_replay(s, c, family, &inputs, None).is_err());
            assert_eq!(c.total_changes(), before);
            assert!(!c.query_row("SELECT EXISTS(SELECT 1 FROM projection_provider_transition_receipts WHERE transaction_id=?1)",[f.staging.transaction_id.as_str()],|r|r.get::<_,bool>(0)).unwrap());
            let mut staging =
                ProviderTransitionIntentRecord::decode(&f.staging.encode().unwrap()).unwrap();
            staging.plan.companion_id = Some(format!("blake3:{}", "7".repeat(64)));
            let mut ready =
                ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
            ready.state = TransitionState::Ready;
            let mut receipt =
                ProviderTransitionReceiptRecord::decode(&f.receipt.encode().unwrap()).unwrap();
            receipt.companion_id = staging.plan.companion_id.clone();
            receipt.transition_plan_digest = staging.plan.digest().unwrap();
            receipt.owner_identity_digest = staging
                .destination
                .as_ref()
                .unwrap()
                .owner_digest(&staging.plan)
                .unwrap();
            receipt.destination_proof_digest = staging
                .destination
                .as_ref()
                .unwrap()
                .digest(&staging.plan)
                .unwrap();
            ready.receipt_id = Some(receipt.digest().unwrap());
            let bad_rows = ManagedReadyData {
                ready: &ready,
                receipt: &receipt,
                ..original
            };
            let base = f.inputs(closure, &bad_rows);
            let inputs = ManagedReadyInputs {
                staging: &staging,
                ..base
            };
            assert!(finalize_staged_or_replay(s, c, family, &inputs, None).is_err());
            assert_eq!(c.total_changes(), before);
            strict_staging(c, f, closure);
        });
    }
);
