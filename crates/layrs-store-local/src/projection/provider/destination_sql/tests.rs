use super::*;
use crate::projection::provider::{
    database_io::BareConnection,
    process_profile::tests::child,
    records::{
        import::{
            ImportClosure, ImportRowset, ImportSource, ImportVersion, ProviderImportKind,
            ProviderImportPrefixRecord,
        },
        transition::{
            ProviderTransitionIntentRecord, TransitionBackendKind, TransitionDestinationProof,
            TransitionObservation, TransitionPlan, TransitionRole, TransitionRowset,
            TransitionState,
        },
        Receipt,
    },
    validation::active_fixture,
};
use layrs_core::scm::WorktreeId;

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
            assert!(initialize_empty(
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
            .is_err());
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
            assert!(destination_validation::read_empty(
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
            .is_err());
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
