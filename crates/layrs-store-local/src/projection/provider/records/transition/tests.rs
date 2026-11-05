use super::*;
use crate::projection::provider::records::import::{ImportClosure, ImportSource, ImportVersion};
use layrs_core::scm::{SpaceId, StoreId, WorktreeId};
use serde_json::json;

const ID: &str = "blake3:1111111111111111111111111111111111111111111111111111111111111111";
const RAW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LEAF: &str = "00000000000000000000000000000001";
const TX: &str = "00000000000000000000000000000002";
const DIR: &str = "fs:v1:1111111111111111:2222222222222222";

fn identity(name: &str) -> ProjectionIdentity {
    ProjectionIdentity {
        store_id: StoreId::parse("store:fixture").unwrap(),
        worktree_id: WorktreeId::parse(format!("worktree:{name}")).unwrap(),
        space_id: SpaceId::parse("space:fixture").unwrap(),
        origin: "https://example.invalid".into(),
        principal_id: "principal:fixture".into(),
    }
}
fn imported(count: u64) -> ImportRowset {
    ImportRowset {
        count,
        digest: ID.into(),
    }
}
fn rowset(count: u64) -> TransitionRowset {
    TransitionRowset {
        count,
        digest: ID.into(),
    }
}
fn prefix() -> ProviderImportPrefixRecord {
    ProviderImportPrefixRecord {
        schema_version: "layrs.projection-provider-import-prefix.v1".into(),
        kind: ProviderImportKind::ManagedOrdinary,
        source: ImportSource::Ordinary {
            admission: None,
            checkout_identity: None,
            identity: identity("source"),
            format_digest: ID.into(),
            private_schema: 8,
            version: ImportVersion {
                generation: 2,
                snapshot_digest: RAW.into(),
                selection_digest: RAW.into(),
                capture_count: 0,
                capture_head: None,
                effective_image_digest: RAW.into(),
            },
        },
        destination_identity: identity("destination"),
        inherited_segments: imported(0),
        ancestry_root_receipt: None,
        closure: ImportClosure {
            denial_observations: None,
            restore_history: None,
            snapshots: imported(3),
            selections: imported(1),
            captures: imported(0),
            publications: imported(0),
            catalogs: imported(0),
            descriptors: imported(0),
            activation_maps: imported(0),
            transition_receipts: imported(0),
            companions: imported(0),
            companion_files: imported(0),
            companion_deletions: imported(0),
            content_mappings: imported(0),
        },
    }
}
fn intent() -> ProviderTransitionIntentRecord {
    let prefix = prefix();
    let prefix_id = prefix.digest().unwrap();
    ProviderTransitionIntentRecord {
        schema_version: SCHEMA.into(),
        transaction_id: TX.into(),
        role: TransitionRole::Source,
        state: TransitionState::Planned,
        local_generation: 2,
        plan: TransitionPlan {
            denial_coverage: None,
            kind: ProviderImportKind::ManagedOrdinary,
            source_prefix_id: prefix_id,
            source_prefix: prefix,
            destination_parent: "C:\\destination-parent".into(),
            destination_parent_identity: DIR.into(),
            destination_leaf_nonce: LEAF.into(),
            observation: TransitionObservation {
                step_id: StepId::parse(ID).unwrap(),
                root_tree_id: TreeId::parse(ID).unwrap(),
                policy_set_id: ObjectId::parse(ID).unwrap(),
                authorization_epoch: 1,
                generation: 3,
                snapshot_digest: RAW.into(),
                selection_digest: RAW.into(),
            },
            placement_generation: 4,
            placement_digest: RAW.into(),
            capture_sequence: 1,
            capture_digest: ID.into(),
            working_overlay: rowset(0),
            companion_id: None,
            companion_files: rowset(0),
            companion_deletions: rowset(0),
            protected_manifests: rowset(0),
            quota_bytes: 0,
        },
        destination: None,
        reservation: None,
        receipt_id: None,
    }
}
fn proof(record: &ProviderTransitionIntentRecord) -> TransitionDestinationProof {
    let root = format!(
        "{}\\{}",
        record.plan.destination_parent, record.plan.destination_leaf_nonce
    );
    let format = serde_json::from_value(json!({
        "schemaVersion":"layrs.projection-store-format.v1", "magic":"LAYRS_NATIVE_STORE",
        "formatVersion":4, "minReaderVersion":4,
        "identity":record.plan.source_prefix.destination_identity,
        "checkoutRoot":root, "checkoutIdentity":DIR, "storeIdentity":DIR,
        "metadataIdentity":DIR, "initialSnapshotDigest":RAW,
        "shared":{
            "root":"C:\\family", "incarnation":"fixture", "rootIdentity":DIR,
            "metadataPath":"C:\\family\\worktrees\\fixture\\metadata", "databaseIdentity":"1:2:3"
        },
        "importDigest":record.plan.source_prefix_id,
    }))
    .unwrap();
    TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format,
        format_file_identity: "1:2:4".into(),
        private_database_identity: "1:2:3".into(),
        family_marker_digest: ID.into(),
        native_control_identity_hex: None,
        native_root_identity: None,
    }
}
fn copy(record: &ProviderTransitionIntentRecord) -> ProviderTransitionIntentRecord {
    ProviderTransitionIntentRecord::decode(&record.encode().unwrap()).unwrap()
}

