use layrs_client_core::native_scm::NativeRemote;
use layrs_core::scm::{
    ConflictId, ObjectId, RepoPathComponent, Step, Tree, TreeEntry, TreeEntryTarget,
};
use layrs_sync::v3::*;
use serde_json::Value;

fn digest(domain: &str, value: &Value) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(domain.as_bytes());
    hash.update(&[0]);
    hash.update(&serde_jcs::to_vec(value).unwrap());
    format!("blake3:{}", hash.finalize())
}
fn rehash(value: &mut Value, field: &str, domain: &str) {
    value.as_object_mut().unwrap().remove(field);
    value[field] = digest(domain, value).into();
}
fn receipt_product(mut product: Value) -> ProductEnvelope {
    let record = &mut product["record"];
    rehash(
        &mut record["determinismProfile"],
        "profileId",
        "layrs-determinism-profile-v1",
    );
    let profile = record["determinismProfile"]["profileId"].clone();
    for execution in record["lensExecutions"].as_array_mut().unwrap() {
        execution["determinismProfileId"] = profile.clone();
        rehash(execution, "executionId", "layrs-lens-execution-v1");
    }
    record["lensExecutions"]
        .as_array_mut()
        .unwrap()
        .sort_by(|a, b| a["executionId"].as_str().cmp(&b["executionId"].as_str()));
    rehash(record, "weaveId", "layrs-weave-receipt-v1");
    product["productId"] = format!("weave:{}", record["weaveId"].as_str().unwrap()).into();
    let mut product: ProductEnvelope = serde_json::from_value(product).unwrap();
    let id = product.computed_object_id().unwrap();
    if let ProductEnvelope::Weave {
        product_object_id, ..
    } = &mut product
    {
        *product_object_id = id;
    }
    product
}

