//! Owned memory SQL recipes. These exercise DATA/CAS and retained receipts;
//! they do not claim physical destination or ordinary admission qualification.
use super::*;
use crate::projection::provider::{
    published_handoff::{read_published_for_ready, PublicationSource},
    records::{
        activation::{ProviderActivationRecord, ProviderActiveMountRecord},
        capture::{
            ProviderCaptureReceiptRecord, ProviderCaptureStage, ProviderCaptureState,
            ProviderCaptureTransactionRecord,
        },
        import::ProviderTransitionReceiptRecord,
        transition::{TransitionBackendKind, TransitionDestinationProof},
    },
    validation::{private_active_publication_with_source, private_active_transition_with_source},
};
use crate::projection::{capture::CapturedPatch, VersionedProjection};
use layrs_core::scm::canonical_json;
use serde_json::json;

type Fixture = crate::projection::provider::validation::tests::fixture::Fixture;

fn copy(intent: &ProviderTransitionIntentRecord) -> ProviderTransitionIntentRecord {
    ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap()
}

fn destination(intent: &ProviderTransitionIntentRecord) -> TransitionDestinationProof {
    let root = format!(
        "{}\\{}",
        intent.plan.destination_parent, intent.plan.destination_leaf_nonce
    );
    let format = serde_json::from_value(json!({
        "schemaVersion":"layrs.projection-store-format.v1", "magic":"LAYRS_NATIVE_STORE",
        "formatVersion":4, "minReaderVersion":4,
        "identity":intent.plan.source_prefix.destination_identity,
        "checkoutRoot":root,
        "checkoutIdentity":"fs:v1:1111111111111111:2222222222222222",
        "storeIdentity":"fs:v1:1111111111111111:2222222222222222",
        "metadataIdentity":"fs:v1:1111111111111111:2222222222222222",
        "initialSnapshotDigest":RAW,
        "shared":{
            "root":"C:\\family", "incarnation":"fixture",
            "rootIdentity":"fs:v1:1111111111111111:2222222222222222",
            "metadataPath":"C:\\family\\worktrees\\fixture\\metadata", "databaseIdentity":"1:2:3"
        },
        "importDigest":intent.plan.source_prefix_id,
    }))
    .unwrap();
    TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format,
        format_file_identity: "1:2:4".into(),
        private_database_identity: "1:2:3".into(),
        family_marker_digest: TOKEN.into(),
        native_control_identity_hex: None,
        native_root_identity: None,
    }
}

fn staged(
    c: &Connection,
    f: &Fixture,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
) -> (
    ProviderTransitionIntentRecord,
    ProviderTransitionReceiptRecord,
) {
    let planned = planned(c, f, activation, activated);
    let apply = |previous, next| {
        apply_source_intent(
            c,
            &f.allocation,
            &f.prepared,
            &activation.binding,
            [0x41; 16],
            previous,
            next,
            None,
        )
        .unwrap()
    };
    assert_eq!(apply(None, &planned), SourceIntentEffect::Inserted);
    let mut copying = copy(&planned);
    copying.state = TransitionState::Copying;
    copying.destination = Some(destination(&copying));
    copying.reservation = Some(copying.reservation_operation().unwrap());
    assert_eq!(apply(Some(&planned), &copying), SourceIntentEffect::Updated);
    let mut staging = copy(&copying);
    staging.state = TransitionState::Staging;
    assert_eq!(apply(Some(&copying), &staging), SourceIntentEffect::Updated);
    let prefix = &staging.plan.source_prefix;
    let ImportSource::Provider {
        identity: source_identity,
        version,
        ..
    } = &prefix.source
    else {
        panic!("provider source fixture required")
    };
    let proof = staging.destination.as_ref().unwrap();
    let imported = |rows: &TransitionRowset| ImportRowset {
        count: rows.count,
        digest: rows.digest.clone(),
    };
    let receipt = ProviderTransitionReceiptRecord {
        denial_coverage: None,
        schema_version: "layrs.projection-provider-transition-receipt.v1".into(),
        owner_identity_digest: proof.owner_digest(&staging.plan).unwrap(),
        transaction_id: staging.transaction_id.clone(),
        transition_plan_digest: staging.plan.digest().unwrap(),
        destination_proof_digest: proof.digest(&staging.plan).unwrap(),
        source_prefix_id: staging.plan.source_prefix_id.clone(),
        source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
        parent_receipt_id: prefix.ancestry_root_receipt.clone(),
        source_identity: source_identity.clone(),
        destination_identity: prefix.destination_identity.clone(),
        imported_max_generation: version.generation,
        destination_observation_generation: staging.plan.observation.generation,
        destination_observation_digest: staging.plan.observation.snapshot_digest.clone(),
        destination_placement_generation: staging.plan.placement_generation,
        destination_placement_digest: staging.plan.placement_digest.clone(),
        imported_capture_count: version.capture_count,
        destination_capture_sequence: staging.plan.capture_sequence,
        destination_capture_digest: staging.plan.capture_digest.clone(),
        protected_manifests: imported(&staging.plan.protected_manifests),
        content_mappings: prefix.closure.content_mappings.clone(),
        companion_id: staging.plan.companion_id.clone(),
        companion_files: imported(&staging.plan.companion_files),
        companion_deletions: imported(&staging.plan.companion_deletions),
    };
    receipt.encode().unwrap();
    (staging, receipt)
}