fn receipt(staging: &ProviderTransitionIntentRecord) -> ProviderTransitionReceiptRecord {
    let prefix =
        ProviderImportPrefixRecord::decode(&staging.plan.source_prefix.encode().unwrap()).unwrap();
    ProviderTransitionReceiptRecord {
        denial_coverage: None,
        schema_version: "layrs.projection-provider-transition-receipt.v1".into(),
        owner_identity_digest: staging
            .destination
            .as_ref()
            .unwrap()
            .owner_digest(&staging.plan)
            .unwrap(),
        transaction_id: TX.into(),
        source_prefix_id: staging.plan.source_prefix_id.clone(),
        transition_plan_digest: staging.plan.digest().unwrap(),
        destination_proof_digest: staging
            .destination
            .as_ref()
            .unwrap()
            .digest(&staging.plan)
            .unwrap(),
        source_prefix: prefix,
        parent_receipt_id: None,
        source_identity: identity("source"),
        destination_identity: identity("destination"),
        imported_max_generation: 2,
        destination_observation_generation: staging.plan.observation.generation,
        destination_observation_digest: staging.plan.observation.snapshot_digest.clone(),
        destination_placement_generation: staging.plan.placement_generation,
        destination_placement_digest: staging.plan.placement_digest.clone(),
        imported_capture_count: 0,
        destination_capture_sequence: staging.plan.capture_sequence,
        destination_capture_digest: staging.plan.capture_digest.clone(),
        protected_manifests: imported(0),
        content_mappings: imported(0),
        companion_id: None,
        companion_files: imported(0),
        companion_deletions: imported(0),
    }
}

#[test]
fn source_plan_is_canonical_bounded_and_projects_exact_sql_row() {
    let planned = intent();
    let bytes = planned.encode().unwrap();
    assert!(bytes.len() < ProviderTransitionIntentRecord::MAX_BYTES);
    assert_eq!(
        ProviderTransitionIntentRecord::decode(&bytes).unwrap(),
        planned
    );
    let digest = planned.digest().unwrap();
    planned.require_prefix(&planned.plan.source_prefix).unwrap();
    planned
        .require_row(TransitionRowData {
            id: TX,
            role: "source",
            state: "planned",
            local_generation: 2,
            reservation: None,
            receipt_id: None,
            payload: &bytes,
            digest: &digest,
        })
        .unwrap();
    assert!(
        planned
            .require_row(TransitionRowData {
                id: TX,
                role: "destination",
                state: "planned",
                local_generation: 2,
                reservation: None,
                receipt_id: None,
                payload: &bytes,
                digest: &digest,
            })
            .is_err()
    );
    let mut noncanonical = bytes;
    noncanonical.push(b'\n');
    assert!(ProviderTransitionIntentRecord::decode(&noncanonical).is_err());
}

