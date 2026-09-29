//! Native local accounts. Bearer secrets are returned once and stored only as digests.
use crate::v3::V3ServiceError;
mod devices;
mod management;
use argon2::{
    Argon2,
    password_hash::{
        PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
        rand_core::{OsRng, RngCore},
    },
};
use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use layrs_sync::v3::PrincipalId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[async_trait]
pub trait BearerVerifier: Send + Sync {
    async fn verify_bearer(&self, token: &str) -> std::result::Result<PrincipalId, V3ServiceError>;
}

#[derive(Clone)]
pub struct PgIdentity {
    pool: PgPool,
    deployment_id: String,
    origin: String,
    password_workers: Arc<Semaphore>,
    dummy_hash: Arc<String>,
}

#[derive(Debug)]
pub enum IdentityError {
    Invalid,
    Unauthorized,
    AlreadyInitialized,
    RateLimited,
    Unavailable,
}
impl IntoResponse for IdentityError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            Self::Invalid => (StatusCode::BAD_REQUEST, "invalid_identity_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::AlreadyInitialized => (StatusCode::CONFLICT, "already_initialized"),
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "identity_unavailable"),
        };
        (
            status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({"error": {"code": code}})),
        )
            .into_response()
    }
}
type Result<T> = std::result::Result<T, IdentityError>;
impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for IdentityError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub login: String,
    pub password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapRequest {
    pub secret: String,
    pub login: String,
    pub password: String,
    pub display_name: String,
}
#[derive(Serialize)]
pub struct Session {
    pub principal_id: PrincipalId,
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangePassword {
    current_password: String,
    new_password: String,
}

impl PgIdentity {
    pub async fn new(pool: PgPool, deployment_id: String, origin: String) -> Result<Self> {
        let dummy_hash = tokio::task::spawn_blocking(|| hash_password(&random_secret()))
            .await
            .map_err(|_| IdentityError::Unavailable)??;
        Ok(Self {
            pool,
            deployment_id,
            origin,
            password_workers: Arc::new(Semaphore::new(4)),
            dummy_hash: Arc::new(dummy_hash),
        })
    }

    pub fn router(self) -> Router {
        Router::new()
            .route("/v3/auth/bootstrap", post(bootstrap))
            .route("/v3/auth/login", post(login))
            .route("/v3/auth/whoami", axum::routing::get(whoami))
            .route("/v3/auth/logout", post(logout))
            .route("/v3/auth/password", post(change_password))
            .route(
                "/v3/auth/accounts",
                post(management::create_account).get(management::list_accounts),
            )
            .route("/v3/auth/accounts/revoke", post(management::revoke_account))
            .route(
                "/v3/auth/service-accounts",
                post(management::create_service),
            )
            .route(
                "/v3/auth/service-accounts/rotate",
                post(management::rotate_service),
            )
            .route(
                "/v3/auth/tokens",
                axum::routing::get(management::list_tokens),
            )
            .route("/v3/auth/tokens/revoke", post(management::revoke_token))
            .route("/v3/auth/devices/start", post(devices::start))
            .route("/v3/auth/devices/approve", post(devices::approve))
            .route("/v3/auth/devices/inspect", post(devices::inspect))
            .route("/v3/auth/devices/poll", post(devices::poll))
            .route("/auth/device", axum::routing::get(devices::page))
            .route("/auth/device.js", axum::routing::get(devices::script))
            .layer(DefaultBodyLimit::max(8192))
            .with_state(self)
    }

    fn check_origin(&self, headers: &HeaderMap) -> Result<()> {
        // Bearer-only API never authenticates cookies. Browsers must be same-origin.
        if let Some(origin) = headers.get(header::ORIGIN) {
            if origin.to_str().ok() != Some(self.origin.as_str()) {
                return Err(IdentityError::Unauthorized);
            }
        }
        Ok(())
    }

    pub(crate) async fn require_administrator_in(
        &self,
        headers: &HeaderMap,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<()> {
        let (_, generation) = management::authenticated_in_tx(self, headers, tx, true).await?;
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_identity.tokens WHERE token_digest=$1 AND authorization_generation=$2 AND revoked_at IS NULL AND expires_at>clock_timestamp() FOR SHARE)")
            .bind(token_digest(bearer(headers)?)).bind(generation).fetch_one(&mut **tx).await.map_err(db)?;
        if !valid {
            return Err(IdentityError::Unauthorized);
        }
        Ok(())
    }

