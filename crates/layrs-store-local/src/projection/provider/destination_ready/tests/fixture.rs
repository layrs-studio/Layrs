//! Real Staging SQL populated through the existing initializer, verified prefix
//! copy and Staging CAS. All filesystem identities are explicitly DATA only.
use super::*;
use crate::projection::provider::{
    destination_sql,
    import_validation::{self, ImportDatabaseKind},
    records::{
        capture::{
            ProviderCaptureReceiptRecord, ProviderCaptureStage, ProviderCaptureState,
            ProviderCaptureTransactionRecord,
        },
        import::{
            ImportClosure, ImportRowset, ImportSourceReferences, ImportVersion, ProviderImportKind,
        },
        transition::{
            TransitionBackendKind, TransitionDestinationProof, TransitionObservation,
            TransitionPlan, TransitionRowset,
        },
    },
    validation::private_active_capture_with_source,
};
use crate::projection::{
    capture::{CapturedChange, CapturedFile, CapturedTarget},
    shared::schema_v2,
};
use layrs_core::scm::{Chunk, ChunkingDescriptor, TreeEntryTarget, WorktreeId};
use serde_json::{json, Value};

pub(super) const TX: &str = "00000000000000000000000000000002";
const LEAF: &str = "00000000000000000000000000000003";
const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";

