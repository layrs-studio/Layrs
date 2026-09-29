use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_core::scm::StepId;
use layrs_sync::v3::*;
use serde_json::json;
use std::path::Path;
fn head(workspace: &NativeWorkspace) -> StepId {
    workspace
        .state()
        .unwrap()
        .view
        .worktrees()
        .iter()
        .find(|w| w.worktree_id() == workspace.worktree_id())
        .unwrap()
        .base_step_id()
        .unwrap()
        .clone()
}
pub fn run(remote: &NativeRemote, root: &Path, origin: &str, token: &str, ca: &str) {
    let directory = root.join("fetch-pagination");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("file"), b"first").unwrap();
    let workspace = NativeWorkspace::initialize(&directory, "Fetch pages").unwrap();
    let first = head(&workspace);
    std::fs::write(directory.join("file"), b"second").unwrap();
    workspace.capture().unwrap();
    let second = head(&workspace);
    let project = remote
        .provision(&workspace, "fetch-pagination", "project")
        .unwrap();
    assert_eq!(
        remote
            .publish(&workspace, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let http = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();
    let url = format!("{origin}/v3/spaces/{}/fetch-plans", project.space_id);
    let claims= [&second,&first].map(|id|json!({"kind":"object","object":{"kind":"step","objectId":id},"root":{"kind":"step","stepId":second},"repoPath":null}));
    let mut request = json!({"protocol":"layrs.sync.v3","type":"fetchPlanRequest","spaceId":project.space_id,"clientId":"client:pagination-fixture","requestId":format!("request:{}",uuid::Uuid::new_v4()),"claims":claims,"pageCursor":null,"maxObjects":1});
    let response = http
        .post(&url)
        .bearer_auth(token)
        .json(&request)
        .send()
        .unwrap();
    assert!(
        response.status().is_success(),
        "first page {}",
        response.status()
    );
    let page: FetchPlanResponse = response.json().unwrap();
    assert!(page.has_more);
    assert_eq!(page.download_capabilities.len(), 1);
    let first_request = request.clone();
    request["pageCursor"] = json!(page.next_page_cursor);
    let response = http
        .post(&url)
        .bearer_auth(token)
        .json(&request)
        .send()
        .unwrap();
    assert!(
        response.status().is_success(),
        "second immutable fetch page {}",
        response.status()
    );
    let next: FetchPlanResponse = response.json().unwrap();
    assert!(!next.has_more);
    assert_eq!(next.download_capabilities.len(), 1);
    assert_eq!(next.plan_id, page.plan_id);
    assert_eq!(
        page.download_capabilities[0].descriptor.object_id.as_str(),
        second.as_str()
    );
    assert_eq!(
        next.download_capabilities[0].descriptor.object_id.as_str(),
        first.as_str()
    );
    for (request, expected) in [(&first_request, &page), (&request, &next)] {
        let replay: FetchPlanResponse = http
            .post(&url)
            .bearer_auth(token)
            .json(&request)
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(&replay, expected);
    }
    for capability in [
        &page.download_capabilities[0],
        &next.download_capabilities[0],
    ] {
        let bytes = http
            .get(&capability.download_url)
            .bearer_auth(&capability.token)
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .unwrap();
        let step: layrs_core::scm::Step = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(step.id().as_str(), capability.descriptor.object_id.as_str());
    }
    let mut altered = request.clone();
    altered["claims"][0]["root"]["stepId"] = json!(first);
    assert!(
        !http
            .post(&url)
            .bearer_auth(token)
            .json(&altered)
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    let mut altered = request.clone();
    altered["requestId"] = json!(format!("request:{}", uuid::Uuid::new_v4()));
    assert!(
        !http
            .post(&url)
            .bearer_auth(token)
            .json(&altered)
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    // A real policy mutation changes epoch even for the still-authorized owner.
    let account:serde_json::Value=http.post(format!("{origin}/v3/auth/accounts")).bearer_auth(token).json(&json!({"login":"fetch-pages-observer@example.invalid","password":"fetch pages observer fixture password","displayName":"Fetch observer"})).send().unwrap().error_for_status().unwrap().json().unwrap();
    http.post(format!("{origin}/v3/spaces/{}/members", project.space_id))
        .bearer_auth(token)
        .json(&json!({"principalId":account["principalId"],"role":"auditor"}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap();
    let stale = http
        .post(&url)
        .bearer_auth(token)
        .json(&request)
        .send()
        .unwrap();
    assert_eq!(stale.status(), reqwest::StatusCode::CONFLICT);
    let stale: serde_json::Value = stale.json().unwrap();
    assert_eq!(stale["error"]["code"], "authorization_epoch_changed");
    assert!(
        !http
            .get(&page.download_capabilities[0].download_url)
            .bearer_auth(&page.download_capabilities[0].token)
            .send()
            .unwrap()
            .status()
            .is_success()
    );
    let mut fresh = first_request;
    fresh["requestId"] = json!(format!("request:{}", uuid::Uuid::new_v4()));
    let fresh: FetchPlanResponse = http
        .post(&url)
        .bearer_auth(token)
        .json(&fresh)
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap();
    assert!(fresh.authorization_epoch > page.authorization_epoch);
    eprintln!(
        "TLS fetch pagination: two immutable pages, exact download/replay, cursor binding and real epoch revocation passed"
    );
}