    async fn password_hash(&self, password: String) -> Result<String> {
        validate_password(&password)?;
        let permit = self
            .password_workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| IdentityError::RateLimited)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash_password(&password)
        })
        .await
        .map_err(|_| IdentityError::Unavailable)?
    }

    async fn password_matches(&self, password: String, hash: String) -> Result<bool> {
        if password.len() > 1024 {
            return Err(IdentityError::Invalid);
        }
        let permit = self
            .password_workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| IdentityError::RateLimited)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let parsed = PasswordHash::new(&hash).map_err(|_| IdentityError::Unavailable)?;
            Ok(Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok())
        })
        .await
        .map_err(|_| IdentityError::Unavailable)?
    }

    pub async fn bootstrap(&self, request: BootstrapRequest) -> Result<Session> {
        let login = normalize_login(&request.login)?;
        if request.display_name.trim().is_empty()
            || request.display_name.len() > 128
            || request.secret.len() != 64
        {
            return Err(IdentityError::Invalid);
        }
        // Check the secret before doing expensive password work; the UPDATE below consumes it atomically.
        let expected: Option<String> = sqlx::query_scalar("SELECT secret_digest FROM layrs_identity.bootstrap WHERE deployment_id=$1 AND consumed_at IS NULL")
            .bind(&self.deployment_id).fetch_optional(&self.pool).await.map_err(db)?;
        let expected = expected.ok_or(IdentityError::AlreadyInitialized)?;
        let actual = token_digest(&request.secret);
        if !constant_time_equal(actual.as_bytes(), expected.as_bytes()) {
            return Err(IdentityError::Unauthorized);
        }
        let password_hash = self.password_hash(request.password).await?;
        let principal = format!("principal:{}", uuid::Uuid::new_v4());
        let actor = format!("actor:{}", uuid::Uuid::new_v4());
        let mut tx = self.pool.begin().await.map_err(db)?;
        let available: Option<String> = sqlx::query_scalar("SELECT deployment_id FROM layrs_identity.bootstrap WHERE deployment_id=$1 AND secret_digest=$2 AND consumed_at IS NULL FOR UPDATE")
            .bind(&self.deployment_id).bind(&actual).fetch_optional(&mut *tx).await.map_err(db)?;
        if available.is_none() {
            return Err(IdentityError::AlreadyInitialized);
        }
        sqlx::query("INSERT INTO principals(principal_id,actor_id,deployment_id,principal_kind,subject_digest,display_name) VALUES($1,$2,$3,'bootstrap_admin',$4,$5)")
            .bind(&principal).bind(actor).bind(&self.deployment_id).bind(token_digest(&format!("account:{login}"))).bind(request.display_name.trim()).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("INSERT INTO layrs_identity.accounts(principal_id,deployment_id,login,password_hash) VALUES($1,$2,$3,$4)")
            .bind(&principal).bind(&self.deployment_id).bind(login).bind(password_hash).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE layrs_identity.bootstrap SET consumed_at=clock_timestamp(),administrator_id=$2 WHERE deployment_id=$1")
            .bind(&self.deployment_id).bind(&principal).execute(&mut *tx).await.map_err(db)?;
        let session = issue_session(&mut tx, &principal, 1).await?;
        tx.commit().await.map_err(db)?;
        Ok(session)
    }

    pub async fn login(&self, request: Credentials) -> Result<Session> {
        let login = normalize_login(&request.login)?;
        // Atomically count before verification: concurrent failures cannot bypass the limit.
        let row = sqlx::query("UPDATE layrs_identity.accounts SET failed_attempts=CASE WHEN attempt_window < clock_timestamp()-interval '15 minutes' THEN 1 ELSE failed_attempts+1 END, attempt_window=CASE WHEN attempt_window < clock_timestamp()-interval '15 minutes' THEN clock_timestamp() ELSE attempt_window END WHERE deployment_id=$1 AND login=$2 AND (failed_attempts<5 OR attempt_window < clock_timestamp()-interval '15 minutes') RETURNING principal_id,password_hash")
            .bind(&self.deployment_id).bind(login).fetch_optional(&self.pool).await.map_err(db)?;
        let hash = row
            .as_ref()
            .map(|r| r.get::<String, _>("password_hash"))
            .unwrap_or_else(|| (*self.dummy_hash).clone());
        let valid = self
            .password_matches(request.password, hash.clone())
            .await?;
        let row = row.filter(|_| valid).ok_or(IdentityError::Unauthorized)?;
        let principal: String = row.get("principal_id");
        let mut tx = self.pool.begin().await.map_err(db)?;
        let generation: Option<i64> = sqlx::query_scalar("SELECT authorization_generation FROM principals WHERE principal_id=$1 AND status='active' FOR UPDATE")
            .bind(&principal).fetch_optional(&mut *tx).await.map_err(db)?;
        let generation = generation.ok_or(IdentityError::Unauthorized)?;
        // Recheck password after acquiring the identity lock (concurrent recovery/password change).
        let current: String = sqlx::query_scalar(
            "SELECT password_hash FROM layrs_identity.accounts WHERE principal_id=$1 FOR UPDATE",
        )
        .bind(&principal)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if current != hash {
            return Err(IdentityError::Unauthorized);
        }
        sqlx::query("UPDATE layrs_identity.accounts SET failed_attempts=0 WHERE principal_id=$1")
            .bind(&principal)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let session = issue_session(&mut tx, &principal, generation).await?;
        tx.commit().await.map_err(db)?;
        Ok(session)
    }

    pub async fn revoke(&self, token: &str) -> Result<()> {
        sqlx::query("UPDATE layrs_identity.tokens SET revoked_at=clock_timestamp() WHERE token_digest=$1 AND revoked_at IS NULL")
            .bind(token_digest(token)).execute(&self.pool).await.map_err(db)?;
        Ok(())
    }

    /// Offline administrator recovery; caller must hold the database maintenance credential.
    pub async fn recover_password(&self, login: &str, password: String) -> Result<()> {
        let login = normalize_login(login)?;
        let hash = self.password_hash(password).await?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        let principal: Option<String> = sqlx::query_scalar("SELECT p.principal_id FROM principals p JOIN layrs_identity.accounts a USING(principal_id) WHERE a.deployment_id=$1 AND a.login=$2 AND p.status='active' FOR UPDATE OF p")
            .bind(&self.deployment_id).bind(login).fetch_optional(&mut *tx).await.map_err(db)?;
        let principal = principal.ok_or(IdentityError::Unauthorized)?;
        sqlx::query("UPDATE principals SET authorization_generation=authorization_generation+1 WHERE principal_id=$1").bind(&principal).execute(&mut *tx).await.map_err(db)?;
        sqlx::query("UPDATE layrs_identity.accounts SET password_hash=$2,failed_attempts=0 WHERE principal_id=$1").bind(principal).bind(hash).execute(&mut *tx).await.map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(())
    }
}