#[test]
fn phase_progression_preserves_plan_and_real_destination_binding() {
    let planned = intent();
    let mut reserved = copy(&planned);
    reserved.reservation = Some(reserved.reservation_operation().unwrap());
    assert_ne!(
        reserved.reservation.as_deref(),
        Some("provider-transition:00000000000000000000000000000002")
    );
    reserved.require_transition(&planned).unwrap();
    let mut copying = copy(&reserved);
    copying.state = TransitionState::Copying;
    copying.destination = Some(proof(&copying));
    copying.require_transition(&reserved).unwrap();
    let mut staging = copy(&copying);
    staging.state = TransitionState::Staging;
    staging.require_transition(&copying).unwrap();
    let mut cancelled = copy(&staging);
    cancelled.state = TransitionState::Cancelled;
    cancelled.require_transition(&staging).unwrap();
    assert!(staging.require_transition(&cancelled).is_err());
    let mut changed = copy(&copying);
    changed.plan.quota_bytes = 1;
    assert!(changed.require_transition(&copying).is_err());
    let mut fake_ready = copy(&staging);
    fake_ready.state = TransitionState::Ready;
    assert!(fake_ready.encode().is_err());
}

#[test]
fn destination_format_file_identity_is_required_and_immutable() {
    let mut copying = intent();
    copying.state = TransitionState::Copying;
    copying.destination = Some(proof(&copying));
    let original = copying
        .destination
        .as_ref()
        .unwrap()
        .owner_digest(&copying.plan)
        .unwrap();
    let mut changed = copy(&copying);
    changed.destination.as_mut().unwrap().format_file_identity = "1:2:5".into();
    assert_ne!(
        changed
            .destination
            .as_ref()
            .unwrap()
            .owner_digest(&changed.plan)
            .unwrap(),
        original
    );
    assert!(changed.require_transition(&copying).is_err());
    changed.destination.as_mut().unwrap().format_file_identity = "1:2:3".into();
    assert!(
        changed.encode().is_err(),
        "FORMAT must not alias the SQL database"
    );
    let mut encoded = serde_json::to_value(&copying).unwrap();
    encoded["destination"]
        .as_object_mut()
        .unwrap()
        .remove("formatFileIdentity");
    assert!(
        ProviderTransitionIntentRecord::decode(&layrs_core::scm::canonical_json(&encoded).unwrap())
            .is_err()
    );
}

#[test]
fn expanded_destination_format_identity_matches_control_identity() {
    let mut record = intent();
    record.plan.kind = ProviderImportKind::ExpandedProvider;
    record.plan.source_prefix.kind = ProviderImportKind::ExpandedProvider;
    record.plan.source_prefix_id = record.plan.source_prefix.digest().unwrap();
    record.state = TransitionState::Copying;
    let mut destination = proof(&record);
    destination.backend = TransitionBackendKind::Provider;
    destination.format_file_identity = "305419896:10:11".into();
    destination.native_root_identity = Some("fixture-native-root".into());
    // Canonical comparison DATA, not a native allocation/registration claim.
    let stage = [
        b"LYRCSTG1\x78\x56\x34\x12".as_slice(),
        b"\x02\0\0\0\x01\0\0\0\x03\0\0\0\0\0\0\0",
        b"\x04\0\0\0\0\0\0\0\x0b\0\0\0\x0a\0\0\0",
        b"\x04\0\0\0C:\\r\x04\0\0\0C:\\p\x06\0\0\0C:\\p\\s",
    ]
    .concat();
    let control = [
        b"LYRCDIR1\x46\0\0\0".as_slice(),
        stage.as_slice(),
        b"\x07\0\0\0",
        b"blake3:0000000000000000000000000000000000000000000000000000000000000000",
    ]
    .concat();
    let decoded = layrs_vfs::ControlDirectoryIdentity::decode(&control).unwrap();
    assert_eq!(
        decoded.stage_identity().format_binding_identity(),
        destination.format_file_identity
    );
    destination.native_control_identity_hex =
        Some(control.iter().map(|byte| format!("{byte:02x}")).collect());
    record.destination = Some(destination);
    record.encode().unwrap();
    record.destination.as_mut().unwrap().format_file_identity = "305419896:10:12".into();
    assert!(record.encode().is_err());
}