pub fn run(
    remote: &NativeRemote,
    origin: &str,
    token: &str,
    ca: &str,
    seed: &SyncTransactionRequest,
    conflict: &Value,
) {
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let before = remote.bootstrap(&seed.space_id).unwrap();
    let original = seed
        .products
        .iter()
        .find(|p| matches!(p, ProductEnvelope::Weave { .. }))
        .unwrap();
    let empty = || {
        let mut request = seed.clone();
        request.idempotency_key = IdempotencyKey::parse(uuid::Uuid::new_v4().to_string()).unwrap();
        request.steps.clear();
        request.products.clear();
        request.view_mutation = None;
        request.known_event_cursor = None;
        request
    };
    for case in 0..7 {
        let mut product = serde_json::to_value(original).unwrap();
        let record = &mut product["record"];
        match case {
            0 => {
                record["determinismProfile"]["hostPolicy"]["ambientCapabilities"]["network"] =
                    true.into()
            }
            1 => {
                record["lensExecutions"][0]["pin"]["digest"] =
                    format!("sha256:{}", "a".repeat(64)).into();
                record["lensExecutions"][0]["implementationDigest"] =
                    format!("sha256:{}", "a".repeat(64)).into();
            }
            2 => {
                record["determinismProfile"]["limits"]["maxMemoryBytes"] =
                    (1024u64 * 1024 * 1024).into()
            }
            3 => record["lensExecutions"][0]["termRoles"][0]["role"] = "target".into(),
            4 => record["lensExecutions"][0]["path"] = "missing.ts".into(),
            5 => {
                record["determinismProfile"]["resolutionPolicy"]["digest"] =
                    format!("sha256:{}", "a".repeat(64)).into()
            }
            _ => record["determinismProfile"]["mergeAlgorithm"]["version"] = "unknown".into(),
        }
        let product = receipt_product(product);
        assert!(product.verify_identity().is_ok());
        let mut request = empty();
        request.products = vec![product];
        rejected(&http, origin, token, &mut request);
    }
    // Intrinsic IDs are all correct, including rehashed anchors; only byte
    // reconstruction can detect that this block was shifted in every original.
    let mut corrupt = conflict.clone();
    let old = corrupt["blocks"][0]["blockId"].clone();
    for span in corrupt["blocks"][0]["occurrenceAnchor"]["spans"]
        .as_array_mut()
        .unwrap()
    {
        span["startByte"] = (span["startByte"].as_u64().unwrap() + 1).into();
        span["endByte"] = (span["endByte"].as_u64().unwrap() + 1).into();
    }
    let mut projection = corrupt["blocks"][0].clone();
    projection.as_object_mut().unwrap().remove("blockId");
    projection.as_object_mut().unwrap().remove("resolution");
    projection["lensPin"] = corrupt["lensPin"].clone();
    projection["lensImplementationDigest"] = corrupt["lensImplementationDigest"].clone();
    let id = digest("layrs-conflict-block-v1", &projection);
    corrupt["blocks"][0]["blockId"] = id.clone().into();
    for segment in corrupt["segments"].as_array_mut().unwrap() {
        if segment["blockId"] == old {
            segment["blockId"] = id.clone().into();
        }
    }
    corrupt["blocks"]
        .as_array_mut()
        .unwrap()
        .sort_by(|a, b| a["blockId"].as_str().cmp(&b["blockId"].as_str()));
    rehash(&mut corrupt, "conflictId", "layrs-conflict-v1");
    let bytes = serde_jcs::to_vec(&corrupt).unwrap();
    layrs_weave::graph_merge::PlannedConflict::from_canonical_bytes(&bytes).unwrap();
    let conflict_id = ConflictId::parse(corrupt["conflictId"].as_str().unwrap()).unwrap();
    upload(
        &http,
        origin,
        token,
        seed,
        ObjectKind::Conflict,
        ObjectId::parse(conflict_id.as_str()).unwrap(),
        bytes,
    );
    let tree = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("code.ts").unwrap(),
        TreeEntryTarget::conflict(conflict_id),
        false,
    )])
    .unwrap();
    upload(
        &http,
        origin,
        token,
        seed,
        ObjectKind::Tree,
        ObjectId::parse(tree.id().as_str()).unwrap(),
        serde_jcs::to_vec(&tree).unwrap(),
    );
    let parent = match original {
        ProductEnvelope::Weave { record, .. } => record.result_step_id.clone(),
        _ => unreachable!(),
    };
    let step = Step::new(vec![parent], tree.id().clone()).unwrap();
    let mut request = empty();
    request.steps = vec![step];
    rejected(&http, origin, token, &mut request);
    // Missing authentication never reaches admission, even for known objects.
    let response = http
        .post(format!("{origin}/v3/spaces/{}/transactions", seed.space_id))
        .json(&request)
        .send()
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    // Knowing the fragment's content ID does not grant access at an unrelated
    // path, even to an authenticated writer of this Space.
    let root_step = match original {
        ProductEnvelope::Weave { record, .. } => record.result_step_id.clone(),
        _ => unreachable!(),
    };
    let fragment = layrs_core::scm::FileId::parse(
        conflict["blocks"][0]["baseValues"][0]["value"]["objectId"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let fetch = FetchPlanRequest {
        protocol: SyncV3Protocol::V3,
        message_type: FetchPlanRequestType::Request,
        space_id: seed.space_id.clone(),
        client_id: seed.client_id.clone(),
        request_id: RequestId::parse(format!("request:{}", uuid::Uuid::new_v4())).unwrap(),
        claims: vec![FetchClaim::File {
            file_id: fragment,
            preferred_manifest_ids: vec![],
            root: AuthorizationRoot::Step { step_id: root_step },
            repo_path: layrs_core::scm::RepoPath::parse("unrelated.ts").unwrap(),
        }],
        page_cursor: Nullable::null(),
        max_objects: 100,
    };
    let response = http
        .post(format!("{origin}/v3/spaces/{}/fetch-plans", seed.space_id))
        .bearer_auth(token)
        .json(&fetch)
        .send()
        .unwrap();
    assert!(response.status().is_success());
    let denied: FetchPlanResponse = response.json().unwrap();
    assert_eq!(denied.denied_claims.len(), 1);
    assert!(denied.download_capabilities.is_empty());
    let after = remote.bootstrap(&seed.space_id).unwrap();
    assert_eq!(
        before.current_repository_view,
        after.current_repository_view
    );
    assert_eq!(before.product_roots, after.product_roots);
    assert_eq!(before.graph_root_step_ids, after.graph_root_step_ids);
}

fn rejected(
    http: &reqwest::blocking::Client,
    origin: &str,
    token: &str,
    request: &mut SyncTransactionRequest,
) {
    request.request_digest = request.computed_request_digest().unwrap();
    let post = || {
        http.post(format!(
            "{origin}/v3/spaces/{}/transactions",
            request.space_id
        ))
        .bearer_auth(token)
        .json(request)
        .send()
        .unwrap()
    };
    let response = post();
    assert!(response.status().is_success(), "HTTP {}", response.status());
    let bytes = response.bytes().unwrap();
    let receipt: SyncTransactionReceipt = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        receipt.status,
        ReceiptStatus::Rejected,
        "{:?}",
        receipt.rejections
    );
    assert!(receipt.accepted_products.is_empty() && receipt.accepted_step_ids.is_empty());
    assert_eq!(bytes, post().bytes().unwrap());
}