async fn whoami(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>> {
    identity.check_origin(&headers)?;
    let principal = identity
        .verify_bearer(bearer(&headers)?)
        .await
        .map_err(|_| IdentityError::Unauthorized)?;
    let row = sqlx::query("SELECT principal_id,actor_id,display_name,principal_kind FROM principals WHERE principal_id=$1 AND status='active'")
        .bind(principal.as_str()).fetch_optional(&identity.pool).await.map_err(|_| IdentityError::Unavailable)?.ok_or(IdentityError::Unauthorized)?;
    Ok(Json(
        serde_json::json!({"principal_id":row.get::<String,_>("principal_id"),"actor_id":row.get::<String,_>("actor_id"),"display_name":row.get::<String,_>("display_name"),"kind":row.get::<String,_>("principal_kind")}),
    ))
}

#[async_trait]
impl BearerVerifier for PgIdentity {
    async fn verify_bearer(&self, token: &str) -> std::result::Result<PrincipalId, V3ServiceError> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(V3ServiceError::Unauthorized);
        }
        let id: Option<String> = sqlx::query_scalar("SELECT p.principal_id FROM layrs_identity.tokens t JOIN principals p USING(principal_id) JOIN deployments d USING(deployment_id) WHERE t.token_digest=$1 AND p.deployment_id=$2 AND t.revoked_at IS NULL AND t.expires_at>clock_timestamp() AND p.status='active' AND d.status='ready' AND p.authorization_generation=t.authorization_generation")
            .bind(token_digest(token)).bind(&self.deployment_id).fetch_optional(&self.pool).await.map_err(|_| V3ServiceError::Unavailable)?;
        PrincipalId::parse(id.ok_or(V3ServiceError::Unauthorized)?)
            .map_err(|_| V3ServiceError::Internal)
    }
}

async fn issue_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &str,
    generation: i64,
) -> Result<Session> {
    let token = random_secret();
    sqlx::query("INSERT INTO layrs_identity.tokens(token_digest,principal_id,authorization_generation,token_kind,label,expires_at) VALUES($1,$2,$3,'session','login',clock_timestamp()+interval '8 hours')")
        .bind(token_digest(&token)).bind(principal).bind(generation).execute(&mut **tx).await.map_err(db)?;
    Ok(Session {
        principal_id: PrincipalId::parse(principal).map_err(|_| IdentityError::Unavailable)?,
        access_token: token,
        token_type: "Bearer",
        expires_in: 8 * 3600,
    })
}