fn ready(
    staging: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) -> ProviderTransitionIntentRecord {
    let mut ready = copy(staging);
    ready.state = TransitionState::Ready;
    ready.receipt_id = Some(receipt.digest().unwrap());
    ready.require_receipt(receipt).unwrap();
    ready
}

fn apply_publication(
    c: &Connection,
    f: &Fixture,
    activation: &ProviderActivationRecord,
    previous: &ProviderTransitionIntentRecord,
    next: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) -> Result<SourceIntentEffect> {
    apply_source_publication(
        c,
        &f.allocation,
        &f.prepared,
        &activation.binding,
        [0x41; 16],
        previous,
        next,
        receipt,
        None,
    )
}

fn publication(
    c: &Connection,
    f: &Fixture,
    activation: &ProviderActivationRecord,
) -> validation::PrivateActivePublicationValidation {
    private_active_publication_with_source(
        c,
        &f.allocation,
        &f.prepared,
        &activation.binding,
        Some([0x41; 16]),
        None,
    )
    .unwrap()
}

fn accepted_empty_capture(
    c: &Connection,
    f: &Fixture,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
) {
    let mut tx = ProviderCaptureTransactionRecord::begin(
        &f.allocation,
        mount,
        activation,
        "after-publication".into(),
        0,
        None,
    )
    .unwrap();
    let stage = ProviderCaptureStage {
        image_digest: RAW.into(),
        image_path_count: 1,
        changes: Vec::new(),
        reservation: "after-publication-reservation".into(),
    };
    let mut patch = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence: 1,
        previous_patch_digest: None,
        projection_generation: activated.generation,
        projection_digest: activated.digest.clone(),
        identity: f.allocation.format.identity.clone(),
        changes: Vec::new(),
        digest: String::new(),
    };
    patch.digest = crate::projection::provider::records::codec::hash(
        "layrs-projection-capture-v1",
        &canonical_json(&patch).unwrap(),
    );
    tx.state = ProviderCaptureState::Accepted {
        stage,
        patch: patch.clone(),
    };
    let receipt = ProviderCaptureReceiptRecord::derive(&tx).unwrap();
    c.execute("INSERT INTO projection_provider_capture_transactions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![tx.transaction_id,tx.mount_id,tx.capture_generation,tx.capture_digest,
            tx.expected_capture_sequence,tx.expected_capture_digest,tx.sql_state(),tx.reservation(),
            tx.encode().unwrap(),tx.digest().unwrap()]).unwrap();
    c.execute(
        "INSERT INTO projection_captures VALUES(?1,?2,?3,?4)",
        params![
            patch.sequence,
            patch.digest,
            patch.projection_generation,
            canonical_json(&patch).unwrap()
        ],
    )
    .unwrap();
    c.execute(
        "INSERT INTO projection_provider_capture_receipts VALUES(?1,?2,?3,?4,?5)",
        params![
            receipt.transaction_id,
            receipt.capture_sequence,
            receipt.capture_digest,
            receipt.encode().unwrap(),
            receipt.digest().unwrap()
        ],
    )
    .unwrap();
    let root = "a".repeat(64);
    c.execute(
        "INSERT INTO projection_claims VALUES(?1,?2,'provider-capture',?3)",
        params![root, root, tx.transaction_id],
    )
    .unwrap();
}

