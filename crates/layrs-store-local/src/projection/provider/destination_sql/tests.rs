use super::*;
use crate::projection::provider::{
    database_io::BareConnection,
    import_validation::{self, ImportDatabaseKind},
    process_profile::tests::child,
    records::{
        Receipt,
        import::{
            ImportClosure, ImportRowset, ImportSource, ImportSourceReferences, ImportVersion,
            ProviderImportKind, ProviderImportPrefixRecord,
        },
        transition::{
            ProviderTransitionIntentRecord, TransitionBackendKind, TransitionDestinationProof,
            TransitionObservation, TransitionPlan, TransitionRole, TransitionRowset,
            TransitionState,
        },
    },
    validation::{active_fixture, private_active_capture_with_source},
};
use layrs_core::scm::WorktreeId;

#[path = "current_tests.rs"]
mod current_tests;

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LEAF: &str = "00000000000000000000000000000001";
const TX: &str = "00000000000000000000000000000002";
const DIR: &str = "fs:v1:1111111111111111:2222222222222222";

fn row(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: TOKEN.into(),
    }
}
fn transition_row() -> TransitionRowset {
    TransitionRowset {
        count: 0,
        digest: TOKEN.into(),
    }
}

struct Inputs {
    fixture: crate::projection::provider::validation::tests::fixture::Fixture,
    activation: ProviderActivationRecord,
    marker: format::Marker,
    prefix: ProviderImportPrefixRecord,
    intent: ProviderTransitionIntentRecord,
}
fn inputs(source: &Connection) -> Inputs {
    let (fixture, _, _, activation, activated) = active_fixture(source, false);
    let mut marker = fixture.allocation.format.clone();
    marker.identity.worktree_id = WorktreeId::parse("worktree:managed-destination").unwrap();
    marker.checkout_root = format!("C:\\managed-parent\\{LEAF}");
    marker.shared.as_mut().unwrap().database_identity = "1:0:123".into();
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-projection-provider-import-segments-v1\0");
    h.update(&0u64.to_be_bytes());
    let prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Provider {
            identity: fixture.allocation.format.identity.clone(),
            format_digest: crate::projection::provider::records::codec::hash(
                "layrs-projection-provider-import-format-v1",
                &fixture.allocation.format.bytes().unwrap(),
            ),
            private_schema: 8,
            allocation: fixture.allocation.data_reference().unwrap(),
            prepared: fixture.prepared.data_reference().unwrap(),
            binding: crate::projection::provider::records::ProviderBootstrapBinding::decode(
                &activation.binding.encode().unwrap(),
            )
            .unwrap(),
            activation_map_id: activation.digest().unwrap(),
            version: ImportVersion {
                generation: 1,
                snapshot_digest: activated.digest,
                selection_digest: activation.selection_digest.clone(),
                capture_count: 0,
                capture_head: None,
                effective_image_digest: RAW.into(),
            },
        },
        destination_identity: marker.identity.clone(),
        inherited_segments: ImportRowset {
            count: 0,
            digest: format!("blake3:{}", h.finalize()),
        },
        ancestry_root_receipt: None,
        closure: ImportClosure {
            denial_observations: None,
            restore_history: None,
            snapshots: row(2),
            selections: row(1),
            captures: row(0),
            publications: row(0),
            catalogs: row(1),
            descriptors: row(1),
            activation_maps: row(1),
            transition_receipts: row(0),
            companions: row(0),
            companion_files: row(0),
            companion_deletions: row(0),
            content_mappings: row(0),
        },
    };
    marker.import_digest = Some(prefix.digest().unwrap());
    let proof = TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format: marker.clone(),
        private_database_identity: "1:0:123".into(),
        family_marker_digest: fixture.allocation.family.marker_digest.clone(),
        format_file_identity: "1:0:124".into(),
        native_control_identity_hex: None,
        native_root_identity: None,
    };
    let intent = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: TX.into(),
        role: TransitionRole::Destination,
        state: TransitionState::Copying,
        local_generation: 0,
        plan: TransitionPlan {
            denial_coverage: None,
            kind: ProviderImportKind::ManagedOrdinary,
            source_prefix_id: prefix.digest().unwrap(),
            source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
            destination_parent: "C:\\managed-parent".into(),
            destination_parent_identity: DIR.into(),
            destination_leaf_nonce: LEAF.into(),
            observation: TransitionObservation {
                step_id: fixture.source.initial().base_step_id.clone(),
                root_tree_id: fixture.source.initial().base_root_tree_id.clone(),
                policy_set_id: fixture.source.initial().observed_policy_set_id.clone(),
                authorization_epoch: fixture.source.initial().observed_authorization_epoch,
                generation: 2,
                snapshot_digest: RAW.into(),
                selection_digest: activation.selection_digest.clone(),
            },
            placement_generation: 3,
            placement_digest: RAW.into(),
            capture_sequence: 1,
            capture_digest: TOKEN.into(),
            working_overlay: transition_row(),
            companion_id: None,
            companion_files: transition_row(),
            companion_deletions: transition_row(),
            protected_manifests: transition_row(),
            quota_bytes: 0,
        },
        destination: Some(proof),
        reservation: None,
        receipt_id: None,
    };
    intent.validate_fields().unwrap();
    Inputs {
        fixture,
        activation,
        marker,
        prefix,
        intent,
    }
}
fn initialize(c: &Connection, data: &Inputs) -> Result<()> {
    initialize_empty(
        c,
        &data.marker,
        &data.fixture.source,
        &data.fixture.allocation,
        &data.fixture.prepared,
        &data.activation,
        &data.prefix,
        &data.intent,
        None,
    )
}
fn inspect(c: &Connection, data: &Inputs) -> Result<destination_validation::EmptyIngress> {
    destination_validation::read_empty(
        c,
        &data.marker,
        &data.fixture.source,
        &data.fixture.allocation,
        &data.fixture.prepared,
        &data.activation,
        &data.prefix,
        &data.intent,
        None,
    )
}