pub(super) struct Fixture {
    pub source: SourceFixture,
    pub activation: ProviderActivationRecord,
    pub marker: format::Marker,
    pub prefix: ProviderImportPrefixRecord,
    pub copying: ProviderTransitionIntentRecord,
    pub staging: ProviderTransitionIntentRecord,
    pub ready: ProviderTransitionIntentRecord,
    pub observation: ProjectionSnapshot,
    pub placement: ProjectionSnapshot,
    pub capture: CapturedPatch,
    pub overlay: Vec<ManagedReadyOverlay>,
    pub required: Vec<FileManifest>,
    pub receipt: ProviderTransitionReceiptRecord,
    pub claims: Vec<ManagedReadyClaim>,
    pub family_marker: Vec<u8>,
    pub source_root: (String, Vec<u8>),
    pub destination_root: (String, Vec<u8>),
    pub historical: FileManifest,
    pub current: FileManifest,
}
impl Fixture {
    pub fn new(c: &Connection) -> (Self, VerifiedImportClosure) {
        let (source, mount, activation, activated, family_marker) = super::source::active(c);
        let bytes = b"same captured bytes under two exact representations";
        let historical = FileManifest::from_chunks(
            ChunkingDescriptor::single(),
            &[Chunk::new(bytes.to_vec()).unwrap()],
        )
        .unwrap();
        let current = FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(1048576).unwrap(),
            &[Chunk::new(bytes.to_vec()).unwrap()],
        )
        .unwrap();
        assert_eq!(historical.file_id(), current.file_id());
        assert_ne!(historical.id(), current.id());
        let entry = &activated.snapshot.entries[0];
        let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
            panic!("fixture base is not file")
        };
        let change = CapturedChange {
            path: entry.path.clone(),
            expected: Some(CapturedTarget {
                file_id: file_id.clone(),
                executable: entry.executable,
            }),
            replacement: Some(CapturedFile {
                file_id: historical.file_id().clone(),
                executable: !entry.executable,
                bytes: historical.byte_length(),
            }),
        };
        let mut old_patch = CapturedPatch {
            schema_version: "layrs.projection-capture.v1".into(),
            sequence: 1,
            previous_patch_digest: None,
            projection_generation: 1,
            projection_digest: activated.digest.clone(),
            identity: source.allocation.format.identity.clone(),
            changes: vec![change.clone()],
            digest: String::new(),
        };
        old_patch.digest = old_patch.computed_digest().unwrap();
        let mut accepted = ProviderCaptureTransactionRecord::begin(
            &source.allocation,
            &mount,
            &activation,
            "source-history".into(),
            0,
            None,
        )
        .unwrap();
        accepted.state = ProviderCaptureState::Accepted {
            stage: ProviderCaptureStage {
                image_digest: RAW.into(),
                image_path_count: 1,
                changes: vec![change.clone()],
                reservation: "source-history-reservation".into(),
            },
            patch: old_patch.clone(),
        };
        accepted.validate_fields().unwrap();
        c.execute(
            "INSERT INTO projection_content VALUES(?1,?2,?3)",
            params![
                historical.file_id().as_str(),
                historical.id().as_str(),
                historical.byte_length()
            ],
        )
        .unwrap();
        c.execute(
            "INSERT INTO projection_captures VALUES(1,?1,1,?2)",
            params![old_patch.digest, canonical_json(&old_patch).unwrap()],
        )
        .unwrap();
        c.execute("INSERT INTO projection_provider_capture_transactions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![accepted.transaction_id,accepted.mount_id,accepted.capture_generation,accepted.capture_digest,accepted.expected_capture_sequence,accepted.expected_capture_digest,accepted.sql_state(),accepted.reservation(),accepted.encode().unwrap(),accepted.digest().unwrap()]).unwrap();
        let old_receipt = ProviderCaptureReceiptRecord::derive(&accepted).unwrap();
        c.execute(
            "INSERT INTO projection_provider_capture_receipts VALUES(?1,1,?2,?3,?4)",
            params![
                accepted.transaction_id,
                old_patch.digest,
                old_receipt.encode().unwrap(),
                old_receipt.digest().unwrap()
            ],
        )
        .unwrap();
        let source_root = protection(
            &source.allocation.format,
            "provider-capture:source-history",
            &[historical.clone()],
        );
        c.execute(
            "INSERT INTO projection_claims VALUES(?1,?1,'provider-capture','source-history')",
            [&source_root.0],
        )
        .unwrap();
        c.execute_batch("BEGIN DEFERRED").unwrap();
        let active = private_active_capture_with_source(
            c,
            &source.allocation,
            &source.prepared,
            &activation.binding,
            Some([0x41; 16]),
            None,
        )
        .unwrap();
        let mut marker = source.allocation.format.clone();
        marker.identity.worktree_id = WorktreeId::parse("worktree:ready-destination").unwrap();
        marker.checkout_root = format!("C:\\managed-parent\\{LEAF}");
        let shared = marker.shared.as_mut().unwrap();
        shared.metadata_path = shared
            .metadata_path
            .parent()
            .unwrap()
            .join("ready-destination");
        shared.database_identity = "1:0:123".into();
        let row = |count| ImportRowset {
            count,
            digest: TOKEN.into(),
        };
        let mut prefix = ProviderImportPrefixRecord {
            schema_version: "layrs.projection-provider-import-prefix.v1".into(),
            kind: ProviderImportKind::ManagedOrdinary,
            source: ImportSource::Provider {
                identity: source.allocation.format.identity.clone(),
                format_digest: crate::projection::provider::records::codec::hash(
                    "layrs-projection-provider-import-format-v1",
                    &source.allocation.format.bytes().unwrap(),
                ),
                private_schema: 8,
                allocation: source.allocation.data_reference().unwrap(),
                prepared: source.prepared.data_reference().unwrap(),
                binding: crate::projection::provider::records::ProviderBootstrapBinding::decode(
                    &activation.binding.encode().unwrap(),
                )
                .unwrap(),
                activation_map_id: activation.digest().unwrap(),
                version: ImportVersion {
                    generation: 1,
                    snapshot_digest: activated.digest,
                    selection_digest: activation.selection_digest.clone(),
                    capture_count: 1,
                    capture_head: Some(old_patch.digest.clone()),
                    effective_image_digest: RAW.into(),
                },
            },
            destination_identity: marker.identity.clone(),
            inherited_segments: ImportRowset {
                count: 0,
                digest: commitment(b"layrs-projection-provider-import-segments-v1\0", &[]),
            },
            ancestry_root_receipt: None,
            closure: ImportClosure {
                denial_observations: None,
                restore_history: None,
                snapshots: row(2),
                selections: row(1),
                captures: row(1),
                publications: row(0),
                catalogs: row(1),
                descriptors: row(1),
                activation_maps: row(1),
                transition_receipts: row(0),
                companions: row(0),
                companion_files: row(0),
                companion_deletions: row(0),
                content_mappings: row(1),
            },
        };
        let references = || ImportSourceReferences::Provider {
            allocation: &source.allocation,
            prepared: &source.prepared,
            activation: &activation,
        };
        let metadata = import_validation::read_source_metadata(
            c,
            &source.allocation.format,
            ImportDatabaseKind::Provider,
            &prefix,
            references(),
            RAW,
            None,
        )
        .unwrap();
        prefix.closure = import_validation::compute_scoped_closure(
            c,
            ImportDatabaseKind::Provider,
            &prefix,
            &metadata,
            None,
        )
        .unwrap();
        let closure = import_validation::verify_source(
            c,
            &source.allocation.format,
            ImportDatabaseKind::Provider,
            &prefix,
            references(),
            Some(&active),
            RAW,
            None,
        )
        .unwrap();
        marker.import_digest = Some(prefix.digest().unwrap());
        let observation = source.source.initial().clone();
        let placement = activated.snapshot;
        let version = match &prefix.source {
            ImportSource::Provider { version, .. } => version,
            _ => unreachable!(),
        };
        let observation_digest = database::digest(
            2,
            Some(&version.snapshot_digest),
            &observation.canonical_bytes().unwrap(),
        );
        let placement_digest = database::digest(
            3,
            Some(&observation_digest),
            &placement.canonical_bytes().unwrap(),
        );
        let mut capture = CapturedPatch {
            schema_version: "layrs.projection-capture.v1".into(),
            sequence: 2,
            previous_patch_digest: Some(old_patch.digest),
            projection_generation: 3,
            projection_digest: placement_digest.clone(),
            identity: marker.identity.clone(),
            changes: vec![change],
            digest: String::new(),
        };
        capture.digest = capture.computed_digest().unwrap();
        let mut required = vec![
            source.source.descriptors()[0].manifest.clone(),
            historical.clone(),
            current.clone(),
        ];
        required.sort_by(|a, b| a.file_id().cmp(b.file_id()).then(a.id().cmp(b.id())));
        let protected_rows: Vec<_> = required
            .iter()
            .map(|m| json!({"fileId":m.file_id(),"manifestId":m.id(),"bytes":m.byte_length()}))
            .collect();
        let overlay_rows = vec![json!({"change":&capture.changes[0],"manifestId":current.id()})];
        let empty = TransitionRowset {
            count: 0,
            digest: commitment(b"layrs-provider-managed-companion-empty-v1\0", &[]),
        };
        let plan = TransitionPlan {
            denial_coverage: None,
            kind: ProviderImportKind::ManagedOrdinary,
            source_prefix_id: prefix.digest().unwrap(),
            source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
            destination_parent: "C:\\managed-parent".into(),
            destination_parent_identity: "fs:v1:1111111111111111:2222222222222222".into(),
            destination_leaf_nonce: LEAF.into(),
            observation: TransitionObservation {
                step_id: observation.base_step_id.clone(),
                root_tree_id: observation.base_root_tree_id.clone(),
                policy_set_id: observation.observed_policy_set_id.clone(),
                authorization_epoch: observation.observed_authorization_epoch,
                generation: 2,
                snapshot_digest: observation_digest,
                selection_digest: source.source.commitment().selection_digest.clone(),
            },
            placement_generation: 3,
            placement_digest,
            capture_sequence: 2,
            capture_digest: capture.digest.clone(),
            working_overlay: TransitionRowset {
                count: 1,
                digest: commitment(b"layrs-provider-managed-overlay-v1\0", &overlay_rows),
            },
            companion_id: None,
            companion_files: empty.clone(),
            companion_deletions: empty,
            protected_manifests: TransitionRowset {
                count: required.len() as u64,
                digest: commitment(
                    b"layrs-provider-managed-protected-manifests-v1\0",
                    &protected_rows,
                ),
            },
            quota_bytes: 1048576,
        };
        let destination = TransitionDestinationProof {
            backend: TransitionBackendKind::Ordinary,
            format: marker.clone(),
            format_file_identity: "1:0:124".into(),
            private_database_identity: "1:0:123".into(),
            family_marker_digest: source.allocation.family.marker_digest.clone(),
            native_control_identity_hex: None,
            native_root_identity: None,
        };
        let copying = ProviderTransitionIntentRecord {
            schema_version: "layrs.projection-provider-transition-intent.v1".into(),
            transaction_id: TX.into(),
            role: TransitionRole::Destination,
            state: TransitionState::Copying,
            local_generation: 0,
            plan,
            destination: Some(destination),
            reservation: None,
            receipt_id: None,
        };
        copying.validate_fields().unwrap();
        let mut staging =
            ProviderTransitionIntentRecord::decode(&copying.encode().unwrap()).unwrap();
        staging.state = TransitionState::Staging;
        staging.reservation = Some(staging.reservation_operation().unwrap());
        staging.require_transition(&copying).unwrap();
        let plan = &staging.plan;
        let proof = staging.destination.as_ref().unwrap();
        let receipt = ProviderTransitionReceiptRecord {
            denial_coverage: None,
            schema_version: "layrs.projection-provider-transition-receipt.v1".into(),
            owner_identity_digest: proof.owner_digest(plan).unwrap(),
            transaction_id: TX.into(),
            transition_plan_digest: plan.digest().unwrap(),
            destination_proof_digest: proof.digest(plan).unwrap(),
            source_prefix_id: prefix.digest().unwrap(),
            source_prefix: ProviderImportPrefixRecord::decode(&prefix.encode().unwrap()).unwrap(),
            parent_receipt_id: None,
            source_identity: source.allocation.format.identity.clone(),
            destination_identity: marker.identity.clone(),
            imported_max_generation: 1,
            destination_observation_generation: 2,
            destination_observation_digest: plan.observation.snapshot_digest.clone(),
            destination_placement_generation: 3,
            destination_placement_digest: plan.placement_digest.clone(),
            imported_capture_count: 1,
            destination_capture_sequence: 2,
            destination_capture_digest: capture.digest.clone(),
            protected_manifests: ImportRowset {
                count: plan.protected_manifests.count,
                digest: plan.protected_manifests.digest.clone(),
            },
            content_mappings: prefix.closure.content_mappings.clone(),
            companion_id: None,
            companion_files: ImportRowset {
                count: 0,
                digest: plan.companion_files.digest.clone(),
            },
            companion_deletions: ImportRowset {
                count: 0,
                digest: plan.companion_deletions.digest.clone(),
            },
        };
        let mut ready = ProviderTransitionIntentRecord::decode(&staging.encode().unwrap()).unwrap();
        ready.state = TransitionState::Ready;
        ready.receipt_id = Some(receipt.digest().unwrap());
        ready.require_receipt(&receipt).unwrap();
        let destination_root = protection(&marker, &format!("provider-import:{TX}"), &required);
        let claims = vec![ManagedReadyClaim {
            root_id: destination_root.0.clone(),
            digest: destination_root.0.clone(),
            kind: "provider-import".into(),
            owner: TX.into(),
        }];
        let overlay = vec![ManagedReadyOverlay {
            path: capture.changes[0].path.clone(),
            manifest_id: Some(current.id().clone()),
        }];
        (
            Self {
                source,
                activation,
                marker,
                prefix,
                copying,
                staging,
                ready,
                observation,
                placement,
                capture,
                overlay,
                required,
                receipt,
                claims,
                family_marker,
                source_root,
                destination_root,
                historical,
                current,
            },
            closure,
        )
    }

    pub fn data(&self) -> ManagedReadyData<'_> {
        ManagedReadyData {
            observation: &self.observation,
            placement: &self.placement,
            capture: &self.capture,
            overlay: &self.overlay,
            required: &self.required,
            receipt: &self.receipt,
            ready: &self.ready,
            claims: &self.claims,
        }
    }
    pub fn inputs<'a>(
        &'a self,
        closure: &'a VerifiedImportClosure,
        data: &'a ManagedReadyData<'a>,
    ) -> ManagedReadyInputs<'a> {
        ManagedReadyInputs {
            marker: &self.marker,
            source: &self.source.source,
            allocation: &self.source.allocation,
            prepared: &self.source.prepared,
            activation: &self.activation,
            prefix: &self.prefix,
            staging: &self.staging,
            closure,
            family_marker_bytes: &self.family_marker,
            data,
            companion: None,
            denial: None,
        }
    }

    pub fn stage(&self, source: &Connection, c: &Connection, closure: &VerifiedImportClosure) {
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
            .unwrap();
        destination_sql::initialize_empty(
            c,
            &self.marker,
            &self.source.source,
            &self.source.allocation,
            &self.source.prepared,
            &self.activation,
            &self.prefix,
            &self.copying,
            None,
        )
        .unwrap();
        destination_sql::promote_or_replay_copied(
            source,
            c,
            &self.marker,
            &self.source.source,
            &self.source.allocation,
            &self.source.prepared,
            &self.activation,
            &self.prefix,
            &self.copying,
            closure,
            None,
        )
        .unwrap();
        destination_sql::stage_copied(
            source,
            c,
            &self.marker,
            &self.source.source,
            &self.source.allocation,
            &self.source.prepared,
            &self.activation,
            &self.prefix,
            &self.copying,
            &self.staging,
            closure,
            None,
        )
        .unwrap();
        c.execute_batch("COMMIT").unwrap();
    }
    pub fn populate_family(&self, c: &Connection) {
        schema_v2::initialize_test_reference(c).unwrap();
        c.execute("INSERT INTO family VALUES(1,?1)", [&self.family_marker])
            .unwrap();
        c.execute("INSERT INTO quota VALUES(1,10485760)", [])
            .unwrap();
        for (marker, state) in [
            (&self.source.allocation.format, "ready"),
            (&self.marker, "initializing"),
        ] {
            let binding = marker.shared.as_ref().unwrap();
            c.execute(
                "INSERT INTO worktrees VALUES(?1,?2,?3,?4,?5)",
                params![
                    marker.identity.worktree_id.as_str(),
                    binding.metadata_path.to_str().unwrap(),
                    binding.database_identity,
                    marker.bytes().unwrap(),
                    state
                ],
            )
            .unwrap();
        }
        for m in &self.required {
            c.execute(
                "INSERT INTO files VALUES(?1,?2) ON CONFLICT DO NOTHING",
                params![m.file_id().as_str(), m.byte_length()],
            )
            .unwrap();
            c.execute(
                "INSERT INTO manifests VALUES(?1,?2,?3,?4)",
                params![
                    m.id().as_str(),
                    m.file_id().as_str(),
                    m.byte_length(),
                    canonical_json(m).unwrap()
                ],
            )
            .unwrap();
            for (ordinal, chunk) in m.chunks().iter().enumerate() {
                c.execute(
                    "INSERT INTO chunks VALUES(?1,?2) ON CONFLICT DO NOTHING",
                    params![chunk.chunk_id().as_str(), chunk.byte_length()],
                )
                .unwrap();
                c.execute(
                    "INSERT INTO manifest_chunks VALUES(?1,?2,?3)",
                    params![m.id().as_str(), ordinal, chunk.chunk_id().as_str()],
                )
                .unwrap();
            }
        }
        for (marker, operation, (id, payload), manifests) in [
            (
                &self.source.allocation.format,
                "provider-capture:source-history".to_owned(),
                &self.source_root,
                vec![&self.historical],
            ),
            (
                &self.marker,
                format!("provider-import:{TX}"),
                &self.destination_root,
                self.required.iter().collect(),
            ),
        ] {
            c.execute(
                "INSERT INTO roots VALUES(?1,?2,?3,?4,?1,?5)",
                params![
                    id,
                    marker.identity.worktree_id.as_str(),
                    marker.shared.as_ref().unwrap().database_identity,
                    operation,
                    payload
                ],
            )
            .unwrap();
            for m in manifests {
                c.execute(
                    "INSERT INTO root_manifests VALUES(?1,?2)",
                    params![id, m.id().as_str()],
                )
                .unwrap();
            }
        }
        assert!(!c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
                [],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
        c.execute_batch("BEGIN DEFERRED").unwrap();
    }
}

fn commitment(domain: &[u8], values: &[Value]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(&(values.len() as u64).to_be_bytes());
    for row in values {
        let bytes = canonical_json(row).unwrap();
        h.update(&(bytes.len() as u64).to_be_bytes());
        h.update(&bytes);
    }
    format!("blake3:{}", h.finalize())
}
fn protection(
    marker: &format::Marker,
    operation: &str,
    manifests: &[FileManifest],
) -> (String, Vec<u8>) {
    let binding = marker.shared.as_ref().unwrap();
    let mut manifests: Vec<_> = manifests.iter().collect();
    manifests.sort_by(|a, b| a.id().cmp(b.id()));
    let value = json!({"incarnation":binding.incarnation,"owner":marker.identity.worktree_id,"databaseIdentity":binding.database_identity,"operation":operation,"manifests":manifests});
    let bytes = canonical_json(&value).unwrap();
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-partial-retention-v1\0");
    h.update(&bytes);
    (h.finalize().to_hex().to_string(), bytes)
}