#[test]
#[ignore = "owned audited Windows SQLite process; source publication rollback and exact replay"]
fn source_publication_ready_published_replay_rollback_and_future_capture() {
    child::run(
        concat!(
            module_path!(),
            "::source_publication_ready_published_replay_rollback_and_future_capture"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, mount, activation, activated) = active_fixture(c, false);
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            let (staging, receipt) = staged(c, &f, &activation, &activated);
            let ready = ready(&staging, &receipt);
            // Target DATA only: this source handoff interpreter does not claim
            // destination SQL/physical authority from a synthesized fixture.
            let mut destination_ready = copy(&ready);
            destination_ready.role = TransitionRole::Destination;
            destination_ready.local_generation = 0;
            let publication_source = PublicationSource {
                allocation: &f.allocation,
                prepared: &f.prepared,
                binding: &activation.binding,
                root_id: [0x41; 16],
            };
            c.execute_batch("COMMIT; BEGIN IMMEDIATE").unwrap();
            let verify_import = |proof: &validation::PrivateActivePublicationValidation| {
                import_validation::verify_source_publication(
                    c,
                    &f.allocation.format,
                    &staging.plan.source_prefix,
                    ImportSourceReferences::Provider {
                        allocation: &f.allocation,
                        prepared: &f.prepared,
                        activation: &activation,
                    },
                    proof,
                    match &staging.plan.source_prefix.source {
                        ImportSource::Provider { version, .. } => &version.effective_image_digest,
                        _ => unreachable!(),
                    },
                    None,
                )
            };
            let staging_proof = publication(c, &f, &activation);
            let staging_closure = verify_import(&staging_proof).unwrap();
            assert_eq!(
                apply_publication(c, &f, &activation, &staging, &ready, &receipt).unwrap(),
                SourceIntentEffect::Updated
            );
            let proof = publication(c, &f, &activation);
            assert!(
                read_published_for_ready(c, publication_source, &destination_ready, &receipt, None)
                    .is_err(),
                "a receipt in SourceReady is not a Published acknowledgement"
            );
            assert!(
                validation::private_active_capture_mutation_with_source(
                    c,
                    &f.allocation,
                    &f.prepared,
                    &activation.binding,
                    None,
                    None,
                )
                .is_err(),
                "Ready remains pending and cannot become a capture gate"
            );
            let ready_closure = verify_import(&proof).unwrap();
            assert_eq!(
                ready_closure.closure(),
                staging_closure.closure(),
                "the outgoing receipt does not change incoming ancestry"
            );
            assert!(staging_closure.require_publication_inputs(&proof).is_err());
            assert!(
                verify_import(&staging_proof).is_err(),
                "a real Ready receipt invalidates the old full proof"
            );
            assert_eq!(
                proof.receipts,
                vec![ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap()]
            );
            assert!(proof.require_no_pending_transition().is_err());
            assert!(private_active_transition_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None
            )
            .is_err());
            c.execute_batch("ROLLBACK").unwrap();
            let before = publication(c, &f, &activation);
            assert_eq!(before.transitions, vec![copy(&staging)]);
            assert!(before.receipts.is_empty());
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            apply_publication(c, &f, &activation, &staging, &ready, &receipt).unwrap();
            c.execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN")
                .unwrap();
            let changes = c.total_changes();
            assert_eq!(
                apply_publication(c, &f, &activation, &staging, &ready, &receipt).unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK; PRAGMA query_only=OFF; BEGIN IMMEDIATE")
                .unwrap();
            let mut published = copy(&ready);
            published.state = TransitionState::Published;
            assert_eq!(
                apply_publication(c, &f, &activation, &ready, &published, &receipt).unwrap(),
                SourceIntentEffect::Updated
            );
            let published_proof = publication(c, &f, &activation)
                .require_no_pending_transition()
                .unwrap();
            let published_closure = verify_import(&published_proof).unwrap();
            assert_eq!(published_closure.closure(), staging_closure.closure());
            assert!(ready_closure
                .require_publication_inputs(&published_proof)
                .is_err());
            let published_gate = validation::private_active_capture_mutation_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            assert!(published_gate.capture().patches.is_empty());
            let handoff =
                read_published_for_ready(c, publication_source, &destination_ready, &receipt, None)
                    .unwrap();
            assert_eq!(handoff.published(), &published);
            assert_eq!(handoff.receipt(), &receipt);
            handoff.require(c, None).unwrap();
            assert!(
                read_published_for_ready(c, publication_source, &ready, &receipt, None).is_err(),
                "a source intent is not destination Ready DATA"
            );
            let mut wrong_destination = copy(&destination_ready);
            wrong_destination.transaction_id = "different-destination".into();
            assert!(read_published_for_ready(
                c,
                publication_source,
                &wrong_destination,
                &receipt,
                None
            )
            .is_err());
            let mut different_plan = copy(&destination_ready);
            different_plan.plan.quota_bytes += 1;
            let mut different_receipt =
                ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap();
            different_receipt.transition_plan_digest = different_plan.plan.digest().unwrap();
            let destination = different_plan.destination.as_ref().unwrap();
            different_receipt.destination_proof_digest =
                destination.digest(&different_plan.plan).unwrap();
            different_receipt.owner_identity_digest =
                destination.owner_digest(&different_plan.plan).unwrap();
            different_plan.receipt_id = Some(different_receipt.digest().unwrap());
            different_plan.require_receipt(&different_receipt).unwrap();
            let error = read_published_for_ready(
                c,
                publication_source,
                &different_plan,
                &different_receipt,
                None,
            )
            .err()
            .expect("internally coherent target cannot substitute a different plan");
            assert!(
                error.to_string().contains("source and destination differ"),
                "{error}"
            );
            accepted_empty_capture(c, &f, &mount, &activation, &activated);
            handoff.require(c, None).unwrap();
            assert_eq!(
                read_published_for_ready(c, publication_source, &destination_ready, &receipt, None)
                    .unwrap()
                    .published(),
                &published,
                "later captures do not revoke historical Published"
            );
            let captured_gate = validation::private_active_capture_mutation_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            )
            .unwrap();
            captured_gate
                .require_same_publications(&published_gate)
                .unwrap();
            assert!(
                captured_gate.require_same_state(&published_gate).is_err(),
                "a moved capture head cannot reuse an older complete context"
            );
            assert_eq!(captured_gate.capture().patches.len(), 1);
            let proof = publication(c, &f, &activation)
                .require_no_pending_transition()
                .unwrap();
            assert_eq!(proof.capture.patches.len(), 1);
            assert_eq!(proof.transitions, vec![copy(&published)]);
            assert_eq!(proof.receipts[0], receipt);
            let before_refusal = c.total_changes();
            assert!(
                verify_import(&proof).is_err(),
                "the publication import wrapper requires the original current capture head"
            );
            assert!(
                verify_import(&published_proof).is_err(),
                "the pre-capture publication proof is stale"
            );
            assert_eq!(c.total_changes(), before_refusal);
            c.execute_batch("COMMIT; PRAGMA query_only=ON; BEGIN")
                .unwrap();
            let changes = c.total_changes();
            assert_eq!(
                apply_publication(c, &f, &activation, &ready, &published, &receipt).unwrap(),
                SourceIntentEffect::AlreadyPresent
            );
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK").unwrap();
            assert!(handoff.require(c, None).is_err());
            drop(handoff);
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; source publication refuses foreign and conflicting DATA"]
fn source_publication_refuses_forgery_fkoff_orphans_and_competing_ready() {
    child::run(
        concat!(
            module_path!(),
            "::source_publication_refuses_forgery_fkoff_orphans_and_competing_ready"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, activated) = active_fixture(c, false);
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            let (staging, receipt) = staged(c, &f, &activation, &activated);
            let ready = ready(&staging, &receipt);
            let mut wrong_previous = copy(&staging);
            wrong_previous.plan.quota_bytes = 1;
            let changes = c.total_changes();
            assert!(
                apply_publication(c, &f, &activation, &wrong_previous, &ready, &receipt).is_err()
            );
            let mut wrong_receipt =
                ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap();
            wrong_receipt.destination_capture_digest = TOKEN.replace('1', "2");
            assert!(
                apply_publication(c, &f, &activation, &staging, &ready, &wrong_receipt).is_err()
            );
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("COMMIT; PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
                .unwrap();
            let changes = c.total_changes();
            assert!(apply_publication(c, &f, &activation, &staging, &ready, &receipt).is_err());
            assert_eq!(
                c.total_changes(),
                changes,
                "FK-off refusal must precede receipt insertion"
            );
            c.execute_batch("ROLLBACK; PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            apply_publication(c, &f, &activation, &staging, &ready, &receipt).unwrap();
            // The schema's partial unique index excludes Ready; the typed reader
            // must still reject this second pending source intent.
            let mut competing = copy(&staging);
            competing.transaction_id = "00000000000000000000000000000003".into();
            competing.state = TransitionState::Planned;
            competing.destination = None;
            competing.reservation = None;
            c.execute("INSERT INTO projection_provider_transitions VALUES(?1,'source','planned',?2,NULL,NULL,?3,?4)",
            params![competing.transaction_id,competing.local_generation,competing.encode().unwrap(),competing.digest().unwrap()]).unwrap();
            let mut published = copy(&ready);
            published.state = TransitionState::Published;
            let changes = c.total_changes();
            assert!(apply_publication(c, &f, &activation, &ready, &published, &receipt).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute(
                "DELETE FROM projection_provider_transitions WHERE id=?1",
                [&competing.transaction_id],
            )
            .unwrap();
            c.execute(
                "UPDATE projection_provider_transition_receipts SET owner_identity_digest=?1",
                [TOKEN],
            )
            .unwrap();
            let changes = c.total_changes();
            assert!(apply_publication(c, &f, &activation, &ready, &published, &receipt).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute(
                "UPDATE projection_provider_transition_receipts SET owner_identity_digest=?1",
                [&receipt.owner_identity_digest],
            )
            .unwrap();
            let mut orphan =
                ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap();
            orphan.transaction_id = "00000000000000000000000000000004".into();
            c.execute(
                "INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,?3,NULL,?4)",
                params![
                    orphan.digest().unwrap(),
                    orphan.owner_identity_digest,
                    orphan.transaction_id,
                    orphan.encode().unwrap()
                ],
            )
            .unwrap();
            let changes = c.total_changes();
            assert!(apply_publication(c, &f, &activation, &ready, &published, &receipt).is_err());
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; publication preflight precedes global FK traversal"]
fn source_publication_bounds_terminal_receipts_before_fk() {
    child::run(
        concat!(
            module_path!(),
            "::source_publication_bounds_terminal_receipts_before_fk"
        ),
        120,
        || {
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let c = bare.connection();
            let (f, _, _, activation, _) = active_fixture(c, false);
            c.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
                .unwrap();
            // Deliberately invalid companion FK and undecodable payload: bounds
            // must reject these rows before a global FK scan or materialization.
            for ordinal in 0..4097 {
                c.execute("INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,?3,?4,zeroblob(0))",
                params![format!("blake3:{ordinal:064x}"), TOKEN, format!("receipt-{ordinal}"), TOKEN]).unwrap();
            }
            let changes = c.total_changes();
            let error = match private_active_publication_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            ) {
                Ok(_) => panic!("overbound publication receipts were admitted"),
                Err(error) => error.to_string(),
            };
            assert!(
                error.contains("source publication receipt count exceeds bound"),
                "{error}"
            );
            assert_eq!(c.total_changes(), changes);
            c.execute("DELETE FROM projection_provider_transition_receipts", [])
                .unwrap();
            c.execute_batch("PRAGMA ignore_check_constraints=ON")
                .unwrap();
            c.execute("INSERT INTO projection_provider_transition_receipts VALUES(?1,?2,'overlong',?3,zeroblob(0))",
            params![format!("{TOKEN}x"),TOKEN,TOKEN]).unwrap();
            let changes = c.total_changes();
            let error = match private_active_publication_with_source(
                c,
                &f.allocation,
                &f.prepared,
                &activation.binding,
                None,
                None,
            ) {
                Ok(_) => panic!("overlong receipt projection was admitted"),
                Err(error) => error.to_string(),
            };
            assert!(
                error
                    .contains("source publication receipt SQL projection or payload exceeds bound"),
                "{error}"
            );
            assert_eq!(c.total_changes(), changes);
            c.execute_batch("ROLLBACK").unwrap();
            bare.finish(Ok(())).unwrap();
        },
    );
}