fn verify_provider_prefix<'a>(
    source_connection: &'a Connection,
    data: &mut Inputs,
) -> import_validation::VerifiedImportClosure {
    let marker = &data.fixture.allocation.format;
    let metadata = import_validation::read_source_metadata(
        source_connection,
        marker,
        ImportDatabaseKind::Provider,
        &data.prefix,
        ImportSourceReferences::Provider {
            allocation: &data.fixture.allocation,
            prepared: &data.fixture.prepared,
            activation: &data.activation,
        },
        RAW,
        None,
    )
    .unwrap();
    data.prefix.closure = import_validation::compute_scoped_closure(
        source_connection,
        ImportDatabaseKind::Provider,
        &data.prefix,
        &metadata,
        None,
    )
    .unwrap();
    data.marker.import_digest = Some(data.prefix.digest().unwrap());
    data.intent.plan.source_prefix =
        ProviderImportPrefixRecord::decode(&data.prefix.encode().unwrap()).unwrap();
    data.intent.plan.source_prefix_id = data.prefix.digest().unwrap();
    data.intent.destination.as_mut().unwrap().format = data.marker.clone();
    data.intent.validate_fields().unwrap();
    let active = private_active_capture_with_source(
        source_connection,
        &data.fixture.allocation,
        &data.fixture.prepared,
        &data.activation.binding,
        Some([0x41; 16]),
        None,
    )
    .unwrap();
    import_validation::verify_source(
        source_connection,
        marker,
        ImportDatabaseKind::Provider,
        &data.prefix,
        ImportSourceReferences::Provider {
            allocation: &data.fixture.allocation,
            prepared: &data.fixture.prepared,
            activation: &data.activation,
        },
        Some(&active),
        RAW,
        None,
    )
    .unwrap()
}

