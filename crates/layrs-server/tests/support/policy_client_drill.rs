//! Real TLS operator preview/apply, uncertain result recovery and exact managed restore.
#[path = "policy_replacement_drill.rs"]
mod replacement;
use layrs_client_core::native_scm::{NativePolicyPreview, NativeRemote, NativeWorkspace};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus, RejectionCode};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

fn call(
    http: &reqwest::blocking::Client,
    method: &str,
    origin: &str,
    path: &str,
    token: &str,
    body: Value,
) -> Value {
    let response = http
        .request(method.parse().unwrap(), format!("{origin}{path}"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .unwrap();
    let status = response.status();
    let value: Value = response.json().unwrap();
    assert!(status.is_success(), "HTTP policy fixture {path}: {status}");
    value
}
pub fn run(
    remote: &NativeRemote,
    root: &Path,
    origin: &str,
    token: &str,
    ca: &str,
    lose: &AtomicBool,
) {
    let aroot = root.join("policy-client-a");
    let broot = root.join("policy-client-b");
    std::fs::create_dir_all(&aroot).unwrap();
    std::fs::create_dir_all(&broot).unwrap();
    std::fs::write(aroot.join("retained.txt"), b"immutable checkout bytes").unwrap();
    let a = NativeWorkspace::initialize(&aroot, "Policy operator").unwrap();
    let project = remote.provision(&a, "policy-client", "operator").unwrap();
    let initial = remote.bootstrap(&project.space_id).unwrap();
    assert_eq!(
        remote
            .publish(&a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let b = remote
        .clone_into(&broot, &project.space_id, "Other checkout")
        .unwrap();
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();
    let account = call(
        &http,
        "POST",
        origin,
        "/v3/auth/accounts",
        token,
        json!({"login":"policy-client-reader@example.com","password":"policy client long password phrase","displayName":"Policy client reader"}),
    );
    let base = format!("/v3/spaces/{}", project.space_id);
    call(
        &http,
        "POST",
        origin,
        &format!("{base}/members"),
        token,
        json!({"principalId":account["principalId"],"role":"member"}),
    );
    let managed = remote.bootstrap(&project.space_id).unwrap();
    assert_ne!(initial.current_policy_set_id, managed.current_policy_set_id);
    let before = a.state().unwrap();
    let before_b = b.state().unwrap();
    let journal_before = std::fs::read(a.store_root().join("native-remote.json")).unwrap();
    let preview = remote
        .preview_stored_policy(&a, &initial.current_policy_set_id)
        .unwrap();
    assert_eq!(
        std::fs::read(a.store_root().join("native-remote.json")).unwrap(),
        journal_before,
        "preview never writes pending"
    );
    assert!(!a.store_root().join("native-policy-intent.json").exists());
    assert_eq!(
        preview.expected_policy_set_id,
        managed.current_policy_set_id
    );
    let encoded = serde_json::to_vec(&preview).unwrap();
    assert!(!String::from_utf8_lossy(&encoded).contains(token));
    let preview: NativePolicyPreview = serde_json::from_slice(&encoded).unwrap();
    for index in 0..5 {
        let mut wrong = preview.clone();
        match index {
            0 => wrong.origin = "https://unrelated.example/".into(),
            1 => wrong.schema_version = "future".into(),
            2 => wrong.expected_authorization_epoch += 1,
            3 => wrong.store_root = "unrelated-store".into(),
            _ => {
                wrong.request.idempotency_key =
                    layrs_sync::v3::IdempotencyKey::parse("policy-client-tampered-key").unwrap()
            }
        }
        assert!(remote.apply_policy_preview(&a, &wrong).is_err());
    }
    assert!(remote.apply_policy_preview(&b, &preview).is_err());
    assert_eq!(
        std::fs::read(a.store_root().join("native-remote.json")).unwrap(),
        journal_before
    );
    lose.store(true, Ordering::SeqCst);
    assert!(
        remote.apply_policy_preview(&a, &preview).is_err(),
        "actual server commit response is discarded"
    );
    let uncertain: Value =
        serde_json::from_slice(&std::fs::read(a.store_root().join("native-remote.json")).unwrap())
            .unwrap();
    assert_eq!(
        uncertain["pending"]["requestDigest"],
        preview.request.request_digest.as_str()
    );
    assert!(a.store_root().join("native-policy-intent.json").exists());
    assert_eq!(
        remote
            .bootstrap(&project.space_id)
            .unwrap()
            .current_policy_set_id,
        initial.current_policy_set_id
    );
    assert_eq!(
        call(
            &http,
            "GET",
            origin,
            &format!("{base}/policy-management"),
            token,
            Value::Null
        )["mode"],
        "custom"
    );
    assert!(remote.publish(&a, LayerMutationMode::Advance).is_err());
    assert!(remote.receive(&a).is_err());
    assert!(
        remote
            .preview_stored_policy(&a, &managed.current_policy_set_id)
            .is_err()
    );
    let recovered = remote
        .preview_stored_policy(&a, &initial.current_policy_set_id)
        .unwrap();
    assert_eq!(
        serde_jcs::to_vec(&recovered).unwrap(),
        serde_jcs::to_vec(&preview).unwrap(),
        "recovery retains original epoch and request"
    );
    let account_session = NativeRemote::login(
        origin,
        "policy-client-reader@example.com",
        "policy client long password phrase",
        Some(ca.as_bytes()),
    )
    .unwrap();
    let other = NativeRemote::with_trusted_ca(origin, &account_session.access_token, ca.as_bytes())
        .unwrap();
    assert!(
        other.apply_policy_preview(&a, &preview).is_err(),
        "pending intent cannot move between accounts"
    );
    let receipt = remote.apply_policy_preview(&a, &recovered).unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Applied);
    assert_eq!(
        remote.apply_policy_preview(&a, &preview).unwrap(),
        receipt,
        "completed local replay is byte-identical too"
    );
    assert!(!a.store_root().join("native-policy-intent.json").exists());
    assert_eq!(a.state().unwrap().operation.id(), before.operation.id());
    assert_eq!(a.state().unwrap().view, before.view);
    assert_eq!(b.state().unwrap().operation.id(), before_b.operation.id());
    assert_eq!(
        std::fs::read(aroot.join("retained.txt")).unwrap(),
        b"immutable checkout bytes"
    );
    let restore = remote
        .preview_stored_policy(&a, &managed.current_policy_set_id)
        .unwrap();
    assert_eq!(
        remote.apply_policy_preview(&a, &restore).unwrap().status,
        ReceiptStatus::Applied
    );
    assert_eq!(
        call(
            &http,
            "GET",
            origin,
            &format!("{base}/policy-management"),
            token,
            Value::Null
        )["mode"],
        "managed"
    );
    let checkpoint = std::fs::read(a.store_root().join("native-remote.json")).unwrap();
    let rule_document = json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[{"effect":"deny","principalIds":[initial.principal_id],"actions":["object.plan_upload"]}]});
    let rule_bytes = serde_json::to_vec(&rule_document).unwrap();
    for malformed in [
        json!({"protocol":"layrs.sync.v3","steps":[]}),
        json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[],"injected":true}),
        json!({"schemaVersion":"future","rules":[]}),
    ] {
        assert!(
            remote
                .preview_policy_rules(&a, &serde_json::to_vec(&malformed).unwrap())
                .is_err()
        );
    }
    assert!(remote.preview_policy_rules(&a, &vec![b' '; 65537]).is_err());
    let inline = remote.preview_policy_rules(&a, &rule_bytes).unwrap();
    assert_eq!(inline.schema_version, "layrs.policy-root-preview.v2");
    assert_eq!(inline.request.products.len(), 1);
    assert_eq!(
        std::fs::read(a.store_root().join("native-remote.json")).unwrap(),
        checkpoint,
        "rules preview is read-only"
    );
    assert!(!a.store_root().join("native-policy-intent.json").exists());
    let mut tampered = inline.clone();
    tampered
        .inline_rules
        .as_mut()
        .unwrap()
        .base_policy_set
        .policy_versions
        .clear();
    assert!(remote.apply_policy_preview(&a, &tampered).is_err());
    let encoded = serde_json::to_vec(&inline).unwrap();
    let inline: NativePolicyPreview = serde_json::from_slice(&encoded).unwrap();
    lose.store(true, Ordering::SeqCst);
    assert!(remote.apply_policy_preview(&a, &inline).is_err());
    let pending: Value = serde_json::from_slice(
        &std::fs::read(a.store_root().join("native-policy-intent.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(pending["evidenceUploaded"], true);
    let recovered = remote.preview_policy_rules(&a, &rule_bytes).unwrap();
    assert_eq!(
        serde_jcs::to_vec(&recovered).unwrap(),
        serde_jcs::to_vec(&inline).unwrap()
    );
    let receipt = remote.apply_policy_preview(&a, &recovered).unwrap();
    assert_eq!(receipt.status, ReceiptStatus::Applied);
    assert_eq!(
        remote.apply_policy_preview(&a, &inline).unwrap(),
        receipt,
        "completed replay does not need revoked upload permission"
    );
    let restore_rules = remote
        .preview_stored_policy(&a, &managed.current_policy_set_id)
        .unwrap();
    assert_eq!(
        remote
            .apply_policy_preview(&a, &restore_rules)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let v2 = json!({"schemaVersion":"layrs.space-policy-rules.v2","rules":[],"contentPolicies":[{"principalIds":[account["principalId"]],"default":"deny","rules":[{"effect":"allow","selector":{"kind":"exact","path":"retained.txt"}}]}]});
    let preview_v2 = remote
        .preview_policy_rules(&a, &serde_json::to_vec(&v2).unwrap())
        .unwrap();
    assert_eq!(
        remote.apply_policy_preview(&a, &preview_v2).unwrap().status,
        ReceiptStatus::Applied
    );
    assert!(
        other
            .bootstrap(&project.space_id)
            .unwrap()
            .content_access
            .is_some()
    );
    let restore_rules = remote
        .preview_stored_policy(&a, &managed.current_policy_set_id)
        .unwrap();
    assert_eq!(
        remote
            .apply_policy_preview(&a, &restore_rules)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    assert!(
        other
            .bootstrap(&project.space_id)
            .unwrap()
            .content_access
            .is_none()
    );
    assert_eq!(a.state().unwrap().operation.id(), before.operation.id());
    replacement::run(remote, &other, &a, lose);
    let stale = remote
        .preview_stored_policy(&a, &initial.current_policy_set_id)
        .unwrap();
    call(
        &http,
        "POST",
        origin,
        &format!("{base}/members"),
        token,
        json!({"principalId":account["principalId"],"role":"auditor"}),
    );
    let rejection = remote.apply_policy_preview(&a, &stale).unwrap();
    assert_eq!(rejection.status, ReceiptStatus::Rejected);
    assert!(
        rejection
            .rejections
            .iter()
            .any(|r| r.code == RejectionCode::AuthorizationEpochChanged)
    );
    assert_eq!(remote.apply_policy_preview(&a, &stale).unwrap(), rejection);
    let journal: Value =
        serde_json::from_slice(&std::fs::read(a.store_root().join("native-remote.json")).unwrap())
            .unwrap();
    assert!(journal["pending"].is_null());
    assert_eq!(a.state().unwrap().operation.id(), before.operation.id());
    eprintln!(
        "HTTPS PolicyRoot client: read-only preview, scoped durable intent, lost response/reopen/exact retry, unchanged checkout, managed restore and stale refusal passed"
    );
}
