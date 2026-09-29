use super::*;
use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use layrs_core::scm::SpaceId;
use layrs_sync::v3::{
    FetchPlanRequest, FetchPlanResponse, ObjectPlanRequest, ObjectPlanResponse, PrincipalId,
    SpaceBootstrap, SpaceEventPage, SpaceEventPageRequest, SyncTransactionReceipt,
    SyncTransactionRequest, ValidateV3,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
};
use tower::ServiceExt;

const OBJECT_PLAN_REQUEST: &str =
    include_str!("../../../../docs/contracts/sync-v3/examples/object-plan-request.json");
const OBJECT_PLAN_RESPONSE: &str =
    include_str!("../../../../docs/contracts/sync-v3/examples/object-plan-response.json");
const FETCH_PLAN_REQUEST: &str =
    include_str!("../../../../docs/contracts/sync-v3/examples/fetch-plan-request.json");
const FETCH_PLAN_RESPONSE: &str =
    include_str!("../../../../docs/contracts/sync-v3/examples/fetch-plan-response.json");
const EVENT_REQUEST: &str =
    include_str!("../../../../docs/contracts/sync-v3/examples/event-page-request.json");
const EVENT_RESPONSE: &str =
    include_str!("../../../../docs/contracts/sync-v3/examples/event-page-response.json");
const TRANSACTION: &str =
    include_str!("../../../../docs/contracts/sync-v3/examples/transaction.json");
const RECEIPT: &str = include_str!("../../../../docs/contracts/sync-v3/examples/receipt.json");

#[derive(Default)]
struct MockFacade {
    upload_chunks: Mutex<Vec<usize>>,
    capabilities: Mutex<Vec<String>>,
    early_upload_return: AtomicBool,
    upload_length_override: Mutex<Option<u64>>,
    download_mode: AtomicU8,
    transaction_calls: AtomicUsize,
    last_receipt_jcs: Mutex<Vec<u8>>,
    mismatch_object_plan_principal: AtomicBool,
}

impl MockFacade {
    fn principal() -> AuthenticatedPrincipal {
        AuthenticatedPrincipal {
            principal_id: PrincipalId::parse("principal:alexa").expect("test principal"),
        }
    }
}

#[async_trait]
impl V3Facade for MockFacade {
    async fn authenticate_bearer(
        &self,
        bearer_token: &str,
    ) -> Result<AuthenticatedPrincipal, V3ServiceError> {
        if bearer_token == "principal-secret" {
            Ok(Self::principal())
        } else {
            Err(V3ServiceError::Unauthorized)
        }
    }

    async fn bootstrap(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space_id: &SpaceId,
    ) -> Result<SpaceBootstrap, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn object_plan(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: ObjectPlanRequest,
    ) -> Result<ObjectPlanResponse, V3ServiceError> {
        let mut response: ObjectPlanResponse =
            serde_json::from_str(OBJECT_PLAN_RESPONSE).map_err(|_| V3ServiceError::Internal)?;
        if self.mismatch_object_plan_principal.load(Ordering::Acquire) {
            response.principal_id =
                PrincipalId::parse("principal:other").map_err(|_| V3ServiceError::Internal)?;
        }
        Ok(response)
    }

    async fn fetch_plan(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: FetchPlanRequest,
    ) -> Result<FetchPlanResponse, V3ServiceError> {
        serde_json::from_str(FETCH_PLAN_RESPONSE).map_err(|_| V3ServiceError::Internal)
    }

    async fn transact(
        &self,
        _principal: &AuthenticatedPrincipal,
        request: SyncTransactionRequest,
    ) -> Result<StoredTransactionReceipt, V3ServiceError> {
        self.transaction_calls.fetch_add(1, Ordering::AcqRel);
        let mut receipt: SyncTransactionReceipt =
            serde_json::from_str(RECEIPT).map_err(|_| V3ServiceError::Internal)?;
        receipt.space_id = request.space_id;
        receipt.client_id = request.client_id;
        receipt.idempotency_key = request.idempotency_key;
        receipt.request_digest = request.request_digest;
        let canonical = serde_jcs::to_vec(&receipt).map_err(|_| V3ServiceError::Internal)?;
        *self
            .last_receipt_jcs
            .lock()
            .map_err(|_| V3ServiceError::Internal)? = canonical.clone();
        StoredTransactionReceipt::new(receipt, Bytes::from(canonical))
    }

