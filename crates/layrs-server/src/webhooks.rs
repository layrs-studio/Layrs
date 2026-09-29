//! Persistent at-least-once webhooks. No destination is permitted by default.
use crate::identity::BearerVerifier;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use layrs_core::scm::SpaceId;
use layrs_store_server::v3::*;
use serde::Deserialize;
use sha2::Sha256;
use sqlx::Row;
use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Default)]
pub struct WebhookPolicy {
    pub allowed_origins: BTreeSet<String>,
    pub private_origins: BTreeSet<String>,
    pub ca_pem: Option<Vec<u8>>,
}
impl WebhookPolicy {
    pub fn from_env() -> Result<Self> {
        let origins = |name: &str| -> Result<BTreeSet<String>> {
            std::env::var(name)
                .unwrap_or_default()
                .split_whitespace()
                .map(|raw| {
                    let url = url::Url::parse(raw)?;
                    if url.scheme() != "https"
                        || url.path() != "/"
                        || url.query().is_some()
                        || url.fragment().is_some()
                        || !url.username().is_empty()
                        || url.password().is_some()
                    {
                        return Err("webhook allowlist requires HTTPS origins".into());
                    }
                    Ok(url.origin().ascii_serialization())
                })
                .collect()
        };
        let allowed_origins = origins("LAYRS_WEBHOOK_ALLOWED_ORIGINS")?;
        let private_origins = origins("LAYRS_WEBHOOK_PRIVATE_ORIGINS")?;
        if !private_origins.is_subset(&allowed_origins) {
            return Err("private webhook origins must be explicitly allowlisted".into());
        }
        let ca_pem = std::env::var_os("LAYRS_WEBHOOK_CA_FILE")
            .map(std::fs::read)
            .transpose()?;
        if ca_pem
            .as_ref()
            .is_some_and(|bytes| bytes.len() > 1024 * 1024)
        {
            return Err("webhook CA too large".into());
        }
        if let Some(pem) = &ca_pem {
            reqwest::Certificate::from_pem(pem)?;
        }
        Ok(Self {
            allowed_origins,
            private_origins,
            ca_pem,
        })
    }
    pub fn validate_url(&self, raw: &str) -> Result<url::Url> {
        if raw.len() > 2048 {
            return Err("webhook URL too long".into());
        }
        let url = url::Url::parse(raw)?;
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_some()
            || !self
                .allowed_origins
                .contains(&url.origin().ascii_serialization())
        {
            return Err("webhook destination is not allowed".into());
        }
        Ok(url)
    }
    async fn client(&self, url: &url::Url) -> Result<reqwest::Client> {
        let host = url.host_str().ok_or("missing webhook host")?;
        let port = url.port_or_known_default().ok_or("missing webhook port")?;
        let addresses: Vec<SocketAddr> = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((host, port)),
        )
        .await??
        .take(33)
        .collect();
        if addresses.is_empty() || addresses.len() > 32 {
            return Err("invalid webhook DNS result".into());
        }
        let private = self
            .private_origins
            .contains(&url.origin().ascii_serialization());
        if !private && addresses.iter().any(|address| !public_ip(address.ip())) {
            return Err("webhook DNS resolved to a prohibited address".into());
        }
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .resolve_to_addrs(host, &addresses);
        if let Some(pem) = &self.ca_pem {
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(pem)?)
        }
        Ok(builder.build()?)
    }
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && o[0] != 0
                && o[0] < 224
                && !(o[0] == 100 && (64..=127).contains(&o[1]))
                && !(o[0] == 192 && o[1] == 0)
                && !(o[0] == 198 && (o[1] == 18 || o[1] == 19 || o[1] == 51))
                && !(o[0] == 203 && o[1] == 0 && o[2] == 113)
        }
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|v4| public_ip(IpAddr::V4(v4)))
            .unwrap_or_else(|| {
                let segments = ip.segments();
                segments[0] & 0xe000 == 0x2000
                    && segments[0] != 0x2002
                    && !(segments[0] == 0x2001
                        && (segments[1] == 0
                            || segments[1] == 0xdb8
                            || segments[1] & 0xfff0 == 0x10))
            }),
    }
}
#[derive(Clone)]
pub struct WebhookService {
    repository: PgV3Repository,
    identity: Arc<dyn BearerVerifier>,
    policy: WebhookPolicy,
    key: Arc<Vec<u8>>,
}
impl WebhookService {
    pub fn new(
        repository: PgV3Repository,
        identity: Arc<dyn BearerVerifier>,
        policy: WebhookPolicy,
        key: Vec<u8>,
    ) -> Result<Self> {
        if key.len() < 32 {
            return Err("webhook signing root too short".into());
        }
        Ok(Self {
            repository,
            identity,
            policy,
            key: Arc::new(key),
        })
    }
    pub fn router(self) -> Router {
        Router::new()
            .route("/v3/spaces/:space/webhooks", get(list).post(create))
            .route(
                "/v3/spaces/:space/webhooks/:hook/deliveries",
                get(deliveries),
            )
            .route("/v3/spaces/:space/webhooks/:hook/rotate", post(rotate))
            .route("/v3/spaces/:space/webhooks/:hook/disable", post(disable))
            .route("/v3/spaces/:space/webhooks/:hook/replay", post(replay))
            .layer(DefaultBodyLimit::max(8192))
            .with_state(self)
    }
    fn secret(&self, id: &str, generation: u64) -> Vec<u8> {
        let mut h =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts arbitrary key length");
        h.update(format!("layrs-webhook-key-v1\0{id}\0{generation}").as_bytes());
        h.finalize().into_bytes().to_vec()
    }
    async fn principal(
        &self,
        headers: &HeaderMap,
    ) -> std::result::Result<PrincipalContext, HttpError> {
        if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
            return Err(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"));
        }
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(HttpError(StatusCode::UNAUTHORIZED, "unauthorized"))?;
        let id = self
            .identity
            .verify_bearer(token)
            .await
            .map_err(|_| HttpError(StatusCode::UNAUTHORIZED, "unauthorized"))?;
        load_principal_context(self.repository.pool(), &id)
            .await
            .map_err(store_error)
    }
    /// Each invocation commits a durable claim; expired claims survive process loss.
    pub async fn dispatch_once(&self) -> Result<bool> {
        let pool = self.repository.pool();
        sqlx::query("UPDATE layrs_collaboration.webhook_deliveries SET state='dead',lease_until=NULL,last_error='attempts_exhausted' WHERE state='sending' AND lease_until<clock_timestamp() AND attempts>=8").execute(pool).await?;
        sqlx::query("INSERT INTO layrs_collaboration.webhook_deliveries(webhook_id,notification_sequence,delivery_id) SELECT h.webhook_id,n.sequence,h.webhook_id||':'||n.sequence FROM layrs_collaboration.webhooks h JOIN layrs_collaboration.notifications n ON n.space_id=h.space_id AND n.sequence>h.start_sequence AND n.event_kind=ANY(h.event_kinds) WHERE h.enabled AND NOT EXISTS(SELECT 1 FROM layrs_collaboration.webhook_deliveries d WHERE d.webhook_id=h.webhook_id AND d.notification_sequence=n.sequence) ORDER BY n.sequence LIMIT 1000 ON CONFLICT DO NOTHING").execute(pool).await?;
        let mut tx = pool.begin().await?;
        let row=sqlx::query("SELECT d.webhook_id,d.notification_sequence,d.delivery_id,d.attempts,h.space_id,h.url,h.secret_generation,h.created_by,n.actor_principal_id,n.event_kind,n.subject_id,n.evidence_object_id,n.created_at::text FROM layrs_collaboration.webhook_deliveries d JOIN layrs_collaboration.webhooks h USING(webhook_id) JOIN layrs_collaboration.notifications n ON n.sequence=d.notification_sequence WHERE h.enabled AND ((d.state='pending' AND d.next_attempt_at<=clock_timestamp()) OR (d.state='sending' AND d.lease_until<clock_timestamp())) ORDER BY d.next_attempt_at,d.delivery_id LIMIT 1 FOR UPDATE OF d SKIP LOCKED").fetch_optional(&mut *tx).await?;
        let Some(row) = row else { return Ok(false) };
        let id: String = row.get("delivery_id");
        let attempt: i32 = row.get::<i32, _>("attempts") + 1;
        sqlx::query("UPDATE layrs_collaboration.webhook_deliveries SET state='sending',attempts=$2,lease_until=clock_timestamp()+interval '60 seconds' WHERE delivery_id=$1").bind(&id).bind(attempt).execute(&mut *tx).await?;
        tx.commit().await?;
        let space = SpaceId::parse(row.get::<String, _>("space_id"))?;
        let creator = layrs_sync::v3::PrincipalId::parse(row.get::<String, _>("created_by"))?;
        let authorization = async {
            let principal = load_principal_context(pool, &creator).await?;
            let row = sqlx::query(
                "SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1",
            )
            .bind(space.as_str())
            .fetch_one(pool)
            .await
            .map_err(|e| V3StoreError::Backend(e.to_string()))?;
            let policy =
                layrs_core::scm::ObjectId::parse(row.get::<String, _>("current_policy_set_id"))
                    .map_err(|e| V3StoreError::Backend(e.to_string()))?;
            let decision = self
                .repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal,
                    action: V3Action::AdministerPolicy,
                    target: AuthorizationTarget::Space(space.clone()),
                    policy_set_id: policy,
                    authorization_epoch: row.get::<i64, _>("authorization_epoch") as u64,
                })
                .await?;
            Ok::<bool, V3StoreError>(decision.allowed)
        }
        .await
        .unwrap_or(false);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let generation = row.get::<i64, _>("secret_generation") as u64;
        let payload = serde_jcs::to_vec(
            &serde_json::json!({"schemaVersion":"layrs.webhook-event.v1","deliveryId":id,"spaceId":space,"sequence":row.get::<i64,_>("notification_sequence"),"kind":row.get::<String,_>("event_kind"),"subjectId":row.get::<String,_>("subject_id"),"evidenceObjectId":row.get::<String,_>("evidence_object_id"),"actorPrincipalId":row.get::<String,_>("actor_principal_id"),"createdAt":row.get::<String,_>("created_at")}),
        )?;
        let secret = self.secret(&row.get::<String, _>("webhook_id"), generation);
        let signature = sign(&secret, &timestamp, &id, &payload);
        let send = async {
            if !authorization {
                return Err("webhook authority revoked".into());
            }
            let url = self.policy.validate_url(&row.get::<String, _>("url"))?;
            let client = self.policy.client(&url).await?;
            let response = client
                .post(url)
                .header("content-type", "application/json")
                .header("x-layrs-delivery", &id)
                .header("x-layrs-timestamp", &timestamp)
                .header("x-layrs-key-generation", generation)
                .header("x-layrs-signature", format!("v1={signature}"))
                .body(payload)
                .send()
                .await?;
            Ok::<u16, Box<dyn std::error::Error + Send + Sync>>(response.status().as_u16())
        }
        .await;
        let status = send.as_ref().ok().copied();
        let passed = status.is_some_and(|s| (200..300).contains(&s));
        let error = if passed {
            None
        } else if !authorization {
            Some("authority_revoked")
        } else if status.is_some() {
            Some("http_rejected")
        } else {
            Some("destination_or_transport_failed")
        };
        let state = if passed {
            "delivered"
        } else if attempt >= 8 || !authorization {
            "dead"
        } else {
            "pending"
        };
        let delay = 5_i64 * 2_i64.pow(attempt.min(10) as u32);
        sqlx::query("UPDATE layrs_collaboration.webhook_deliveries SET state=$2,lease_until=NULL,next_attempt_at=clock_timestamp()+($3::bigint*interval '1 second'),last_status=$4,last_error=$5,delivered_at=CASE WHEN $2='delivered' THEN clock_timestamp() ELSE NULL END WHERE delivery_id=$1 AND state='sending' AND attempts=$6").bind(&id).bind(state).bind(delay).bind(status.map(i32::from)).bind(error).bind(attempt).execute(pool).await?;
        Ok(true)
    }
    pub async fn run(self) {
        loop {
            match self.dispatch_once().await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(Duration::from_secs(2)).await,
                Err(_) => {
                    eprintln!("{{\"event\":\"webhook_dispatch_failed\"}}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }
}
pub fn sign(secret: &[u8], timestamp: &str, id: &str, payload: &[u8]) -> String {
    let mut h = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts arbitrary key length");
    h.update(timestamp.as_bytes());
    h.update(b".");
    h.update(id.as_bytes());
    h.update(b".");
    h.update(payload);
    hex::encode(h.finalize().into_bytes())
}
struct HttpError(StatusCode, &'static str);
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (
            self.0,
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({"error":{"code":self.1}})),
        )
            .into_response()
    }
}
fn store_error(e: V3StoreError) -> HttpError {
    HttpError(
        match e {
            V3StoreError::NotFound => StatusCode::NOT_FOUND,
            V3StoreError::InvalidCapability => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_REQUEST,
        },
        e.code(),
    )
}
fn space(id: String) -> std::result::Result<SpaceId, HttpError> {
    SpaceId::parse(id).map_err(|_| HttpError(StatusCode::BAD_REQUEST, "invalid_space"))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Create {
    url: String,
    event_kinds: Vec<String>,
}
async fn create(
    State(s): State<WebhookService>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<Create>,
) -> std::result::Result<Json<serde_json::Value>, HttpError> {
    let principal = s.principal(&h).await?;
    let space = space(id)?;
    s.policy
        .validate_url(&request.url)
        .map_err(|_| HttpError(StatusCode::BAD_REQUEST, "destination_not_allowed"))?;
    if request.event_kinds.is_empty()
        || request.event_kinds.len() > 8
        || request.event_kinds.iter().any(|s| {
            !matches!(
                s.as_str(),
                "review.proposed"
                    | "review.action"
                    | "review.integrated"
                    | "review.comment_corrected"
                    | "proof.completed"
                    | "proof.cancelled"
                    | "protection.changed"
                    | "membership.changed"
                    | "team.changed"
            )
        })
    {
        return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_event_kinds"));
    }
    let id = format!("webhook:{}", uuid::Uuid::new_v4());
    PgCollaborationStore::new(s.repository.clone())
        .create_webhook(&principal, &space, &id, &request.url, &request.event_kinds)
        .await
        .map_err(store_error)?;
    Ok(Json(
        serde_json::json!({"webhookId":id,"secretGeneration":1,"secret":hex::encode(s.secret(&id,1))}),
    ))
}
async fn list(
    State(s): State<WebhookService>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> std::result::Result<Json<serde_json::Value>, HttpError> {
    Ok(Json(
        PgCollaborationStore::new(s.repository.clone())
            .list_webhooks(&s.principal(&h).await?, &space(id)?)
            .await
            .map_err(store_error)?,
    ))
}
async fn deliveries(
    State(s): State<WebhookService>,
    Path((id, hook)): Path<(String, String)>,
    h: HeaderMap,
) -> std::result::Result<Json<serde_json::Value>, HttpError> {
    Ok(Json(
        PgCollaborationStore::new(s.repository.clone())
            .webhook_deliveries(&s.principal(&h).await?, &space(id)?, &hook)
            .await
            .map_err(store_error)?,
    ))
}
async fn rotate(
    State(s): State<WebhookService>,
    Path((id, hook)): Path<(String, String)>,
    h: HeaderMap,
) -> std::result::Result<Json<serde_json::Value>, HttpError> {
    let generation = PgCollaborationStore::new(s.repository.clone())
        .mutate_webhook(&s.principal(&h).await?, &space(id)?, &hook, "rotate", None)
        .await
        .map_err(store_error)?;
    Ok(Json(
        serde_json::json!({"secretGeneration":generation,"secret":hex::encode(s.secret(&hook,generation))}),
    ))
}
async fn disable(
    State(s): State<WebhookService>,
    Path((id, hook)): Path<(String, String)>,
    h: HeaderMap,
) -> std::result::Result<StatusCode, HttpError> {
    PgCollaborationStore::new(s.repository.clone())
        .mutate_webhook(&s.principal(&h).await?, &space(id)?, &hook, "disable", None)
        .await
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Replay {
    delivery_id: String,
}
async fn replay(
    State(s): State<WebhookService>,
    Path((id, hook)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<Replay>,
) -> std::result::Result<StatusCode, HttpError> {
    PgCollaborationStore::new(s.repository.clone())
        .mutate_webhook(
            &s.principal(&h).await?,
            &space(id)?,
            &hook,
            "replay",
            Some(&request.delivery_id),
        )
        .await
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_private_addresses_and_unlisted_destinations() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.64.1.1",
            "::1",
            "::ffff:127.0.0.1",
            "fd00::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()));
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        let p = WebhookPolicy::default();
        assert!(p.validate_url("https://example.com/event").is_err());
    }
    #[test]
    fn signature_binds_delivery_date_and_exact_bytes() {
        let key = b"example only";
        let signature = sign(key, "123", "event:1", b"{}");
        assert_ne!(signature, sign(key, "124", "event:1", b"{}"));
        assert_ne!(signature, sign(key, "123", "event:2", b"{}"));
        assert_ne!(signature, sign(key, "123", "event:1", b"{ }"));
    }
}