pub fn reject_rollback(
    remote: &NativeRemote,
    origin: &str,
    token: &str,
    ca: &str,
    seed: &SyncTransactionRequest,
    parent: layrs_core::scm::StepId,
) {
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .build()
        .unwrap();
    let original = seed
        .products
        .iter()
        .find_map(|p| match p {
            ProductEnvelope::Weave { record, .. } => Some(&record.result_step_id),
            _ => None,
        })
        .unwrap();
    let root = seed
        .steps
        .iter()
        .find(|step| step.id() == original)
        .unwrap()
        .root_tree_id()
        .clone();
    let mut request = seed.clone();
    request.idempotency_key = IdempotencyKey::parse(uuid::Uuid::new_v4().to_string()).unwrap();
    request.steps = vec![Step::new(vec![parent], root).unwrap()];
    request.products.clear();
    request.view_mutation = None;
    request.known_event_cursor = None;
    let before = remote.bootstrap(&seed.space_id).unwrap();
    rejected(&http, origin, token, &mut request);
    let after = remote.bootstrap(&seed.space_id).unwrap();
    assert_eq!(
        before.current_repository_view,
        after.current_repository_view
    );
    assert_eq!(before.graph_root_step_ids, after.graph_root_step_ids);
}
fn upload(
    http: &reqwest::blocking::Client,
    origin: &str,
    token: &str,
    seed: &SyncTransactionRequest,
    kind: ObjectKind,
    id: ObjectId,
    bytes: Vec<u8>,
) {
    let request = ObjectPlanRequest {
        protocol: SyncV3Protocol::V3,
        message_type: ObjectPlanRequestType::Request,
        space_id: seed.space_id.clone(),
        client_id: seed.client_id.clone(),
        request_id: RequestId::parse(format!("request:{}", uuid::Uuid::new_v4())).unwrap(),
        objects: vec![ObjectDescriptor {
            kind,
            object_id: id,
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        }],
    };
    let response = http
        .post(format!("{origin}/v3/spaces/{}/object-plans", seed.space_id))
        .bearer_auth(token)
        .json(&request)
        .send()
        .unwrap();
    assert!(
        response.status().is_success(),
        "plan HTTP {}",
        response.status()
    );
    let plan: ObjectPlanResponse = response.json().unwrap();
    let capability = &plan.upload_capabilities[0];
    let response = http
        .put(&capability.upload_url)
        .bearer_auth(&capability.token)
        .body(bytes)
        .send()
        .unwrap();
    assert!(
        response.status().is_success(),
        "upload HTTP {}",
        response.status()
    );
}