    async fn events(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: SpaceEventPageRequest,
    ) -> Result<SpaceEventPage, V3ServiceError> {
        serde_json::from_str(EVENT_RESPONSE).map_err(|_| V3ServiceError::Internal)
    }

    async fn upload(
        &self,
        capability_token: &str,
        declared_content_length: u64,
        mut body: UploadStream,
    ) -> Result<UploadResult, V3ServiceError> {
        self.capabilities
            .lock()
            .map_err(|_| V3ServiceError::Internal)?
            .push(capability_token.to_owned());
        if self.early_upload_return.load(Ordering::Acquire) {
            return Ok(UploadResult {
                disposition: UploadDisposition::Installed,
                encoded_len: declared_content_length,
            });
        }
        while let Some(bytes) = body.next().await {
            self.upload_chunks
                .lock()
                .map_err(|_| V3ServiceError::Internal)?
                .push(bytes?.len());
        }
        let encoded_len = self
            .upload_length_override
            .lock()
            .map_err(|_| V3ServiceError::Internal)?
            .unwrap_or(declared_content_length);
        Ok(UploadResult {
            disposition: UploadDisposition::Installed,
            encoded_len,
        })
    }

    async fn download(
        &self,
        capability_token: &str,
        requested_range: Option<RequestedRange>,
    ) -> Result<DownloadResult, V3ServiceError> {
        self.capabilities
            .lock()
            .map_err(|_| V3ServiceError::Internal)?
            .push(capability_token.to_owned());
        let bytes = b"abcdefghij";
        let mode = self.download_mode.load(Ordering::Acquire);
        let mut range = match requested_range {
            Some(requested) => resolve_range(requested, bytes.len() as u64).ok_or(
                V3ServiceError::RangeNotSatisfiable {
                    total_encoded_len: bytes.len() as u64,
                },
            )?,
            None => ByteRange::new(0, bytes.len() as u64).expect("full range"),
        };
        if mode == 1 {
            range = ByteRange::new(0, bytes.len() as u64).expect("wrong full range");
        } else if mode == 2 {
            range = ByteRange::new(0, bytes.len() as u64 + 1).expect("wrong long range");
        }
        let body: DownloadStream = if mode == 3 {
            Box::pin(stream::iter([Err(V3ServiceError::Internal)]))
        } else {
            let end = range.end_exclusive.min(bytes.len() as u64) as usize;
            let start = range.start.min(end as u64) as usize;
            Box::pin(stream::iter([Ok(Bytes::copy_from_slice(
                &bytes[start..end],
            ))]))
        };
        Ok(DownloadResult {
            total_encoded_len: bytes.len() as u64,
            served_range: range,
            content_type: "application/octet-stream",
            body,
        })
    }
}

fn test_router(facade: Arc<dyn V3Facade>, limits: V3HttpLimits) -> axum::Router {
    router(V3HttpState::new(facade, limits))
}

fn request(method: &str, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .expect("test request")
}

fn authorize(mut request: Request<Body>, token: &str) -> Request<Body> {
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().expect("authorization"),
    );
    request
}

fn json_request(method: &str, uri: &str, body: impl Into<Body>) -> Request<Body> {
    let mut request = authorize(request(method, uri, body.into()), "principal-secret");
    request
        .headers_mut()
        .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    request
}

fn upload_request(body: Body, declared_length: Option<u64>) -> Request<Body> {
    let mut request = authorize(
        request("PUT", "/v3/transfers/uploads", body),
        "capability-secret",
    );
    if let Some(length) = declared_length {
        request
            .headers_mut()
            .insert(header::CONTENT_LENGTH, length.into());
    }
    request
}

fn transaction_request() -> SyncTransactionRequest {
    let mut request: SyncTransactionRequest =
        serde_json::from_str(TRANSACTION).expect("transaction fixture");
    request.request_digest = request
        .computed_request_digest()
        .expect("transaction digest");
    request
}

async fn response_body(response: axum::response::Response) -> Bytes {
    to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body")
}

