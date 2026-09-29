//! A published squash is admitted separately, then selected by authorized Choose.
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::IntoResponse,
};
use layrs_client_core::native_scm::{NativeChoosePreview, NativeRemote, NativeWorkspace};
use layrs_core::scm::StepId;
use layrs_store_local::scm_v1::checkout::CheckoutContent;
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus, SyncTransactionReceipt};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Default)]
pub struct LostResponse {
    armed: AtomicBool,
    receipt: Mutex<Option<SyncTransactionReceipt>>,
}
pub fn wrap(app: Router) -> (Router, Arc<LostResponse>) {
    let probe = Arc::new(LostResponse::default());
    let state = probe.clone();
    let app = app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let state = state.clone();
        async move {
            let lose = request.method() == axum::http::Method::POST
                && request.uri().path().ends_with("/transactions")
                && state.armed.swap(false, Ordering::AcqRel);
            let response = next.run(request).await;
            if lose {
                assert!(response.status().is_success());
                let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
                    .await
                    .unwrap();
                *state.receipt.lock().unwrap() = Some(serde_json::from_slice(&bytes).unwrap());
                axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
            } else {
                response
            }
        }
    }));
    (app, probe)
}
fn head(workspace: &NativeWorkspace) -> StepId {
    workspace.state().unwrap().view.layer_heads()[0].step_ids()[0].clone()
}
fn contents(remote: &NativeRemote, space: &layrs_core::scm::SpaceId, step: &StepId) -> Vec<u8> {
    let files = remote.fetch_step_files(space, step).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "history.txt");
    match &files[0].content {
        CheckoutContent::File { bytes, .. } => bytes.clone(),
        _ => panic!("expected file"),
    }
}
pub fn run(
    remote: &NativeRemote,
    root: &Path,
    probe: &LostResponse,
    origin: &str,
    token: &str,
    ca: &str,
) {
    let aroot = root.join("published-rewrite-a");
    let broot = root.join("published-rewrite-b");
    let croot = root.join("published-rewrite-c");
    for path in [&aroot, &broot, &croot] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(aroot.join("history.txt"), b"baseline").unwrap();
    let a = NativeWorkspace::initialize(&aroot, "Published rewrite").unwrap();
    let project = remote.provision(&a, "published-rewrite", "main").unwrap();
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();
    let account:serde_json::Value=http.post(format!("{origin}/v3/auth/accounts"))
        .bearer_auth(token).json(&serde_json::json!({"login":"rewrite-member@example.invalid","password":"rewrite member fixture password phrase","displayName":"Rewrite member"}))
        .send().unwrap().error_for_status().unwrap().json().unwrap();
    let member_session = NativeRemote::login(
        origin,
        "rewrite-member@example.invalid",
        "rewrite member fixture password phrase",
        Some(ca.as_bytes()),
    )
    .unwrap();
    let member =
        NativeRemote::with_trusted_ca(origin, &member_session.access_token, ca.as_bytes()).unwrap();
    assert!(member.bootstrap(&project.space_id).is_err());
    http.post(format!("{origin}/v3/spaces/{}/members", project.space_id))
        .bearer_auth(token)
        .json(&serde_json::json!({"principalId":account["principalId"],"role":"member"}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        remote
            .publish(&a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    std::fs::write(aroot.join("history.txt"), b"first historical state").unwrap();
    a.capture().unwrap();
    let first = head(&a);
    let marker = a
        .create_marker("before-squash", "Retain exact old target")
        .unwrap();
    let release = a.create_release("1.0.0", "Immutable old target").unwrap();
    std::fs::write(aroot.join("history.txt"), b"second historical state").unwrap();
    a.capture().unwrap();
    let second = head(&a);
    assert_eq!(
        remote
            .publish(&a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let b = remote
        .clone_into(&broot, &project.space_id, "Before published rewrite")
        .unwrap();
    let before = remote.bootstrap(&project.space_id).unwrap();
    let plan = a.prepare_squash(&first).unwrap();
    assert_eq!(plan.summary().rewrite_map.len(), 2);
    let rewritten = a.apply_rewrite(&plan).unwrap();
    assert_ne!(rewritten, second);
    assert_eq!(
        remote
            .publish(&a, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Rejected
    );
    assert!(
        remote
            .preview_choose(
                &project.space_id,
                &project.layer_id,
                std::slice::from_ref(&rewritten),
                "Not yet admitted"
            )
            .is_err()
    );
    // Change authorization after the local binding was captured. A new
    // candidate-only intent must bootstrap fresh authority without reanchoring
    // any remote reference or replacing an already persisted pending intent.
    http.post(format!("{origin}/v3/spaces/{}/members", project.space_id))
        .bearer_auth(token)
        .json(&serde_json::json!({"principalId":account["principalId"],"role":"member"}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap();
    assert!(
        remote
            .bootstrap(&project.space_id)
            .unwrap()
            .authorization_epoch
            > before.authorization_epoch
    );
    // The server commits candidate admission, while the client loses that receipt.
    probe.armed.store(true, Ordering::Release);
    assert!(remote.publish_candidates(&a).is_err());
    let lost = probe
        .receipt
        .lock()
        .unwrap()
        .clone()
        .expect("actual server receipt");
    assert_eq!(lost.status, ReceiptStatus::Applied);
    assert!(lost.view_result.is_none());
    let journal = serde_json::from_slice::<serde_json::Value>(
        &std::fs::read(a.store_root().join("native-remote.json")).unwrap(),
    )
    .unwrap();
    assert!(!journal["pending"].is_null());
    let replay = remote.publish_candidates(&a).unwrap();
    assert_eq!(replay, lost);
    let admitted = remote.bootstrap(&project.space_id).unwrap();
    assert_eq!(
        admitted.current_repository_view,
        before.current_repository_view
    );
    assert_eq!(
        admitted.current_repository_view_generation,
        before.current_repository_view_generation
    );
    assert_eq!(admitted.markers, before.markers);
    assert_eq!(admitted.releases, before.releases);
    let unauthorized = member
        .preview_choose(
            &project.space_id,
            &project.layer_id,
            std::slice::from_ref(&rewritten),
            "Member must not replace published history",
        )
        .unwrap();
    let denied = member.apply_choose(&unauthorized).unwrap();
    assert_eq!(denied.status, ReceiptStatus::Rejected);
    assert!(
        denied
            .rejections
            .iter()
            .any(|r| r.code == layrs_sync::v3::RejectionCode::PermissionDenied)
    );
    assert_eq!(
        remote
            .bootstrap(&project.space_id)
            .unwrap()
            .current_repository_view,
        before.current_repository_view
    );
    let preview = remote
        .preview_choose(
            &project.space_id,
            &project.layer_id,
            std::slice::from_ref(&rewritten),
            "Replace published linear selection by its squash; retain history",
        )
        .unwrap();
    assert_eq!(preview.previous_heads, vec![second.clone()]);
    assert_eq!(preview.discarded_heads, vec![second.clone()]);
    let preview: NativeChoosePreview =
        serde_json::from_slice(&serde_json::to_vec(&preview).unwrap()).unwrap();
    let selected = remote.apply_choose(&preview).unwrap();
    assert_eq!(selected.status, ReceiptStatus::Applied);
    assert_eq!(remote.apply_choose(&preview).unwrap(), selected);
    let after = remote.bootstrap(&project.space_id).unwrap();
    assert_eq!(after.markers, before.markers);
    assert_eq!(after.releases, before.releases);
    assert_eq!(
        after.current_repository_view.record.layer_heads()[0].step_ids(),
        std::slice::from_ref(&rewritten)
    );
    // An existing client's divergent local head remains explicit, never erased by receive.
    remote.receive(&b).unwrap();
    assert_eq!(b.state().unwrap().view.layer_heads()[0].step_ids().len(), 2);
    assert_eq!(b.markers().unwrap(), vec![marker.clone()]);
    assert_eq!(b.releases().unwrap(), vec![release.clone()]);
    let c = member
        .clone_into(&croot, &project.space_id, "After published rewrite")
        .unwrap();
    assert_eq!(head(&c), rewritten);
    assert_eq!(c.markers().unwrap(), vec![marker]);
    assert_eq!(c.releases().unwrap(), vec![release]);
    assert_eq!(
        contents(&member, &project.space_id, &first),
        b"first historical state"
    );
    assert_eq!(
        contents(&member, &project.space_id, &second),
        b"second historical state"
    );
    let old = c.read_step_files(&first).unwrap();
    assert_eq!(old[0].path, "history.txt");
    assert!(
        matches!(&old[0].content,CheckoutContent::File{bytes,..} if bytes==b"first historical state")
    );
    eprintln!(
        "HTTPS published rewrite: squash, refused Advance, candidate admission with lost-response replay, authorized Choose/retry, old Steps and immutable named points retained across clients passed"
    );
}
