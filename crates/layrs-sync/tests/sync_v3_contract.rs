#![recursion_limit = "256"]

use base64::Engine as _;
use layrs_core::scm::{RepositoryView, SpaceId, WorktreeId, WorktreeStatus, WorktreeViewState};
use layrs_sync::v3::*;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/contracts/sync-v3/examples")
        .join(name);
    std::fs::read_to_string(path).expect("read Sync V3 fixture")
}

fn parses<T: DeserializeOwned>(name: &str) -> T {
    serde_json::from_str(&fixture(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn repository_view_with_local_worktree() -> RepositoryView {
    RepositoryView::new(
        SpaceId::parse("space:demo").unwrap(),
        vec![],
        vec![],
        vec![],
        vec![WorktreeViewState::new(
            WorktreeId::parse("worktree:local-only").unwrap(),
            None,
            None,
            None,
            WorktreeStatus::Ready,
        )],
    )
    .unwrap()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdentityVectors {
    schema_version: String,
    hash_vectors: Vec<HashVector>,
    reject_vectors: Vec<RejectVector>,
}

#[derive(Deserialize)]
#[serde(tag = "kind")]
enum HashVector {
    #[serde(rename = "evidence")]
    Evidence {
        name: String,
        domain: String,
        #[serde(rename = "inputBase64")]
        input_base64: String,
        #[serde(rename = "expectedByteLength")]
        expected_byte_length: u64,
        #[serde(rename = "expectedObjectId")]
        expected_object_id: layrs_core::scm::ObjectId,
    },
    #[serde(rename = "product")]
    Product {
        name: String,
        domain: String,
        input: Box<ProductEnvelope>,
        #[serde(rename = "expectedIdentityUtf8")]
        expected_identity_utf8: String,
        #[serde(rename = "expectedObjectId")]
        expected_object_id: layrs_core::scm::ObjectId,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind")]
enum RejectVector {
    #[serde(rename = "wrongProductClaim")]
    WrongClaim {
        name: String,
        source: String,
        #[serde(rename = "claimedObjectId")]
        claimed_object_id: layrs_core::scm::ObjectId,
    },
    #[serde(rename = "wrongObjectKind")]
    WrongKind {
        name: String,
        source: String,
        #[serde(rename = "descriptorKind")]
        descriptor_kind: ObjectKind,
    },
    #[serde(rename = "nonJcsProduct")]
    NonJcs {
        name: String,
        source: String,
        #[serde(rename = "wireEncoding")]
        wire_encoding: String,
    },
}

fn identity_vectors() -> IdentityVectors {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/contracts/sync-v3/identity-vectors.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn product_and_evidence_identity_vectors_are_exact() {
    let vectors = identity_vectors();
    assert_eq!(vectors.schema_version, "layrs.sync.identity-vectors.v1");
    assert_eq!(vectors.hash_vectors.len(), 3);
    for vector in vectors.hash_vectors {
        match vector {
            HashVector::Evidence {
                name,
                domain,
                input_base64,
                expected_byte_length,
                expected_object_id,
            } => {
                assert_eq!(domain, EVIDENCE_IDENTITY_DOMAIN, "{name}");
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(input_base64)
                    .unwrap();
                let identity = hash_evidence_reader(bytes.as_slice()).unwrap();
                assert_eq!(identity.byte_len, expected_byte_length, "{name}");
                assert_eq!(identity.object_id, expected_object_id, "{name}");
            }
            HashVector::Product {
                name,
                domain,
                input,
                expected_identity_utf8,
                expected_object_id,
            } => {
                assert_eq!(domain, PRODUCT_IDENTITY_DOMAIN, "{name}");
                assert_eq!(
                    input.identity_bytes().unwrap(),
                    expected_identity_utf8.as_bytes(),
                    "{name}"
                );
                assert_eq!(
                    input.computed_object_id().unwrap(),
                    expected_object_id,
                    "{name}"
                );
                input.verify_identity().unwrap();
            }
        }
    }
}

#[test]
fn product_identity_reject_vectors_are_enforced() {
    let vectors = identity_vectors();
    let product: ProductEnvelope = parses("layer-product.json");
    product.validate_v3().unwrap();
    let canonical = product.canonical_bytes().unwrap();
    product.verify_canonical_wire(&canonical).unwrap();
    for vector in vectors.reject_vectors {
        match vector {
            RejectVector::WrongClaim {
                name,
                source,
                claimed_object_id,
            } => {
                assert_eq!(source, "product-layer-variant");
                let descriptor = ObjectDescriptor {
                    kind: ObjectKind::Product,
                    object_id: claimed_object_id,
                    canonical_byte_len: canonical.len() as u64,
                    encoded_byte_len: canonical.len() as u64,
                    compression: Compression::Identity,
                };
                assert!(product.verify_descriptor(&descriptor).is_err(), "{name}");
            }
            RejectVector::WrongKind {
                name,
                source,
                descriptor_kind,
            } => {
                assert_eq!(source, "product-layer-variant");
                let descriptor = ObjectDescriptor {
                    kind: descriptor_kind,
                    object_id: product.object_id().clone(),
                    canonical_byte_len: canonical.len() as u64,
                    encoded_byte_len: canonical.len() as u64,
                    compression: Compression::Identity,
                };
                assert!(product.verify_descriptor(&descriptor).is_err(), "{name}");
            }
            RejectVector::NonJcs {
                name,
                source,
                wire_encoding,
            } => {
                assert_eq!(source, "product-layer-variant");
                assert_eq!(wire_encoding, "pretty-json");
                let pretty = serde_json::to_vec_pretty(&product).unwrap();
                assert!(product.verify_canonical_wire(&pretty).is_err(), "{name}");
            }
        }
    }
}

#[test]
fn evidence_streaming_enforces_length_limit_and_claims() {
    use std::io::Read as _;
    let too_large = std::io::repeat(0).take(MAX_EVIDENCE_BYTE_LEN + 1);
    assert!(matches!(
        hash_evidence_reader(too_large),
        Err(EvidenceIdentityError::TooLarge { .. })
    ));

    let wrong = layrs_core::scm::ObjectId::parse(format!("blake3:{}", "f".repeat(64))).unwrap();
    assert!(matches!(
        verify_evidence_reader(&b"evidence"[..], &wrong, 8),
        Err(EvidenceIdentityError::ClaimMismatch { .. })
    ));
    let correct = hash_evidence_reader(&b"evidence"[..]).unwrap();
    assert!(matches!(
        verify_evidence_reader(&b"evidence"[..], &correct.object_id, 7),
        Err(EvidenceIdentityError::LengthMismatch { .. })
    ));
}

#[test]
fn contract_examples_decode_into_v3_types() {
    let _: SyncTransactionRequest = parses("transaction.json");
    let _: SyncTransactionRequest = parses("policy-root-transaction.json");
    let _: SyncTransactionReceipt = parses("receipt.json");
    let _: SyncTransactionReceipt = parses("rejected-receipt.json");
    let _: ObjectPlanRequest = parses("object-plan-request.json");
    let _: ObjectPlanResponse = parses("object-plan-response.json");
    let _: FetchPlanRequest = parses("fetch-plan-request.json");
    let _: FetchPlanResponse = parses("fetch-plan-response.json");
    let _: SpaceEventPageRequest = parses("event-page-request.json");
    let _: SpaceEventPage = parses("event-page-response.json");
    let _: SpaceEventPage = parses("event-page-policy-response.json");
    let _: PolicySet = parses("policy-set.json");
    let _: AuthorizationSnapshot = parses("authorization-snapshot.json");
    let _: ProductEnvelope = parses("layer-product.json");
}

#[test]
fn choose_requires_exact_decision_product_object_and_rejects_legacy_field() {
    let mut legacy: Value = serde_json::from_str(&fixture("transaction.json")).unwrap();
    legacy["viewMutation"]["layerMutations"][0] = json!({
        "layerId": "layer:main",
        "mode": "choose",
        "decisionProductId": "decision:legacy"
    });
    assert!(serde_json::from_value::<SyncTransactionRequest>(legacy).is_err());

    let mut exact: Value = serde_json::from_str(&fixture("transaction.json")).unwrap();
    exact["viewMutation"]["layerMutations"][0] = json!({
        "layerId": "layer:main",
        "mode": "choose",
        "decisionProductObjectId": "blake3:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
    });
    let exact: SyncTransactionRequest = serde_json::from_value(exact).unwrap();
    exact.view_mutation.as_ref().unwrap().validate_v3().unwrap();
}

#[test]
fn choose_decision_chosen_heads_equal_inline_proposed_heads() {
    let chosen: layrs_core::scm::StepId =
        serde_json::from_value(json!(format!("blake3:{}", "a".repeat(64)))).unwrap();
    let mut product_json = json!({
        "kind": "decision",
        "productId": "decision:choose-main",
        "productObjectId": format!("blake3:{}", "0".repeat(64)),
        "dependsOnStepIds": [chosen],
        "dependsOnProductIds": ["layer:main"],
        "record": {
            "schemaVersion": "layrs.ref-decision.v1",
            "decisionId": "decision:choose-main",
            "spaceId": "space:demo",
            "layerId": "layer:main",
            "discardedHeadStepIds": [],
            "chosenHeadStepIds": [chosen],
            "reason": "explicit choice",
            "decidedBy": "actor:alexa",
            "decidedAt": "2026-07-19T12:00:00Z"
        }
    });
    let provisional: ProductEnvelope = serde_json::from_value(product_json.clone()).unwrap();
    product_json["productObjectId"] =
        serde_json::to_value(provisional.computed_object_id().unwrap()).unwrap();
    let product: ProductEnvelope = serde_json::from_value(product_json).unwrap();

    let layer_id = layrs_core::scm::LayerId::parse("layer:main").unwrap();
    let matching_view = RepositoryView::new(
        SpaceId::parse("space:demo").unwrap(),
        vec![layrs_core::scm::LayerHeadSet::new(layer_id.clone(), vec![chosen]).unwrap()],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let mismatching_view = RepositoryView::new(
        SpaceId::parse("space:demo").unwrap(),
        vec![layrs_core::scm::LayerHeadSet::new(layer_id, vec![]).unwrap()],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();

    let mut request: SyncTransactionRequest = parses("transaction.json");
    let mutation = request.view_mutation.as_mut().unwrap();
    mutation.proposed_view = ProposedRepositoryView::Inline {
        view: matching_view,
    };
    mutation.layer_mutations[0].mode = LayerMutationMode::Choose;
    mutation.layer_mutations[0].decision_product_object_id = Some(product.object_id().clone());
    request.products = vec![product];
    request.request_digest = request.computed_request_digest().unwrap();
    request.validate_v3().unwrap();

    request.view_mutation.as_mut().unwrap().proposed_view = ProposedRepositoryView::Inline {
        view: mismatching_view,
    };
    request.request_digest = request.computed_request_digest().unwrap();
    assert_eq!(
        request.validate_v3().unwrap_err().code,
        "invalid_view_mutation"
    );
}

#[test]
fn repository_view_mutation_rejects_local_worktree_state_on_wire() {
    let request: SyncTransactionRequest = parses("transaction.json");
    let mut mutation = request.view_mutation.unwrap();
    mutation.proposed_view = ProposedRepositoryView::Inline {
        view: repository_view_with_local_worktree(),
    };
    let error = mutation.validate_v3().unwrap_err();
    assert_eq!(error.code, "wire_worktree_forbidden");
    assert_eq!(error.field, "proposedView.view.worktrees");
}

#[test]
fn policy_expectation_is_exact_captured_product_object_set() {
    let request: SyncTransactionRequest = parses("transaction.json");
    let mut mutation = request.view_mutation.unwrap();
    let mut policy_set: PolicySet = parses("policy-set.json");
    let stale_subset = mutation
        .verify_captured_policy_versions(&policy_set)
        .unwrap_err();
    assert_eq!(stale_subset.code(), "authorization_epoch_changed");
    mutation.expected_policy_version_object_ids =
        captured_policy_version_object_ids(&policy_set).unwrap();
    mutation
        .verify_captured_policy_versions(&policy_set)
        .unwrap();

    let policy_object =
        layrs_core::scm::ObjectId::parse(format!("blake3:{}", "a".repeat(64))).unwrap();
    policy_set.policy_versions.push(PolicyVersionEntry {
        policy_id: PolicyId::parse("policy:required").unwrap(),
        policy_version_id: PolicyVersionId::parse("policy-version:required-v1").unwrap(),
        policy_product_object_id: policy_object.clone(),
        dependency_object_ids: vec![],
    });
    let stale = mutation
        .verify_captured_policy_versions(&policy_set)
        .unwrap_err();
    assert_eq!(stale.code(), "authorization_epoch_changed");
    mutation.expected_policy_version_object_ids =
        captured_policy_version_object_ids(&policy_set).unwrap();
    mutation
        .verify_captured_policy_versions(&policy_set)
        .unwrap();
}

#[test]
fn policy_root_cas_requires_the_expected_authorization_epoch() {
    let request: SyncTransactionRequest = parses("policy-root-transaction.json");
    let mut mutation = request.policy_root_mutation.unwrap();
    assert_eq!(mutation.expected_authorization_epoch, 1);
    mutation.expected_authorization_epoch = 0;
    assert_eq!(mutation.validate_v3().unwrap_err().code, "invalid_epoch");

    let mut missing: Value =
        serde_json::from_str(&fixture("policy-root-transaction.json")).unwrap();
    missing["policyRootMutation"]
        .as_object_mut()
        .unwrap()
        .remove("expectedAuthorizationEpoch");
    assert!(serde_json::from_value::<SyncTransactionRequest>(missing).is_err());
}

#[test]
fn policy_set_contains_only_one_version_per_logical_policy() {
    let mut policy_set: PolicySet = parses("policy-set.json");
    let mut duplicate = policy_set.policy_versions[0].clone();
    duplicate.policy_version_id = PolicyVersionId::parse("policy-version:second-version").unwrap();
    duplicate.policy_product_object_id =
        layrs_core::scm::ObjectId::parse(format!("blake3:{}", "f".repeat(64))).unwrap();
    policy_set.policy_versions.push(duplicate);
    assert_eq!(
        policy_set.validate_v3().unwrap_err().code,
        "non_canonical_set"
    );
}

#[test]
fn semantic_rejection_is_a_storable_receipt_without_partial_space_state() {
    let receipt: SyncTransactionReceipt = parses("rejected-receipt.json");
    receipt.validate_v3().unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Rejected);
    assert!(
        receipt
            .authorization_snapshot_id
            .as_str()
            .starts_with("blake3:")
    );
    assert!(receipt.accepted_step_ids.is_empty());
    assert!(receipt.accepted_products.is_empty());
    assert!(receipt.event_ids.is_empty());
    assert!(receipt.event_cursor.is_none());
    assert_eq!(
        serde_jcs::to_vec(&receipt).unwrap(),
        serde_jcs::to_vec(&receipt).unwrap()
    );
}

#[test]
fn digest_mismatches_are_closed_presemantic_errors_not_receipts() {
    let error: SyncTransactionError = parses("transaction-error.json");
    error.validate_v3().unwrap();
    assert_eq!(error.http_status, 409);
    assert!(!error.receipt_created);

    let mut receipt: Value = serde_json::from_str(&fixture("rejected-receipt.json")).unwrap();
    receipt["rejections"][0]["code"] = json!("idempotency_digest_mismatch");
    assert!(serde_json::from_value::<SyncTransactionReceipt>(receipt).is_err());

    let mut invalid = error;
    invalid.code = TransactionErrorCode::RequestDigestMismatch;
    assert_eq!(
        invalid.validate_v3().unwrap_err().code,
        "invalid_transaction_error"
    );
}

#[test]
fn receipt_storage_digest_and_replay_binding_are_exact() {
    let receipt: SyncTransactionReceipt = parses("receipt.json");
    let expected = layrs_core::scm::ObjectId::parse(
        "blake3:69e2aa49f6dc0e0398918f4f3f865e274d7d3ee0307714e2722f7f97b66eda1d",
    )
    .unwrap();
    assert_eq!(receipt.canonical_storage_bytes().unwrap().len(), 1552);
    assert_eq!(receipt.computed_storage_digest().unwrap(), expected);
    assert!(receipt.verify_storage_digest(&expected).unwrap());

    let principal = PrincipalId::parse("principal:alexa").unwrap();
    let client = ClientId::parse("client:desktop-a").unwrap();
    let space = layrs_core::scm::SpaceId::parse("space:demo").unwrap();
    receipt
        .authorize_replay(Some(&principal), &client, &space, false)
        .unwrap();
    assert_eq!(
        receipt
            .authorize_replay(Some(&principal), &client, &space, true)
            .unwrap_err(),
        ReplayRefusal::Revoked
    );
    assert_eq!(
        receipt
            .authorize_replay(None, &client, &space, false)
            .unwrap_err(),
        ReplayRefusal::Unauthenticated
    );
}

#[test]
fn divergence_created_has_one_exact_definition() {
    let mut receipt: SyncTransactionReceipt = parses("receipt.json");
    let layer = &mut receipt.view_result.0.as_mut().unwrap().layer_results[0];
    layer.resulting_heads = vec![
        serde_json::from_value(json!(format!("blake3:{}", "a".repeat(64)))).unwrap(),
        serde_json::from_value(json!(format!("blake3:{}", "b".repeat(64)))).unwrap(),
    ];
    layer.proposed_heads = layer.resulting_heads.clone();
    assert_eq!(receipt.validate_v3().unwrap_err().code, "invalid_receipt");
    receipt.view_result.0.as_mut().unwrap().layer_results[0].divergence_created = true;
    receipt.validate_v3().unwrap();
}

#[test]
fn bootstrap_uses_identity_checking_scm_records() {
    let result = serde_json::from_str::<SpaceBootstrap>(&fixture("bootstrap.json"));
    assert!(
        result.is_err(),
        "example contains a deliberately illustrative, not computed, RepositoryView ID"
    );
}

#[test]
fn bootstrap_rejects_local_worktree_state_on_wire() {
    let view = repository_view_with_local_worktree();
    let mut value: Value = serde_json::from_str(&fixture("bootstrap.json")).unwrap();
    value["currentRepositoryView"] = json!({
        "repositoryViewId": view.id(),
        "record": view,
    });
    let bootstrap: SpaceBootstrap = serde_json::from_value(value).unwrap();
    let error = bootstrap.validate_v3().unwrap_err();
    assert_eq!(error.code, "wire_worktree_forbidden");
    assert_eq!(error.field, "currentRepositoryView.record.worktrees");
}

#[test]
fn request_digest_is_domain_separated_jcs_and_stable() {
    let request: SyncTransactionRequest = parses("transaction.json");
    let digest = request.computed_request_digest().unwrap();
    assert_eq!(
        digest.as_str(),
        "blake3:5775f0a3fc3c47751b7c653a168de1da464bbffc348111b3dcc22e4651dbfe67"
    );

    let mut reordered: Value = serde_json::from_str(&fixture("transaction.json")).unwrap();
    let object = reordered.as_object_mut().unwrap();
    let digest_field = object.remove("requestDigest").unwrap();
    object.insert("requestDigest".into(), digest_field);
    let reordered: SyncTransactionRequest = serde_json::from_value(reordered).unwrap();
    assert_eq!(reordered.computed_request_digest().unwrap(), digest);

    let mut wrong = reordered;
    wrong.request_digest =
        serde_json::from_value(json!(format!("blake3:{}", "f".repeat(64)))).unwrap();
    let error = wrong.presemantic_digest_error().unwrap().unwrap();
    assert_eq!(error.code, TransactionErrorCode::RequestDigestMismatch);
    assert!(!error.receipt_created);
    error.validate_v3().unwrap();
}

#[test]
fn v1_v2_unknown_versions_and_unknown_fields_fail_closed() {
    let mut value: Value = serde_json::from_str(&fixture("transaction.json")).unwrap();
    value["protocol"] = json!("layrs.sync.v2");
    assert!(serde_json::from_value::<SyncTransactionRequest>(value.clone()).is_err());
    value["protocol"] = json!("layrs.sync.v1");
    assert!(serde_json::from_value::<SyncTransactionRequest>(value.clone()).is_err());
    value["protocol"] = json!("layrs.sync.v4");
    assert!(serde_json::from_value::<SyncTransactionRequest>(value.clone()).is_err());
    value["protocol"] = json!("layrs.sync.v3");
    value["operation"] = json!({"operationId": "operation:forbidden"});
    assert!(serde_json::from_value::<SyncTransactionRequest>(value).is_err());
}

#[test]
fn required_nullable_transaction_fields_distinguish_absent_from_null() {
    for field in ["knownEventCursor", "viewMutation", "policyRootMutation"] {
        let mut value: Value = serde_json::from_str(&fixture("transaction.json")).unwrap();
        value.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<SyncTransactionRequest>(value).is_err(),
            "missing {field} must reject"
        );
    }
    let mut value: Value = serde_json::from_str(&fixture("transaction.json")).unwrap();
    value["knownEventCursor"] = Value::Null;
    assert!(serde_json::from_value::<SyncTransactionRequest>(value).is_ok());
}

#[test]
fn required_nullable_fields_are_required_across_receipts_events_and_plans() {
    for (name, path) in [
        ("receipt.json", "viewResult"),
        ("event-page-policy-response.json", "events.0.actor.deviceId"),
        ("fetch-plan-request.json", "pageCursor"),
        ("fetch-plan-response.json", "nextPageCursor"),
    ] {
        let mut value: Value = serde_json::from_str(&fixture(name)).unwrap();
        match path {
            "events.0.actor.deviceId" => {
                value["events"][0]["actor"]
                    .as_object_mut()
                    .unwrap()
                    .remove("deviceId");
            }
            field => {
                value.as_object_mut().unwrap().remove(field);
            }
        }
        let rejects = match name {
            "receipt.json" => serde_json::from_value::<SyncTransactionReceipt>(value).is_err(),
            "event-page-policy-response.json" => {
                serde_json::from_value::<SpaceEventPage>(value).is_err()
            }
            "fetch-plan-request.json" => serde_json::from_value::<FetchPlanRequest>(value).is_err(),
            "fetch-plan-response.json" => {
                serde_json::from_value::<FetchPlanResponse>(value).is_err()
            }
            _ => unreachable!(),
        };
        assert!(rejects, "missing required nullable {path} must reject");
    }
}

fn weave_product() -> Value {
    json!({
        "kind": "weave",
        "productId": "weave:merge-1",
        "productObjectId": "blake3:1010101010101010101010101010101010101010101010101010101010101010",
        "dependsOnStepIds": [],
        "dependsOnProductIds": [],
        "record": {
            "schemaVersion": "layrs.scm.weave_receipt.v1",
            "weaveId": "blake3:1010101010101010101010101010101010101010101010101010101010101010",
            "spaceId": "space:demo",
            "sourceHeads": ["blake3:1111111111111111111111111111111111111111111111111111111111111111"],
            "targetHeadsBefore": ["blake3:2222222222222222222222222222222222222222222222222222222222222222"],
            "mergeBases": [],
            "resultStepId": "blake3:3333333333333333333333333333333333333333333333333333333333333333",
            "outcome": "clean",
            "conflictIds": [],
            "lensExecutions": [],
            "determinismProfile": {
                "profileId": "blake3:4444444444444444444444444444444444444444444444444444444444444444",
                "canonicalization": {"id": "rfc8785", "version": "RFC8785"},
                "contentHash": {"id": "blake3", "version": "1"},
                "mergeAlgorithm": {"id": "merge", "version": "1"},
                "baseAlgorithm": {"id": "base", "version": "1"},
                "hostPolicy": {
                    "id": "host", "version": "1",
                    "digest": "sha256:5555555555555555555555555555555555555555555555555555555555555555",
                    "ambientCapabilities": {"network": false, "wallClock": false, "randomness": false, "locale": false, "hostFilesystem": false, "environment": false}
                },
                "resolutionPolicy": {
                    "id": "resolve", "version": "1",
                    "digest": "sha256:6666666666666666666666666666666666666666666666666666666666666666",
                    "allowedMethods": ["manual"], "automaticSideSelection": false,
                    "absentSemantics": "distinct_from_empty_file", "combineOrder": "term_id_ascii",
                    "manualInput": "allowed", "renameResolution": "forbidden",
                    "onUnsupportedLens": "reject", "onUnavailableLens": "reject", "onTimeout": "reject",
                    "onQuotaExceeded": "reject", "onTrap": "reject", "onInvalidOutput": "reject",
                    "onInvalidSignature": "reject", "onDigestMismatch": "reject"
                },
                "limits": {"maxMemoryBytes": 1, "maxExecutionMs": 1, "maxInputBytes": 1, "maxOutputBytes": 1, "maxFuel": null, "maxBlocks": 1, "maxSegments": 1, "maxPaths": 1, "maxRecursionDepth": 1}
            },
            "operationId": "operation:merge-1",
            "gateSnapshotId": null,
            "createdAt": "2026-07-19T12:00:00Z"
        }
    })
}

#[test]
fn weave_product_is_fully_typed_and_nested_unknowns_fail_closed() {
    let value = weave_product();
    assert!(serde_json::from_value::<ProductEnvelope>(value.clone()).is_ok());
    let mut unknown = value;
    unknown["record"]["determinismProfile"]["limits"]["futureLimit"] = json!(1);
    assert!(serde_json::from_value::<ProductEnvelope>(unknown).is_err());
}

#[test]
fn product_version_cardinality_is_closed_by_kind() {
    let policy = json!({
        "kind": "policy",
        "productId": "policy:default",
        "productObjectId": format!("blake3:{}", "1".repeat(64)),
        "dependsOnStepIds": [],
        "dependsOnProductIds": [],
        "previousProductObjectId": null,
        "record": {
            "schemaVersion": "layrs.policy.v1",
            "policyId": "policy:default",
            "versionId": "policy-version:default-v1",
            "spaceId": "space:demo",
            "rulesObjectId": format!("blake3:{}", "2".repeat(64)),
            "createdAt": "2026-07-19T12:00:00Z"
        }
    });
    assert!(serde_json::from_value::<ProductEnvelope>(policy.clone()).is_ok());
    let mut missing_previous = policy;
    missing_previous
        .as_object_mut()
        .unwrap()
        .remove("previousProductObjectId");
    assert!(serde_json::from_value::<ProductEnvelope>(missing_previous).is_err());

    let gate = json!({
        "kind": "gateDefinition",
        "productId": "gate-definition:ci",
        "productObjectId": format!("blake3:{}", "3".repeat(64)),
        "dependsOnStepIds": [],
        "dependsOnProductIds": [],
        "previousProductObjectId": null,
        "record": {
            "schemaVersion": "layrs.gate-definition.v1",
            "gateDefinitionId": "gate-definition:ci",
            "spaceId": "space:demo",
            "name": "CI",
            "recipeObjectIds": [],
            "policyVersionObjectIds": [],
            "createdAt": "2026-07-19T12:00:00Z"
        }
    });
    assert!(serde_json::from_value::<ProductEnvelope>(gate).is_ok());

    let singleton_marker = json!({
        "kind": "marker",
        "productId": "marker:checkpoint",
        "productObjectId": format!("blake3:{}", "4".repeat(64)),
        "dependsOnStepIds": [format!("blake3:{}", "5".repeat(64))],
        "dependsOnProductIds": [],
        "previousProductObjectId": null,
        "record": {
            "schemaVersion": "layrs.marker.v1",
            "markerId": "marker:checkpoint",
            "spaceId": "space:demo",
            "targetStepId": format!("blake3:{}", "5".repeat(64)),
            "kind": "checkpoint",
            "name": "checkpoint",
            "title": "",
            "notes": "",
            "createdBy": null,
            "createdAt": "2026-07-19T12:00:00Z",
            "proofIds": [],
            "metadata": []
        }
    });
    assert!(serde_json::from_value::<ProductEnvelope>(singleton_marker).is_err());
}

#[test]
fn closed_unions_and_prefixed_ids_reject_unknown_values() {
    let mut event: Value =
        serde_json::from_str(&fixture("event-page-policy-response.json")).unwrap();
    event["events"][0]["kind"] = json!("unknown.event");
    assert!(serde_json::from_value::<SpaceEventPage>(event).is_err());

    let mut product: Value = serde_json::from_str(&fixture("layer-product.json")).unwrap();
    product["kind"] = json!("unknown");
    assert!(serde_json::from_value::<ProductEnvelope>(product).is_err());

    assert!(serde_json::from_value::<ClientId>(json!("device:not-a-client")).is_err());
    assert!(serde_json::from_value::<Cursor>(json!("")).is_err());
}

#[test]
fn upload_is_full_object_only_and_download_exposes_resume_contract() {
    let upload: ObjectPlanResponse = parses("object-plan-response.json");
    upload.validate_v3().unwrap();
    let upload_json = serde_json::to_value(&upload).unwrap();
    assert!(upload_json["uploadCapabilities"][0].get("offset").is_none());
    assert!(upload_json["uploadCapabilities"][0].get("range").is_none());

    let mut download: Value = serde_json::from_str(&fixture("fetch-plan-response.json")).unwrap();
    let capability = json!({
        "principalId": "principal:alexa",
        "authorizationEpoch": 1,
        "spaceId": "space:demo",
        "clientId": "client:desktop-a",
        "requestId": "request:fetch-1",
        "planId": "plan:fetch-1",
        "claim": {
            "kind": "authorizationSnapshot",
            "authorizationSnapshotId": "blake3:3333333333333333333333333333333333333333333333333333333333333333",
            "purpose": "audit"
        },
        "descriptor": {
            "kind": "authorization_snapshot",
            "objectId": "blake3:3333333333333333333333333333333333333333333333333333333333333333",
            "canonicalByteLen": 10,
            "encodedByteLen": 10,
            "compression": "identity"
        },
        "claimDigest": "blake3:9999999999999999999999999999999999999999999999999999999999999999",
        "method": "GET",
        "downloadUrl": "https://layrs.example/v3/transfers/downloads",
        "token": "00000000000000000000000000000000",
        "expiresAt": "2026-07-19T12:05:00Z",
        "resumeToken": "resume-token-0001",
        "acceptRanges": true
    });
    download["downloadCapabilities"] = json!([capability]);
    let download: FetchPlanResponse = serde_json::from_value(download).unwrap();
    assert!(download.download_capabilities[0].accept_ranges);
    download.validate_v3().unwrap();
}

#[test]
fn transfer_urls_are_generic_and_never_carry_sensitive_identifiers() {
    let upload: ObjectPlanResponse = parses("object-plan-response.json");
    assert_eq!(
        upload.upload_capabilities[0].upload_url,
        "https://layrs.example/v3/transfers/uploads"
    );
    let mut sensitive = upload.clone();
    sensitive.upload_capabilities[0].upload_url = format!(
        "https://layrs.example/v3/transfers/uploads/{}?token=secret",
        sensitive.missing_objects[0].object_id
    );
    assert_eq!(sensitive.validate_v3().unwrap_err().code, "invalid_uri");

    for invalid in [
        "https://exa mple/v3/transfers/uploads",
        "https://:/v3/transfers/uploads",
        "https://user@layrs.example/v3/transfers/uploads",
        "https://layrs.example/v3/transfers/uploads?token=secret",
        "https://layrs.example/v3/transfers/uploads#fragment",
        "https://layrs.example/v3/transfers/uploads/",
        "https://layrs.example/v3/ignored/../transfers/uploads",
    ] {
        let mut response = upload.clone();
        response.upload_capabilities[0].upload_url = invalid.into();
        assert_eq!(
            response.validate_v3().unwrap_err().code,
            "invalid_uri",
            "{invalid}"
        );
    }
}

#[test]
fn validation_rejects_client_authorization_snapshot_upload() {
    let mut request: ObjectPlanRequest = parses("object-plan-request.json");
    request.objects[0].kind = ObjectKind::AuthorizationSnapshot;
    let error = request.validate_v3().unwrap_err();
    assert_eq!(error.code, "invalid_object_kind");
}
