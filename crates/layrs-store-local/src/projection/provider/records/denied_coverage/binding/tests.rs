use super::*;
use crate::projection::provider::records::transition::{
    TransitionObservation, TransitionPlan, TransitionRole, TransitionRowset, TransitionState,
};

const TX: &str = "00000000000000000000000000000001";
const TOKEN: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";

fn fixture() -> (
    TransitionDeniedCoverageRecord,
    TransitionObservationData,
    ProviderTransitionIntentRecord,
) {
    let (prefix, observation) = super::super::test_data::observation();
    let body =
        TransitionDeniedCoverageRecord::from_observation(&prefix, TX, &observation, None).unwrap();
    let (policy, epoch) = match &body.authority {
        DeniedAuthorityData::PreservedEpoch {
            policy_set_id,
            authorization_epoch,
        }
        | DeniedAuthorityData::ReobservedExpandedScope {
            policy_set_id,
            authorization_epoch,
        } => (policy_set_id.clone(), *authorization_epoch),
    };
    let empty = || TransitionRowset {
        count: 0,
        digest: TOKEN.into(),
    };
    let intent = ProviderTransitionIntentRecord {
        schema_version: "layrs.projection-provider-transition-intent.v2".into(),
        transaction_id: TX.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: body.source_version.generation,
        plan: TransitionPlan {
            kind: prefix.kind,
            source_prefix_id: prefix.digest().unwrap(),
            source_prefix: prefix,
            destination_parent: "C:\\coverage-parent".into(),
            destination_parent_identity: "fs:v1:1111111111111111:2222222222222222".into(),
            destination_leaf_nonce: "00000000000000000000000000000002".into(),
            observation: TransitionObservation {
                step_id: body.step_id.clone(),
                root_tree_id: body.root_tree_id.clone(),
                policy_set_id: policy,
                authorization_epoch: epoch,
                generation: body.observation_generation,
                snapshot_digest: body.observation_snapshot_digest.clone(),
                selection_digest: body.observation_selection_digest.clone(),
            },
            placement_generation: body.observation_generation + 1,
            placement_digest: "a".repeat(64),
            capture_sequence: 1,
            capture_digest: TOKEN.into(),
            working_overlay: empty(),
            companion_id: None,
            companion_files: empty(),
            companion_deletions: empty(),
            protected_manifests: empty(),
            quota_bytes: 0,
            denial_coverage: Some(body.reference().unwrap()),
        },
        destination: None,
        reservation: None,
        receipt_id: None,
    };
    intent.validate_fields().unwrap();
    (body, observation, intent)
}

#[test]
fn coverage_binds_actual_intent_even_when_changed_body_is_rehashed() {
    let (body, _, intent) = fixture();
    body.require_intent(&intent, None).unwrap();
    for field in [
        "transactionId",
        "sourcePrefixId",
        "sourceIdentity",
        "destinationIdentity",
        "sourceVersion",
        "stepId",
        "rootTreeId",
        "observationGeneration",
        "observationSnapshotDigest",
        "observationSelectionDigest",
    ] {
        let mut changed = serde_json::to_value(&body).unwrap();
        match field {
            "transactionId" => changed[field] = "00000000000000000000000000000003".into(),
            "sourceIdentity" | "destinationIdentity" => {
                changed[field]["worktreeId"] = "worktree:foreign".into()
            }
            "sourceVersion" => changed[field]["effectiveImageDigest"] = "b".repeat(64).into(),
            "observationGeneration" => changed[field] = (body.observation_generation + 1).into(),
            "observationSnapshotDigest" | "observationSelectionDigest" => {
                changed[field] = "b".repeat(64).into()
            }
            _ => {
                let candidate = format!("blake3:{}", "2".repeat(64));
                changed[field] = if changed[field].as_str() == Some(candidate.as_str()) {
                    format!("blake3:{}", "3".repeat(64)).into()
                } else {
                    candidate.into()
                };
            }
        }
        assert_ne!(
            changed,
            serde_json::to_value(&body).unwrap(),
            "unchanged {field}"
        );
        let changed: TransitionDeniedCoverageRecord = serde_json::from_value(changed).unwrap();
        changed.validate_fields().unwrap();
        let mut linked = ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
        linked.plan.denial_coverage = Some(changed.reference().unwrap());
        linked.validate_fields().unwrap();
        assert!(
            changed.require_intent(&linked, None).is_err(),
            "accepted changed {field}"
        );
    }
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(body.require_intent(&intent, Some(&cancel)).is_err());
}

#[test]
fn coverage_reconstruction_rejects_relabelled_path_semantics() {
    let (body, observation, intent) = fixture();
    body.require_observation(&intent.plan.source_prefix, TX, &observation, None)
        .unwrap();
    let mut changed = TransitionDeniedCoverageRecord::decode(&body.encode().unwrap()).unwrap();
    changed.path_semantics = match body.path_semantics {
        PathSemantics::CaseSensitive => PathSemantics::PortableWindows,
        PathSemantics::PortableWindows => PathSemantics::CaseSensitive,
    };
    changed.validate_fields().unwrap();
    assert!(
        changed
            .require_observation(&intent.plan.source_prefix, TX, &observation, None)
            .is_err()
    );
}

