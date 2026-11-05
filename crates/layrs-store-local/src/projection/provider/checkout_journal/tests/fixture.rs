//! Structurally valid expected DATA. These fixtures do not pretend to qualify
//! source/destination admission: the SQL tests exercise this journal kernel.
use super::*;
use crate::projection::provider::records::{
    import::{
        ImportClosure, ImportRowset, ImportSource, ImportVersion, ProviderImportPrefixRecord,
    },
    transition::{
        TransitionDestinationProof, TransitionObservation, TransitionPlan, TransitionRole,
        TransitionRowset,
    },
    PreparedBinding, ProviderBootstrapBinding, Receipt,
};
use crate::projection::{database, retention, selection, RecoveryBinding};
use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest, WorktreeId};

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
pub(super) const IMAGE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(super) const STAGE: &str = "fs:v1:0000000000000001:0000000000001000";
pub(super) const DIRECTORY: &str = "fs:v1:0000000000000001:0000000000001001";

pub(super) struct Fixture {
    pub staging: ProviderTransitionIntentRecord,
    pub files: Vec<CheckoutFile>,
    pub deletions: Vec<RepoPath>,
}
impl Fixture {
    pub fn new() -> Self {
        let f = crate::projection::provider::validation::tests::fixture::Fixture::new("G1", false);
        let binding = ProviderBootstrapBinding {
            schema_version: "layrs.projection-provider-bootstrap-binding.v1".into(),
            recovery: RecoveryBinding {
                identity: f.allocation.format.identity.clone(),
                checkout_identity: f.allocation.format.checkout_identity().into(),
                marker_digest: super::super::super::records::codec::hash(
                    "layrs-projection-recovery-marker-v1",
                    &f.allocation.format.bytes().unwrap(),
                ),
                database_identity: f.allocation.private.database_file_identity.clone(),
                family_marker_digest: Some(f.allocation.family.marker_digest.clone()),
            },
            transaction_id: f.allocation.transaction_id.clone(),
            allocation: f.allocation.data_reference().unwrap(),
            prepared: Some(PreparedBinding {
                receipt: f.prepared.data_reference().unwrap(),
                control_identity_hex: f.prepared.control_identity_hex.clone(),
            }),
            abandoned: None,
        };
        let mut marker = serde_json::to_value(&f.allocation.format).unwrap();
        marker["identity"]["worktreeId"] = "worktree:checkout-destination".into();
        marker["checkoutRoot"] = "C:\\checkout-parent\\00000000000000000000000000000003".into();
        marker["checkoutIdentity"] = "fs:v1:0000000000000001:0000000000000100".into();
        marker["storeIdentity"] = "fs:v1:0000000000000001:0000000000000101".into();
        marker["metadataIdentity"] = "fs:v1:0000000000000001:0000000000000102".into();
        marker["shared"]["databaseIdentity"] = "1:0:123".into();
        let mut marker: format::Marker = serde_json::from_value(marker).unwrap();
        assert_eq!(
            marker.identity.worktree_id,
            WorktreeId::parse("worktree:checkout-destination").unwrap()
        );
        let row = |count| ImportRowset {
            count,
            digest: TOKEN.into(),
        };
        let prefix = ProviderImportPrefixRecord {
            schema_version: "layrs.projection-provider-import-prefix.v1".into(),
            kind: ProviderImportKind::ManagedOrdinary,
            source: ImportSource::Provider {
                identity: f.allocation.format.identity.clone(),
                format_digest: TOKEN.into(),
                private_schema: 8,
                allocation: f.allocation.data_reference().unwrap(),
                prepared: f.prepared.data_reference().unwrap(),
                binding,
                activation_map_id: TOKEN.into(),
                version: ImportVersion {
                    generation: 1,
                    snapshot_digest: "b".repeat(64),
                    selection_digest: f.source.commitment().selection_digest.clone(),
                    capture_count: 0,
                    capture_head: None,
                    effective_image_digest: IMAGE.into(),
                },
            },
            destination_identity: marker.identity.clone(),
            inherited_segments: row(0),
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
        let empty = TransitionRowset {
            count: 0,
            digest: TOKEN.into(),
        };
        let plan = TransitionPlan {
            denial_coverage: None,
            kind: ProviderImportKind::ManagedOrdinary,
            source_prefix_id: prefix.digest().unwrap(),
            source_prefix: prefix,
            destination_parent: "C:\\checkout-parent".into(),
            destination_parent_identity: "fs:v1:0000000000000001:0000000000000103".into(),
            destination_leaf_nonce: "00000000000000000000000000000003".into(),
            observation: TransitionObservation {
                step_id: f.source.initial().base_step_id.clone(),
                root_tree_id: f.source.initial().base_root_tree_id.clone(),
                policy_set_id: f.source.initial().observed_policy_set_id.clone(),
                authorization_epoch: f.source.initial().observed_authorization_epoch,
                generation: 2,
                snapshot_digest: "c".repeat(64),
                selection_digest: f.source.commitment().selection_digest.clone(),
            },
            placement_generation: 3,
            placement_digest: "d".repeat(64),
            capture_sequence: 1,
            capture_digest: TOKEN.into(),
            working_overlay: empty.clone(),
            companion_id: None,
            companion_files: empty.clone(),
            companion_deletions: empty,
            protected_manifests: TransitionRowset {
                count: 2,
                digest: TOKEN.into(),
            },
            quota_bytes: 1048576,
        };
        let destination = TransitionDestinationProof {
            backend: TransitionBackendKind::Ordinary,
            format: marker,
            format_file_identity: "1:0:124".into(),
            private_database_identity: "1:0:123".into(),
            family_marker_digest: f.allocation.family.marker_digest.clone(),
            native_control_identity_hex: None,
            native_root_identity: None,
        };
        let mut staging = ProviderTransitionIntentRecord {
            schema_version: "layrs.projection-provider-transition-intent.v1".into(),
            transaction_id: "00000000000000000000000000000002".into(),
            role: TransitionRole::Destination,
            state: TransitionState::Staging,
            local_generation: 0,
            plan,
            destination: Some(destination),
            reservation: None,
            receipt_id: None,
        };
        staging.reservation = Some(staging.reservation_operation().unwrap());
        staging.validate_fields().unwrap();
        let files = [
            ("dir/a", b"first".as_slice()),
            ("dir/b", b"second".as_slice()),
        ]
        .into_iter()
        .map(|(path, bytes)| {
            let m = FileManifest::from_chunks(
                ChunkingDescriptor::single(),
                &[Chunk::new(bytes.to_vec()).unwrap()],
            )
            .unwrap();
            CheckoutFile {
                path: RepoPath::parse(path).unwrap(),
                manifest_id: m.id().clone(),
                file_id: m.file_id().clone(),
                bytes: m.byte_length(),
                executable: false,
            }
        })
        .collect();
        Self {
            staging,
            files,
            deletions: vec![RepoPath::parse("removed").unwrap()],
        }
    }
    pub fn expected(&self) -> ExpectedCheckout {
        ExpectedCheckout::new(&self.staging, &self.files, &self.deletions, IMAGE, None).unwrap()
    }
}
pub(super) fn schema(c: &Connection, expected: &ExpectedCheckout) {
    c.execute_batch(
        &[
            database::SQL,
            selection::SQL,
            retention::SQL,
            crate::projection::provider::schema::SQL,
            "PRAGMA user_version=8;",
        ]
        .concat(),
    )
    .unwrap();
    c.execute(
        "INSERT INTO projection_meta VALUES(1,?1)",
        [expected.marker.bytes().unwrap()],
    )
    .unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
        .unwrap();
}