#[test]
fn ready_is_bound_to_exact_prefix_receipt_and_observation() {
    let mut staging = intent();
    staging.state = TransitionState::Staging;
    staging.reservation = Some(staging.reservation_operation().unwrap());
    staging.destination = Some(proof(&staging));
    let receipt = receipt(&staging);
    let mut ready = copy(&staging);
    ready.state = TransitionState::Ready;
    ready.receipt_id = Some(receipt.digest().unwrap());
    ready.require_transition(&staging).unwrap();
    ready.require_receipt(&receipt).unwrap();
    let mut forged = copy(&ready);
    forged.plan.observation.snapshot_digest =
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    assert!(forged.require_receipt(&receipt).is_err());
    let mut wrong = ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap();
    wrong.destination_capture_digest = ID.replace('1', "2");
    assert!(ready.require_receipt(&wrong).is_err());
    let mut forged_plan = copy(&ready);
    forged_plan.plan.quota_bytes = 1;
    assert!(forged_plan.require_receipt(&receipt).is_err());
    let mut forged_overlay = copy(&ready);
    forged_overlay.plan.working_overlay.digest =
        "blake3:2222222222222222222222222222222222222222222222222222222222222222".into();
    assert!(forged_overlay.require_receipt(&receipt).is_err());
    let mut forged_selection = copy(&ready);
    forged_selection.plan.observation.selection_digest =
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
    assert!(forged_selection.require_receipt(&receipt).is_err());
    let mut forged_destination = copy(&ready);
    forged_destination
        .destination
        .as_mut()
        .unwrap()
        .family_marker_digest =
        "blake3:2222222222222222222222222222222222222222222222222222222222222222".into();
    assert!(forged_destination.require_receipt(&receipt).is_err());
}

#[test]
fn bounds_and_roles_reject_impossible_destination_history() {
    let mut planned = intent();
    planned.role = TransitionRole::Destination;
    planned.local_generation = 0;
    assert!(planned.encode().is_err());
    planned.state = TransitionState::Copying;
    planned.destination = Some(proof(&planned));
    let bytes = planned.encode().unwrap();
    planned
        .require_row(TransitionRowData {
            id: TX,
            role: "destination",
            state: "copying",
            local_generation: 0,
            reservation: None,
            receipt_id: None,
            payload: &bytes,
            digest: &planned.digest().unwrap(),
        })
        .unwrap();
    let mut overflow = intent();
    overflow.plan.quota_bytes = i64::MAX as u64 + 1;
    assert!(overflow.encode().is_err());
    let mut wrong = intent();
    wrong.plan.observation.generation = wrong.local_generation;
    assert!(wrong.encode().is_err());
    let mut huge = intent();
    huge.plan.working_overlay.count = MAX_ROWS + 1;
    assert!(huge.encode().is_err());
}

fn denial_reference() -> DeniedCoverageRef {
    DeniedCoverageRef {
        id: ID.into(),
        count: 1,
        path_bytes: 10,
        rows_digest: ID.into(),
    }
}