#[test]
fn coverage_binds_actual_policy_and_epoch_even_with_rehashed_reference() {
    let (body, _, intent) = fixture();
    for policy in [true, false] {
        let mut changed = TransitionDeniedCoverageRecord::decode(&body.encode().unwrap()).unwrap();
        let (policy_set_id, authorization_epoch) = match &mut changed.authority {
            DeniedAuthorityData::PreservedEpoch {
                policy_set_id,
                authorization_epoch,
            }
            | DeniedAuthorityData::ReobservedExpandedScope {
                policy_set_id,
                authorization_epoch,
            } => (policy_set_id, authorization_epoch),
        };
        if policy {
            let candidate = ObjectId::parse(TOKEN).unwrap();
            *policy_set_id = if *policy_set_id == candidate {
                ObjectId::parse(&format!("blake3:{}", "2".repeat(64))).unwrap()
            } else {
                candidate
            };
        } else {
            *authorization_epoch += 1;
        }
        changed.authority_data_digest = changed.authority.digest().unwrap();
        changed.validate_fields().unwrap();
        let mut linked = ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
        linked.plan.denial_coverage = Some(changed.reference().unwrap());
        linked.validate_fields().unwrap();
        assert!(changed.require_intent(&linked, None).is_err());
    }
}

#[test]
fn coverage_stored_rows_bind_commitments_and_actual_namespace() {
    let (body, observation, _) = fixture();
    let check =
        |snapshot: &ProjectionSnapshot, generation, digest: &str, selection_digest: &str| {
            body.require_observation_rows(
                snapshot,
                observation.selection(),
                generation,
                digest,
                selection_digest,
                None,
            )
        };
    check(
        observation.snapshot(),
        body.observation_generation,
        &body.observation_snapshot_digest,
        &body.observation_selection_digest,
    )
    .unwrap();
    assert!(
        check(
            observation.snapshot(),
            body.observation_generation + 1,
            &body.observation_snapshot_digest,
            &body.observation_selection_digest
        )
        .is_err()
    );
    assert!(
        check(
            observation.snapshot(),
            body.observation_generation,
            &"b".repeat(64),
            &body.observation_selection_digest
        )
        .is_err()
    );
    assert!(
        check(
            observation.snapshot(),
            body.observation_generation,
            &body.observation_snapshot_digest,
            &"b".repeat(64)
        )
        .is_err()
    );
    for field in ["epoch", "semantics", "overlap", "alias", "deniedAncestor"] {
        let mut snapshot = observation.snapshot().clone();
        match field {
            "epoch" => snapshot.observed_authorization_epoch += 1,
            "semantics" => {
                snapshot.path_semantics = match snapshot.path_semantics {
                    PathSemantics::CaseSensitive => PathSemantics::PortableWindows,
                    PathSemantics::PortableWindows => PathSemantics::CaseSensitive,
                }
            }
            _ => {
                let mut entry = snapshot.entries[0].clone();
                entry.path = RepoPath::parse(match field {
                    "overlap" => "created.txt",
                    "alias" => "CREATED.txt",
                    _ => "created.txt/child",
                })
                .unwrap();
                snapshot.entries.push(entry);
                snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
            }
        }
        // Commitments are the already-authenticated caller's inputs. Namespace
        // consistency must also hold, even if those inputs are self-consistent.
        let mut linked = TransitionDeniedCoverageRecord::decode(&body.encode().unwrap()).unwrap();
        if field == "alias" {
            snapshot.path_semantics = PathSemantics::PortableWindows;
            linked.path_semantics = PathSemantics::PortableWindows;
        }
        linked.observation_snapshot_digest = crate::projection::database::digest(
            body.observation_generation,
            Some(&body.source_version.snapshot_digest),
            &codec::encode(&snapshot, codec::SNAPSHOT_MAX).unwrap(),
        );
        linked.observation_selection_digest = crate::projection::database::digest(
            body.observation_generation,
            Some(&linked.observation_snapshot_digest),
            &crate::projection::selection::encode(observation.selection()).unwrap(),
        );
        assert!(
            linked
                .require_observation_rows(
                    &snapshot,
                    observation.selection(),
                    body.observation_generation,
                    &linked.observation_snapshot_digest,
                    &linked.observation_selection_digest,
                    None
                )
                .is_err(),
            "accepted {field}"
        );
    }
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(
        body.require_observation_rows(
            observation.snapshot(),
            observation.selection(),
            body.observation_generation,
            &body.observation_snapshot_digest,
            &body.observation_selection_digest,
            Some(&cancelled)
        )
        .is_err()
    );
}

#[test]
fn coverage_cost_uses_complete_canonical_row_not_path_bytes_or_compact_reference() {
    use crate::projection::provider::managed_budget::DeniedCoverageCost;
    let (body, _, intent) = fixture();
    let cost = DeniedCoverageCost::new(&intent, &body, None).unwrap();
    assert_eq!(
        cost.document_bytes(),
        body.encode().unwrap().len() as u64 + 71
    );
    assert!(cost.document_bytes() > body.path_bytes + 71);
    let mut changed = ProviderTransitionIntentRecord::decode(&intent.encode().unwrap()).unwrap();
    changed.transaction_id = "00000000000000000000000000000003".into();
    assert!(DeniedCoverageCost::new(&changed, &body, None).is_err());
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(DeniedCoverageCost::new(&intent, &body, Some(&cancel)).is_err());
}