#[test]
#[ignore = "owned audited Windows SQLite process; EmptyIngress transaction boundary"]
fn empty_ingress_roundtrip_and_transaction_rollback() {
    child::run(
        concat!(
            module_path!(),
            "::empty_ingress_roundtrip_and_transaction_rollback"
        ),
        120,
        || {
            child::declare();
            let source = BareConnection::reference().unwrap();
            let data = inputs(source.connection());
            let destination = BareConnection::reference().unwrap();
            let c = destination.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            let mut wrong_intent =
                ProviderTransitionIntentRecord::decode(&data.intent.encode().unwrap()).unwrap();
            wrong_intent
                .destination
                .as_mut()
                .unwrap()
                .family_marker_digest = TOKEN.into();
            assert!(
                initialize_empty(
                    c,
                    &data.marker,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &wrong_intent,
                    None,
                )
                .is_err()
            );
            assert!(database_io::schema_inventory(c).unwrap().is_empty());
            initialize(c, &data).unwrap();
            inspect(c, &data).unwrap();
            assert!(initialize(c, &data).is_err());
            c.execute_batch("ROLLBACK").unwrap();
            let version: u64 = c
                .query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(version, 0);
            assert!(database_io::schema_inventory(c).unwrap().is_empty());
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            initialize(c, &data).unwrap();
            c.execute_batch("COMMIT; BEGIN DEFERRED").unwrap();
            inspect(c, &data).unwrap();
            c.execute_batch("COMMIT").unwrap();
            destination.finish(Ok(())).unwrap();
            source.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; EmptyIngress strict reader"]
fn empty_ingress_refuses_marker_backend_and_foreign_rows() {
    child::run(
        concat!(
            module_path!(),
            "::empty_ingress_refuses_marker_backend_and_foreign_rows"
        ),
        120,
        || {
            child::declare();
            let source = BareConnection::reference().unwrap();
            let data = inputs(source.connection());
            let destination = BareConnection::reference().unwrap();
            let c = destination.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            initialize(c, &data).unwrap();
            let mut wrong = data.marker.clone();
            wrong.identity.worktree_id = WorktreeId::parse("worktree:wrong").unwrap();
            assert!(
                destination_validation::read_empty(
                    c,
                    &wrong,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &data.intent,
                    None
                )
                .is_err()
            );
            c.execute_batch("SAVEPOINT backend").unwrap();
            c.execute(
                "UPDATE projection_backend SET backend='provider',mount_id='bad'",
                [],
            )
            .unwrap_err();
            c.execute("DELETE FROM projection_backend", []).unwrap();
            assert!(inspect(c, &data).is_err());
            c.execute_batch("ROLLBACK TO backend; RELEASE backend")
                .unwrap();
            c.execute_batch("SAVEPOINT initial").unwrap();
            c.execute(
                "UPDATE projection_snapshots SET digest=?1 WHERE generation=0",
                ["0".repeat(64)],
            )
            .unwrap();
            assert!(inspect(c, &data).is_err());
            c.execute_batch("ROLLBACK TO initial; RELEASE initial")
                .unwrap();
            c.execute_batch("SAVEPOINT foreign_row").unwrap();
            c.execute(
                "INSERT INTO projection_provider_companions VALUES(?1,0,0,?2,?2,?3)",
                rusqlite::params![TOKEN, TOKEN, b"{}".to_vec()],
            )
            .unwrap();
            assert!(inspect(c, &data).is_err());
            c.execute_batch("ROLLBACK TO foreign_row; RELEASE foreign_row; COMMIT")
                .unwrap();
            destination.finish(Ok(())).unwrap();
            source.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; required current body missing"]
fn empty_ingress_refuses_v2_when_required_current_body_is_missing() {
    child::run(
        concat!(
            module_path!(),
            "::empty_ingress_refuses_v2_when_required_current_body_is_missing"
        ),
        120,
        || {
            child::declare();
            let source = BareConnection::reference().unwrap();
            let mut data = inputs(source.connection());
            let destination = BareConnection::reference().unwrap();
            let c = destination.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            initialize(c, &data).unwrap();
            inspect(c, &data).unwrap();
            let legacy = data.intent.encode().unwrap();
            c.execute_batch("SAVEPOINT unsupported_v2").unwrap();
            data.intent.schema_version = "layrs.projection-provider-transition-intent.v2".into();
            data.intent.plan.denial_coverage = Some(
                crate::projection::provider::records::denied_coverage::DeniedCoverageRef {
                    id: TOKEN.into(),
                    count: 0,
                    path_bytes: 0,
                    rows_digest: TOKEN.into(),
                },
            );
            data.intent.validate_fields().unwrap();
            c.execute(
                "UPDATE projection_provider_transitions SET payload=?1,digest=?2",
                rusqlite::params![data.intent.encode().unwrap(), data.intent.digest().unwrap()],
            )
            .unwrap();
            let changes = c.total_changes();
            let error = inspect(c, &data)
                .err()
                .expect("v2 without its actual body accepted");
            assert!(matches!(error,
                crate::projection::provider::recovery::error::ProviderBootstrapCause::Validation(ref cause)
                if matches!(cause.as_ref(), crate::projection::ProjectionStoreError::Invalid(message)
                    if message == "destination EmptyIngress table cardinality differs")
            ));
            assert_eq!(changes, c.total_changes());
            c.execute_batch("ROLLBACK TO unsupported_v2; RELEASE unsupported_v2")
                .unwrap();
            data.intent = ProviderTransitionIntentRecord::decode(&legacy).unwrap();
            inspect(c, &data).unwrap();
            c.execute_batch("ROLLBACK").unwrap();
            destination.finish(Ok(())).unwrap();
            source.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; bounded hostile TEXT preflight"]
fn empty_ingress_refuses_oversized_parent_and_mount_before_text_read() {
    child::run(
        concat!(
            module_path!(),
            "::empty_ingress_refuses_oversized_parent_and_mount_before_text_read"
        ),
        120,
        || {
            child::declare();
            let source = BareConnection::reference().unwrap();
            let data = inputs(source.connection());
            let destination = BareConnection::reference().unwrap();
            let c = destination.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            initialize(c, &data).unwrap();
            c.execute_batch("COMMIT; PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
                .unwrap();
            c.execute_batch("SAVEPOINT parent").unwrap();
            c.execute(
                "UPDATE projection_snapshots SET parent_digest=?1 WHERE generation=0",
                ["x".repeat(1024 * 1024)],
            )
            .unwrap();
            assert!(inspect(c, &data).is_err());
            c.execute_batch("ROLLBACK TO parent; RELEASE parent")
                .unwrap();
            c.execute_batch("SAVEPOINT selection_digest").unwrap();
            c.execute(
                "UPDATE projection_selections SET digest=?1 WHERE generation=0",
                [format!("{RAW}\0tail")],
            )
            .unwrap();
            assert!(inspect(c, &data).is_err());
            c.execute_batch("ROLLBACK TO selection_digest; RELEASE selection_digest")
                .unwrap();
            c.execute_batch("SAVEPOINT mount").unwrap();
            c.execute(
                "UPDATE projection_backend SET backend='provider',mount_id=?1",
                ["x".repeat(1024 * 1024)],
            )
            .unwrap();
            assert!(inspect(c, &data).is_err());
            c.execute_batch("ROLLBACK TO mount; RELEASE mount").unwrap();
            inspect(c, &data).unwrap();
            c.execute_batch("COMMIT").unwrap();
            destination.finish(Ok(())).unwrap();
            source.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; copied provider prefix promotion"]
fn copied_prefix_promotes_replays_and_refuses_tail_or_head_corruption() {
    child::run(
        concat!(
            module_path!(),
            "::copied_prefix_promotes_replays_and_refuses_tail_or_head_corruption"
        ),
        120,
        || {
            child::declare();
            let source = BareConnection::reference().unwrap();
            let source_c = source.connection();
            let mut data = inputs(source_c);
            source_c.execute_batch("BEGIN DEFERRED").unwrap();
            let proof = verify_provider_prefix(source_c, &mut data);
            let destination = BareConnection::reference().unwrap();
            let c = destination.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            initialize(c, &data).unwrap();
            let wrong_source = BareConnection::reference().unwrap();
            wrong_source
                .connection()
                .execute_batch("BEGIN DEFERRED")
                .unwrap();
            assert!(
                promote_or_replay_copied(
                    wrong_source.connection(),
                    c,
                    &data.marker,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &data.intent,
                    &proof,
                    None,
                )
                .is_err()
            );
            inspect(c, &data).unwrap();
            wrong_source.connection().execute_batch("COMMIT").unwrap();
            wrong_source.finish(Ok(())).unwrap();
            let promote = || {
                promote_or_replay_copied(
                    source_c,
                    c,
                    &data.marker,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &data.intent,
                    &proof,
                    None,
                )
            };
            promote().unwrap();
            let before_replay = c.total_changes();
            promote().unwrap();
            assert_eq!(
                c.total_changes(),
                before_replay,
                "copied replay must not write"
            );
            let head: u64 = c
                .query_row("SELECT generation FROM projection_head", [], |r| r.get(0))
                .unwrap();
            assert_eq!(head, 1);
            c.execute_batch("SAVEPOINT tail").unwrap();
            c.execute(
                "INSERT INTO projection_snapshots(generation,digest,parent_digest,payload) SELECT 2,?1,digest,payload FROM projection_snapshots WHERE generation=1",
                ["9".repeat(64)],
            ).unwrap();
            assert!(promote().is_err());
            c.execute_batch("ROLLBACK TO tail; RELEASE tail").unwrap();
            c.execute_batch("SAVEPOINT head").unwrap();
            c.execute("UPDATE projection_head SET generation=0", [])
                .unwrap();
            assert!(promote().is_err());
            c.execute_batch("ROLLBACK TO head; RELEASE head").unwrap();
            c.execute_batch("ROLLBACK").unwrap();
            assert!(database_io::schema_inventory(c).unwrap().is_empty());
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            initialize(c, &data).unwrap();
            promote().unwrap();
            c.execute_batch("COMMIT; BEGIN DEFERRED").unwrap();
            destination_validation::read_copied(
                c,
                &data.marker,
                &data.fixture.source,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation,
                &data.prefix,
                &data.intent,
                &proof,
                None,
            )
            .unwrap();
            c.execute_batch("COMMIT").unwrap();
            source_c.execute_batch("COMMIT").unwrap();
            destination.finish(Ok(())).unwrap();
            source.finish(Ok(())).unwrap();
        },
    );
}

#[test]
#[ignore = "owned audited Windows SQLite process; copied prefix staging CAS"]
fn copied_prefix_staging_is_exact_replayable_and_rollback_safe() {
    child::run(
        concat!(
            module_path!(),
            "::copied_prefix_staging_is_exact_replayable_and_rollback_safe"
        ),
        120,
        || {
            child::declare();
            let source = BareConnection::reference().unwrap();
            let source_c = source.connection();
            let mut data = inputs(source_c);
            source_c.execute_batch("BEGIN DEFERRED").unwrap();
            let proof = verify_provider_prefix(source_c, &mut data);
            let destination = BareConnection::reference().unwrap();
            let c = destination.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .unwrap();
            initialize(c, &data).unwrap();
            promote_or_replay_copied(
                source_c,
                c,
                &data.marker,
                &data.fixture.source,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation,
                &data.prefix,
                &data.intent,
                &proof,
                None,
            )
            .unwrap();
            let mut staging =
                ProviderTransitionIntentRecord::decode(&data.intent.encode().unwrap()).unwrap();
            staging.state = TransitionState::Staging;
            staging.reservation = Some(staging.reservation_operation().unwrap());
            staging.require_transition(&data.intent).unwrap();
            let stage = |expected: &ProviderTransitionIntentRecord| {
                stage_copied(
                    source_c,
                    c,
                    &data.marker,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &data.intent,
                    expected,
                    &proof,
                    None,
                )
            };
            assert_eq!(stage(&staging).unwrap(), CopiedStagingEffect::Updated);
            assert!(
                destination_validation::read_copied(
                    c,
                    &data.marker,
                    &data.fixture.source,
                    &data.fixture.allocation,
                    &data.fixture.prepared,
                    &data.activation,
                    &data.prefix,
                    &data.intent,
                    &proof,
                    None,
                )
                .is_err(),
                "Copying reader must reject Staging"
            );
            let before_replay = c.total_changes();
            assert_eq!(
                stage(&staging).unwrap(),
                CopiedStagingEffect::AlreadyPresent
            );
            assert_eq!(
                c.total_changes(),
                before_replay,
                "staging replay must not write"
            );
            let mut forged =
                ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
            forged.plan.quota_bytes += 1;
            assert!(stage(&forged).is_err());
            assert_eq!(c.total_changes(), before_replay);
            c.execute_batch("SAVEPOINT tail").unwrap();
            c.execute(
                "INSERT INTO projection_snapshots(generation,digest,parent_digest,payload) SELECT 2,?1,digest,payload FROM projection_snapshots WHERE generation=1",
                ["9".repeat(64)],
            ).unwrap();
            assert!(stage(&staging).is_err());
            c.execute_batch("ROLLBACK TO tail; RELEASE tail").unwrap();
            c.execute_batch("ROLLBACK").unwrap();
            assert!(database_io::schema_inventory(c).unwrap().is_empty());
            c.execute_batch("BEGIN IMMEDIATE").unwrap();
            initialize(c, &data).unwrap();
            promote_or_replay_copied(
                source_c,
                c,
                &data.marker,
                &data.fixture.source,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation,
                &data.prefix,
                &data.intent,
                &proof,
                None,
            )
            .unwrap();
            assert_eq!(stage(&staging).unwrap(), CopiedStagingEffect::Updated);
            c.execute_batch("COMMIT; BEGIN DEFERRED").unwrap();
            destination_validation::read_staging(
                c,
                &data.marker,
                &data.fixture.source,
                &data.fixture.allocation,
                &data.fixture.prepared,
                &data.activation,
                &data.prefix,
                &staging,
                &proof,
                None,
            )
            .unwrap();
            c.execute_batch("COMMIT").unwrap();
            source_c.execute_batch("COMMIT").unwrap();
            destination.finish(Ok(())).unwrap();
            source.finish(Ok(())).unwrap();
        },
    );
}
