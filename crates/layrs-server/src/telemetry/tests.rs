use super::*;
use axum::{body::Body, http::StatusCode, middleware};
use tower::ServiceExt;

#[tokio::test]
async fn request_ids_are_generated_and_metrics_have_bounded_labels() {
    let telemetry = Telemetry::default();
    let app = Router::new()
        .route(
            "/test/:object",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        )
        .layer(middleware::from_fn_with_state(telemetry.clone(), observe));
    let request = Request::builder()
        .uri("/test/private-name?token=never-log-me")
        .header("authorization", "Bearer secret")
        .header("x-request-id", "attacker-supplied")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let id = response.headers()["x-request-id"].to_str().unwrap();
    uuid::Uuid::parse_str(id).unwrap();
    assert_ne!(id, "attacker-supplied");
    let report = telemetry.snapshot();
    assert_eq!(report["http"]["completed"], 1);
    assert_eq!(report["http"]["inFlight"], 0);
    assert_eq!(report["http"]["statusClasses"][4]["count"], 1);
    let record = request_record(id, "GET", "/test/:object", 503, 10).to_string();
    for secret in [
        "private-name",
        "never-log-me",
        "attacker-supplied",
        "Bearer secret",
    ] {
        assert!(!record.contains(secret));
        assert!(!report.to_string().contains(secret));
    }
    let response = app
        .oneshot(
            Request::builder()
                .uri("/unmatched-private-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(telemetry.snapshot()["http"]["completed"], 2);
}

#[test]
fn readiness_histogram_and_interrupted_request_are_distinct() {
    let telemetry = Telemetry::default();
    {
        let _guard = telemetry.begin();
    }
    {
        let mut guard = telemetry.begin();
        guard.complete(200, 25_001);
    }
    telemetry.record_readiness(false, true);
    telemetry.record_readiness(true, false);
    let report = telemetry.snapshot();
    assert_eq!(report["readiness"]["checks"], 2);
    assert_eq!(report["readiness"]["databaseFailures"], 1);
    assert_eq!(report["readiness"]["casFailures"], 1);
    assert_eq!(report["http"]["interruptedBeforeHeaders"], 1);
    assert_eq!(report["http"]["inFlight"], 0);
    assert_eq!(report["http"]["headersLatencyBuckets"][1]["count"], 0);
    assert_eq!(report["http"]["headersLatencyBuckets"][2]["count"], 1);
}
