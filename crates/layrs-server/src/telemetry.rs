//! Bounded, process-local HTTP observations. No bodies, tokens, supplied IDs,
//! actual URLs, user identifiers or arbitrary error strings enter these records.
use crate::identity::{IdentityError, PgIdentity};
use axum::{
    Json, Router,
    extract::{MatchedPath, Request, State},
    http::{HeaderMap, HeaderValue},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const BUCKETS_MS: [u64; 6] = [5, 25, 100, 500, 2_000, 10_000];
#[derive(Clone)]
pub struct Telemetry(Arc<Counters>);
struct Counters {
    started: Instant,
    in_flight: AtomicU64,
    completed: AtomicU64,
    interrupted: AtomicU64,
    status_classes: [AtomicU64; 6],
    duration_us: AtomicU64,
    latency_buckets: [AtomicU64; 6],
    readiness_checks: AtomicU64,
    database_failures: AtomicU64,
    cas_failures: AtomicU64,
}
impl Default for Telemetry {
    fn default() -> Self {
        Self(Arc::new(Counters {
            started: Instant::now(),
            in_flight: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            interrupted: AtomicU64::new(0),
            duration_us: AtomicU64::new(0),
            status_classes: std::array::from_fn(|_| AtomicU64::new(0)),
            latency_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            readiness_checks: AtomicU64::new(0),
            database_failures: AtomicU64::new(0),
            cas_failures: AtomicU64::new(0),
        }))
    }
}
impl Telemetry {
    pub fn snapshot(&self) -> Value {
        let state = &self.0;
        json!({
            "schemaVersion":"layrs.telemetry.v1", "scope":"current_process",
            "observedUnixMs":unix_ms(), "uptimeSeconds":state.started.elapsed().as_secs(),
            "http":{
                "inFlight":state.in_flight.load(Relaxed), "completed":state.completed.load(Relaxed),
                "interruptedBeforeHeaders":state.interrupted.load(Relaxed),
                "statusClasses":(1..6).map(|class|json!({"class":format!("{class}xx"),"count":state.status_classes[class].load(Relaxed)})).collect::<Vec<_>>(),
                "headersLatencyMicrosecondsTotal":state.duration_us.load(Relaxed),
                "headersLatencyBuckets":BUCKETS_MS.iter().zip(&state.latency_buckets).map(|(bound,count)|json!({"lessOrEqualMs":bound,"count":count.load(Relaxed)})).collect::<Vec<_>>()
            },
            "readiness":{"checks":state.readiness_checks.load(Relaxed),"databaseFailures":state.database_failures.load(Relaxed),"casFailures":state.cas_failures.load(Relaxed)}
        })
    }
    pub fn record_readiness(&self, database: bool, cas: bool) {
        self.0.readiness_checks.fetch_add(1, Relaxed);
        if !database {
            self.0.database_failures.fetch_add(1, Relaxed);
        }
        if !cas {
            self.0.cas_failures.fetch_add(1, Relaxed);
        }
        if !database || !cas {
            eprintln!(
                "{}",
                json!({"event":"dependency_probe_failed","unixMs":unix_ms(),"databaseReady":database,"casReady":cas})
            );
        }
    }
    fn begin(&self) -> InFlight {
        self.0.in_flight.fetch_add(1, Relaxed);
        InFlight {
            telemetry: self.clone(),
            completed: false,
        }
    }
}
struct InFlight {
    telemetry: Telemetry,
    completed: bool,
}
impl InFlight {
    fn complete(&mut self, status: u16, elapsed_us: u64) {
        let state = &self.telemetry.0;
        state.completed.fetch_add(1, Relaxed);
        state.status_classes[usize::from(status / 100).min(5)].fetch_add(1, Relaxed);
        state.duration_us.fetch_add(elapsed_us, Relaxed);
        for (bound, bucket) in BUCKETS_MS.iter().zip(&state.latency_buckets) {
            if elapsed_us <= bound * 1000 {
                bucket.fetch_add(1, Relaxed);
            }
        }
        self.completed = true;
    }
}
impl Drop for InFlight {
    fn drop(&mut self) {
        self.telemetry.0.in_flight.fetch_sub(1, Relaxed);
        if !self.completed {
            self.telemetry.0.interrupted.fetch_add(1, Relaxed);
        }
    }
}
fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
fn request_record(id: &str, method: &str, route: &str, status: u16, elapsed_us: u64) -> Value {
    json!({"event":"http_request","unixMs":unix_ms(),"requestId":id,"method":method,"route":route,"status":status,"headersLatencyMicroseconds":elapsed_us,"level":if status>=500{"error"}else if status>=400{"warning"}else{"info"}})
}
pub async fn observe(State(telemetry): State<Telemetry>, request: Request, next: Next) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    // MatchedPath is the router's static template. A 404 must never log its URL.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let method = match request.method().as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        "HEAD" => "HEAD",
        "OPTIONS" => "OPTIONS",
        _ => "OTHER",
    };
    let start = Instant::now();
    let mut guard = telemetry.begin();
    let mut response = next.run(request).await;
    let elapsed_us = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    guard.complete(response.status().as_u16(), elapsed_us);
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&id).expect("UUID header"),
    );
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    eprintln!(
        "{}",
        request_record(&id, method, &route, response.status().as_u16(), elapsed_us)
    );
    response
}

#[derive(Clone)]
struct HttpState {
    identity: PgIdentity,
    pool: PgPool,
    telemetry: Telemetry,
}
pub fn router(identity: PgIdentity, pool: PgPool, telemetry: Telemetry) -> Router {
    Router::new()
        .route("/v3/admin/metrics", get(metrics))
        .with_state(HttpState {
            identity,
            pool,
            telemetry,
        })
}
async fn metrics(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Response, IdentityError> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|_| IdentityError::Unavailable)?;
    state
        .identity
        .require_administrator_in(&headers, &mut tx)
        .await?;
    let report = state.telemetry.snapshot();
    tx.commit().await.map_err(|_| IdentityError::Unavailable)?;
    Ok(([("cache-control", "no-store")], Json(report)).into_response())
}

#[cfg(test)]
mod tests;
