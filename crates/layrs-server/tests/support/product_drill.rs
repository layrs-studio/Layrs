use layrs_sync::v3::*;

pub fn run(origin: &str, token: &str, ca: &str, original: &serde_json::Value) {
    let client = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let original: SyncTransactionRequest = serde_json::from_value(original.clone()).unwrap();
    let marker = original
        .products
        .iter()
        .find(|p| matches!(p, ProductEnvelope::Marker { .. }))
        .unwrap();
    for case in 0..3 {
        let mut value = serde_json::to_value(marker).unwrap();
        let expected = match case {
            0 => {
                value["record"]["name"] = "forged immutable name".into();
                RejectionCode::DuplicateIdentity
            }
            1 => {
                let id = format!("marker:{}", uuid::Uuid::new_v4());
                value["productId"] = id.clone().into();
                value["record"]["markerId"] = id.into();
                let step = format!("blake3:{}", "a".repeat(64));
                value["record"]["targetStepId"] = step.clone().into();
                value["dependsOnStepIds"] = serde_json::json!([step]);
                RejectionCode::MissingParent
            }
            _ => {
                let id = format!("marker:{}", uuid::Uuid::new_v4());
                value["productId"] = id.clone().into();
                value["record"]["markerId"] = id.into();
                let proof = format!("proof:{}", uuid::Uuid::new_v4());
                value["record"]["proofIds"] = serde_json::json!([proof]);
                value["dependsOnProductIds"] = serde_json::json!([proof]);
                RejectionCode::MissingObject
            }
        };
        let mut product: ProductEnvelope = serde_json::from_value(value).unwrap();
        let computed = product.computed_object_id().unwrap();
        if let ProductEnvelope::Marker {
            product_object_id, ..
        } = &mut product
        {
            *product_object_id = computed;
        }
        let mut request = original.clone();
        request.idempotency_key =
            IdempotencyKey::parse(format!("idempotency:{}", uuid::Uuid::new_v4())).unwrap();
        request.steps.clear();
        request.products = vec![product];
        request.view_mutation = None;
        request.request_digest = request.computed_request_digest().unwrap();
        let post = || {
            let response = client
                .post(format!(
                    "{origin}/v3/spaces/{}/transactions",
                    request.space_id
                ))
                .bearer_auth(token)
                .json(&request)
                .send()
                .unwrap();
            assert!(
                response.status().is_success(),
                "case {case}: HTTP {}",
                response.status()
            );
            response.bytes().unwrap()
        };
        let bytes = post();
        let receipt: SyncTransactionReceipt = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(receipt.status, ReceiptStatus::Rejected);
        assert!(
            receipt.rejections.iter().any(|r| r.code == expected),
            "{:?}",
            receipt.rejections
        );
        assert!(receipt.accepted_products.is_empty() && receipt.accepted_step_ids.is_empty());
        assert_eq!(bytes, post(), "rejected receipt must replay byte for byte");
    }
}

pub fn protected_releases(
    origin: &str,
    token: &str,
    ca: &str,
    original: &serde_json::Value,
    proved: &layrs_core::scm::StepId,
    unproved: &layrs_core::scm::StepId,
) {
    let client = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let original: SyncTransactionRequest = serde_json::from_value(original.clone()).unwrap();
    let release = original
        .products
        .iter()
        .find(|p| matches!(p, ProductEnvelope::Release { .. }))
        .unwrap();
    for (step, expected) in [
        (unproved, ReceiptStatus::Rejected),
        (proved, ReceiptStatus::Applied),
    ] {
        let mut value = serde_json::to_value(release).unwrap();
        let id = format!("release:{}", uuid::Uuid::new_v4());
        value["productId"] = id.clone().into();
        value["record"]["releaseId"] = id.into();
        value["record"]["targetStepId"] = step.as_str().into();
        value["dependsOnStepIds"] = serde_json::json!([step]);
        let mut product: ProductEnvelope = serde_json::from_value(value).unwrap();
        let computed = product.computed_object_id().unwrap();
        if let ProductEnvelope::Release {
            product_object_id, ..
        } = &mut product
        {
            *product_object_id = computed;
        }
        let mut request = original.clone();
        request.idempotency_key =
            IdempotencyKey::parse(format!("idempotency:{}", uuid::Uuid::new_v4())).unwrap();
        request.steps.clear();
        request.products = vec![product];
        request.view_mutation = None;
        request.known_event_cursor = None;
        request.request_digest = request.computed_request_digest().unwrap();
        let response = client
            .post(format!(
                "{origin}/v3/spaces/{}/transactions",
                request.space_id
            ))
            .bearer_auth(token)
            .json(&request)
            .send()
            .unwrap();
        assert!(
            response.status().is_success(),
            "Release HTTP {}",
            response.status()
        );
        let receipt: SyncTransactionReceipt = response.json().unwrap();
        assert_eq!(receipt.status, expected, "{:?}", receipt.rejections);
        if expected == ReceiptStatus::Rejected {
            assert!(
                receipt
                    .rejections
                    .iter()
                    .any(|r| r.code == RejectionCode::GateRejected)
            );
            assert!(receipt.accepted_products.is_empty());
        } else {
            assert_eq!(receipt.accepted_products.len(), 1);
        }
    }
}