#[test]
fn legacy_intent_and_plan_keep_exact_closed_bytes_and_domains() {
    let value = intent();
    let plan = json!({
        "kind": "managedOrdinary", "sourcePrefixId": value.plan.source_prefix_id,
        "sourcePrefix": prefix(), "destinationParent": "C:\\destination-parent",
        "destinationParentIdentity": DIR, "destinationLeafNonce": LEAF,
        "observation": { "stepId": ID, "rootTreeId": ID, "policySetId": ID,
            "authorizationEpoch": 1, "generation": 3, "snapshotDigest": RAW, "selectionDigest": RAW },
        "placementGeneration": 4, "placementDigest": RAW, "captureSequence": 1,
        "captureDigest": ID, "workingOverlay": {"count": 0, "digest": ID},
        "companionId": null, "companionFiles": {"count": 0, "digest": ID},
        "companionDeletions": {"count": 0, "digest": ID},
        "protectedManifests": {"count": 0, "digest": ID}, "quotaBytes": 0
    });
    let plan_bytes = layrs_core::scm::canonical_json(&plan).unwrap();
    assert_eq!(
        codec::encode(&value.plan, ProviderTransitionIntentRecord::MAX_BYTES).unwrap(),
        plan_bytes
    );
    assert_eq!(
        value.plan.digest().unwrap(),
        codec::hash(PLAN_DOMAIN, &plan_bytes)
    );
    let legacy = json!({ "schemaVersion": SCHEMA, "transactionId": TX, "role": "source",
        "state": "planned", "localGeneration": 2, "plan": plan,
        "destination": null, "reservation": null, "receiptId": null });
    let bytes = layrs_core::scm::canonical_json(&legacy).unwrap();
    assert_eq!(value.encode().unwrap(), bytes);
    assert_eq!(value.digest().unwrap(), codec::hash(DOMAIN, &bytes));
    assert_eq!(
        ProviderTransitionIntentRecord::decode(&bytes).unwrap(),
        value
    );
}

#[test]
fn v2_intent_requires_closed_nonnull_coverage_and_preserves_duplicate_detection() {
    let mut value = intent();
    value.schema_version = SCHEMA_V2.into();
    value.plan.denial_coverage = Some(denial_reference());
    let bytes = value.encode().unwrap();
    assert!(bytes.len() < ProviderTransitionIntentRecord::MAX_BYTES);
    assert_eq!(
        ProviderTransitionIntentRecord::decode(&bytes).unwrap(),
        value
    );
    assert_eq!(value.digest().unwrap(), codec::hash(DOMAIN_V2, &bytes));
    let plan_bytes = codec::encode(&value.plan, ProviderTransitionIntentRecord::MAX_BYTES).unwrap();
    assert_eq!(
        value.plan.digest().unwrap(),
        codec::hash(PLAN_DOMAIN_V2, &plan_bytes)
    );
    assert_ne!(
        value.plan.digest().unwrap(),
        codec::hash(PLAN_DOMAIN, &plan_bytes)
    );
    let wire = serde_json::to_value(&value).unwrap();
    for malformed in 0..6 {
        let mut wrong = wire.clone();
        match malformed {
            0 => {
                wrong["plan"]
                    .as_object_mut()
                    .unwrap()
                    .remove("denialCoverage");
            }
            1 => wrong["plan"]["denialCoverage"] = serde_json::Value::Null,
            2 => wrong["schemaVersion"] = SCHEMA.into(),
            3 => wrong["plan"]["denialCoverage"]["unexpected"] = true.into(),
            4 => {
                wrong["plan"]["denialCoverage"]
                    .as_object_mut()
                    .unwrap()
                    .remove("rowsDigest");
            }
            _ => wrong["unexpected"] = true.into(),
        }
        let bytes = layrs_core::scm::canonical_json(&wrong).unwrap();
        assert!(serde_json::from_slice::<ProviderTransitionIntentRecord>(&bytes).is_err());
        assert!(ProviderTransitionIntentRecord::decode(&bytes).is_err());
    }
    let text = std::str::from_utf8(&bytes).unwrap();
    let reference = serde_json::to_string(&denial_reference()).unwrap();
    for duplicate in [
        format!("{{\"schemaVersion\":\"{SCHEMA_V2}\",{}", &text[1..]),
        text.replacen(
            "\"denialCoverage\":",
            &format!("\"denialCoverage\":{reference},\"denialCoverage\":"),
            1,
        ),
        text.replacen(
            "\"rowsDigest\":",
            &format!("\"rowsDigest\":\"{ID}\",\"rowsDigest\":"),
            1,
        ),
    ] {
        assert!(serde_json::from_str::<ProviderTransitionIntentRecord>(&duplicate).is_err());
    }
    // Legacy decoding cannot accept even a null extension under its old name.
    let mut legacy_null = serde_json::to_value(intent()).unwrap();
    legacy_null["plan"]["denialCoverage"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<ProviderTransitionIntentRecord>(legacy_null).is_err());
    let mut bare_extra = serde_json::to_value(&value.plan).unwrap();
    bare_extra["unexpected"] = true.into();
    assert!(serde_json::from_value::<TransitionPlan>(bare_extra).is_err());
    let mut bare = serde_json::to_value(&value.plan).unwrap();
    bare["denialCoverage"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<TransitionPlan>(bare).is_err());
    value.schema_version = SCHEMA.into();
    assert!(value.encode().is_err());
    value.plan.denial_coverage = None;
    value.schema_version = SCHEMA_V2.into();
    assert!(value.encode().is_err());
}

