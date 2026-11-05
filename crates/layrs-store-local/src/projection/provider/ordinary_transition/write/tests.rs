//! Pure canonical DATA and capacity arithmetic. This fixture authenticates no
//! SQL admission, native identity, source image or physical destination.
use super::*;
use crate::projection::provider::{
    records::{
        import::{AdmittedSourceBinding, ImportClosure, ImportRowset, ImportVersion},
        transition::{TransitionObservation, TransitionPlan, TransitionRowset},
    },
    validation::tests::fixture::Fixture,
};
use layrs_core::scm::WorktreeId;

const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn rowset(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: TOKEN.into(),
    }
}

fn planned() -> ProviderTransitionIntentRecord {
    let pure = Fixture::new("G1", false);
    let identity = pure.allocation.format.identity.clone();
    let mut destination = identity.clone();
    destination.worktree_id = WorktreeId::parse("worktree:cancellation-capacity").unwrap();
    let prefix = ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v2".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            identity,
            format_digest: TOKEN.into(),
            private_schema: 9,
            version: ImportVersion {
                generation: 3,
                snapshot_digest: RAW.into(),
                selection_digest: RAW.into(),
                capture_count: 1,
                capture_head: Some(TOKEN.into()),
                effective_image_digest: RAW.into(),
            },
            checkout_identity: Some(pure.allocation.format.checkout_identity().into()),
            admission: Some(AdmittedSourceBinding {
                admission_digest: TOKEN.into(),
                incoming_prefix_id: TOKEN.into(),
                incoming_receipt_id: TOKEN.into(),
                database_identity: "1:0:11".into(),
            }),
        },
        destination_identity: destination,
        inherited_segments: rowset(1),
        ancestry_root_receipt: Some(TOKEN.into()),
        closure: ImportClosure {
            denial_observations: None,
            snapshots: rowset(4),
            selections: rowset(1),
            captures: rowset(1),
            publications: rowset(0),
            catalogs: rowset(0),
            descriptors: rowset(0),
            activation_maps: rowset(0),
            transition_receipts: rowset(1),
            companions: rowset(0),
            companion_files: rowset(0),
            companion_deletions: rowset(0),
            content_mappings: rowset(0),
            restore_history: Some(rowset(0)),
        },
    };
    let empty = || TransitionRowset {
        count: 0,
        digest: TOKEN.into(),
    };
    let observation = pure.source.initial();
    let record = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v1".into(),
        transaction_id: "00000000000000000000000000000002".into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: 3,
        plan: TransitionPlan {
            denial_coverage: None,
            kind: ProviderImportKind::ManagedOrdinary,
            source_prefix_id: prefix.digest().unwrap(),
            source_prefix: prefix,
            destination_parent: "C:\\ordinary-capacity-parent".into(),
            destination_parent_identity: "fs:v1:1111111111111111:2222222222222222".into(),
            destination_leaf_nonce: "00000000000000000000000000000003".into(),
            observation: TransitionObservation {
                step_id: observation.base_step_id.clone(),
                root_tree_id: observation.base_root_tree_id.clone(),
                policy_set_id: observation.observed_policy_set_id.clone(),
                authorization_epoch: observation.observed_authorization_epoch,
                generation: 4,
                snapshot_digest: RAW.into(),
                selection_digest: RAW.into(),
            },
            placement_generation: 5,
            placement_digest: RAW.into(),
            capture_sequence: 2,
            capture_digest: TOKEN.into(),
            working_overlay: empty(),
            companion_id: None,
            companion_files: empty(),
            companion_deletions: empty(),
            protected_manifests: empty(),
            quota_bytes: 0,
        },
        destination: None,
        reservation: None,
        receipt_id: None,
    };
    require_shape(&record).unwrap();
    assert_eq!(
        ProviderTransitionIntentRecord::decode(&record.encode().unwrap()).unwrap(),
        record
    );
    record
}

#[test]
fn planned_only_capacity_refuses_before_an_uncancellable_insert() {
    let planned = planned();
    let payload = planned.encode().unwrap();
    let digest = planned.digest().unwrap();
    let terminal = cancelled(&planned).unwrap();
    let terminal_payload = terminal.encode().unwrap();
    let terminal_digest = terminal.digest().unwrap();
    let planned_bytes = normalized_bytes(&planned, "planned", &payload, &digest);
    let terminal_bytes =
        normalized_bytes(&terminal, "cancelled", &terminal_payload, &terminal_digest);
    assert_eq!(terminal_payload.len(), payload.len() + 2);
    assert_eq!(terminal_bytes, planned_bytes + 4);
    let used = MAX_TRANSITION_BYTES - planned_bytes;
    assert_eq!(used + planned_bytes, MAX_TRANSITION_BYTES);
    assert!(require_insert_capacity(1, used, &planned, &payload, &digest).is_err());
}

#[test]
fn exact_cancelled_capacity_accepts_both_canonical_states() {
    let planned = planned();
    let payload = planned.encode().unwrap();
    let digest = planned.digest().unwrap();
    let terminal = cancelled(&planned).unwrap();
    terminal.require_transition(&planned).unwrap();
    let terminal_payload = terminal.encode().unwrap();
    let terminal_digest = terminal.digest().unwrap();
    let terminal_bytes =
        normalized_bytes(&terminal, "cancelled", &terminal_payload, &terminal_digest);
    let used = MAX_TRANSITION_BYTES - terminal_bytes;
    require_insert_capacity(1, used, &planned, &payload, &digest).unwrap();
    assert_eq!(used + terminal_bytes, MAX_TRANSITION_BYTES);
    assert!(require_insert_capacity(1, used + 1, &planned, &payload, &digest).is_err());
}

#[test]
fn planned_insert_reserves_one_count_slot_without_relaxing_the_reader_limit() {
    let planned = planned();
    let payload = planned.encode().unwrap();
    let digest = planned.digest().unwrap();
    require_insert_capacity(MAX_TRANSITIONS - 1, 0, &planned, &payload, &digest).unwrap();
    assert!(require_insert_capacity(MAX_TRANSITIONS, 0, &planned, &payload, &digest).is_err());
}
