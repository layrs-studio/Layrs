//! Real chronological expiry, using the production signer and SQL grants.
//! Only this isolated fixture shortens a plan; production lifetime is unchanged.
use axum::{
    Router,
    extract::Request,
    middleware::{self, Next},
    response::Response,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_sync::v3::{FetchPlanResponse, LayerMutationMode, ReceiptStatus};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;

#[derive(Default)]
pub struct Probe {
    armed: AtomicBool,
    cut: AtomicBool,
    expires: AtomicUsize,
    tokens: Mutex<Option<(String, String)>>,
    refused: AtomicUsize,
    resumed: AtomicUsize,
    refreshed: AtomicUsize,
}
fn digest(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub fn wrap(app: Router, short_plans: Router) -> (Router, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let state = probe.clone();
    let app = app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let state = state.clone();
        let short_plans = short_plans.clone();
        async move {
            let path = request.uri().path().to_owned();
            let token = request
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(digest);
            let nonzero = request
                .headers()
                .get("range")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| !v.starts_with("bytes=0-"));
            const PLAN_LIMIT: usize = 4 * 1024 * 1024;
            let short = if path.ends_with("/fetch-plans") && state.armed.load(Ordering::SeqCst) {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, PLAN_LIMIT).await.unwrap();
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                let is_file = value["claims"]
                    .as_array()
                    .is_some_and(|claims| claims.iter().any(|claim| claim["kind"] == "file"));
                let mut request = Request::from_parts(parts, axum::body::Body::from(bytes));
                if is_file && state.armed.swap(false, Ordering::SeqCst) {
                    // A second Router must resolve its own path parameters,
                    // not append to the outer router's already matched params.
                    request.extensions_mut().clear();
                    (true, short_plans.oneshot(request).await.unwrap())
                } else {
                    (false, next.run(request).await)
                }
            } else {
                (false, next.run(request).await)
            };
            let (short, response) = short;
            if path.ends_with("/fetch-plans")
                && response.status().is_success()
                && (short || state.tokens.lock().unwrap().is_some())
            {
                let (mut parts, body) = response.into_parts();
                let bytes = axum::body::to_bytes(body, 8 * 1024 * 1024).await.unwrap();
                let plan: FetchPlanResponse = serde_json::from_slice(&bytes).unwrap();
                if short {
                    let cap = plan
                        .download_capabilities
                        .iter()
                        .find(|cap| cap.descriptor.encoded_byte_len >= 32 * 1024)
                        .unwrap();
                    *state.tokens.lock().unwrap() = Some((
                        digest(&cap.token),
                        cap.descriptor.object_id.as_str().to_owned(),
                    ));
                    state.expires.store(now() as usize + 3, Ordering::SeqCst);
                } else if let Some((_, object)) = state.tokens.lock().unwrap().as_ref() {
                    if state.refused.load(Ordering::SeqCst) > 0
                        && state.resumed.load(Ordering::SeqCst) == 0
                        && plan
                            .download_capabilities
                            .iter()
                            .any(|c| c.descriptor.object_id.as_str() == object)
                    {
                        state.refreshed.fetch_add(1, Ordering::SeqCst);
                    }
                }
                let bytes = serde_json::to_vec(&plan).unwrap();
                parts.headers.insert("content-length", bytes.len().into());
                return Response::from_parts(parts, axum::body::Body::from(bytes));
            }
            if path == "/v3/transfers/downloads" {
                let target = state.tokens.lock().unwrap().as_ref().map(|v| v.0.clone());
                if token.is_some() && token == target {
                    if response.status().as_u16() == 404 {
                        state.refused.fetch_add(1, Ordering::SeqCst);
                    } else if response.status().is_success()
                        && !state.cut.swap(true, Ordering::SeqCst)
                    {
                        let (parts, body) = response.into_parts();
                        let bytes = axum::body::to_bytes(body, 64 * 1024 * 1024).await.unwrap();
                        let prefix = bytes.slice(..bytes.len() / 2);
                        let expires = state.expires.load(Ordering::SeqCst) as u64;
                        let stream =
                            futures_util::stream::unfold(Some(prefix), move |value| async move {
                                match value {
                                    Some(bytes) => Some((Ok::<_, std::io::Error>(bytes), None)),
                                    None => {
                                        tokio::time::sleep(Duration::from_secs(
                                            expires.saturating_sub(now()) + 1,
                                        ))
                                        .await;
                                        Some((
                                            Err(std::io::Error::other(
                                                "fixture expiry after prefix",
                                            )),
                                            None,
                                        ))
                                    }
                                }
                            });
                        return Response::from_parts(parts, axum::body::Body::from_stream(stream));
                    }
                } else if nonzero
                    && state.refused.load(Ordering::SeqCst) > 0
                    && response.status().is_success()
                {
                    state.resumed.fetch_add(1, Ordering::SeqCst);
                }
            }
            response
        }
    }));
    (app, probe)
}

pub fn run(remote: &NativeRemote, root: &Path, origin: &str, token: &str, ca: &str, probe: &Probe) {
    let source = root.join("expiry-source");
    std::fs::create_dir(&source).unwrap();
    let expected = (0..128 * 1024)
        .map(|i| ((i * 37) % 251) as u8)
        .collect::<Vec<_>>();
    std::fs::write(source.join("asset.bin"), &expected).unwrap();
    let workspace = NativeWorkspace::initialize(&source, "Expiry").unwrap();
    let provision = remote.provision(&workspace, "expiry", "expiry").unwrap();
    assert_eq!(
        remote
            .publish(&workspace, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let target = root.join("expiry-clone");
    std::fs::create_dir(&target).unwrap();
    let client = NativeRemote::with_trusted_ca(origin, token, ca.as_bytes())
        .unwrap()
        .with_download_cache(root.join("expiry-cache"));
    probe.armed.store(true, Ordering::SeqCst);
    client
        .clone_into(&target, &provision.space_id, "Renewed")
        .unwrap();
    assert!(!probe.armed.load(Ordering::SeqCst));
    assert_eq!(
        probe.refused.load(Ordering::SeqCst),
        1,
        "real expired signed token refused once"
    );
    assert_eq!(
        probe.refreshed.load(Ordering::SeqCst),
        1,
        "fresh exact claim requested once"
    );
    assert_eq!(
        probe.resumed.load(Ordering::SeqCst),
        1,
        "new capability continues missing range"
    );
    assert_eq!(std::fs::read(target.join("asset.bin")).unwrap(), expected);
    assert!(
        NativeWorkspace::open(&target)
            .unwrap()
            .working_diff()
            .unwrap()
            .is_empty()
    );
    *probe.tokens.lock().unwrap() = None;
    eprintln!(
        "Chronological capability expiry PASS: signed 3s plan, real 404, fresh claim, durable prefix, exact bytes"
    );
}
