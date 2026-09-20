use super::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    routing::post,
};
use layrs_server::identity::BearerVerifier;
use layrs_server::webhooks::{WebhookPolicy, WebhookService, sign};
use layrs_store_server::v3::{PgCollaborationStore, ReviewProposalRequest, load_principal_context};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tower::ServiceExt;
#[derive(Clone)]
struct Receiver {
    secret: Arc<Mutex<Vec<u8>>>,
    deliveries: Arc<Mutex<Vec<String>>>,
    fail: Arc<AtomicBool>,
}
async fn receive(State(state): State<Receiver>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let timestamp = headers.get("x-layrs-timestamp").unwrap().to_str().unwrap();
    let id = headers.get("x-layrs-delivery").unwrap().to_str().unwrap();
    assert_eq!(
        headers.get("x-layrs-signature").unwrap().to_str().unwrap(),
        format!(
            "v1={}",
            sign(&state.secret.lock().unwrap(), timestamp, id, &body)
        )
    );
    let document: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(document["deliveryId"], id);
    assert_eq!(document["kind"], "review.proposed");
    state.deliveries.lock().unwrap().push(id.into());
    if state.fail.swap(false, Ordering::SeqCst) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::NO_CONTENT
    }
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 8192).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}
pub async fn run(
    repository: PgV3Repository,
    identity: Arc<PgIdentity>,
    admin: &str,
    space: &SpaceId,
    layer: &LayerId,
    step: &Step,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let ca = cert.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        ca.clone().into_bytes(),
        cert.key_pair.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let receiver = Receiver {
        secret: Arc::new(Mutex::new(vec![])),
        deliveries: Arc::new(Mutex::new(vec![])),
        fail: Arc::new(AtomicBool::new(true)),
    };
    let app = Router::new()
        .route("/events", post(receive))
        .route(
            "/redirect",
            post(|| async { axum::response::Redirect::temporary("https://127.0.0.1:1/forbidden") }),
        )
        .with_state(receiver.clone());
    let server = tokio::spawn(
        axum_server::from_tcp_rustls(listener.into_std().unwrap(), tls)
            .serve(app.into_make_service()),
    );
    let policy = WebhookPolicy {
        allowed_origins: BTreeSet::from([origin.clone()]),
        private_origins: BTreeSet::from([origin.clone()]),
        ca_pem: Some(ca.into_bytes()),
    };
    let key = vec![77; 32];
    let service = WebhookService::new(
        repository.clone(),
        identity.clone(),
        policy.clone(),
        key.clone(),
    )
    .unwrap();
    let routes = service.clone().router();
    let base = format!("/v3/spaces/{space}/webhooks");
    assert_eq!(
        call(
            &routes,
            "POST",
            &base,
            admin,
            json!({"url":"https://forbidden.example/events","eventKinds":["review.proposed"]})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, hook) = call(
        &routes,
        "POST",
        &base,
        admin,
        json!({"url":format!("{origin}/events"),"eventKinds":["review.proposed"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{hook}");
    *receiver.secret.lock().unwrap() = hex::decode(hook["secret"].as_str().unwrap()).unwrap();
    let hook_id = hook["webhookId"].as_str().unwrap();
    let owner = identity.verify_bearer(admin).await.unwrap();
    let principal = load_principal_context(repository.pool(), &owner)
        .await
        .unwrap();
    let store = PgCollaborationStore::new(repository.clone());
    let create = |key: &str| ReviewProposalRequest {
        idempotency_key: key.into(),
        layer_id: layer.clone(),
        proposed_step_id: step.id().clone(),
        title: "Webhook committed event".into(),
        participants: vec![],
        expected_version: 0,
    };
    store
        .propose_review(&principal, space, None, create("webhook-review-000001"))
        .await
        .unwrap();
    // Event is committed before the dispatcher exists again: restart cannot lose it.
    drop(service);
    let service = WebhookService::new(
        repository.clone(),
        identity.clone(),
        policy.clone(),
        key.clone(),
    )
    .unwrap();
    assert!(service.dispatch_once().await.unwrap());
    let pending = call(
        &routes,
        "GET",
        &format!("{base}/{hook_id}/deliveries"),
        admin,
        Value::Null,
    )
    .await
    .1;
    assert_eq!(pending["deliveries"][0]["state"], "pending");
    assert_eq!(pending["deliveries"][0]["lastStatus"], 503);
    sqlx::query("UPDATE layrs_collaboration.webhook_deliveries SET next_attempt_at=clock_timestamp()-interval '1 second'").execute(repository.pool()).await.unwrap();
    assert!(service.dispatch_once().await.unwrap());
    let delivered = call(
        &routes,
        "GET",
        &format!("{base}/{hook_id}/deliveries"),
        admin,
        Value::Null,
    )
    .await
    .1;
    assert_eq!(delivered["deliveries"][0]["state"], "delivered");
    assert_eq!(delivered["deliveries"][0]["attempts"], 2);
    let ids = receiver.deliveries.lock().unwrap().clone();
    assert_eq!(ids.len(), 2);
    assert_eq!(
        ids[0], ids[1],
        "retries preserve recipient deduplication identity"
    );
    let (status, rotated) = call(
        &routes,
        "POST",
        &format!("{base}/{hook_id}/rotate"),
        admin,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(rotated["secret"], hook["secret"]);
    *receiver.secret.lock().unwrap() = hex::decode(rotated["secret"].as_str().unwrap()).unwrap();
    assert_eq!(
        call(
            &routes,
            "POST",
            &format!("{base}/{hook_id}/replay"),
            admin,
            json!({"deliveryId":ids[0]})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(service.dispatch_once().await.unwrap());
    let list = call(&routes, "GET", &base, admin, Value::Null).await.1;
    // Simulate a worker loss after its durable claim but before recording success.
    sqlx::query("UPDATE layrs_collaboration.webhook_deliveries SET state='sending',lease_until=clock_timestamp()-interval '1 second' WHERE webhook_id=$1").bind(hook_id).execute(repository.pool()).await.unwrap();
    assert!(service.dispatch_once().await.unwrap());
    assert!(
        !list
            .to_string()
            .contains(rotated["secret"].as_str().unwrap())
    );
    // Private DNS/IP requires the separate explicit private-origin permission.
    store
        .propose_review(&principal, space, None, create("webhook-review-000002"))
        .await
        .unwrap();
    let mut public_only = policy.clone();
    public_only.private_origins.clear();
    let denied = WebhookService::new(
        repository.clone(),
        identity.clone(),
        public_only,
        key.clone(),
    )
    .unwrap();
    assert!(denied.dispatch_once().await.unwrap());
    assert_eq!(receiver.deliveries.lock().unwrap().len(), 4);
    assert_eq!(
        call(
            &routes,
            "POST",
            &format!("{base}/{hook_id}/disable"),
            admin,
            json!({})
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(!service.dispatch_once().await.unwrap());
    let (status, redirect) = call(
        &routes,
        "POST",
        &base,
        admin,
        json!({"url":format!("{origin}/redirect"),"eventKinds":["review.proposed"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    store
        .propose_review(&principal, space, None, create("webhook-review-000003"))
        .await
        .unwrap();
    assert!(service.dispatch_once().await.unwrap());
    let delivery = call(
        &routes,
        "GET",
        &format!(
            "{base}/{}/deliveries",
            redirect["webhookId"].as_str().unwrap()
        ),
        admin,
        Value::Null,
    )
    .await
    .1;
    assert_eq!(
        delivery["deliveries"][0]["lastStatus"], 307,
        "redirect must not be followed"
    );
    server.abort();
}
