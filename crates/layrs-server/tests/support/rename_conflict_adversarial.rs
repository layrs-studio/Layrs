use layrs_client_core::native_scm::NativeRemote;
use layrs_core::scm::{
    ConflictId, FileId, ObjectId, RepoPathComponent, Step, StepId, Tree, TreeEntry, TreeEntryTarget,
};
use layrs_sync::v3::*;
use serde_json::Value;

fn rehash(value: &mut Value, field: &str, domain: &str) {
    value.as_object_mut().unwrap().remove(field);
    let mut hash = blake3::Hasher::new();
    hash.update(domain.as_bytes());
    hash.update(&[0]);
    hash.update(&serde_jcs::to_vec(value).unwrap());
    value[field] = format!("blake3:{}", hash.finalize()).into();
}
pub fn run(
    remote: &NativeRemote,
    origin: &str,
    token: &str,
    ca: &str,
    seed: &SyncTransactionRequest,
    document: &Value,
) {
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let before = remote.bootstrap(&seed.space_id).unwrap();
    let parent = seed
        .products
        .iter()
        .find_map(|p| match p {
            ProductEnvelope::Weave { record, .. } => Some(record.result_step_id.clone()),
            _ => None,
        })
        .unwrap();
    for case in 0..4 {
        let mut document = document.clone();
        if case == 2 {
            // Keep every intrinsic hash valid while lying about a base File.
            let value = document["bases"][0]["values"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|v| v["value"]["kind"] == "file")
                .unwrap();
            value["value"]["objectId"] = FileId::from_bytes(b"changed").as_str().into();
            rehash(
                &mut document["bases"][0],
                "termId",
                "layrs-conflict-term-v1",
            );
            rehash(&mut document, "conflictId", "layrs-conflict-v1");
        }
        let parsed = layrs_weave::graph_merge::PlannedConflict::from_canonical_bytes(
            &serde_jcs::to_vec(&document).unwrap(),
        )
        .unwrap();
        let conflict = ConflictId::parse(document["conflictId"].as_str().unwrap()).unwrap();
        if case == 2 {
            upload(
                &http,
                origin,
                token,
                seed,
                ObjectKind::Conflict,
                ObjectId::parse(conflict.as_str()).unwrap(),
                serde_jcs::to_vec(&document).unwrap(),
            );
        }
        let mut paths = parsed
            .anchor_paths()
            .unwrap()
            .into_iter()
            .map(|p| p.as_str().to_owned())
            .collect::<Vec<_>>();
        if case == 0 {
            paths.pop();
        } // A single surviving anchor may not stand for all placements.
        if case == 1 {
            paths[0] = "wrong".into();
        }
        let tree = Tree::new(
            paths
                .into_iter()
                .map(|p| {
                    TreeEntry::new(
                        RepoPathComponent::parse(p).unwrap(),
                        TreeEntryTarget::conflict(conflict.clone()),
                        false,
                    )
                })
                .collect(),
        )
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
        let parent = if case == 3 {
            StepId::parse(
                document["bases"][0]["provenance"]["stepId"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap()
        } else {
            parent.clone()
        };
        let mut request = seed.clone();
        request.idempotency_key = IdempotencyKey::parse(uuid::Uuid::new_v4().to_string()).unwrap();
        request.steps = vec![Step::new(vec![parent], tree.id().clone()).unwrap()];
        request.products.clear();
        request.view_mutation = None;
        request.known_event_cursor = None;
        request.request_digest = request.computed_request_digest().unwrap();
        let post = || {
            http.post(format!("{origin}/v3/spaces/{}/transactions", seed.space_id))
                .bearer_auth(token)
                .json(&request)
                .send()
                .unwrap()
        };
        let response = post();
        assert!(
            response.status().is_success(),
            "case {case}: {}",
            response.status()
        );
        let bytes = response.bytes().unwrap();
        let receipt: SyncTransactionReceipt = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            receipt.status,
            ReceiptStatus::Rejected,
            "case {case}: {:?}",
            receipt.rejections
        );
        assert!(receipt.accepted_step_ids.is_empty() && receipt.accepted_products.is_empty());
        assert_eq!(bytes, post().bytes().unwrap());
    }
    let after = remote.bootstrap(&seed.space_id).unwrap();
    assert_eq!(
        before.current_repository_view,
        after.current_repository_view
    );
    assert_eq!(before.graph_root_step_ids, after.graph_root_step_ids);
    assert_eq!(before.product_roots, after.product_roots);
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
    assert!(response.status().is_success());
    let plan: ObjectPlanResponse = response.json().unwrap();
    if let Some(cap) = plan.upload_capabilities.first() {
        let response = http
            .put(&cap.upload_url)
            .bearer_auth(&cap.token)
            .body(bytes)
            .send()
            .unwrap();
        assert!(response.status().is_success());
    }
}
