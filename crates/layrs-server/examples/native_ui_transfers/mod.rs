//! Private disposable-fixture controls; never mounted by the production server.
//! Kept as a module so Cargo cannot discover it as a standalone example binary.
use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
    routing::post,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use tokio::sync::watch;

#[derive(Clone)]
pub struct Control {
    bearer: Arc<str>,
    paused: watch::Sender<bool>,
    reached: Arc<AtomicBool>,
    target: Arc<AtomicU8>,
}
impl Control {
    pub fn new(token: &str) -> Self {
        let (paused, _) = watch::channel(false);
        Self {
            bearer: format!("Bearer {token}").into(),
            paused,
            reached: Arc::new(AtomicBool::new(false)),
            target: Arc::new(AtomicU8::new(0)),
        }
    }
    pub fn router(&self) -> Router {
        Router::new()
            .route("/test-transfer-control/:action", post(command))
            .with_state(self.clone())
    }
}
async fn command(
    State(control): State<Control>,
    axum::extract::Path(action): axum::extract::Path<String>,
    request: Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        != Some(&control.bearer)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    match action.as_str() {
        "arm" | "arm-upload" | "arm-transaction" => {
            control.target.store(
                match action.as_str() {
                    "arm-upload" => 1,
                    "arm-transaction" => 2,
                    _ => 0,
                },
                Ordering::SeqCst,
            );
            control.reached.store(false, Ordering::SeqCst);
            control.paused.send_replace(true);
        }
        "release" => {
            control.paused.send_replace(false);
        }
        "status" => {}
        _ => return Err(StatusCode::NOT_FOUND),
    }
    Ok(Json(
        serde_json::json!({"downloadReached":control.reached.load(Ordering::SeqCst)}),
    ))
}
pub async fn pause_download(
    State(control): State<Control>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let selected = match control.target.load(Ordering::SeqCst) {
        0 => request.method() == "GET" && path == "/v3/transfers/downloads",
        1 => request.method() == "PUT" && path == "/v3/transfers/uploads",
        2 => {
            request.method() == "POST"
                && path.starts_with("/v3/spaces/")
                && path.ends_with("/transactions")
        }
        _ => false,
    };
    if selected {
        let mut paused = control.paused.subscribe();
        if *paused.borrow() {
            control.reached.store(true, Ordering::SeqCst);
        }
        while *paused.borrow_and_update() {
            if paused.changed().await.is_err() {
                break;
            }
        }
    }
    next.run(request).await
}