#[tokio::test]
async fn all_seven_routes_have_exact_methods_and_no_v1_mount() {
    let facade = Arc::new(MockFacade::default());
    let app = test_router(facade, V3HttpLimits::default());
    let transaction = serde_json::to_vec(&transaction_request()).unwrap();
    let calls = vec![
        authorize(
            request("GET", "/v3/spaces/space:demo/bootstrap", Body::empty()),
            "principal-secret",
        ),
        json_request(
            "POST",
            "/v3/spaces/space:demo/object-plans",
            OBJECT_PLAN_REQUEST,
        ),
        json_request(
            "POST",
            "/v3/spaces/space:demo/fetch-plans",
            FETCH_PLAN_REQUEST,
        ),
        json_request("POST", "/v3/spaces/space:demo/transactions", transaction),
        json_request("POST", "/v3/spaces/space:demo/events/page", EVENT_REQUEST),
        upload_request(Body::from("abcd"), Some(4)),
        authorize(
            request("GET", "/v3/transfers/downloads", Body::empty()),
            "capability-secret",
        ),
    ];
    let expected_statuses = [
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
    ];
    for (index, (call, expected)) in calls.into_iter().zip(expected_statuses).enumerate() {
        let response = app.clone().oneshot(call).await.unwrap();
        assert_eq!(response.status(), expected, "route index {index}");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }

    for uri in [
        "/v3/spaces/space:demo/bootstrap",
        "/v3/spaces/space:demo/object-plans",
        "/v3/spaces/space:demo/fetch-plans",
        "/v3/spaces/space:demo/transactions",
        "/v3/spaces/space:demo/events/page",
        "/v3/transfers/uploads",
        "/v3/transfers/downloads",
    ] {
        let response = app
            .clone()
            .oneshot(request("DELETE", uri, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
    assert_eq!(
        app.clone()
            .oneshot(request("GET", "/v1/routes", Body::empty()))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.oneshot(request("GET", "/v3/unknown", Body::empty()))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn unavailable_facade_fails_closed_on_every_route() {
    let app = test_router(Arc::new(UnavailableV3Facade), V3HttpLimits::default());
    let transaction = serde_json::to_vec(&transaction_request()).unwrap();
    let calls = vec![
        authorize(
            request("GET", "/v3/spaces/space:demo/bootstrap", Body::empty()),
            "principal-secret",
        ),
        json_request(
            "POST",
            "/v3/spaces/space:demo/object-plans",
            OBJECT_PLAN_REQUEST,
        ),
        json_request(
            "POST",
            "/v3/spaces/space:demo/fetch-plans",
            FETCH_PLAN_REQUEST,
        ),
        json_request("POST", "/v3/spaces/space:demo/transactions", transaction),
        json_request("POST", "/v3/spaces/space:demo/events/page", EVENT_REQUEST),
        upload_request(Body::empty(), Some(0)),
        authorize(
            request("GET", "/v3/transfers/downloads", Body::empty()),
            "capability-secret",
        ),
    ];
    for call in calls {
        assert_eq!(
            app.clone().oneshot(call).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

#[tokio::test]
async fn auth_is_strict_no_store_and_never_reflects_tokens() {
    let app = test_router(Arc::new(MockFacade::default()), V3HttpLimits::default());
    let missing = app
        .clone()
        .oneshot(request(
            "GET",
            "/v3/spaces/space:demo/bootstrap",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(missing.headers()[header::WWW_AUTHENTICATE], "Bearer");
    assert_eq!(missing.headers()[header::CACHE_CONTROL], "no-store");
    assert!(!String::from_utf8_lossy(&response_body(missing).await).contains("secret"));

    let mut duplicate = authorize(
        request("GET", "/v3/spaces/space:demo/bootstrap", Body::empty()),
        "principal-secret",
    );
    duplicate
        .headers_mut()
        .append(header::AUTHORIZATION, "Bearer second".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(duplicate).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    let transfer = app
        .oneshot(request("GET", "/v3/transfers/downloads", Body::empty()))
        .await
        .unwrap();
    assert_eq!(transfer.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(transfer.headers()[header::WWW_AUTHENTICATE], "Bearer");
}

#[tokio::test]
async fn json_media_type_limits_space_and_body_read_errors_are_distinct() {
    let limits = V3HttpLimits {
        object_plan_json_bytes: 2048,
        ..V3HttpLimits::default()
    };
    let app = test_router(Arc::new(MockFacade::default()), limits);
    let no_type = authorize(
        request(
            "POST",
            "/v3/spaces/space:demo/object-plans",
            Body::from(OBJECT_PLAN_REQUEST),
        ),
        "principal-secret",
    );
    assert_eq!(
        app.clone().oneshot(no_type).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    let mismatch = OBJECT_PLAN_REQUEST.replace("space:demo", "space:other");
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v3/spaces/space:demo/object-plans",
            mismatch,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let oversized = format!("{{\"padding\":\"{}\"}}", "x".repeat(2200));
    assert_eq!(
        app.clone()
            .oneshot(json_request(
                "POST",
                "/v3/spaces/space:demo/object-plans",
                oversized,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );

    let broken = stream::iter([Err::<Bytes, _>(std::io::Error::other("broken"))]);
    let response = app
        .oneshot(json_request(
            "POST",
            "/v3/spaces/space:demo/object-plans",
            Body::from_stream(broken),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&response_body(response).await).contains("body_read_error"));
}

#[tokio::test]
async fn semantic_transaction_reaches_engine_and_returns_exact_stored_jcs() {
    let facade = Arc::new(MockFacade::default());
    let app = test_router(facade.clone(), V3HttpLimits::default());
    let mut request = transaction_request();
    let mutation = request.view_mutation.as_mut().expect("view mutation");
    mutation
        .layer_mutations
        .push(mutation.layer_mutations[0].clone());
    request.request_digest = request.computed_request_digest().unwrap();
    assert!(
        request.validate_v3().is_err(),
        "fixture must be semantically invalid"
    );

    let response = app
        .oneshot(json_request(
            "POST",
            "/v3/spaces/space:demo/transactions",
            serde_json::to_vec(&request).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(facade.transaction_calls.load(Ordering::Acquire), 1);
    let expected = facade.last_receipt_jcs.lock().unwrap().clone();
    assert_eq!(
        response.headers()[header::CONTENT_LENGTH]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        expected.len()
    );
    assert_eq!(response_body(response).await.as_ref(), expected.as_slice());
}

#[tokio::test]
async fn transaction_digest_mismatch_is_transport_rejection() {
    let facade = Arc::new(MockFacade::default());
    let app = test_router(facade.clone(), V3HttpLimits::default());
    let mut request = transaction_request();
    request.request_digest =
        layrs_core::scm::ObjectId::parse(format!("blake3:{}", "f".repeat(64))).unwrap();
    let response = app
        .oneshot(json_request(
            "POST",
            "/v3/spaces/space:demo/transactions",
            serde_json::to_vec(&request).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(facade.transaction_calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn mismatched_service_response_is_not_serialized() {
    let facade = Arc::new(MockFacade::default());
    facade
        .mismatch_object_plan_principal
        .store(true, Ordering::Release);
    let response = test_router(facade, V3HttpLimits::default())
        .oneshot(json_request(
            "POST",
            "/v3/spaces/space:demo/object-plans",
            OBJECT_PLAN_REQUEST,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn upload_requires_complete_exact_length_stream() {
    let facade = Arc::new(MockFacade::default());
    let app = test_router(facade.clone(), V3HttpLimits::default());

    assert_eq!(
        app.clone()
            .oneshot(upload_request(Body::from("abc"), None))
            .await
            .unwrap()
            .status(),
        StatusCode::LENGTH_REQUIRED
    );
    assert_eq!(
        app.clone()
            .oneshot(upload_request(Body::from("abc"), Some(5)))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.clone()
            .oneshot(upload_request(Body::from("abc"), Some(2)))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.clone()
            .oneshot(upload_request(Body::empty(), Some(0)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    facade.early_upload_return.store(true, Ordering::Release);
    assert_eq!(
        app.clone()
            .oneshot(upload_request(Body::from("abc"), Some(3)))
            .await
            .unwrap()
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    facade.early_upload_return.store(false, Ordering::Release);
    *facade.upload_length_override.lock().unwrap() = Some(4);
    assert_eq!(
        app.oneshot(upload_request(Body::from("abc"), Some(3)))
            .await
            .unwrap()
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn upload_is_streamed_and_distinguishes_interrupt_from_limit() {
    let facade = Arc::new(MockFacade::default());
    let limits = V3HttpLimits {
        upload_encoded_bytes: 6,
        ..V3HttpLimits::default()
    };
    let app = test_router(facade.clone(), limits);
    let chunks = stream::iter([
        Ok::<_, std::convert::Infallible>(Bytes::from_static(b"abc")),
        Ok(Bytes::from_static(b"def")),
    ]);
    assert_eq!(
        app.clone()
            .oneshot(upload_request(Body::from_stream(chunks), Some(6)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(*facade.upload_chunks.lock().unwrap(), vec![3, 3]);

    let broken = stream::iter([Err::<Bytes, _>(std::io::Error::other("broken"))]);
    let response = app
        .clone()
        .oneshot(upload_request(Body::from_stream(broken), Some(1)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&response_body(response).await).contains("body_read_error"));

    assert_eq!(
        app.oneshot(upload_request(Body::from("1234567"), Some(7)))
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn upload_refuses_content_range_queries_and_duplicate_auth() {
    let app = test_router(Arc::new(MockFacade::default()), V3HttpLimits::default());
    let mut ranged = upload_request(Body::from("abc"), Some(3));
    ranged
        .headers_mut()
        .insert(header::CONTENT_RANGE, "bytes 0-2/3".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(ranged).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let query = authorize(
        request("PUT", "/v3/transfers/uploads?token=secret", Body::empty()),
        "capability-secret",
    );
    assert_eq!(
        app.clone().oneshot(query).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let mut duplicate = upload_request(Body::empty(), Some(0));
    duplicate
        .headers_mut()
        .append(header::AUTHORIZATION, "Bearer second".parse().unwrap());
    assert_eq!(
        app.oneshot(duplicate).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn ranges_cover_suffix_duplicates_unsatisfied_and_service_incoherence() {
    let facade = Arc::new(MockFacade::default());
    let app = test_router(facade.clone(), V3HttpLimits::default());
    let mut suffix = authorize(
        request("GET", "/v3/transfers/downloads", Body::empty()),
        "capability-secret",
    );
    suffix
        .headers_mut()
        .insert(header::RANGE, "bytes=-3".parse().unwrap());
    let response = app.clone().oneshot(suffix).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response_body(response).await, Bytes::from_static(b"hij"));

    let mut duplicate = authorize(
        request("GET", "/v3/transfers/downloads", Body::empty()),
        "capability-secret",
    );
    duplicate
        .headers_mut()
        .append(header::RANGE, "bytes=0-1".parse().unwrap());
    duplicate
        .headers_mut()
        .append(header::RANGE, "bytes=2-3".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(duplicate).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    let mut unsatisfied = authorize(
        request("GET", "/v3/transfers/downloads", Body::empty()),
        "capability-secret",
    );
    unsatisfied
        .headers_mut()
        .insert(header::RANGE, "bytes=99-".parse().unwrap());
    let response = app.clone().oneshot(unsatisfied).await.unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */10");

    facade.download_mode.store(1, Ordering::Release);
    let mut inconsistent = authorize(
        request("GET", "/v3/transfers/downloads", Body::empty()),
        "capability-secret",
    );
    inconsistent
        .headers_mut()
        .insert(header::RANGE, "bytes=2-4".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(inconsistent).await.unwrap().status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    facade.download_mode.store(2, Ordering::Release);
    assert_eq!(
        app.oneshot(authorize(
            request("GET", "/v3/transfers/downloads", Body::empty()),
            "capability-secret"
        ))
        .await
        .unwrap()
        .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn download_stream_failure_is_not_silently_truncated() {
    let facade = Arc::new(MockFacade::default());
    facade.download_mode.store(3, Ordering::Release);
    let response = test_router(facade, V3HttpLimits::default())
        .oneshot(authorize(
            request("GET", "/v3/transfers/downloads", Body::empty()),
            "capability-secret",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(to_bytes(response.into_body(), 32).await.is_err());
}