#[test]
fn v2_ready_receipt_and_cas_require_the_exact_version_and_entire_reference() {
    let mut staging = intent();
    staging.schema_version = SCHEMA_V2.into();
    staging.plan.denial_coverage = Some(denial_reference());
    staging.state = TransitionState::Staging;
    staging.reservation = Some(staging.reservation_operation().unwrap());
    staging.destination = Some(proof(&staging));
    let mut receipt = receipt(&staging);
    receipt.schema_version = "layrs.projection-provider-transition-receipt.v2".into();
    receipt.denial_coverage = staging.plan.denial_coverage.clone();
    let mut ready = copy(&staging);
    ready.state = TransitionState::Ready;
    ready.receipt_id = Some(receipt.digest().unwrap());
    ready.require_transition(&staging).unwrap();
    ready.require_receipt(&receipt).unwrap();
    for field in 0..4 {
        let mut changed =
            ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap();
        let reference = changed.denial_coverage.as_mut().unwrap();
        match field {
            0 => reference.id = ID.replace('1', "2"),
            1 => reference.count += 1,
            2 => reference.path_bytes += 1,
            _ => reference.rows_digest = ID.replace('1', "2"),
        }
        // Even repair every compact digest that otherwise links this forged
        // receipt: the whole coverage reference must independently match.
        let mut forged = copy(&ready);
        forged.receipt_id = Some(changed.digest().unwrap());
        assert!(forged.require_receipt(&changed).is_err());
        forged.plan.denial_coverage = changed.denial_coverage.clone();
        assert!(forged.require_transition(&ready).is_err());
    }
    let mut downgraded =
        ProviderTransitionReceiptRecord::decode(&receipt.encode().unwrap()).unwrap();
    downgraded.schema_version = "layrs.projection-provider-transition-receipt.v1".into();
    downgraded.denial_coverage = None;
    let mut forged = copy(&ready);
    forged.receipt_id = Some(downgraded.digest().unwrap());
    assert!(forged.require_receipt(&downgraded).is_err());
    let mut changed_version = copy(&staging);
    changed_version.schema_version = SCHEMA.into();
    changed_version.plan.denial_coverage = None;
    assert!(changed_version.require_transition(&staging).is_err());
}