fn bearer(headers: &HeaderMap) -> Result<&str> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(IdentityError::Unauthorized);
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() == 64)
        .ok_or(IdentityError::Unauthorized)
}
async fn bootstrap(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<BootstrapRequest>,
) -> Result<impl IntoResponse> {
    identity.check_origin(&headers)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(identity.bootstrap(request).await?),
    ))
}
async fn login(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<Credentials>,
) -> Result<impl IntoResponse> {
    identity.check_origin(&headers)?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(identity.login(request).await?),
    ))
}
async fn logout(State(identity): State<PgIdentity>, headers: HeaderMap) -> Result<StatusCode> {
    identity.check_origin(&headers)?;
    identity.revoke(bearer(&headers)?).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn change_password(
    State(identity): State<PgIdentity>,
    headers: HeaderMap,
    Json(request): Json<ChangePassword>,
) -> Result<StatusCode> {
    identity.check_origin(&headers)?;
    let principal = identity
        .verify_bearer(bearer(&headers)?)
        .await
        .map_err(|_| IdentityError::Unauthorized)?;
    let row = sqlx::query(
        "SELECT login,password_hash FROM layrs_identity.accounts WHERE principal_id=$1",
    )
    .bind(principal.as_str())
    .fetch_optional(&identity.pool)
    .await
    .map_err(db)?
    .ok_or(IdentityError::Unauthorized)?;
    let previous_hash: String = row.get("password_hash");
    if !identity
        .password_matches(request.current_password, previous_hash.clone())
        .await?
    {
        return Err(IdentityError::Unauthorized);
    }
    let replacement = identity.password_hash(request.new_password).await?;
    let mut tx = identity.pool.begin().await.map_err(db)?;
    let active: Option<String> = sqlx::query_scalar(
        "SELECT principal_id FROM principals WHERE principal_id=$1 AND status='active' FOR UPDATE",
    )
    .bind(principal.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(db)?;
    if active.is_none() {
        return Err(IdentityError::Unauthorized);
    }
    let changed = sqlx::query("UPDATE layrs_identity.accounts SET password_hash=$2,failed_attempts=0 WHERE principal_id=$1 AND password_hash=$3")
        .bind(principal.as_str()).bind(replacement).bind(previous_hash).execute(&mut *tx).await.map_err(db)?;
    if changed.rows_affected() != 1 {
        return Err(IdentityError::Unauthorized);
    }
    sqlx::query("UPDATE principals SET authorization_generation=authorization_generation+1 WHERE principal_id=$1")
        .bind(principal.as_str()).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}
pub fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
pub fn token_digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}
fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|v| v.to_string())
        .map_err(|_| IdentityError::Unavailable)
}
fn validate_password(password: &str) -> Result<()> {
    if password.len() < 12 || password.len() > 1024 {
        Err(IdentityError::Invalid)
    } else {
        Ok(())
    }
}
fn normalize_login(login: &str) -> Result<String> {
    let value = login.trim().to_ascii_lowercase();
    if value.len() < 3
        || value.len() > 254
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b))
    {
        Err(IdentityError::Invalid)
    } else {
        Ok(value)
    }
}
fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}
fn db(_: sqlx::Error) -> IdentityError {
    IdentityError::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn passwords_use_argon2id_and_random_salts() {
        let a = hash_password("correct horse battery staple").unwrap();
        let b = hash_password("correct horse battery staple").unwrap();
        assert!(a.starts_with("$argon2id$"));
        assert_ne!(a, b);
        assert!(
            Argon2::default()
                .verify_password(
                    b"correct horse battery staple",
                    &PasswordHash::new(&a).unwrap()
                )
                .is_ok()
        );
        assert!(
            Argon2::default()
                .verify_password(b"wrong", &PasswordHash::new(&a).unwrap())
                .is_err()
        );
    }
    #[test]
    fn tokens_are_random_and_only_digests_are_stored() {
        let a = random_secret();
        let b = random_secret();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert_ne!(token_digest(&a), a);
    }
    #[test]
    fn input_validation() {
        assert_eq!(
            normalize_login(" Alice@Example.com ").unwrap(),
            "alice@example.com"
        );
        assert!(normalize_login("bad login").is_err());
        assert!(validate_password("short").is_err());
        assert!(validate_password(&"a".repeat(1025)).is_err());
    }
    #[test]
    fn duplicate_authorization_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::AUTHORIZATION,
            format!("Bearer {}", random_secret()).parse().unwrap(),
        );
        assert!(bearer(&headers).is_ok());
        headers.append(header::AUTHORIZATION, "Bearer other".parse().unwrap());
        assert!(bearer(&headers).is_err());
    }
}
