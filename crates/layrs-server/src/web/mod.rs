use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Argon2, password_hash::rand_core::OsRng};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Form, Path, Query, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

use crate::{AuthRequirement, HttpMethod, ROUTES, RouteDescriptor};

mod access;
mod auth;
mod devices;
mod layers;
mod repository;
mod spaces;
mod studio;
mod teams;
mod workspaces;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../layrs-store-server/migrations");

const SESSION_COOKIE_NAME: &str = "layrs_session";
const SESSION_MAX_AGE_SECONDS: i64 = 7 * 24 * 60 * 60;
const DESKTOP_TOKEN_EXPIRES_SECONDS: i64 = 30 * 24 * 60 * 60;
const DEVICE_FLOW_EXPIRES_SECONDS: i64 = 10 * 60;
const DEVICE_FLOW_POLL_INTERVAL_SECONDS: i64 = 2;
const DEFAULT_ARTIFACT_DIFF_WINDOW_LIMIT: usize = 400;
const MAX_ARTIFACT_DIFF_WINDOW_LIMIT: usize = 1000;
const MAX_DIFF_COLUMN_WINDOW_LIMIT: usize = 4000;
const MAX_REQUEST_BODY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct WebServerConfig {
    pub addr: String,
    pub studio_url: String,
    pub database_url: String,
    pub deployment_id: String,
    pub cookie_secure: bool,
}

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    config: WebServerConfig,
}

#[derive(Clone, Debug)]
struct UserPrincipal {
    id: String,
    email: String,
    display_name: String,
}

#[derive(Debug)]
pub struct ServerError(String);

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ServerError {}

pub async fn serve(config: WebServerConfig) -> Result<(), ServerError> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .map_err(|error| ServerError(format!("could not connect to Postgres: {error}")))?;

    MIGRATOR
        .run(&pool)
        .await
        .map_err(|error| ServerError(format!("could not run migrations: {error}")))?;

    let state = AppState {
        pool,
        config: config.clone(),
    };
    let app = router(state)?;
    let addr = config
        .addr
        .parse::<SocketAddr>()
        .map_err(|error| ServerError(format!("invalid server address {}: {error}", config.addr)))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| ServerError(format!("could not bind {addr}: {error}")))?;

    println!("Layrs Server listening on http://{addr}");
    println!("Studio Web expected at {}", config.studio_url);
    println!("Auth/store: postgres/sqlx; migrations applied");

    axum::serve(listener, app)
        .await
        .map_err(|error| ServerError(format!("server failed: {error}")))
}

fn router(state: AppState) -> Result<Router, ServerError> {
    let origin = HeaderValue::from_str(&state.config.studio_url).map_err(|error| {
        ServerError(format!(
            "invalid Studio Web URL for CORS {}: {error}",
            state.config.studio_url
        ))
    })?;
    let cors = CorsLayer::new()
        .allow_origin(origin)
        .allow_credentials(true)
        .allow_headers([CONTENT_TYPE, AUTHORIZATION])
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ]);

    Ok(Router::new()
        .route("/", get(server_page))
        .route("/server", get(server_page))
        .route("/healthz", get(healthz))
        .route("/v1/routes", get(routes))
        .route("/v1/auth/signup", post(auth::signup))
        .route("/v1/auth/login", post(auth::login))
        .route("/v1/auth/logout", post(auth::logout))
        .route("/v1/auth/session", get(auth::auth_session))
        .route("/v1/me", get(auth::me))
        .route("/v1/lenses", get(list_lenses))
        .route("/v1/studio/snapshot", get(studio::studio_snapshot))
        .route(
            "/v1/workspaces",
            get(workspaces::list_workspaces).post(workspaces::create_workspace),
        )
        .route(
            "/v1/workspaces/:workspace_id/teams",
            get(teams::list_teams).post(teams::create_team),
        )
        .route(
            "/v1/workspaces/:workspace_id/teams/:team_id",
            get(teams::get_team),
        )
        .route(
            "/v1/workspaces/:workspace_id/teams/:team_id/members",
            get(teams::list_team_members).post(teams::add_team_member),
        )
        .route(
            "/v1/workspaces/:workspace_id/teams/:team_id/members/:account_id",
            delete(teams::remove_team_member),
        )
        .route(
            "/v1/workspaces/:workspace_id/members",
            get(teams::list_workspace_members),
        )
        .route(
            "/v1/workspaces/:workspace_id/invitations",
            get(teams::list_workspace_invitations).post(teams::create_invitation),
        )
        .route("/v1/me/invitations", get(teams::list_my_invitations))
        .route(
            "/v1/invitations/:invitation_id/accept",
            post(teams::accept_invitation),
        )
        .route(
            "/v1/invitations/:invitation_id/decline",
            post(teams::decline_invitation),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces",
            post(spaces::create_space),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id",
            delete(spaces::delete_space),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/from-local",
            post(spaces::create_space_from_local),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers",
            post(layers::create_layer),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id",
            delete(layers::delete_layer),
        )
        .route("/v1/devices", get(devices::list_devices))
        .route(
            "/v1/workspaces/:workspace_id/audit-events",
            get(list_audit_events),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/local-space-bootstrap",
            get(local_space_bootstrap),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/sync/receive",
            post(receive_local_space_sync),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/sync/publish",
            post(publish_local_space_sync),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/chunks/prepare",
            post(prepare_space_chunks),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/chunks/:chunk_id",
            put(put_space_chunk).get(get_space_chunk),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/access",
            get(access::get_layer_access_policy).put(access::put_layer_access_policy),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/timeline",
            get(list_layer_timeline),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/steps",
            get(list_layer_steps),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/steps/:step_id",
            get(get_layer_step),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/steps/:step_id/diff",
            get(get_layer_step_diff),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/artifacts",
            get(list_layer_artifacts),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/artifacts/:artifact_id/content",
            get(get_layer_artifact_content),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/artifacts/:artifact_id/diff",
            get(get_layer_artifact_diff),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/access/rules",
            post(access::create_layer_access_rule),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/access/rules/:rule_id",
            patch(access::update_layer_access_rule).delete(access::delete_layer_access_rule),
        )
        .route("/v1/desktop/device/start", post(devices::start_device_flow))
        .route("/v1/desktop/device/poll", post(devices::poll_device_flow))
        .route("/v1/desktop/device", get(devices::device_verification_page))
        .route(
            "/v1/desktop/device/approve",
            post(devices::approve_device_flow),
        )
        .route("/v1/desktop/bootstrap", get(devices::desktop_bootstrap))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(cors)
        .with_state(state))
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(value: sqlx::Error) -> Self {
        if let sqlx::Error::Database(database_error) = &value {
            if database_error.code().as_deref() == Some("23505") {
                return Self::conflict("resource already exists");
            }
        }

        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database_error",
            value.to_string(),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": {
                    "code": self.code,
                    "message": self.message
                }
            })),
        )
            .into_response()
    }
}

#[derive(Deserialize)]
struct SignupRequest {
    email: String,
    password: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct CreateWorkspaceBody {
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct CreateTeamBody {
    name: String,
    #[serde(default)]
    purpose: Option<String>,
}

#[derive(Deserialize)]
struct AddTeamMemberBody {
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default, rename = "accountId")]
    account_id_camel: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Deserialize)]
struct CreateInvitationBody {
    email: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    team_ids: Vec<String>,
    #[serde(default, rename = "teamIds")]
    team_ids_camel: Vec<String>,
}

#[derive(Deserialize)]
struct CreateSpaceBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "teamId")]
    team_id: Option<String>,
}

#[derive(Deserialize)]
struct CreateSpaceFromLocalBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    local_space_id: Option<String>,
    #[serde(default, rename = "localSpaceId")]
    local_space_id_camel: Option<String>,
    #[serde(default)]
    layers: Vec<LocalLayerImportBody>,
}

#[derive(Deserialize)]
struct LocalLayerImportBody {
    #[serde(default)]
    local_layer_id: Option<String>,
    #[serde(default, rename = "localLayerId")]
    local_layer_id_camel: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parent_local_layer_id: Option<String>,
    #[serde(default, rename = "parentLocalLayerId")]
    parent_local_layer_id_camel: Option<String>,
}

#[derive(Deserialize)]
struct CreateLayerBody {
    name: String,
    #[serde(default, rename = "parentId")]
    parent_id: Option<String>,
    #[serde(default)]
    parent_layer_id: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

#[derive(Deserialize)]
struct SnapshotQuery {
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
struct TimelineQuery {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct ArtifactDiffQuery {
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    column_start: Option<usize>,
    #[serde(default, rename = "columnStart")]
    column_start_camel: Option<usize>,
    #[serde(default)]
    column_limit: Option<usize>,
    #[serde(default, rename = "columnLimit")]
    column_limit_camel: Option<usize>,
    #[serde(default)]
    base_layer_id: Option<String>,
    #[serde(default, rename = "baseLayerId")]
    base_layer_id_camel: Option<String>,
}

#[derive(Deserialize)]
struct StepDiffQuery {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    start: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    column_start: Option<usize>,
    #[serde(default, rename = "columnStart")]
    column_start_camel: Option<usize>,
    #[serde(default)]
    column_limit: Option<usize>,
    #[serde(default, rename = "columnLimit")]
    column_limit_camel: Option<usize>,
}

#[derive(Deserialize)]
struct SyncReceiveBody {
    #[serde(default)]
    layer_id: Option<String>,
    #[serde(default, rename = "layerId")]
    layer_id_camel: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct SyncPublishBody {
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    layer_id: Option<String>,
    #[serde(default, rename = "layerId")]
    layer_id_camel: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    artifacts: Vec<PublishArtifactBody>,
    #[serde(default)]
    artifact: Option<PublishArtifactBody>,
    #[serde(default)]
    deleted_paths: Vec<String>,
    #[serde(default, rename = "deletedPaths")]
    deleted_paths_camel: Vec<String>,
    #[serde(default)]
    policy_epoch: Option<i64>,
    #[serde(default, rename = "policyEpoch")]
    policy_epoch_camel: Option<i64>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default, rename = "idempotencyKey")]
    idempotency_key_camel: Option<String>,
    #[serde(default)]
    source_client_id: Option<String>,
    #[serde(default, rename = "sourceClientId")]
    source_client_id_camel: Option<String>,
    #[serde(default)]
    root_tree_id: Option<String>,
    #[serde(default, rename = "rootTreeId")]
    root_tree_id_camel: Option<String>,
    #[serde(default)]
    base_tree_id: Option<String>,
    #[serde(default, rename = "baseTreeId")]
    base_tree_id_camel: Option<String>,
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default, rename = "changedPaths")]
    changed_paths_camel: Vec<String>,
    #[serde(default)]
    store_objects: Option<PublishStoreObjectsBody>,
    #[serde(default, rename = "storeObjects")]
    store_objects_camel: Option<PublishStoreObjectsBody>,
    #[serde(default)]
    step: Option<SyncStepBody>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncStepBody {
    #[serde(default, alias = "step_id")]
    step_id: Option<String>,
    #[serde(default, alias = "parent_step_id")]
    parent_step_id: Option<String>,
    #[serde(default, alias = "base_layer_id")]
    base_layer_id: Option<String>,
    #[serde(default, alias = "base_tree_id")]
    base_tree_id: Option<String>,
    #[serde(default, alias = "root_tree_id")]
    root_tree_id: Option<String>,
    #[serde(default, alias = "changed_paths")]
    changed_paths: Vec<String>,
    #[serde(default, alias = "captured_at_unix")]
    captured_at_unix: Option<i64>,
}

#[derive(Deserialize)]
struct PublishArtifactBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    artifact_id: Option<String>,
    #[serde(default, rename = "artifactId")]
    artifact_id_camel: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    logical_path: Option<String>,
    #[serde(default, rename = "logicalPath")]
    logical_path_camel: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default, rename = "type")]
    artifact_type: Option<String>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default, rename = "mediaType")]
    media_type_camel: Option<String>,
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    file_object_id: Option<String>,
    #[serde(default, rename = "fileObjectId")]
    file_object_id_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
    #[serde(default)]
    tree_id: Option<String>,
    #[serde(default, rename = "treeId")]
    tree_id_camel: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default, rename = "contentHash")]
    content_hash: Option<String>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default)]
    chunks: Vec<PublishArtifactChunkBody>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    deleted: Option<bool>,
}

#[derive(Deserialize)]
struct PublishArtifactChunkBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    chunk_id: Option<String>,
    #[serde(default, rename = "chunkId")]
    chunk_id_camel: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default)]
    byte_offset: Option<i64>,
    #[serde(default, rename = "byteOffset")]
    byte_offset_camel: Option<i64>,
}

#[derive(Deserialize)]
struct PrepareChunksBody {
    #[serde(default)]
    chunks: Vec<PrepareChunkItem>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PrepareChunkItem {
    Id(String),
    Object {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        chunk_id: Option<String>,
        #[serde(default, rename = "chunkId")]
        chunk_id_camel: Option<String>,
        #[serde(default)]
        sha256: Option<String>,
        #[serde(default)]
        size_bytes: Option<i64>,
        #[serde(default, rename = "sizeBytes")]
        size_bytes_camel: Option<i64>,
        #[serde(default)]
        media_type: Option<String>,
        #[serde(default, rename = "mediaType")]
        media_type_camel: Option<String>,
    },
}

async fn server_page(State(state): State<AppState>) -> Html<String> {
    Html(format!(
        r#"<!doctype html>
<html lang="en">
  <head><meta charset="utf-8"><title>Layrs Server</title></head>
  <body>
    <main>
      <h1>Layrs Server</h1>
      <p>Postgres/sqlx runtime is active.</p>
      <p><a href="/healthz">/healthz</a> · <a href="/v1/routes">/v1/routes</a> · <a href="{studio}">Studio Web</a></p>
    </main>
  </body>
</html>"#,
        studio = escape_html(&state.config.studio_url)
    ))
}

async fn healthz(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    sqlx::query("SELECT 1").execute(&state.pool).await?;

    Ok(Json(json!({
        "service": "layrs-server",
        "status": "ok",
        "runtime": "axum",
        "store": "postgres",
        "deployment_id": state.config.deployment_id
    })))
}

async fn routes() -> Json<Value> {
    Json(Value::Array(ROUTES.iter().map(route_json).collect()))
}

async fn list_lenses() -> Json<Value> {
    Json(crate::lenses::registry_response_value_from_env())
}

async fn signup(
    State(state): State<AppState>,
    Json(body): Json<SignupRequest>,
) -> Result<Response, ApiError> {
    let email = normalize_email(&body.email)?;
    validate_password(&body.password)?;
    let display_name = body
        .display_name
        .or(body.name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| email.clone());
    let account_id = prefixed_id("account");
    let password_hash = hash_password(&body.password)?;

    let mut tx = state.pool.begin().await?;
    sqlx::query("INSERT INTO accounts (account_id, email, display_name) VALUES ($1, $2, $3)")
        .bind(&account_id)
        .bind(&email)
        .bind(&display_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO account_passwords (account_id, password_hash) VALUES ($1, $2)")
        .bind(&account_id)
        .bind(&password_hash)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let user = UserPrincipal {
        id: account_id,
        email,
        display_name,
    };
    write_audit(
        &state.pool,
        None,
        Some(&user.id),
        "auth.signup",
        "account",
        Some(&user.id),
        json!({}),
    )
    .await?;
    session_response(&state, &user, StatusCode::CREATED).await
}

async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let email = normalize_email(&body.email)?;
    let row = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name, p.password_hash
        FROM accounts a
        JOIN account_passwords p ON p.account_id = a.account_id
        WHERE a.email = $1 AND a.status = 'active'
        "#,
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("email or password is incorrect"))?;
    let password_hash: String = row.get("password_hash");
    verify_password(&body.password, &password_hash)?;
    let user = row_user(&row);

    write_audit(
        &state.pool,
        None,
        Some(&user.id),
        "auth.login",
        "studio",
        None,
        json!({}),
    )
    .await?;
    session_response(&state, &user, StatusCode::OK).await
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let Some(session_token) = session_cookie(&headers) else {
        return Err(ApiError::unauthorized("authentication is required"));
    };
    let digest = digest_secret(&session_token);
    sqlx::query("UPDATE web_sessions SET revoked_at = now() WHERE session_token_digest = $1")
        .bind(digest)
        .execute(&state.pool)
        .await?;

    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&expired_session_cookie(&state.config))
            .map_err(|error| ApiError::bad_request(error.to_string()))?,
    );
    Ok(response)
}

async fn auth_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    Ok(Json(auth_session_value(&state.pool, &user).await?))
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    Ok(Json(json!({ "user": user_wire_json(&user) })))
}

async fn list_workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    Ok(Json(json!({
        "items": workspace_values(&state.pool, &user.id).await?
    })))
}

async fn create_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateWorkspaceBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    let name = required_body_text("name", &body.name)?;
    let slug = body.slug.unwrap_or_else(|| slugify(&name));
    let workspace_id = prefixed_id("workspace");

    create_workspace_owner_only(&state.pool, &user.id, &workspace_id, &name, &slug).await?;

    write_audit(
        &state.pool,
        Some(&workspace_id),
        Some(&user.id),
        "workspace.create",
        "workspace",
        Some(&workspace_id),
        json!({ "name": name, "slug": slug }),
    )
    .await?;

    Ok(Json(workspace_value(
        &workspace_id,
        &name,
        &slug,
        body.description.as_deref().unwrap_or_default(),
        "2026-06-29T00:00:00Z",
    )))
}

async fn create_team(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateTeamBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let team_id = prefixed_id("team");
    let name = required_body_text("name", &body.name)?;
    let purpose = body.purpose.unwrap_or_default();

    sqlx::query("INSERT INTO teams (team_id, workspace_id, name, purpose) VALUES ($1, $2, $3, $4)")
        .bind(&team_id)
        .bind(&workspace_id)
        .bind(&name)
        .bind(&purpose)
        .execute(&state.pool)
        .await?;
    sqlx::query(
        "INSERT INTO team_memberships (team_id, account_id, role) VALUES ($1, $2, 'maintainer')",
    )
    .bind(&team_id)
    .bind(&user.id)
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({
        "id": team_id,
        "workspaceId": workspace_id,
        "name": name,
        "purpose": purpose,
        "members": 1,
        "gateResponsibility": "workspace"
    })))
}

async fn list_teams(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(
        json!({ "items": team_values(&state.pool, &workspace_id).await? }),
    ))
}

async fn get_team(
    State(state): State<AppState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(
        team_value(&state.pool, &workspace_id, &team_id).await?,
    ))
}

async fn list_team_members(
    State(state): State<AppState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_team_in_workspace(&state.pool, &workspace_id, &team_id).await?;
    Ok(Json(json!({
        "items": team_member_values(&state.pool, &workspace_id, &team_id).await?
    })))
}

async fn add_team_member(
    State(state): State<AppState>,
    Path((workspace_id, team_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<AddTeamMemberBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_team_in_workspace(&state.pool, &workspace_id, &team_id).await?;
    let role = team_member_role(body.role.as_deref())?;
    let account_id = body.account_id.or(body.account_id_camel);

    if let Some(account_id) = account_id {
        ensure_account_in_workspace(&state.pool, &workspace_id, &account_id).await?;
        upsert_team_member(&state.pool, &workspace_id, &team_id, &account_id, role).await?;
        return Ok(Json(json!({
            "member": team_member_value(&state.pool, &workspace_id, &team_id, &account_id).await?
        })));
    }

    let email = normalize_email(
        body.email
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("account_id or email is required"))?,
    )?;
    if let Some(existing) = load_user_by_email(&state.pool, &email).await? {
        ensure_workspace_membership(&state.pool, &workspace_id, &existing.id, "member").await?;
        upsert_team_member(&state.pool, &workspace_id, &team_id, &existing.id, role).await?;
        return Ok(Json(json!({
            "member": team_member_value(&state.pool, &workspace_id, &team_id, &existing.id).await?
        })));
    }

    let invitation_id = create_pending_invitation(
        &state.pool,
        &workspace_id,
        &email,
        "member",
        &user.id,
        &[team_id],
        role,
    )
    .await?;
    Ok(Json(json!({
        "invitation": invitation_value(&state.pool, &invitation_id).await?
    })))
}

async fn remove_team_member(
    State(state): State<AppState>,
    Path((workspace_id, team_id, account_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_team_in_workspace(&state.pool, &workspace_id, &team_id).await?;
    let result = sqlx::query(
        r#"
        DELETE FROM team_memberships tm
        USING teams t
        WHERE tm.team_id = t.team_id
          AND t.workspace_id = $1
          AND tm.team_id = $2
          AND tm.account_id = $3
        "#,
    )
    .bind(&workspace_id)
    .bind(&team_id)
    .bind(&account_id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("team member not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn list_workspace_members(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(json!({
        "items": workspace_member_values(&state.pool, &workspace_id).await?
    })))
}

async fn list_workspace_invitations(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(json!({
        "items": invitation_values_for_workspace(&state.pool, &workspace_id).await?
    })))
}

async fn create_invitation(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateInvitationBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let email = normalize_email(&body.email)?;
    let role = workspace_role(body.role.as_deref())?;
    let team_ids = invitation_team_ids(body.team_ids, body.team_ids_camel);
    ensure_teams_in_workspace(&state.pool, &workspace_id, &team_ids).await?;
    let invitation_id = create_pending_invitation(
        &state.pool,
        &workspace_id,
        &email,
        role,
        &user.id,
        &team_ids,
        "member",
    )
    .await?;
    Ok(Json(invitation_value(&state.pool, &invitation_id).await?))
}

async fn list_my_invitations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    Ok(Json(json!({
        "items": invitation_values_for_email(&state.pool, &user.email).await?
    })))
}

async fn accept_invitation(
    State(state): State<AppState>,
    Path(invitation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    accept_or_decline_invitation(&state.pool, &invitation_id, &user, true).await?;
    Ok(Json(invitation_value(&state.pool, &invitation_id).await?))
}

async fn decline_invitation(
    State(state): State<AppState>,
    Path(invitation_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    accept_or_decline_invitation(&state.pool, &invitation_id, &user, false).await?;
    Ok(Json(invitation_value(&state.pool, &invitation_id).await?))
}

async fn create_space(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateSpaceBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let space_id = prefixed_id("space");
    let layer_id = prefixed_id("layer");
    let name = required_body_text("name", &body.name)?;
    let slug = slugify(&name);

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO spaces (space_id, workspace_id, slug, name, description, created_by_account_id) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&space_id)
    .bind(&workspace_id)
    .bind(&slug)
    .bind(&name)
    .bind(body.description.as_deref().unwrap_or_default())
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO space_memberships (membership_id, space_id, account_id, role) VALUES ($1, $2, $3, 'admin')",
    )
    .bind(prefixed_id("membership"))
    .bind(&space_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO layers (layer_id, workspace_id, space_id, name, created_by_account_id) VALUES ($1, $2, $3, 'Main', $4)",
    )
    .bind(&layer_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    insert_empty_layer_policy(&mut tx, &workspace_id, &space_id, &layer_id, Some(&user.id)).await?;
    write_timeline_in_tx(
        &mut tx,
        &workspace_id,
        Some(&space_id),
        Some(&layer_id),
        "layer.created",
        "Layer created",
        json!({
            "layerId": layer_id,
            "name": "Main",
            "parentLayerId": Value::Null,
            "source": "space.create"
        }),
    )
    .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "id": space_id,
        "workspaceId": workspace_id,
        "teamId": body.team_id.unwrap_or_default(),
        "name": name,
        "description": body.description.unwrap_or_default(),
        "status": "pending",
        "currentLayerId": layer_id,
        "updatedAt": "2026-06-29T00:00:00Z"
    })))
}

async fn delete_space(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;

    let row = sqlx::query(
        r#"
        SELECT
            s.name,
            (SELECT count(*)::bigint FROM layers l WHERE l.workspace_id = s.workspace_id AND l.space_id = s.space_id) AS layer_count,
            (SELECT count(*)::bigint FROM artifacts a WHERE a.workspace_id = s.workspace_id AND a.space_id = s.space_id) AS artifact_count
        FROM spaces s
        WHERE s.workspace_id = $1 AND s.space_id = $2
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .fetch_one(&state.pool)
    .await?;
    let space_name = row.get::<String, _>("name");
    let layer_count = row.get::<i64, _>("layer_count");
    let artifact_count = row.get::<i64, _>("artifact_count");

    let mut tx = state.pool.begin().await?;
    delete_space_storage_in_tx(&mut tx, &workspace_id, &space_id).await?;
    sqlx::query("DELETE FROM spaces WHERE workspace_id = $1 AND space_id = $2")
        .bind(&workspace_id)
        .bind(&space_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    write_audit(
        &state.pool,
        Some(&workspace_id),
        Some(&user.id),
        "space.deleted",
        "space",
        Some(&space_id),
        json!({
            "spaceId": space_id,
            "name": space_name,
            "deletedLayers": layer_count,
            "deletedArtifacts": artifact_count
        }),
    )
    .await?;

    Ok(Json(json!({
        "id": space_id,
        "workspaceId": workspace_id,
        "deleted": true,
        "deletedLayers": layer_count,
        "deletedArtifacts": artifact_count
    })))
}

async fn create_space_from_local(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateSpaceFromLocalBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;

    let name = required_body_text("name", &body.name)?;
    let slug = slugify(&name);
    let space_id = prefixed_id("space");
    let main_layer_id = prefixed_id("layer");
    let local_space_id = body
        .local_space_id
        .or(body.local_space_id_camel)
        .unwrap_or_default();
    let mut local_layers = body.layers;
    if local_layers.is_empty() {
        local_layers.push(LocalLayerImportBody {
            local_layer_id: Some("local_layer_main".to_string()),
            local_layer_id_camel: None,
            name: Some("Main".to_string()),
            parent_local_layer_id: None,
            parent_local_layer_id_camel: None,
        });
    }

    let first_local_layer_id = local_layer_import_id(&local_layers[0], "local_layer_main");
    let mut local_to_server = BTreeMap::<String, String>::new();
    local_to_server.insert(first_local_layer_id.clone(), main_layer_id.clone());
    let mut layer_mappings = vec![json!({
        "localLayerId": first_local_layer_id,
        "serverLayerId": main_layer_id,
        "name": "Main"
    })];

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO spaces (space_id, workspace_id, slug, name, description, created_by_account_id) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&space_id)
    .bind(&workspace_id)
    .bind(&slug)
    .bind(&name)
    .bind(body.description.as_deref().unwrap_or_default())
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO space_memberships (membership_id, space_id, account_id, role) VALUES ($1, $2, $3, 'admin')",
    )
    .bind(prefixed_id("membership"))
    .bind(&space_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO layers (layer_id, workspace_id, space_id, name, created_by_account_id) VALUES ($1, $2, $3, 'Main', $4)",
    )
    .bind(&main_layer_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    insert_empty_layer_policy(
        &mut tx,
        &workspace_id,
        &space_id,
        &main_layer_id,
        Some(&user.id),
    )
    .await?;
    write_timeline_in_tx(
        &mut tx,
        &workspace_id,
        Some(&space_id),
        Some(&main_layer_id),
        "layer.created",
        "Layer created",
        json!({
            "layerId": main_layer_id,
            "name": "Main",
            "parentLayerId": Value::Null,
            "source": "local-space.import",
            "localSpaceId": local_space_id
        }),
    )
    .await?;

    for local_layer in local_layers.iter().skip(1) {
        let local_layer_id = local_layer_import_id(local_layer, "local_layer");
        let display_name = local_layer
            .name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Imported Layer")
            .trim()
            .to_string();
        let server_layer_id = prefixed_id("layer");
        let parent_local_id = local_layer
            .parent_local_layer_id
            .clone()
            .or_else(|| local_layer.parent_local_layer_id_camel.clone());
        let parent_layer_id = parent_local_id
            .as_ref()
            .and_then(|id| local_to_server.get(id))
            .cloned()
            .unwrap_or_else(|| main_layer_id.clone());

        sqlx::query(
            "INSERT INTO layers (layer_id, workspace_id, space_id, parent_layer_id, name, created_by_account_id) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&server_layer_id)
        .bind(&workspace_id)
        .bind(&space_id)
        .bind(&parent_layer_id)
        .bind(&display_name)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
        insert_empty_layer_policy(
            &mut tx,
            &workspace_id,
            &space_id,
            &server_layer_id,
            Some(&user.id),
        )
        .await?;
        write_timeline_in_tx(
            &mut tx,
            &workspace_id,
            Some(&space_id),
            Some(&server_layer_id),
            "layer.created",
            "Layer created",
            json!({
                "layerId": server_layer_id,
                "name": display_name,
                "parentLayerId": parent_layer_id,
                "source": "local-space.import",
                "localSpaceId": local_space_id,
                "localLayerId": local_layer_id
            }),
        )
        .await?;

        local_to_server.insert(local_layer_id.clone(), server_layer_id.clone());
        layer_mappings.push(json!({
            "localLayerId": local_layer_id,
            "serverLayerId": server_layer_id,
            "name": display_name,
            "parentServerLayerId": parent_layer_id
        }));
    }

    tx.commit().await?;

    Ok(Json(json!({
        "space": {
            "id": space_id,
            "workspaceId": workspace_id,
            "name": name,
            "description": body.description.unwrap_or_default(),
            "currentLayerId": main_layer_id,
            "status": "linked"
        },
        "localSpaceId": local_space_id,
        "layerMappings": layer_mappings
    })))
}

async fn create_layer(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CreateLayerBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let layer_id = prefixed_id("layer");
    let name = required_body_text("name", &body.name)?;
    let parent_layer_id = body.parent_id.or(body.parent_layer_id);
    if let Some(parent_layer_id) = &parent_layer_id {
        ensure_layer_in_space(&state.pool, &workspace_id, &space_id, parent_layer_id).await?;
    } else {
        ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    }

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO layers (layer_id, workspace_id, space_id, parent_layer_id, name, created_by_account_id) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&layer_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(parent_layer_id.as_deref())
    .bind(&name)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    let policy_id =
        insert_empty_layer_policy(&mut tx, &workspace_id, &space_id, &layer_id, Some(&user.id))
            .await?;
    if let Some(parent_layer_id) = &parent_layer_id {
        inherit_layer_access_rules_in_tx(
            &mut tx,
            &workspace_id,
            &space_id,
            parent_layer_id,
            &policy_id,
        )
        .await?;
    }
    write_timeline_in_tx(
        &mut tx,
        &workspace_id,
        Some(&space_id),
        Some(&layer_id),
        "layer.created",
        "Layer created",
        json!({
            "layerId": layer_id,
            "name": name,
            "parentLayerId": parent_layer_id,
            "source": "layers.create"
        }),
    )
    .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "id": layer_id,
        "spaceId": space_id,
        "parentId": parent_layer_id,
        "name": name,
        "kind": if parent_layer_id.is_some() { "proposal" } else { "base" },
        "status": "active",
        "summary": body.summary.unwrap_or_default(),
        "artifactIds": [],
        "stepIds": [],
        "gateIds": []
    })))
}

async fn delete_layer(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;

    let layer_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM layers WHERE workspace_id = $1 AND space_id = $2",
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .fetch_one(&state.pool)
    .await?;
    if layer_count <= 1 {
        return Err(ApiError::conflict("space must keep at least one layer"));
    }

    let child_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM layers WHERE workspace_id = $1 AND space_id = $2 AND parent_layer_id = $3",
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&layer_id)
    .fetch_one(&state.pool)
    .await?;
    if child_count > 0 {
        return Err(ApiError::conflict(
            "delete child layers before deleting their parent layer",
        ));
    }

    let layer_name: String = sqlx::query_scalar(
        "SELECT name FROM layers WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&layer_id)
    .fetch_one(&state.pool)
    .await?;

    let mut tx = state.pool.begin().await?;
    write_timeline_in_tx(
        &mut tx,
        &workspace_id,
        Some(&space_id),
        None,
        "layer.deleted",
        "Layer deleted",
        json!({
            "layerId": layer_id.clone(),
            "name": layer_name.clone(),
            "source": "layers.delete"
        }),
    )
    .await?;
    delete_layer_storage_in_tx(&mut tx, &workspace_id, &space_id, &layer_id).await?;
    sqlx::query("DELETE FROM layers WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3")
        .bind(&workspace_id)
        .bind(&space_id)
        .bind(&layer_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "id": layer_id,
        "spaceId": space_id,
        "deleted": true
    })))
}

async fn delete_layer_storage_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        DELETE FROM sync_batch_changes
        WHERE sync_batch_id IN (
            SELECT sync_batch_id
            FROM sync_batches
            WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM sync_batches WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM layer_heads WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM layer_states WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE tree_entries
        SET artifact_id = NULL
        WHERE artifact_id IN (
            SELECT artifact_id
            FROM artifacts
            WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE artifacts
        SET current_file_object_id = NULL,
            current_tree_id = NULL,
            updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM artifacts WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn delete_space_storage_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        DELETE FROM sync_batch_changes
        WHERE sync_batch_id IN (
            SELECT sync_batch_id
            FROM sync_batches
            WHERE workspace_id = $1 AND space_id = $2
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM sync_batches WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM layer_heads WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM layer_states WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        r#"
        DELETE FROM tree_entries
        WHERE tree_id IN (
            SELECT tree_id
            FROM tree_objects
            WHERE workspace_id = $1 AND space_id = $2
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE artifacts
        SET current_file_object_id = NULL,
            current_tree_id = NULL,
            updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM artifacts WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        r#"
        DELETE FROM file_object_chunks
        WHERE file_object_id IN (
            SELECT file_object_id
            FROM file_objects
            WHERE workspace_id = $1 AND space_id = $2
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM tree_objects WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM file_objects WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM object_chunks WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn studio_snapshot(
    State(state): State<AppState>,
    Query(query): Query<SnapshotQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    let workspaces = workspace_values(&state.pool, &user.id).await?;
    let workspace_id = query
        .workspace_id
        .or_else(|| {
            workspaces
                .first()
                .and_then(|workspace| workspace.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| ApiError::not_found("no workspace exists for this account"))?;
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.get("id").and_then(Value::as_str) == Some(&workspace_id))
        .cloned()
        .ok_or_else(|| ApiError::not_found("workspace not found"))?;

    Ok(Json(json!({
        "account": studio_account_json(&user),
        "session": session_json(&user, Some(&workspace_id)),
        "workspace": workspace,
        "workspaces": workspaces,
        "teams": team_values(&state.pool, &workspace_id).await?,
        "members": workspace_member_values(&state.pool, &workspace_id).await?,
        "invitations": invitation_values_for_workspace(&state.pool, &workspace_id).await?,
        "spaces": space_values(&state.pool, &workspace_id).await?,
        "layers": layer_values(&state.pool, &workspace_id).await?,
        "artifacts": artifact_values(&state.pool, &workspace_id).await?,
        "steps": [],
        "weaves": [],
        "proofs": [],
        "gates": [],
        "policies": [],
        "timeline": [],
        "accessRegistries": access_registry_values(&state.pool, &workspace_id).await?,
        "devices": device_values(&state.pool, &user.id).await?,
        "auditEvents": audit_event_values(&state.pool, &workspace_id).await?
    })))
}

async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    Ok(Json(
        json!({ "items": device_values(&state.pool, &user.id).await? }),
    ))
}

async fn list_audit_events(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(
        json!({ "items": audit_event_values(&state.pool, &workspace_id).await? }),
    ))
}

async fn local_space_bootstrap(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    let workspace = workspace_value_for_account(&state.pool, &workspace_id, &user.id).await?;
    let space = space_value(&state.pool, &workspace_id, &space_id).await?;
    let layers = layer_values_for_space(&state.pool, &workspace_id, &space_id).await?;
    let layer_ids = layers
        .iter()
        .filter_map(|layer| layer.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let timeline = timeline_event_values(
        &state.pool,
        &workspace_id,
        Some(&space_id),
        None,
        None,
        Some(50),
    )
    .await?;

    Ok(Json(json!({
        "workspace": workspace,
        "space": space,
        "layers": layers,
        "accessRegistries": access_registry_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids).await?,
        "timeline": {
            "cursor": latest_timeline_cursor(&timeline),
            "events": timeline
        },
        "lenses": crate::lenses::load_lens_registry_from_env().items,
        "artifacts": artifact_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids, &user.id).await?
    })))
}

async fn receive_local_space_sync(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<SyncReceiveBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    let layer_id = body
        .layer_id
        .or(body.layer_id_camel)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(layer_id) = layer_id.as_deref() {
        ensure_layer_in_space(&state.pool, &workspace_id, &space_id, layer_id).await?;
    } else {
        ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    }
    let layers = layer_values_for_space(&state.pool, &workspace_id, &space_id).await?;
    let layer_ids = layers
        .iter()
        .filter_map(|layer| layer.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let request_cursor = body.cursor;
    let timeline = timeline_event_values(
        &state.pool,
        &workspace_id,
        Some(&space_id),
        None,
        request_cursor.as_deref(),
        body.limit,
    )
    .await?;
    let response_cursor = latest_timeline_cursor(&timeline).or(request_cursor);
    let access_registries =
        access_registry_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids)
            .await?;
    let steps =
        layer_step_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids).await?;
    let content_objects = receive_store_objects_for_layers(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_ids,
        &user.id,
    )
    .await?;
    let layer_head =
        layer_head_value(&state.pool, &workspace_id, &space_id, layer_id.as_deref()).await?;
    let root_tree_id = layer_head.get("rootTreeId").cloned().unwrap_or(Value::Null);

    Ok(Json(json!({
        "protocol": "layrs.sync.v2",
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "rootTreeId": root_tree_id,
        "cursor": response_cursor,
        "layerHead": layer_head,
        "layers": layers,
        "accessRegistries": access_registries,
        "steps": steps,
        "timeline": timeline,
        "artifacts": artifact_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids, &user.id).await?,
        "contentObjects": content_objects,
        "contents": []
    })))
}

async fn publish_local_space_sync(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<SyncPublishBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_write(&state.pool, &workspace_id, &user.id).await?;
    let layer_id = require_layer_id(body.layer_id, body.layer_id_camel)?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let received_cursor = body.cursor;
    let protocol = body.protocol;
    let policy_epoch = body.policy_epoch.or(body.policy_epoch_camel);
    let idempotency_key = body.idempotency_key.or(body.idempotency_key_camel);
    let source_client_id = body.source_client_id.or(body.source_client_id_camel);
    let root_tree_id = body.root_tree_id.or(body.root_tree_id_camel);
    let base_tree_id = body.base_tree_id.or(body.base_tree_id_camel);
    let step = body.step;
    let mut changed_paths = body.changed_paths;
    changed_paths.extend(body.changed_paths_camel);
    let mut store_objects = Vec::new();
    if let Some(objects) = body.store_objects {
        store_objects.extend(objects.into_flat()?);
    }
    if let Some(objects) = body.store_objects_camel {
        store_objects.extend(objects.into_flat()?);
    }
    let mut artifacts = body.artifacts;
    if let Some(artifact) = body.artifact {
        artifacts.push(artifact);
    }
    let mut deleted_paths = body.deleted_paths;
    deleted_paths.extend(body.deleted_paths_camel);
    let mut publish_artifacts = Vec::new();
    for artifact in artifacts {
        if artifact_requests_deletion(&artifact) {
            deleted_paths.push(required_artifact_path(&artifact)?);
        } else {
            publish_artifacts.push(artifact);
        }
    }
    let deleted_paths = normalize_deleted_paths(&deleted_paths)?;

    let protocol_value = protocol.as_deref();
    if protocol_value != Some("layrs.sync.v2") {
        return Err(ApiError::bad_request(
            "protocol layrs.sync.v2 is required; inline artifact publish is not supported",
        ));
    }
    if store_objects.is_empty()
        && publish_artifacts.is_empty()
        && deleted_paths.is_empty()
        && step.is_none()
    {
        return Err(ApiError::bad_request(
            "at least one store object, artifact, deleted path, or step is required",
        ));
    }

    publish_local_space_sync_v2(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_id,
        &user.id,
        received_cursor,
        policy_epoch,
        idempotency_key,
        source_client_id,
        base_tree_id,
        root_tree_id,
        protocol,
        changed_paths,
        step,
        store_objects,
        publish_artifacts,
        deleted_paths,
    )
    .await
    .map(Json)
}

async fn publish_local_space_sync_v2(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
    received_cursor: Option<String>,
    expected_policy_epoch: Option<i64>,
    idempotency_key: Option<String>,
    source_client_id: Option<String>,
    requested_base_tree_id: Option<String>,
    requested_root_tree_id: Option<String>,
    protocol: Option<String>,
    changed_paths: Vec<String>,
    step: Option<SyncStepBody>,
    store_objects: Vec<PublishStoreObjectBody>,
    mut publish_artifacts: Vec<PublishArtifactBody>,
    mut deleted_paths: Vec<String>,
) -> Result<Value, ApiError> {
    if let Some(key) = idempotency_key.as_deref() {
        if let Some(response) = load_sync_batch_response(pool, workspace_id, space_id, key).await? {
            return Ok(response);
        }
    }
    let policy_epoch = current_policy_epoch(pool, workspace_id, space_id, layer_id).await?;
    if let Some(expected) = expected_policy_epoch {
        if expected != policy_epoch {
            return Err(ApiError::conflict(format!(
                "policy_epoch mismatch: expected {expected}, current {policy_epoch}"
            )));
        }
    }

    let mut tx = pool.begin().await?;
    let sync_batch_id = prefixed_id("sync_batch");
    if let Some(key) = idempotency_key.as_deref() {
        sqlx::query(
            r#"
            INSERT INTO sync_batches
                (sync_batch_id, workspace_id, space_id, layer_id, idempotency_key,
                 source_client_id, base_cursor, policy_epoch, status, created_by_account_id, request_json)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, 'reserved', $9, $10)
            "#,
        )
        .bind(&sync_batch_id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(layer_id)
        .bind(key)
        .bind(source_client_id.as_deref())
        .bind(received_cursor.as_deref())
        .bind(policy_epoch)
        .bind(account_id)
        .bind(json!({
            "protocol": protocol,
            "changedPaths": changed_paths,
            "storeObjectCount": store_objects.len(),
            "artifactCount": publish_artifacts.len(),
            "deletedPathCount": deleted_paths.len()
        }))
        .execute(&mut *tx)
        .await?;
    }

    let store_index =
        upsert_store_objects_in_tx(&mut tx, workspace_id, space_id, account_id, store_objects)
            .await?;
    deleted_paths.extend(store_index.deleted_paths.iter().cloned());
    if publish_artifacts.is_empty() {
        for (path, file) in &store_index.file_by_path {
            publish_artifacts.push(PublishArtifactBody {
                id: None,
                artifact_id: None,
                artifact_id_camel: None,
                path: Some(path.clone()),
                logical_path: None,
                logical_path_camel: None,
                kind: Some("file".to_string()),
                artifact_type: None,
                media_type: file.media_type.clone(),
                media_type_camel: None,
                content: None,
                file_object_id: Some(file.file_object_id.clone()),
                file_object_id_camel: None,
                object_id: None,
                object_id_camel: None,
                tree_id: None,
                tree_id_camel: None,
                sha256: Some(file.digest.clone()),
                content_hash: None,
                size_bytes: Some(file.size_bytes),
                size_bytes_camel: None,
                chunks: Vec::new(),
                state: None,
                operation: None,
                action: None,
                deleted: None,
            });
        }
    }
    let mut published_ids = Vec::new();
    let mut deleted_values = Vec::new();
    let mut event_ids = Vec::new();
    let mut change_index = 0;
    for mut artifact in publish_artifacts {
        if artifact.content.is_some() {
            return Err(ApiError::bad_request(
                "inline artifact content is not supported; upload chunks before publish",
            ));
        }
        if !publish_artifact_uses_v2(&artifact) {
            apply_store_object_to_artifact(&mut artifact, &store_index)?;
        }
        let artifact_path = required_artifact_path(&artifact).ok();
        if !publish_artifact_uses_v2(&artifact) {
            return Err(ApiError::bad_request(
                "artifact publish requires a fileObjectId or uploaded chunks",
            ));
        }
        let (artifact_id, event_id, file_object_id) = publish_artifact_v2_in_tx(
            pool,
            &mut tx,
            workspace_id,
            space_id,
            layer_id,
            account_id,
            artifact,
        )
        .await?;
        if idempotency_key.is_some() {
            insert_sync_batch_change_in_tx(
                &mut tx,
                &sync_batch_id,
                change_index,
                "upsert_file",
                Some(&artifact_id),
                artifact_path.as_deref(),
                file_object_id.as_deref(),
                None,
                json!({ "eventId": event_id }),
            )
            .await?;
            change_index += 1;
        }
        published_ids.push(artifact_id);
        event_ids.push(event_id);
    }
    for path in deleted_paths {
        let (artifact_id, artifact_value, event_id) = delete_artifact_tombstone_in_tx(
            pool,
            &mut tx,
            workspace_id,
            space_id,
            layer_id,
            account_id,
            &path,
        )
        .await?;
        if idempotency_key.is_some() {
            insert_sync_batch_change_in_tx(
                &mut tx,
                &sync_batch_id,
                change_index,
                "delete_path",
                Some(&artifact_id),
                Some(&path),
                None,
                None,
                json!({ "eventId": event_id }),
            )
            .await?;
            change_index += 1;
        }
        deleted_values.push(artifact_value);
        event_ids.push(event_id);
    }

    let root_tree_id =
        if let Some(root_tree_id) = requested_root_tree_id.or(store_index.root_tree_id) {
            ensure_tree_in_space_in_tx(&mut tx, workspace_id, space_id, &root_tree_id).await?;
            Some(root_tree_id)
        } else {
            rebuild_layer_tree_in_tx(&mut tx, workspace_id, space_id, layer_id, account_id).await?
        };
    let server_cursor = event_ids.last().cloned();
    let layer_state_id = advance_layer_head_in_tx(
        &mut tx,
        workspace_id,
        space_id,
        layer_id,
        root_tree_id.as_deref(),
        policy_epoch,
        server_cursor.as_deref(),
        account_id,
    )
    .await?;
    let step_id = insert_layer_step_in_tx(
        &mut tx,
        workspace_id,
        space_id,
        layer_id,
        step.as_ref(),
        requested_base_tree_id.as_deref(),
        root_tree_id.as_deref(),
        &changed_paths,
        source_client_id.as_deref(),
        if idempotency_key.is_some() {
            Some(sync_batch_id.as_str())
        } else {
            None
        },
        account_id,
    )
    .await?;
    if idempotency_key.is_some() {
        insert_sync_batch_change_in_tx(
            &mut tx,
            &sync_batch_id,
            change_index,
            "advance_head",
            None,
            None,
            None,
            root_tree_id.as_deref(),
            json!({ "layerStateId": layer_state_id }),
        )
        .await?;
        change_index += 1;
        insert_sync_batch_change_in_tx(
            &mut tx,
            &sync_batch_id,
            change_index,
            "record_step",
            None,
            None,
            None,
            root_tree_id.as_deref(),
            json!({ "stepId": step_id }),
        )
        .await?;
        sqlx::query(
            "UPDATE sync_batches SET status = 'applied', server_cursor = $1, updated_at = now() WHERE sync_batch_id = $2",
        )
        .bind(server_cursor.as_deref())
        .bind(&sync_batch_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let layer_artifacts =
        artifact_values_for_layer(pool, workspace_id, space_id, layer_id, account_id).await?;
    let published = published_ids
        .iter()
        .filter_map(|artifact_id| {
            layer_artifacts
                .iter()
                .find(|artifact| {
                    artifact.get("id").and_then(Value::as_str) == Some(artifact_id.as_str())
                })
                .cloned()
        })
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    for event_id in &event_ids {
        events.push(timeline_event_by_id(pool, workspace_id, event_id).await?);
    }
    let response = json!({
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "receivedCursor": received_cursor,
        "serverCursor": latest_timeline_cursor(&events).or(server_cursor),
        "policyEpoch": policy_epoch,
        "layerHead": {
            "layerId": layer_id,
            "layerStateId": layer_state_id,
            "rootTreeId": root_tree_id,
            "policyEpoch": policy_epoch
        },
        "step": {
            "stepId": step_id,
            "layerId": layer_id
        },
        "syncBatchId": if idempotency_key.is_some() { Some(sync_batch_id.as_str()) } else { None },
        "published": published,
        "deleted": deleted_values,
        "timeline": events
    });
    if idempotency_key.is_some() {
        sqlx::query("UPDATE sync_batches SET response_json = $1, updated_at = now() WHERE sync_batch_id = $2")
            .bind(&response)
            .bind(&sync_batch_id)
            .execute(pool)
            .await?;
    }
    Ok(response)
}

async fn prepare_space_chunks(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<PrepareChunksBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_write(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    if body.chunks.is_empty() {
        return Err(ApiError::bad_request("at least one chunk is required"));
    }

    let mut items = Vec::new();
    for item in body.chunks {
        let prepared = prepared_chunk_from_item(item)?;
        let exists =
            object_chunk_exists(&state.pool, &workspace_id, &space_id, &prepared.chunk_id).await?;
        items.push(json!({
            "chunkId": prepared.chunk_id,
            "digest": prepared.digest,
            "sizeBytes": prepared.size_bytes,
            "mediaType": prepared.media_type,
            "exists": exists,
            "uploadRequired": !exists,
            "uploadUrl": format!("/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/{}", prepared.chunk_id)
        }));
    }

    Ok(Json(json!({
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "items": items,
        "missing": items.iter().filter(|item| item.get("uploadRequired").and_then(Value::as_bool) == Some(true)).cloned().collect::<Vec<_>>()
    })))
}

async fn put_space_chunk(
    State(state): State<AppState>,
    Path((workspace_id, space_id, chunk_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_write(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    let chunk_id = validate_chunk_id(&chunk_id)?;
    let digest = blake3_digest_for_bytes(&bytes);
    if chunk_id != digest {
        return Err(ApiError::bad_request(
            "chunk_id must match the uploaded bytes blake3 digest",
        ));
    }
    let size_bytes = bytes.len() as i64;
    let object_key = format!("chunks/{workspace_id}/{space_id}/{chunk_id}");
    sqlx::query(
        r#"
        INSERT INTO object_chunks
            (chunk_id, workspace_id, space_id, digest, size_bytes, object_key, state, content_bytes, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, 'available', $7, $8)
        ON CONFLICT (chunk_id) DO UPDATE SET
            digest = EXCLUDED.digest,
            size_bytes = EXCLUDED.size_bytes,
            object_key = EXCLUDED.object_key,
            state = 'available',
            content_bytes = EXCLUDED.content_bytes,
            updated_at = now()
        WHERE object_chunks.workspace_id = EXCLUDED.workspace_id
          AND object_chunks.space_id = EXCLUDED.space_id
        "#,
    )
    .bind(&chunk_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&digest)
    .bind(size_bytes)
    .bind(&object_key)
    .bind(bytes.to_vec())
    .bind(&user.id)
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({
        "chunkId": chunk_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "digest": digest,
        "sizeBytes": size_bytes,
        "objectKey": object_key,
        "state": "available"
    })))
}

async fn get_space_chunk(
    State(state): State<AppState>,
    Path((workspace_id, space_id, chunk_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    let chunk_id = validate_chunk_id(&chunk_id)?;
    let row = sqlx::query(
        r#"
        SELECT content_bytes, media_type, size_bytes, digest
        FROM object_chunks
        WHERE workspace_id = $1 AND space_id = $2 AND chunk_id = $3 AND state = 'available'
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&chunk_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("chunk not found"))?;
    let bytes = row
        .try_get::<Vec<u8>, _>("content_bytes")
        .ok()
        .ok_or_else(|| ApiError::not_found("chunk bytes are not available"))?;
    let media_type = row
        .try_get::<String, _>("media_type")
        .ok()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, media_type)
        .header("x-layrs-chunk-id", chunk_id)
        .header("x-layrs-digest", row.get::<String, _>("digest"))
        .header(
            "content-length",
            row.get::<i64, _>("size_bytes").to_string(),
        )
        .body(Body::from(bytes))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(response)
}

async fn list_layer_timeline(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    Query(query): Query<TimelineQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let request_cursor = query.cursor;
    let events = timeline_event_values(
        &state.pool,
        &workspace_id,
        Some(&space_id),
        Some(&layer_id),
        request_cursor.as_deref(),
        query.limit,
    )
    .await?;
    let response_cursor = latest_timeline_cursor(&events).or(request_cursor);

    Ok(Json(json!({
        "cursor": response_cursor,
        "items": events
    })))
}

async fn list_layer_steps(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let steps = layer_step_detail_values(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_id,
        None,
        &user.id,
    )
    .await?;
    Ok(Json(json!({ "items": steps })))
}

async fn get_layer_step(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, step_id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let mut steps = layer_step_detail_values(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_id,
        Some(&step_id),
        &user.id,
    )
    .await?;
    let step = steps
        .pop()
        .ok_or_else(|| ApiError::not_found("step not found"))?;
    Ok(Json(step))
}

async fn get_layer_step_diff(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, step_id)): Path<(String, String, String, String)>,
    Query(query): Query<StepDiffQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let start = query.start.unwrap_or(0);
    let limit = artifact_window_limit(query.limit)?;
    let column_start = query.column_start.or(query.column_start_camel).unwrap_or(0);
    let column_limit = artifact_column_limit(query.column_limit.or(query.column_limit_camel))?;
    Ok(Json(
        step_diff_window_value(
            &state.pool,
            &workspace_id,
            &space_id,
            &layer_id,
            &step_id,
            query.path.as_deref(),
            &user.id,
            WindowRequest {
                start,
                limit,
                column_start,
                column_limit,
            },
        )
        .await?,
    ))
}

async fn list_layer_artifacts(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;

    Ok(Json(json!({
        "items": artifact_values_for_layer(&state.pool, &workspace_id, &space_id, &layer_id, &user.id).await?
    })))
}

async fn get_layer_artifact_content(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, artifact_id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    Ok(Json(
        artifact_content_value(
            &state.pool,
            &workspace_id,
            &space_id,
            &layer_id,
            &artifact_id,
            &user.id,
        )
        .await?,
    ))
}

async fn get_layer_artifact_diff(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, artifact_id)): Path<(String, String, String, String)>,
    Query(query): Query<ArtifactDiffQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let start = query.start.unwrap_or(0);
    let limit = artifact_window_limit(query.limit)?;
    let column_start = query.column_start.or(query.column_start_camel).unwrap_or(0);
    let column_limit = artifact_column_limit(query.column_limit.or(query.column_limit_camel))?;
    let base_layer_id = query.base_layer_id.or(query.base_layer_id_camel);

    Ok(Json(
        artifact_diff_window_value(
            &state.pool,
            &workspace_id,
            &space_id,
            &layer_id,
            &artifact_id,
            &user.id,
            WindowRequest {
                start,
                limit,
                column_start,
                column_limit,
            },
            base_layer_id.as_deref(),
        )
        .await?,
    ))
}

async fn get_layer_access_policy(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn put_layer_access_policy(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<LayerAccessPolicyBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let policy_id = upsert_layer_policy(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_id,
        Some(&user.id),
    )
    .await?;
    replace_layer_policy_rules(&state.pool, &workspace_id, &policy_id, &body).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn create_layer_access_rule(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<LayerAccessRuleBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    validate_layer_access_rule(&state.pool, &workspace_id, &body).await?;
    let rule_id = body
        .id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| prefixed_id("access_rule"));
    let policy_id = policy_id_for_layer(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    insert_layer_access_rule(&state.pool, &policy_id, &rule_id, &body, Some(&user.id)).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn update_layer_access_rule(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, rule_id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<LayerAccessRuleBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    validate_layer_access_rule(&state.pool, &workspace_id, &body).await?;
    let policy_id = policy_id_for_layer(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    update_layer_access_rule_row(&state.pool, &policy_id, &rule_id, &body, Some(&user.id)).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn delete_layer_access_rule(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, rule_id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let policy_id = policy_id_for_layer(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    delete_layer_access_rule_row(&state.pool, &policy_id, &rule_id, Some(&user.id)).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn start_device_flow(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let device_flow_id = prefixed_id("device_flow");
    let device_code = token("device");
    let user_code = format!(
        "LAYRS-{}",
        &Uuid::new_v4().simple().to_string()[..6].to_uppercase()
    );

    sqlx::query(
        r#"
        INSERT INTO device_authorization_flows
            (device_flow_id, device_code_digest, user_code_digest, account_id, client_name, public_key_thumbprint, interval_seconds, expires_at)
        VALUES
            ($1, $2, $3, $4, 'Layrs Desktop', 'local-dev-thumbprint', $5, now() + ($6 || ' seconds')::interval)
        "#,
    )
    .bind(&device_flow_id)
    .bind(digest_secret(&device_code))
    .bind(digest_secret(&user_code))
    .bind(Option::<String>::None)
    .bind(DEVICE_FLOW_POLL_INTERVAL_SECONDS as i32)
    .bind(DEVICE_FLOW_EXPIRES_SECONDS)
    .execute(&state.pool)
    .await?;

    let verification_uri = format!("http://{}/v1/desktop/device", state.config.addr);
    let verification_uri_complete = format!("{verification_uri}?user_code={user_code}");

    Ok(Json(json!({
        "device_code": device_code,
        "user_code": user_code,
        "verification_uri": verification_uri,
        "verification_uri_complete": verification_uri_complete,
        "interval_seconds": DEVICE_FLOW_POLL_INTERVAL_SECONDS,
        "expires_in_seconds": DEVICE_FLOW_EXPIRES_SECONDS
    })))
}

#[derive(Deserialize)]
struct DevicePageQuery {
    #[serde(default)]
    user_code: Option<String>,
    #[serde(default, rename = "userCode")]
    user_code_camel: Option<String>,
}

#[derive(Deserialize)]
struct DeviceApproveForm {
    #[serde(default)]
    user_code: Option<String>,
    #[serde(default, rename = "userCode")]
    user_code_camel: Option<String>,
}

#[derive(Deserialize)]
struct DevicePollRequest {
    #[serde(default)]
    device_code: Option<String>,
    #[serde(default, rename = "deviceCode")]
    device_code_camel: Option<String>,
}

async fn device_verification_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<DevicePageQuery>,
) -> Html<String> {
    let user_code = query
        .user_code
        .or(query.user_code_camel)
        .unwrap_or_default();
    let account = optional_session(&state.pool, &headers).await.ok().flatten();
    let message = account
        .as_ref()
        .map(|user| {
            format!(
                "Approve this device as {}. Only Spaces visible to this account will appear in Desktop.",
                user.email
            )
        })
        .unwrap_or_else(|| {
            "Sign in to Layrs Studio in this browser before approving this device.".to_string()
        });
    Html(device_verification_html(
        &user_code,
        &message,
        account.is_none(),
        account.as_ref(),
        account.is_some(),
    ))
}

async fn approve_device_flow(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(body): Form<DeviceApproveForm>,
) -> Result<Response, ApiError> {
    let user_code = normalize_user_code(
        &body
            .user_code
            .or(body.user_code_camel)
            .ok_or_else(|| ApiError::bad_request("user_code is required"))?,
    )?;
    let user = match require_session(&state.pool, &headers).await {
        Ok(user) => user,
        Err(_) => {
            return Ok((
                StatusCode::UNAUTHORIZED,
                Html(device_verification_html(
                    &user_code,
                    "Sign in to Layrs Studio in this browser before approving this device.",
                    true,
                    None,
                    false,
                )),
            )
                .into_response());
        }
    };
    let row = sqlx::query(
        r#"
        SELECT device_flow_id, status, expires_at < now() AS expired, approved_at IS NOT NULL AS has_approved_at
        FROM device_authorization_flows
        WHERE user_code_digest = $1
        "#,
    )
    .bind(digest_secret(&user_code))
    .fetch_optional(&state.pool)
    .await?;

    let Some(row) = row else {
        return Ok((
            StatusCode::NOT_FOUND,
            Html(device_verification_html(
                &user_code,
                "This device code was not found. Check the code and try again.",
                true,
                Some(&user),
                false,
            )),
        )
            .into_response());
    };

    let status: String = row.get("status");
    let expired: bool = row.get("expired");
    let has_approved_at: bool = row.get("has_approved_at");
    let rejection = if expired || status == "expired" {
        if has_approved_at {
            Some("This device code was already used. Start a new login from Layrs Desktop.")
        } else {
            Some("This device code has expired. Start a new login from Layrs Desktop.")
        }
    } else if status == "denied" {
        Some("This device code was denied. Start a new login from Layrs Desktop.")
    } else if status == "approved" {
        Some("This device code is already approved. Return to Layrs Desktop and click Check now.")
    } else {
        None
    };

    if let Some(message) = rejection {
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(device_verification_html(
                &user_code,
                message,
                true,
                Some(&user),
                false,
            )),
        )
            .into_response());
    }

    let approved = sqlx::query(
        r#"
        UPDATE device_authorization_flows
        SET status = 'approved', approved_at = now(), account_id = $2
        WHERE device_flow_id = $1 AND status = 'pending' AND expires_at >= now()
        "#,
    )
    .bind(row.get::<String, _>("device_flow_id"))
    .bind(&user.id)
    .execute(&state.pool)
    .await?;
    if approved.rows_affected() != 1 {
        return Ok((
            StatusCode::BAD_REQUEST,
            Html(device_verification_html(
                &user_code,
                "This device code could not be approved. Start a new login from Layrs Desktop.",
                true,
                Some(&user),
                false,
            )),
        )
            .into_response());
    }

    Ok(Html(device_verification_html(
        &user_code,
        &format!(
            "Device approved for {}. Return to Layrs Desktop and click Check now.",
            user.email
        ),
        false,
        Some(&user),
        false,
    ))
    .into_response())
}

#[derive(Deserialize)]
struct LayerAccessPolicyBody {
    #[serde(default)]
    rules: Vec<LayerAccessRuleBody>,
    #[serde(default)]
    signature: Option<LayerAccessSignatureBody>,
}

#[derive(Deserialize)]
struct LayerAccessRuleBody {
    #[serde(default)]
    id: Option<String>,
    path: String,
    #[serde(default)]
    artifact_id: Option<String>,
    mode: String,
    #[serde(default = "default_stub_visibility")]
    visibility: String,
    permissions: LayerAccessPermissionsBody,
}

#[derive(Deserialize)]
struct LayerAccessPermissionsBody {
    #[serde(default)]
    read: LayerAccessPrincipalsBody,
    #[serde(default)]
    write: LayerAccessPrincipalsBody,
    #[serde(default)]
    admin: LayerAccessPrincipalsBody,
}

#[derive(Default, Deserialize)]
struct LayerAccessPrincipalsBody {
    #[serde(default)]
    accounts: Vec<String>,
    #[serde(default)]
    teams: Vec<String>,
}

#[derive(Deserialize)]
struct LayerAccessSignatureBody {
    key_id: String,
    value: String,
}

async fn poll_device_flow(
    State(state): State<AppState>,
    Json(body): Json<DevicePollRequest>,
) -> Result<Response, ApiError> {
    let device_code = body
        .device_code
        .or(body.device_code_camel)
        .ok_or_else(|| ApiError::bad_request("device_code is required"))?;
    let code_digest = digest_secret(&device_code);
    let row = sqlx::query(
        r#"
        UPDATE device_authorization_flows
        SET poll_count = poll_count + 1
        WHERE device_code_digest = $1
        RETURNING device_flow_id, account_id, status, poll_count, expires_at < now() AS expired, approved_at IS NOT NULL AS has_approved_at
        "#,
    )
    .bind(code_digest)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("device code is unknown"))?;

    let expired: bool = row.get("expired");
    let status: String = row.get("status");
    let has_approved_at: bool = row.get("has_approved_at");
    if expired || status == "expired" || status == "denied" {
        if status == "expired" && has_approved_at {
            return Err(ApiError::bad_request("device code was already consumed"));
        }
        return Err(ApiError::bad_request("device code is expired or denied"));
    }
    if status == "pending" {
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "authorization_pending",
                "interval_seconds": DEVICE_FLOW_POLL_INTERVAL_SECONDS
            })),
        )
            .into_response());
    }

    if status != "approved" {
        return Err(ApiError::bad_request("device code is not approved"));
    }

    let account_id: Option<String> = row.get("account_id");
    let account_id = account_id
        .ok_or_else(|| ApiError::unauthorized("device approval is missing a Studio account"))?;
    let device_flow_id: String = row.get("device_flow_id");
    let (user, access_token) =
        issue_desktop_token_for_flow(&state, &device_flow_id, &account_id).await?;

    Ok(Json(json!({
        "status": "authorized",
        "user": desktop_user_json(&user),
        "desktop_token": {
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in_seconds": DESKTOP_TOKEN_EXPIRES_SECONDS
        }
    }))
    .into_response())
}

async fn desktop_bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_bearer(&state.pool, &headers).await?;
    let workspaces = workspace_values(&state.pool, &user.id).await?;
    let workspace_ids = workspaces
        .iter()
        .filter_map(|workspace| {
            workspace
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let spaces = space_summaries_for_workspaces(&state.pool, &workspace_ids).await?;
    let layers = layer_summaries_for_workspaces(&state.pool, &workspace_ids).await?;

    Ok(Json(json!({
        "user": desktop_user_json(&user),
        "account": desktop_account_json(&user),
        "workspaces": workspaces,
        "spaces": spaces,
        "layers": layers,
        "server": {
            "mode": "postgres",
            "database_url_configured": true,
            "routes_path": "/v1/routes"
        }
    })))
}

async fn issue_desktop_token_for_flow(
    state: &AppState,
    device_flow_id: &str,
    account_id: &str,
) -> Result<(UserPrincipal, String), ApiError> {
    let user = load_user_by_id(&state.pool, account_id)
        .await?
        .ok_or_else(|| ApiError::unauthorized("device account is unavailable"))?;
    let access_token = token("desktop");
    let refresh_token = token("desktop_refresh");
    let device_id = prefixed_id("device");
    let token_id = prefixed_id("desktop_token");

    let mut tx = state.pool.begin().await?;
    let consumed = sqlx::query(
        r#"
        UPDATE device_authorization_flows
        SET status = 'expired', approved_at = COALESCE(approved_at, now())
        WHERE device_flow_id = $1 AND status IN ('pending', 'approved') AND expires_at >= now()
        "#,
    )
    .bind(device_flow_id)
    .execute(&mut *tx)
    .await?;
    if consumed.rows_affected() != 1 {
        return Err(ApiError::bad_request(
            "device code is expired, denied or already consumed",
        ));
    }

    sqlx::query(
        r#"
        INSERT INTO desktop_devices (device_id, account_id, display_name, public_key_thumbprint, last_seen_at)
        VALUES ($1, $2, 'Layrs Desktop', 'local-dev-thumbprint', now())
        "#,
    )
    .bind(&device_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO desktop_device_tokens
            (token_id, device_id, account_id, access_token_digest, refresh_token_digest, expires_at)
        VALUES
            ($1, $2, $3, $4, $5, now() + ($6 || ' seconds')::interval)
        "#,
    )
    .bind(&token_id)
    .bind(&device_id)
    .bind(&user.id)
    .bind(digest_secret(&access_token))
    .bind(digest_secret(&refresh_token))
    .bind(DESKTOP_TOKEN_EXPIRES_SECONDS)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((user, access_token))
}

async fn session_response(
    state: &AppState,
    user: &UserPrincipal,
    status: StatusCode,
) -> Result<Response, ApiError> {
    let session_token = token("session");
    sqlx::query(
        r#"
        INSERT INTO web_sessions (session_id, account_id, session_token_digest, expires_at)
        VALUES ($1, $2, $3, now() + ($4 || ' seconds')::interval)
        "#,
    )
    .bind(prefixed_id("session"))
    .bind(&user.id)
    .bind(digest_secret(&session_token))
    .bind(SESSION_MAX_AGE_SECONDS)
    .execute(&state.pool)
    .await?;

    let mut response = (status, Json(auth_session_value(&state.pool, user).await?)).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&build_session_cookie(&session_token, &state.config))
            .map_err(|error| ApiError::bad_request(error.to_string()))?,
    );
    Ok(response)
}

async fn auth_session_value(pool: &PgPool, user: &UserPrincipal) -> Result<Value, ApiError> {
    let workspaces = workspace_values(pool, &user.id).await?;
    let active_workspace_id = workspaces
        .first()
        .and_then(|workspace| workspace.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(json!({
        "state": "authenticated",
        "account": studio_account_json(user),
        "session": session_json(user, active_workspace_id.as_deref()),
        "workspaces": workspaces,
        "activeWorkspaceId": active_workspace_id
    }))
}

async fn require_session(pool: &PgPool, headers: &HeaderMap) -> Result<UserPrincipal, ApiError> {
    let token = session_cookie(headers)
        .ok_or_else(|| ApiError::unauthorized("authentication is required"))?;
    let digest = digest_secret(&token);
    let row = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name
        FROM web_sessions s
        JOIN accounts a ON a.account_id = s.account_id
        WHERE s.session_token_digest = $1
          AND s.revoked_at IS NULL
          AND s.expires_at > now()
          AND a.status = 'active'
        "#,
    )
    .bind(digest)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("authentication is required"))?;

    Ok(row_user(&row))
}

async fn optional_session(
    pool: &PgPool,
    headers: &HeaderMap,
) -> Result<Option<UserPrincipal>, ApiError> {
    let Some(token) = session_cookie(headers) else {
        return Ok(None);
    };
    let digest = digest_secret(&token);
    let row = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name
        FROM web_sessions s
        JOIN accounts a ON a.account_id = s.account_id
        WHERE s.session_token_digest = $1
          AND s.revoked_at IS NULL
          AND s.expires_at > now()
          AND a.status = 'active'
        "#,
    )
    .bind(digest)
    .fetch_optional(pool)
    .await?;

    Ok(row.as_ref().map(row_user))
}

async fn require_principal(pool: &PgPool, headers: &HeaderMap) -> Result<UserPrincipal, ApiError> {
    if let Ok(user) = require_session(pool, headers).await {
        return Ok(user);
    }
    require_bearer(pool, headers).await
}

async fn require_bearer(pool: &PgPool, headers: &HeaderMap) -> Result<UserPrincipal, ApiError> {
    let token = bearer_token(headers)
        .ok_or_else(|| ApiError::unauthorized("missing desktop bearer token"))?;
    let digest = digest_secret(&token);
    let row = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name
        FROM desktop_device_tokens t
        JOIN accounts a ON a.account_id = t.account_id
        WHERE t.access_token_digest = $1
          AND t.revoked_at IS NULL
          AND t.expires_at > now()
          AND a.status = 'active'
        "#,
    )
    .bind(digest)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::unauthorized("desktop bearer token is invalid"))?;

    Ok(row_user(&row))
}

async fn load_user_by_id(
    pool: &PgPool,
    account_id: &str,
) -> Result<Option<UserPrincipal>, ApiError> {
    let row = sqlx::query(
        "SELECT account_id, email, display_name FROM accounts WHERE account_id = $1 AND status = 'active'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_user(&row)))
}

fn row_user(row: &sqlx::postgres::PgRow) -> UserPrincipal {
    UserPrincipal {
        id: row.get("account_id"),
        email: row.get("email"),
        display_name: row.get("display_name"),
    }
}

async fn ensure_workspace_member(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active')",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::forbidden("workspace membership is required"))
    }
}

async fn ensure_workspace_admin(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active'",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    match role.as_deref() {
        Some("owner" | "admin") => Ok(()),
        Some(_) => Err(ApiError::forbidden(
            "workspace admin permission is required",
        )),
        None => Err(ApiError::forbidden("workspace membership is required")),
    }
}

async fn ensure_workspace_membership(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
    role: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role, state)
        VALUES ($1, $2, $3, $4, 'active')
        ON CONFLICT (workspace_id, account_id) DO UPDATE
        SET role = CASE
                WHEN workspace_memberships.role IN ('owner', 'admin') THEN workspace_memberships.role
                ELSE excluded.role
            END,
            state = 'active',
            updated_at = now()
        "#,
    )
    .bind(prefixed_id("membership"))
    .bind(workspace_id)
    .bind(account_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_account_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active')",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::bad_request("account is not a workspace member"))
    }
}

async fn ensure_accounts_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    account_ids: &[String],
) -> Result<(), ApiError> {
    let account_ids = unique_strings(account_ids);
    if account_ids.is_empty() {
        return Ok(());
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT account_id)::bigint FROM workspace_memberships WHERE workspace_id = $1 AND state = 'active' AND account_id = ANY($2)",
    )
    .bind(workspace_id)
    .bind(&account_ids)
    .fetch_one(pool)
    .await?;
    if count == account_ids.len() as i64 {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "access rule references an account outside this workspace",
        ))
    }
}

async fn ensure_team_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    team_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM teams WHERE workspace_id = $1 AND team_id = $2)",
    )
    .bind(workspace_id)
    .bind(team_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("team not found"))
    }
}

async fn ensure_teams_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    team_ids: &[String],
) -> Result<(), ApiError> {
    let team_ids = unique_strings(team_ids);
    if team_ids.is_empty() {
        return Ok(());
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT team_id)::bigint FROM teams WHERE workspace_id = $1 AND team_id = ANY($2)",
    )
    .bind(workspace_id)
    .bind(&team_ids)
    .fetch_one(pool)
    .await?;
    if count == team_ids.len() as i64 {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "request references a team outside this workspace",
        ))
    }
}

async fn ensure_layer_in_space(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM layers WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3)",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("layer not found"))
    }
}

async fn ensure_space_in_workspace(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM spaces WHERE workspace_id = $1 AND space_id = $2)",
    )
    .bind(workspace_id)
    .bind(space_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found("space not found"))
    }
}

async fn ensure_workspace_write(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active'",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    match role.as_deref() {
        Some("owner" | "admin" | "member") => Ok(()),
        Some(_) => Err(ApiError::forbidden(
            "workspace write permission is required",
        )),
        None => Err(ApiError::forbidden("workspace membership is required")),
    }
}

async fn workspace_value_for_account(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<Value, ApiError> {
    workspace_values(pool, account_id)
        .await?
        .into_iter()
        .find(|workspace| workspace.get("id").and_then(Value::as_str) == Some(workspace_id))
        .ok_or_else(|| ApiError::not_found("workspace not found"))
}

async fn space_value(pool: &PgPool, workspace_id: &str, space_id: &str) -> Result<Value, ApiError> {
    space_values(pool, workspace_id)
        .await?
        .into_iter()
        .find(|space| space.get("id").and_then(Value::as_str) == Some(space_id))
        .ok_or_else(|| ApiError::not_found("space not found"))
}

async fn layer_values_for_space(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
) -> Result<Vec<Value>, ApiError> {
    Ok(layer_values(pool, workspace_id)
        .await?
        .into_iter()
        .filter(|layer| layer.get("spaceId").and_then(Value::as_str) == Some(space_id))
        .collect())
}

async fn access_registry_values_for_layers(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for layer_id in layer_ids {
        values.push(layer_access_policy_value(pool, workspace_id, space_id, layer_id).await?);
    }
    Ok(values)
}

async fn artifact_values_for_layers(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
    account_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for layer_id in layer_ids {
        values.extend(
            artifact_values_for_layer(pool, workspace_id, space_id, layer_id, account_id).await?,
        );
    }
    Ok(values)
}

async fn receive_store_objects_for_layers(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
    account_id: &str,
) -> Result<Value, ApiError> {
    let mut chunks_by_id = BTreeMap::<String, Value>::new();
    let mut file_objects_by_id = BTreeMap::<String, Value>::new();
    let mut tree_objects = Vec::new();

    for layer_id in layer_ids {
        let root_tree_id = sqlx::query_scalar::<_, Option<String>>(
            r#"
            SELECT root_tree_id
            FROM layer_heads
            WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
            "#,
        )
        .bind(workspace_id)
        .bind(space_id)
        .bind(layer_id)
        .fetch_optional(pool)
        .await?
        .flatten();
        let Some(root_tree_id) = root_tree_id else {
            tree_objects.push(json!({
                "treeId": blake3_digest_for_bytes(b""),
                "layerId": layer_id,
                "entries": []
            }));
            continue;
        };

        push_received_tree_object_for_layer(
            pool,
            workspace_id,
            space_id,
            layer_id,
            &root_tree_id,
            account_id,
            &mut chunks_by_id,
            &mut file_objects_by_id,
            &mut tree_objects,
        )
        .await?;
    }

    let step_tree_rows = sqlx::query(
        r#"
        SELECT DISTINCT layer_id,
               COALESCE(base_layer_id, layer_id) AS base_layer_id,
               root_tree_id,
               base_tree_id
        FROM layer_steps
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = ANY($3)
          AND (root_tree_id IS NOT NULL OR base_tree_id IS NOT NULL)
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_ids)
    .fetch_all(pool)
    .await?;
    let mut seen_step_trees = tree_objects
        .iter()
        .filter_map(|tree| {
            tree.get("treeId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<HashSet<_>>();
    for row in step_tree_rows {
        let layer_id = row.get::<String, _>("layer_id");
        let base_layer_id = row.get::<String, _>("base_layer_id");
        for (tree_id, access_layer_id) in [
            (
                row.try_get::<String, _>("root_tree_id").ok(),
                layer_id.as_str(),
            ),
            (
                row.try_get::<String, _>("base_tree_id").ok(),
                base_layer_id.as_str(),
            ),
        ] {
            let Some(tree_id) = tree_id else {
                continue;
            };
            if !seen_step_trees.insert(tree_id.clone()) {
                continue;
            }
            push_received_tree_object_for_layer(
                pool,
                workspace_id,
                space_id,
                access_layer_id,
                &tree_id,
                account_id,
                &mut chunks_by_id,
                &mut file_objects_by_id,
                &mut tree_objects,
            )
            .await?;
        }
    }

    Ok(json!({
        "chunks": chunks_by_id.into_values().collect::<Vec<_>>(),
        "fileObjects": file_objects_by_id.into_values().collect::<Vec<_>>(),
        "treeObjects": tree_objects
    }))
}

async fn push_received_tree_object_for_layer(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    tree_id: &str,
    account_id: &str,
    chunks_by_id: &mut BTreeMap<String, Value>,
    file_objects_by_id: &mut BTreeMap<String, Value>,
    tree_objects: &mut Vec<Value>,
) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT te.logical_path, te.file_object_id,
               f.digest, f.size_bytes, f.media_type,
               a.artifact_id, a.state
        FROM tree_entries te
        JOIN file_objects f ON f.file_object_id = te.file_object_id
        LEFT JOIN artifacts a ON a.workspace_id = $2
            AND a.space_id = $3
            AND a.layer_id = $4
            AND a.logical_path = te.logical_path
        WHERE te.tree_id = $1
        ORDER BY te.logical_path ASC
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_all(pool)
    .await?;

    let mut entries = Vec::new();
    for row in rows {
        let path = row.get::<String, _>("logical_path");
        let artifact_id = row.try_get::<String, _>("artifact_id").ok();
        let state = row.try_get::<String, _>("state").ok();
        let decision = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            layer_id,
            &path,
            artifact_id.as_deref(),
            account_id,
        )
        .await?;
        if state.as_deref() == Some("redacted") || !decision.can_read {
            continue;
        }

        let file_object_id = row.get::<String, _>("file_object_id");
        let size_bytes = row.get::<i64, _>("size_bytes");
        let chunks =
            chunk_values_for_file_object(pool, workspace_id, space_id, &file_object_id).await?;
        for chunk in &chunks {
            if let Some(chunk_id) = chunk.get("chunkId").and_then(Value::as_str) {
                chunks_by_id
                    .entry(chunk_id.to_string())
                    .or_insert_with(|| chunk.clone());
            }
        }
        file_objects_by_id
            .entry(file_object_id.clone())
            .or_insert_with(|| {
                json!({
                    "fileObjectId": file_object_id,
                    "hash": row.get::<String, _>("digest"),
                    "digest": row.get::<String, _>("digest"),
                    "size": size_bytes,
                    "sizeBytes": size_bytes,
                    "mediaType": row.try_get::<String, _>("media_type").ok().unwrap_or_else(|| "application/octet-stream".to_string()),
                    "chunks": chunks.iter().map(|chunk| json!({
                        "chunkId": chunk.get("chunkId").cloned().unwrap_or(Value::Null),
                        "digest": chunk.get("digest").cloned().unwrap_or(Value::Null),
                        "size": chunk.get("size").cloned().unwrap_or(Value::Null),
                        "sizeBytes": chunk.get("sizeBytes").cloned().unwrap_or(Value::Null),
                        "byteOffset": chunk.get("byteOffset").cloned().unwrap_or(Value::Null)
                    })).collect::<Vec<_>>()
                })
            });
        entries.push(json!({
            "path": path,
            "fileObjectId": file_object_id,
            "size": size_bytes,
            "sizeBytes": size_bytes
        }));
    }

    tree_objects.push(json!({
        "treeId": tree_id,
        "layerId": layer_id,
        "entries": entries
    }));
    Ok(())
}

async fn layer_step_values_for_layers(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
) -> Result<Vec<Value>, ApiError> {
    if layer_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT step_id, layer_id, parent_step_id, base_layer_id, base_tree_id,
               root_tree_id, changed_paths, source_client_id, sync_batch_id,
               EXTRACT(EPOCH FROM captured_at)::bigint AS captured_at_unix,
               to_char(captured_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS captured_at
        FROM layer_steps
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = ANY($3)
        ORDER BY captured_at ASC, created_at ASC, step_id ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_ids)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "stepId": row.get::<String, _>("step_id"),
                "layerId": row.get::<String, _>("layer_id"),
                "parentStepId": row.try_get::<String, _>("parent_step_id").ok(),
                "baseLayerId": row.try_get::<String, _>("base_layer_id").ok(),
                "baseTreeId": row.try_get::<String, _>("base_tree_id").ok(),
                "rootTreeId": row.try_get::<String, _>("root_tree_id").ok(),
                "changedPaths": row.try_get::<Vec<String>, _>("changed_paths").unwrap_or_default(),
                "sourceClientId": row.try_get::<String, _>("source_client_id").ok(),
                "syncBatchId": row.try_get::<String, _>("sync_batch_id").ok(),
                "capturedAtUnix": row.get::<i64, _>("captured_at_unix"),
                "capturedAt": row.get::<String, _>("captured_at")
            })
        })
        .collect())
}

async fn layer_step_detail_values(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    step_id: Option<&str>,
    account_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT step_id, layer_id, parent_step_id, base_layer_id, base_tree_id,
               root_tree_id, changed_paths, source_client_id, sync_batch_id,
               EXTRACT(EPOCH FROM captured_at)::bigint AS captured_at_unix,
               to_char(captured_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS captured_at
        FROM layer_steps
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = $3
          AND ($4::text IS NULL OR step_id = $4)
        ORDER BY captured_at DESC, created_at DESC, step_id DESC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(step_id)
    .fetch_all(pool)
    .await?;

    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        let step_id = row.get::<String, _>("step_id");
        let step_layer_id = row.get::<String, _>("layer_id");
        let base_layer_id = row.try_get::<String, _>("base_layer_id").ok();
        let base_tree_id = row.try_get::<String, _>("base_tree_id").ok();
        let root_tree_id = row.try_get::<String, _>("root_tree_id").ok();
        let changed_paths = row
            .try_get::<Vec<String>, _>("changed_paths")
            .unwrap_or_default();
        let files = step_changed_file_values(
            pool,
            workspace_id,
            space_id,
            &step_layer_id,
            base_layer_id.as_deref().unwrap_or(&step_layer_id),
            base_tree_id.as_deref(),
            root_tree_id.as_deref(),
            &changed_paths,
            account_id,
        )
        .await?;
        values.push(json!({
            "id": step_id,
            "stepId": step_id,
            "workspaceId": workspace_id,
            "spaceId": space_id,
            "layerId": step_layer_id,
            "status": "passing",
            "parentStepId": row.try_get::<String, _>("parent_step_id").ok(),
            "baseLayerId": base_layer_id,
            "baseTreeId": base_tree_id,
            "rootTreeId": root_tree_id,
            "changedPaths": changed_paths,
            "changedFiles": files.len(),
            "diffStats": {
                "files": files.len(),
                "additions": 0,
                "removals": 0
            },
            "files": files,
            "sourceClientId": row.try_get::<String, _>("source_client_id").ok(),
            "syncBatchId": row.try_get::<String, _>("sync_batch_id").ok(),
            "capturedAtUnix": row.get::<i64, _>("captured_at_unix"),
            "capturedAt": row.get::<String, _>("captured_at")
        }));
    }
    Ok(values)
}

async fn step_changed_file_values(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    base_layer_id: &str,
    base_tree_id: Option<&str>,
    root_tree_id: Option<&str>,
    changed_paths: &[String],
    account_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for path in changed_paths {
        let path = validate_publish_path(path.trim())?;
        let decision = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            layer_id,
            &path,
            None,
            account_id,
        )
        .await?;
        let base =
            tree_file_ref_for_path(pool, workspace_id, space_id, base_tree_id, &path).await?;
        let target =
            tree_file_ref_for_path(pool, workspace_id, space_id, root_tree_id, &path).await?;
        let action = match (base.is_some(), target.is_some()) {
            (false, true) => "added",
            (true, false) => "deleted",
            (true, true) => "modified",
            (false, false) => "missing",
        };
        let media_type = target
            .as_ref()
            .or(base.as_ref())
            .and_then(|file| file.media_type.clone())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        values.push(json!({
            "path": path,
            "name": path.rsplit('/').next().unwrap_or(path.as_str()),
            "action": action,
            "lensId": lens_id_for_path_and_media_type(&path, &media_type),
            "mediaType": media_type,
            "baseLayerId": base_layer_id,
            "baseFileObjectId": base.as_ref().map(|file| file.file_object_id.as_str()),
            "targetFileObjectId": target.as_ref().map(|file| file.file_object_id.as_str()),
            "sizeBytes": target.as_ref().or(base.as_ref()).map(|file| file.size_bytes),
            "access": {
                "canOpen": decision.can_read,
                "isRedacted": !decision.can_read,
                "reason": if decision.can_read { None } else { Some(decision.reason) }
            }
        }));
    }
    Ok(values)
}

#[derive(Clone, Debug)]
struct TreeFileRef {
    file_object_id: String,
    size_bytes: i64,
    media_type: Option<String>,
}

async fn tree_file_ref_for_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    tree_id: Option<&str>,
    path: &str,
) -> Result<Option<TreeFileRef>, ApiError> {
    let Some(tree_id) = tree_id else {
        return Ok(None);
    };
    let row = sqlx::query(
        r#"
        SELECT f.file_object_id, f.digest, f.size_bytes, f.media_type
        FROM tree_entries te
        JOIN file_objects f ON f.file_object_id = te.file_object_id
        WHERE te.tree_id = $1
          AND f.workspace_id = $2
          AND f.space_id = $3
          AND te.logical_path = $4
          AND te.entry_kind = 'file'
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(path)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| TreeFileRef {
        file_object_id: row.get("file_object_id"),
        size_bytes: row.get("size_bytes"),
        media_type: row.try_get("media_type").ok(),
    }))
}

fn lens_id_for_path_and_media_type(path: &str, media_type: &str) -> &'static str {
    if is_code_path(path) {
        "layrs.code"
    } else if is_textual_artifact(path, media_type) {
        "layrs.text"
    } else if media_type.starts_with("image/") {
        "layrs.image"
    } else {
        "layrs.raw"
    }
}

async fn step_diff_window_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    step_id: &str,
    requested_path: Option<&str>,
    account_id: &str,
    window_request: WindowRequest,
) -> Result<Value, ApiError> {
    let mut steps = layer_step_detail_values(
        pool,
        workspace_id,
        space_id,
        layer_id,
        Some(step_id),
        account_id,
    )
    .await?;
    let step = steps
        .pop()
        .ok_or_else(|| ApiError::not_found("step not found"))?;
    let root_tree_id = step.get("rootTreeId").and_then(Value::as_str);
    let base_tree_id = step.get("baseTreeId").and_then(Value::as_str);
    let base_layer_id = step
        .get("baseLayerId")
        .and_then(Value::as_str)
        .unwrap_or(layer_id);
    let changed_paths = step
        .get("changedPaths")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let path = step_diff_path(
        pool,
        workspace_id,
        space_id,
        requested_path,
        &changed_paths,
        root_tree_id,
        base_tree_id,
    )
    .await?;
    let target = tree_text_window_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        root_tree_id,
        &path,
        account_id,
        window_request,
    )
    .await?;
    let base = tree_text_window_for_path(
        pool,
        workspace_id,
        space_id,
        base_layer_id,
        base_tree_id,
        &path,
        account_id,
        window_request,
    )
    .await?;
    if target.is_none() && base.is_none() {
        return Err(ApiError::not_found("step path content not found"));
    }
    let summary = match (base.is_some(), target.is_some()) {
        (false, true) => "Windowed step diff preview; file added in this step",
        (true, false) => "Windowed step diff preview; file deleted in this step",
        (true, true) => "Windowed step diff preview",
        (false, false) => "Windowed step diff preview; file content is not available",
    };

    Ok(lens_runtime_diff_value(LensRuntimeDiffRender {
        workspace_id,
        space_id,
        layer_id,
        artifact_id: None,
        step_id: Some(step_id),
        base_layer_id: Some(base_layer_id),
        path: &path,
        target: target.as_ref(),
        base: base.as_ref(),
        window_request,
        summary,
        mode: "stepWindow",
        limitation: Some(
            "Windowed step comparison is line-aligned inside the requested line and column window.",
        ),
    }))
}

async fn step_diff_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    requested_path: Option<&str>,
    changed_paths: &[Value],
    root_tree_id: Option<&str>,
    base_tree_id: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(path) = cleaned_optional_text(requested_path) {
        return validate_publish_path(&path);
    }
    if let Some(path) = changed_paths
        .iter()
        .filter_map(Value::as_str)
        .find_map(|path| cleaned_optional_text(Some(path)))
    {
        return validate_publish_path(&path);
    }
    if let Some(path) = first_tree_file_path(pool, workspace_id, space_id, root_tree_id).await? {
        return Ok(path);
    }
    if let Some(path) = first_tree_file_path(pool, workspace_id, space_id, base_tree_id).await? {
        return Ok(path);
    }
    Err(ApiError::not_found("step does not reference any file path"))
}

async fn first_tree_file_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    tree_id: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let Some(tree_id) = tree_id else {
        return Ok(None);
    };
    sqlx::query_scalar(
        r#"
        SELECT te.logical_path
        FROM tree_entries te
        JOIN tree_objects t ON t.tree_id = te.tree_id
        WHERE te.tree_id = $1
          AND t.workspace_id = $2
          AND t.space_id = $3
          AND te.entry_kind = 'file'
        ORDER BY te.logical_path ASC
        LIMIT 1
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)
}

async fn tree_text_window_for_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    tree_id: Option<&str>,
    path: &str,
    account_id: &str,
    window_request: WindowRequest,
) -> Result<Option<ArtifactTextWindow>, ApiError> {
    let Some(tree_id) = tree_id else {
        return Ok(None);
    };
    let Some(file) =
        tree_file_ref_for_path(pool, workspace_id, space_id, Some(tree_id), path).await?
    else {
        return Ok(None);
    };
    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        path,
        None,
        account_id,
    )
    .await?;
    if !decision.can_read {
        return Err(ApiError::forbidden(
            "step path is redacted by layer access policy",
        ));
    }
    let mut window = file_object_text_window(
        pool,
        workspace_id,
        space_id,
        path,
        "file".to_string(),
        &file.file_object_id,
        window_request,
    )
    .await?;
    window.source = json!({
        "kind": "tree_entry",
        "treeId": tree_id,
        "path": path,
        "fileObject": window.source
    });
    Ok(Some(window))
}

async fn file_object_bytes(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<Vec<u8>, ApiError> {
    let chunk_rows = sqlx::query(
        r#"
        SELECT oc.content_bytes
        FROM file_object_chunks foc
        JOIN object_chunks oc ON oc.chunk_id = foc.chunk_id
        WHERE foc.file_object_id = $1
          AND oc.workspace_id = $2
          AND oc.space_id = $3
          AND oc.state = 'available'
        ORDER BY foc.chunk_index ASC
        "#,
    )
    .bind(file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?;
    let mut bytes = Vec::new();
    for row in chunk_rows {
        let chunk_bytes = row
            .try_get::<Vec<u8>, _>("content_bytes")
            .ok()
            .ok_or_else(|| ApiError::not_found("chunk bytes are not available"))?;
        bytes.extend(chunk_bytes);
    }
    Ok(bytes)
}

async fn chunk_values_for_file_object(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT foc.chunk_index, foc.byte_offset, foc.size_bytes,
               oc.chunk_id, oc.digest, oc.object_key
        FROM file_object_chunks foc
        JOIN object_chunks oc ON oc.chunk_id = foc.chunk_id
        WHERE foc.file_object_id = $1
          AND oc.workspace_id = $2
          AND oc.space_id = $3
          AND oc.state = 'available'
        ORDER BY foc.chunk_index ASC
        "#,
    )
    .bind(file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "chunkId": row.get::<String, _>("chunk_id"),
                "digest": row.get::<String, _>("digest"),
                "objectKey": row.get::<String, _>("object_key"),
                "index": row.get::<i32, _>("chunk_index"),
                "byteOffset": row.get::<i64, _>("byte_offset"),
                "size": row.get::<i64, _>("size_bytes"),
                "sizeBytes": row.get::<i64, _>("size_bytes"),
                "downloadUrl": format!("/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/{}", row.get::<String, _>("chunk_id"))
            })
        })
        .collect())
}

async fn layer_head_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    requested_layer_id: Option<&str>,
) -> Result<Value, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT layer_id, layer_state_id, root_tree_id, policy_epoch, server_cursor,
               to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM layer_heads
        WHERE workspace_id = $1
          AND space_id = $2
          AND ($3::text IS NULL OR layer_id = $3)
        ORDER BY updated_at DESC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(requested_layer_id)
    .fetch_all(pool)
    .await?;
    if let Some(layer_id) = requested_layer_id {
        if let Some(row) = rows.first() {
            return Ok(json!({
                "layerId": row.get::<String, _>("layer_id"),
                "layerStateId": row.try_get::<String, _>("layer_state_id").ok(),
                "rootTreeId": row.try_get::<String, _>("root_tree_id").ok(),
                "policyEpoch": row.get::<i64, _>("policy_epoch"),
                "serverCursor": row.try_get::<String, _>("server_cursor").ok(),
                "updatedAt": row.get::<String, _>("updated_at")
            }));
        }
        let policy_epoch = current_policy_epoch(pool, workspace_id, space_id, layer_id)
            .await
            .unwrap_or(1);
        return Ok(json!({
            "layerId": layer_id,
            "rootTreeId": Value::Null,
            "policyEpoch": policy_epoch,
            "serverCursor": Value::Null
        }));
    }
    Ok(json!(
        rows.iter()
            .map(|row| json!({
                "layerId": row.get::<String, _>("layer_id"),
                "layerStateId": row.try_get::<String, _>("layer_state_id").ok(),
                "rootTreeId": row.try_get::<String, _>("root_tree_id").ok(),
                "policyEpoch": row.get::<i64, _>("policy_epoch"),
                "serverCursor": row.try_get::<String, _>("server_cursor").ok(),
                "updatedAt": row.get::<String, _>("updated_at")
            }))
            .collect::<Vec<_>>()
    ))
}

#[derive(Debug)]
struct AccessDecision {
    can_read: bool,
    can_write: bool,
    reason: String,
}

#[derive(Debug)]
struct AccessRuleRow {
    path: String,
    artifact_id: Option<String>,
    mode: String,
    read_accounts: HashSet<String>,
    read_teams: HashSet<String>,
    write_accounts: HashSet<String>,
    write_teams: HashSet<String>,
    admin_accounts: HashSet<String>,
    admin_teams: HashSet<String>,
}

async fn access_decision_for_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    path: &str,
    artifact_id: Option<&str>,
    account_id: &str,
) -> Result<AccessDecision, ApiError> {
    let workspace_role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active'",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    let Some(workspace_role) = workspace_role else {
        return Ok(AccessDecision {
            can_read: false,
            can_write: false,
            reason: "Workspace membership is required".to_string(),
        });
    };
    let team_ids = account_team_ids(pool, workspace_id, account_id).await?;
    let rules = access_rule_rows_for_layer(pool, workspace_id, space_id, layer_id).await?;
    let mut best: Option<AccessRuleRow> = None;
    for rule in rules {
        let artifact_matches = match (artifact_id, rule.artifact_id.as_deref()) {
            (Some(artifact_id), Some(rule_artifact_id)) => artifact_id == rule_artifact_id,
            _ => false,
        };
        if artifact_matches || path_matches_rule(path, &rule.path) {
            let replace = best
                .as_ref()
                .map(|current| rule.path.len() > current.path.len())
                .unwrap_or(true);
            if replace {
                best = Some(rule);
            }
        }
    }

    let workspace_can_write = matches!(workspace_role.as_str(), "owner" | "admin" | "member");
    let Some(rule) = best else {
        return Ok(AccessDecision {
            can_read: true,
            can_write: workspace_can_write,
            reason: "No restrictive Layer access rule matched".to_string(),
        });
    };
    if rule.mode == "reserved_redacted" {
        return Ok(AccessDecision {
            can_read: false,
            can_write: false,
            reason: "Path is reserved redacted by Layer access policy".to_string(),
        });
    }

    let account_read = rule.read_accounts.contains(account_id)
        || rule.write_accounts.contains(account_id)
        || rule.admin_accounts.contains(account_id);
    let team_read = intersects(&team_ids, &rule.read_teams)
        || intersects(&team_ids, &rule.write_teams)
        || intersects(&team_ids, &rule.admin_teams);
    let account_write =
        rule.write_accounts.contains(account_id) || rule.admin_accounts.contains(account_id);
    let team_write =
        intersects(&team_ids, &rule.write_teams) || intersects(&team_ids, &rule.admin_teams);

    Ok(AccessDecision {
        can_read: account_read || team_read,
        can_write: workspace_can_write && (account_write || team_write),
        reason: "Restricted by Layer access policy".to_string(),
    })
}

async fn access_rule_rows_for_layer(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Vec<AccessRuleRow>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT r.path, r.artifact_id, r.mode,
               r.read_account_ids, r.read_team_ids, r.write_account_ids, r.write_team_ids,
               r.admin_account_ids, r.admin_team_ids
        FROM layer_access_policy_rules r
        JOIN layer_access_policies p ON p.policy_id = r.policy_id
        WHERE p.workspace_id = $1 AND p.space_id = $2 AND p.layer_id = $3
        ORDER BY length(r.path) DESC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| AccessRuleRow {
            path: row.get("path"),
            artifact_id: row.try_get("artifact_id").ok(),
            mode: row.get("mode"),
            read_accounts: hashset(
                row.try_get::<Vec<String>, _>("read_account_ids")
                    .unwrap_or_default(),
            ),
            read_teams: hashset(
                row.try_get::<Vec<String>, _>("read_team_ids")
                    .unwrap_or_default(),
            ),
            write_accounts: hashset(
                row.try_get::<Vec<String>, _>("write_account_ids")
                    .unwrap_or_default(),
            ),
            write_teams: hashset(
                row.try_get::<Vec<String>, _>("write_team_ids")
                    .unwrap_or_default(),
            ),
            admin_accounts: hashset(
                row.try_get::<Vec<String>, _>("admin_account_ids")
                    .unwrap_or_default(),
            ),
            admin_teams: hashset(
                row.try_get::<Vec<String>, _>("admin_team_ids")
                    .unwrap_or_default(),
            ),
        })
        .collect())
}

async fn account_team_ids(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<HashSet<String>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT tm.team_id
        FROM team_memberships tm
        JOIN teams t ON t.team_id = tm.team_id
        WHERE t.workspace_id = $1 AND tm.account_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| row.get::<String, _>("team_id"))
        .collect())
}

async fn workspace_values(pool: &PgPool, account_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT w.workspace_id, w.name, w.slug,
               to_char(w.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM workspaces w
        JOIN workspace_memberships m ON m.workspace_id = w.workspace_id
        WHERE m.account_id = $1 AND m.state = 'active'
        ORDER BY w.created_at ASC
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            workspace_value(
                row.get("workspace_id"),
                row.get("name"),
                row.get("slug"),
                "",
                row.get("updated_at"),
            )
        })
        .collect())
}

fn workspace_value(id: &str, name: &str, slug: &str, description: &str, updated_at: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "slug": slug,
        "description": description,
        "health": "pending",
        "updatedAt": updated_at
    })
}

async fn team_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT t.team_id, t.name, t.purpose, count(tm.account_id)::bigint AS members
        FROM teams t
        LEFT JOIN team_memberships tm ON tm.team_id = t.team_id
        WHERE t.workspace_id = $1
        GROUP BY t.team_id, t.name, t.purpose, t.created_at
        ORDER BY t.created_at ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("team_id"),
                "workspaceId": workspace_id,
                "name": row.get::<String, _>("name"),
                "purpose": row.get::<String, _>("purpose"),
                "members": row.get::<i64, _>("members"),
                "gateResponsibility": "workspace"
            })
        })
        .collect())
}

async fn team_value(pool: &PgPool, workspace_id: &str, team_id: &str) -> Result<Value, ApiError> {
    team_values(pool, workspace_id)
        .await?
        .into_iter()
        .find(|team| team.get("id").and_then(Value::as_str) == Some(team_id))
        .ok_or_else(|| ApiError::not_found("team not found"))
}

async fn workspace_member_values(
    pool: &PgPool,
    workspace_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name, m.role, m.state,
               to_char(m.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM workspace_memberships m
        JOIN accounts a ON a.account_id = m.account_id
        WHERE m.workspace_id = $1 AND m.state = 'active'
        ORDER BY a.display_name ASC, a.email ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "accountId": row.get::<String, _>("account_id"),
                "email": row.get::<String, _>("email"),
                "displayName": row.get::<String, _>("display_name"),
                "role": row.get::<String, _>("role"),
                "state": row.get::<String, _>("state"),
                "createdAt": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn team_member_values(
    pool: &PgPool,
    workspace_id: &str,
    team_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT a.account_id, a.email, a.display_name, tm.role,
               to_char(tm.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM team_memberships tm
        JOIN teams t ON t.team_id = tm.team_id
        JOIN accounts a ON a.account_id = tm.account_id
        JOIN workspace_memberships wm ON wm.workspace_id = t.workspace_id
            AND wm.account_id = a.account_id
            AND wm.state = 'active'
        WHERE t.workspace_id = $1 AND tm.team_id = $2
        ORDER BY a.display_name ASC, a.email ASC
        "#,
    )
    .bind(workspace_id)
    .bind(team_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "teamId": team_id,
                "accountId": row.get::<String, _>("account_id"),
                "email": row.get::<String, _>("email"),
                "displayName": row.get::<String, _>("display_name"),
                "role": row.get::<String, _>("role"),
                "createdAt": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn team_member_value(
    pool: &PgPool,
    workspace_id: &str,
    team_id: &str,
    account_id: &str,
) -> Result<Value, ApiError> {
    team_member_values(pool, workspace_id, team_id)
        .await?
        .into_iter()
        .find(|member| member.get("accountId").and_then(Value::as_str) == Some(account_id))
        .ok_or_else(|| ApiError::not_found("team member not found"))
}

async fn upsert_team_member(
    pool: &PgPool,
    workspace_id: &str,
    team_id: &str,
    account_id: &str,
    role: &str,
) -> Result<(), ApiError> {
    ensure_team_in_workspace(pool, workspace_id, team_id).await?;
    sqlx::query(
        r#"
        INSERT INTO team_memberships (team_id, account_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (team_id, account_id) DO UPDATE
        SET role = excluded.role
        "#,
    )
    .bind(team_id)
    .bind(account_id)
    .bind(role)
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_pending_invitation(
    pool: &PgPool,
    workspace_id: &str,
    email: &str,
    workspace_role: &str,
    invited_by_account_id: &str,
    team_ids: &[String],
    team_role: &str,
) -> Result<String, ApiError> {
    let invitation_id = prefixed_id("invitation");
    let mut tx = pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO invitations
            (invitation_id, workspace_id, email, role, invited_by_account_id, expires_at, status)
        VALUES
            ($1, $2, $3, $4, $5, now() + interval '14 days', 'pending')
        "#,
    )
    .bind(&invitation_id)
    .bind(workspace_id)
    .bind(email)
    .bind(workspace_role)
    .bind(invited_by_account_id)
    .execute(&mut *tx)
    .await?;

    for team_id in unique_strings(team_ids) {
        sqlx::query(
            "INSERT INTO invitation_team_assignments (invitation_id, team_id, role) VALUES ($1, $2, $3)",
        )
        .bind(&invitation_id)
        .bind(&team_id)
        .bind(team_role)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(invitation_id)
}

async fn invitation_value(pool: &PgPool, invitation_id: &str) -> Result<Value, ApiError> {
    invitation_values(pool, Some(invitation_id), None, None)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("invitation not found"))
}

async fn invitation_values_for_workspace(
    pool: &PgPool,
    workspace_id: &str,
) -> Result<Vec<Value>, ApiError> {
    invitation_values(pool, None, Some(workspace_id), None).await
}

async fn invitation_values_for_email(pool: &PgPool, email: &str) -> Result<Vec<Value>, ApiError> {
    invitation_values(pool, None, None, Some(email)).await
}

async fn invitation_values(
    pool: &PgPool,
    invitation_id: Option<&str>,
    workspace_id: Option<&str>,
    email: Option<&str>,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT i.invitation_id, i.workspace_id, w.name AS workspace_name, i.email, i.role, i.status,
               to_char(i.expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS expires_at,
               to_char(i.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at,
               coalesce(array_remove(array_agg(ita.team_id ORDER BY t.name), NULL), '{}') AS team_ids
        FROM invitations i
        JOIN workspaces w ON w.workspace_id = i.workspace_id
        LEFT JOIN invitation_team_assignments ita ON ita.invitation_id = i.invitation_id
        LEFT JOIN teams t ON t.team_id = ita.team_id
        WHERE ($1::text IS NULL OR i.invitation_id = $1)
          AND ($2::text IS NULL OR i.workspace_id = $2)
          AND ($3::text IS NULL OR lower(i.email) = lower($3))
        GROUP BY i.invitation_id, i.workspace_id, w.name, i.email, i.role, i.status, i.expires_at, i.created_at
        ORDER BY i.created_at DESC
        "#,
    )
    .bind(invitation_id)
    .bind(workspace_id)
    .bind(email)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("invitation_id"),
                "workspaceId": row.get::<String, _>("workspace_id"),
                "workspaceName": row.get::<String, _>("workspace_name"),
                "email": row.get::<String, _>("email"),
                "role": row.get::<String, _>("role"),
                "status": row.get::<String, _>("status"),
                "teamIds": row.try_get::<Vec<String>, _>("team_ids").unwrap_or_default(),
                "expiresAt": row.get::<String, _>("expires_at"),
                "createdAt": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn accept_or_decline_invitation(
    pool: &PgPool,
    invitation_id: &str,
    user: &UserPrincipal,
    accept: bool,
) -> Result<(), ApiError> {
    let row = sqlx::query(
        r#"
        SELECT invitation_id, workspace_id, email, role, status, expires_at < now() AS expired
        FROM invitations
        WHERE invitation_id = $1
        "#,
    )
    .bind(invitation_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("invitation not found"))?;

    let email: String = row.get("email");
    if normalize_email(&email)? != user.email {
        return Err(ApiError::forbidden("invitation belongs to another email"));
    }
    let status: String = row.get("status");
    let expired: bool = row.get("expired");
    if status != "pending" || expired {
        return Err(ApiError::bad_request("invitation is not pending"));
    }

    let workspace_id: String = row.get("workspace_id");
    let workspace_role: String = row.get("role");
    let mut tx = pool.begin().await?;
    if accept {
        sqlx::query(
            "UPDATE invitations SET status = 'accepted', accepted_at = now() WHERE invitation_id = $1",
        )
        .bind(invitation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role, state)
            VALUES ($1, $2, $3, $4, 'active')
            ON CONFLICT (workspace_id, account_id) DO UPDATE
            SET role = excluded.role, state = 'active', updated_at = now()
            "#,
        )
        .bind(prefixed_id("membership"))
        .bind(&workspace_id)
        .bind(&user.id)
        .bind(&workspace_role)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO team_memberships (team_id, account_id, role)
            SELECT team_id, $2, role
            FROM invitation_team_assignments
            WHERE invitation_id = $1
            ON CONFLICT (team_id, account_id) DO UPDATE
            SET role = excluded.role
            "#,
        )
        .bind(invitation_id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "UPDATE invitations SET status = 'declined', declined_at = now() WHERE invitation_id = $1",
        )
        .bind(invitation_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn space_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT s.space_id, s.name, s.description,
               to_char(s.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at,
               (
                 SELECT l.layer_id
                 FROM layers l
                 WHERE l.space_id = s.space_id
                 ORDER BY l.created_at ASC
                 LIMIT 1
               ) AS current_layer_id
        FROM spaces s
        WHERE s.workspace_id = $1
        ORDER BY s.created_at ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("space_id"),
                "workspaceId": workspace_id,
                "teamId": "",
                "name": row.get::<String, _>("name"),
                "description": row.get::<String, _>("description"),
                "status": "pending",
                "currentLayerId": row.try_get::<String, _>("current_layer_id").unwrap_or_default(),
                "updatedAt": row.get::<String, _>("updated_at")
            })
        })
        .collect())
}

async fn layer_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT l.layer_id, l.space_id, l.parent_layer_id, l.name,
               COALESCE(
                   (
                       SELECT array_agg(a.artifact_id ORDER BY a.created_at)
                       FROM artifacts a
                       WHERE a.workspace_id = l.workspace_id
                         AND a.layer_id = l.layer_id
                         AND a.state <> 'deleted'
                   ),
                   ARRAY[]::text[]
               ) AS artifact_ids,
               COALESCE(
                   (
                       SELECT array_agg(s.step_id ORDER BY s.captured_at, s.created_at)
                       FROM layer_steps s
                       WHERE s.workspace_id = l.workspace_id
                         AND s.space_id = l.space_id
                         AND s.layer_id = l.layer_id
                   ),
                   ARRAY[]::text[]
               ) AS step_ids
        FROM layers l
        WHERE l.workspace_id = $1
        ORDER BY l.created_at ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            let parent_id = row.try_get::<String, _>("parent_layer_id").ok();
            let artifact_ids = row
                .try_get::<Vec<String>, _>("artifact_ids")
                .unwrap_or_default();
            let step_ids = row
                .try_get::<Vec<String>, _>("step_ids")
                .unwrap_or_default();
            json!({
                "id": row.get::<String, _>("layer_id"),
                "spaceId": row.get::<String, _>("space_id"),
                "parentId": parent_id,
                "name": row.get::<String, _>("name"),
                "kind": if parent_id.is_some() { "proposal" } else { "base" },
                "status": "active",
                "summary": "Persisted Layer",
                "artifactIds": artifact_ids,
                "stepIds": step_ids,
                "gateIds": []
            })
        })
        .collect())
}

async fn artifact_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT artifact_id, space_id, layer_id, logical_path, artifact_kind, state,
               to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM artifacts
        WHERE workspace_id = $1 AND state <> 'deleted'
        ORDER BY logical_path ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            let path = row.get::<String, _>("logical_path");
            let state = row.get::<String, _>("state");
            let redacted = state == "redacted";
            json!({
                "id": row.get::<String, _>("artifact_id"),
                "spaceId": row.get::<String, _>("space_id"),
                "layerId": row.get::<String, _>("layer_id"),
                "name": path.rsplit('/').next().unwrap_or(&path),
                "type": artifact_type(row.get::<String, _>("artifact_kind").as_str()),
                "summary": if redacted { "Restricted by Layer access policy" } else { "Persisted artifact" },
                "location": path,
                "updatedAt": row.get::<String, _>("updated_at"),
                "sizeLabel": if redacted { "redacted" } else { "stored" },
                "proofIds": [],
                "access": {
                    "mode": if redacted { "none" } else { "read" },
                    "canOpen": !redacted,
                    "isRedacted": redacted,
                    "reason": if redacted { "Restricted by Layer access policy" } else { "" }
                }
            })
        })
        .collect())
}

async fn artifact_values_for_layer(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT artifact_id, logical_path, artifact_kind, state, current_file_object_id, current_tree_id,
               to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND state <> 'deleted'
        ORDER BY logical_path ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_all(pool)
    .await?;

    let mut values = Vec::new();
    for row in rows {
        let artifact_id = row.get::<String, _>("artifact_id");
        let path = row.get::<String, _>("logical_path");
        let state = row.get::<String, _>("state");
        let decision = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            layer_id,
            &path,
            Some(&artifact_id),
            account_id,
        )
        .await?;
        let redacted = state == "redacted" || !decision.can_read;
        let reason = if state == "redacted" {
            "Artifact state is redacted"
        } else {
            decision.reason.as_str()
        };
        values.push(artifact_metadata_value(
            workspace_id,
            space_id,
            layer_id,
            &artifact_id,
            &path,
            row.get::<String, _>("artifact_kind").as_str(),
            row.get::<String, _>("updated_at").as_str(),
            row.try_get::<String, _>("current_file_object_id").ok(),
            row.try_get::<String, _>("current_tree_id").ok(),
            redacted,
            reason,
        ));
    }
    Ok(values)
}

async fn artifact_content_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    account_id: &str,
) -> Result<Value, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT artifact_id, logical_path, artifact_kind, state, current_file_object_id
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND artifact_id = $4 AND state <> 'deleted'
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(artifact_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("artifact not found"))?;
    let path = row.get::<String, _>("logical_path");
    let state = row.get::<String, _>("state");
    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        &path,
        Some(artifact_id),
        account_id,
    )
    .await?;
    if state == "redacted" || !decision.can_read {
        return Err(ApiError::forbidden(
            "artifact content is redacted by layer access policy",
        ));
    }
    if let Ok(file_object_id) = row.try_get::<String, _>("current_file_object_id") {
        return artifact_v2_content_value(
            pool,
            workspace_id,
            space_id,
            layer_id,
            artifact_id,
            &path,
            row.get::<String, _>("artifact_kind").as_str(),
            &file_object_id,
        )
        .await;
    }

    let content_row = sqlx::query(
        r#"
        SELECT event_id, body_json,
               to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM timeline_events
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = $3
          AND event_kind = 'artifact.published'
          AND body_json->>'artifactId' = $4
          AND body_json ? 'content'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(artifact_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("artifact content is not available"))?;
    let body = content_row.get::<Value, _>("body_json");

    Ok(json!({
        "artifactId": artifact_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "path": path,
        "type": artifact_type(row.get::<String, _>("artifact_kind").as_str()),
        "content": {
            "encoding": "json",
            "mediaType": body.get("mediaType").cloned().unwrap_or_else(|| json!("application/json")),
            "sha256": body.get("contentHash").cloned().unwrap_or(Value::Null),
            "value": body.get("content").cloned().unwrap_or(Value::Null)
        },
        "source": {
            "kind": "timeline_event",
            "eventId": content_row.get::<String, _>("event_id"),
            "createdAt": content_row.get::<String, _>("created_at")
        }
    }))
}

async fn artifact_v2_content_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    path: &str,
    artifact_kind: &str,
    file_object_id: &str,
) -> Result<Value, ApiError> {
    let file_row = sqlx::query(
        r#"
        SELECT file_object_id, digest, size_bytes, media_type
        FROM file_objects
        WHERE workspace_id = $1 AND space_id = $2 AND file_object_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("file object not found"))?;
    let bytes = file_object_bytes(pool, workspace_id, space_id, file_object_id).await?;
    let chunks = chunk_values_for_file_object(pool, workspace_id, space_id, file_object_id).await?;
    let stored_media_type = file_row
        .try_get::<String, _>("media_type")
        .ok()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let media_type = preview_media_type_for_path(path, &stored_media_type);

    Ok(json!({
        "artifactId": artifact_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "path": path,
        "type": artifact_type(artifact_kind),
        "content": {
            "encoding": "base64",
            "mediaType": media_type,
            "digest": file_row.get::<String, _>("digest"),
            "value": BASE64.encode(bytes)
        },
        "fileObject": {
            "fileObjectId": file_row.get::<String, _>("file_object_id"),
            "sizeBytes": file_row.get::<i64, _>("size_bytes"),
            "chunks": chunks
        },
        "source": {
            "kind": "file_object",
            "storage": "file_object_chunks_v2"
        }
    }))
}

fn artifact_window_limit(limit: Option<usize>) -> Result<usize, ApiError> {
    let limit = limit.unwrap_or(DEFAULT_ARTIFACT_DIFF_WINDOW_LIMIT);
    if limit == 0 {
        return Err(ApiError::bad_request("limit must be greater than zero"));
    }
    Ok(limit.min(MAX_ARTIFACT_DIFF_WINDOW_LIMIT))
}

fn artifact_column_limit(limit: Option<usize>) -> Result<Option<usize>, ApiError> {
    match limit {
        None => Ok(None),
        Some(0) => Err(ApiError::bad_request(
            "columnLimit must be greater than zero",
        )),
        Some(limit) => Ok(Some(limit.min(MAX_DIFF_COLUMN_WINDOW_LIMIT))),
    }
}

#[derive(Clone, Copy, Debug)]
struct WindowRequest {
    start: usize,
    limit: usize,
    column_start: usize,
    column_limit: Option<usize>,
}

struct LensRuntimeDiffRender<'a> {
    workspace_id: &'a str,
    space_id: &'a str,
    layer_id: &'a str,
    artifact_id: Option<&'a str>,
    step_id: Option<&'a str>,
    base_layer_id: Option<&'a str>,
    path: &'a str,
    target: Option<&'a ArtifactTextWindow>,
    base: Option<&'a ArtifactTextWindow>,
    window_request: WindowRequest,
    summary: &'a str,
    mode: &'a str,
    limitation: Option<&'a str>,
}

fn lens_runtime_diff_value(input: LensRuntimeDiffRender<'_>) -> Value {
    let preview_window = input
        .target
        .or(input.base)
        .expect("lens runtime needs a target or base text window");
    let has_more = window_has_more(
        input.window_request.start,
        preview_window.lines.len(),
        preview_window.total_lines,
    );
    let has_long_lines =
        text_window_has_long_columns(input.target) || text_window_has_long_columns(input.base);
    let window_value = json!({
        "start": input.window_request.start,
        "limit": input.window_request.limit,
        "count": preview_window.lines.len(),
        "totalLines": preview_window.total_lines,
        "hasMore": has_more,
        "hasMoreBefore": input.window_request.start > 0,
        "hasMoreAfter": has_more
    });
    let column_window_value = json!({
        "columnStart": input.window_request.column_start,
        "columnLimit": input.window_request.column_limit,
        "hasLongLines": has_long_lines
    });
    let compare_missing_base_as_insert = input.step_id.is_some();
    let diff_lines = lens_runtime_diff_lines(
        input.target,
        input.base,
        input.window_request.start,
        compare_missing_base_as_insert,
    );
    let old_line_count = input
        .base
        .map(|window| window.lines.len())
        .unwrap_or_else(|| {
            if compare_missing_base_as_insert {
                0
            } else {
                preview_window.lines.len()
            }
        });
    let new_line_count = input
        .target
        .map(|window| window.lines.len())
        .unwrap_or_default();
    let new_line_count = if input.target.is_some() {
        new_line_count
    } else {
        0
    };
    let runtime_value = json!({
        "id": "layrs.server.lens-runtime.text",
        "lensId": lens_id_for_path_and_media_type(input.path, &preview_window.media_type)
    });
    let source_value = json!({
        "kind": "lens_runtime",
        "runtime": runtime_value,
        "target": input.target.map(|window| window.source.clone()),
        "base": input.base.map(|window| window.source.clone())
    });

    json!({
        "artifactId": input.artifact_id,
        "stepId": input.step_id,
        "workspaceId": input.workspace_id,
        "spaceId": input.space_id,
        "layerId": input.layer_id,
        "baseLayerId": input.base_layer_id,
        "path": input.path,
        "type": artifact_type(&preview_window.artifact_kind),
        "window": window_value,
        "preview": {
            "kind": preview_kind_for_path(input.path, &preview_window.media_type),
            "title": input.path,
            "body": preview_window.lines.iter().map(|line| line.text_segment.as_str()).collect::<Vec<_>>().join("\n"),
            "mediaType": preview_window.media_type,
            "fields": {
                "lines": preview_window.lines.iter().map(text_window_line_value).collect::<Vec<_>>(),
                "window": window_value,
                "columnWindow": column_window_value,
                "windowed": true,
                "contentHash": preview_window.content_hash,
                "baseLayerId": input.base_layer_id,
                "exactBaseDiff": false,
                "runtime": runtime_value,
                "source": source_value
            }
        },
        "diff": {
            "kind": "textLines",
            "summary": input.summary,
            "hunks": [{
                "oldStart": input.window_request.start + 1,
                "oldLines": old_line_count,
                "newStart": input.window_request.start + 1,
                "newLines": new_line_count,
                "lines": diff_lines
            }],
            "fields": {
                "mode": input.mode,
                "window": window_value,
                "columnWindow": column_window_value,
                "windowed": true,
                "totalLines": preview_window.total_lines,
                "oldTotalLines": input.base.map(|window| window.total_lines),
                "newTotalLines": input.target.map(|window| window.total_lines),
                "renderedLineCount": preview_window.lines.len(),
                "hasMore": has_more,
                "hasMoreBefore": input.window_request.start > 0,
                "hasMoreAfter": has_more,
                "hasLongLines": has_long_lines,
                "contentHash": preview_window.content_hash,
                "baseLayerId": input.base_layer_id,
                "exactBaseDiff": false,
                "limitation": input.limitation,
                "runtime": runtime_value
            }
        },
        "source": source_value
    })
}

async fn artifact_diff_window_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    account_id: &str,
    window_request: WindowRequest,
    base_layer_id: Option<&str>,
) -> Result<Value, ApiError> {
    let window = artifact_text_window(
        pool,
        workspace_id,
        space_id,
        layer_id,
        artifact_id,
        account_id,
        window_request,
    )
    .await?;
    let base_requested = base_layer_id.is_some();
    let summary = if base_requested {
        "Windowed artifact preview; exact base diff is not available in this server version"
    } else {
        "Windowed artifact preview"
    };

    Ok(lens_runtime_diff_value(LensRuntimeDiffRender {
        workspace_id,
        space_id,
        layer_id,
        artifact_id: Some(artifact_id),
        step_id: None,
        base_layer_id,
        path: &window.path,
        target: Some(&window),
        base: None,
        window_request,
        summary,
        mode: "preview",
        limitation: Some(
            "V1 returns a server-windowed artifact preview. Exact base comparison is not computed yet.",
        ),
    }))
}

async fn artifact_text_window(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    account_id: &str,
    window_request: WindowRequest,
) -> Result<ArtifactTextWindow, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT artifact_id, logical_path, artifact_kind, state, current_file_object_id
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND artifact_id = $4 AND state <> 'deleted'
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(artifact_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("artifact not found"))?;
    let path = row.get::<String, _>("logical_path");
    let state = row.get::<String, _>("state");
    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        &path,
        Some(artifact_id),
        account_id,
    )
    .await?;
    if state == "redacted" || !decision.can_read {
        return Err(ApiError::forbidden(
            "artifact content is redacted by layer access policy",
        ));
    }
    let artifact_kind = row.get::<String, _>("artifact_kind");
    if let Ok(file_object_id) = row.try_get::<String, _>("current_file_object_id") {
        return file_object_text_window(
            pool,
            workspace_id,
            space_id,
            &path,
            artifact_kind,
            &file_object_id,
            window_request,
        )
        .await;
    }

    Err(ApiError::not_found(
        "artifact content is not available in the chunked store",
    ))
}

async fn file_object_text_window(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    path: &str,
    artifact_kind: String,
    file_object_id: &str,
    window_request: WindowRequest,
) -> Result<ArtifactTextWindow, ApiError> {
    let file_row = sqlx::query(
        r#"
        SELECT file_object_id, digest, size_bytes, media_type
        FROM file_objects
        WHERE workspace_id = $1 AND space_id = $2 AND file_object_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("file object not found"))?;
    let stored_media_type = file_row
        .try_get::<String, _>("media_type")
        .ok()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    if !is_textual_artifact(path, &stored_media_type) {
        return Err(ApiError::bad_request(
            "windowed diff preview is available for text artifacts only",
        ));
    }
    let media_type = preview_media_type_for_path(path, &stored_media_type).to_string();

    let chunk_rows = sqlx::query(
        r#"
        SELECT oc.content_bytes
        FROM file_object_chunks foc
        JOIN object_chunks oc ON oc.chunk_id = foc.chunk_id
        WHERE foc.file_object_id = $1
          AND oc.workspace_id = $2
          AND oc.space_id = $3
          AND oc.state = 'available'
        ORDER BY foc.chunk_index ASC
        "#,
    )
    .bind(file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?;
    let mut builder = TextLineWindowBuilder::new(window_request);
    for row in chunk_rows {
        let bytes = row
            .try_get::<Vec<u8>, _>("content_bytes")
            .ok()
            .ok_or_else(|| ApiError::not_found("chunk bytes are not available"))?;
        builder.push_lossy_utf8(&bytes);
    }
    let text_window = builder.finish();

    Ok(ArtifactTextWindow {
        lines: text_window.lines,
        total_lines: text_window.total_lines,
        path: path.to_string(),
        artifact_kind,
        media_type,
        content_hash: Some(file_row.get::<String, _>("digest")),
        source: json!({
            "kind": "file_object",
            "storage": "file_object_chunks_v2",
            "fileObjectId": file_row.get::<String, _>("file_object_id"),
            "sizeBytes": file_row.get::<i64, _>("size_bytes")
        }),
    })
}

#[derive(Debug)]
struct ArtifactTextWindow {
    lines: Vec<TextWindowLine>,
    total_lines: usize,
    path: String,
    artifact_kind: String,
    media_type: String,
    content_hash: Option<String>,
    source: Value,
}

#[derive(Debug)]
struct TextLineWindow {
    lines: Vec<TextWindowLine>,
    total_lines: usize,
}

#[derive(Debug)]
struct TextWindowLine {
    text_segment: String,
    text_length: usize,
    column_start: usize,
    column_end: usize,
    has_more_columns: bool,
}

struct TextLineWindowBuilder {
    start: usize,
    limit: usize,
    column_start: usize,
    column_limit: Option<usize>,
    lines: Vec<TextWindowLine>,
    total_lines: usize,
    current_line_segment: String,
    current_line_segment_len: usize,
    current_line_len: usize,
    saw_content: bool,
}

impl TextLineWindowBuilder {
    fn new(request: WindowRequest) -> Self {
        Self {
            start: request.start,
            limit: request.limit,
            column_start: request.column_start,
            column_limit: request.column_limit,
            lines: Vec::new(),
            total_lines: 0,
            current_line_segment: String::new(),
            current_line_segment_len: 0,
            current_line_len: 0,
            saw_content: false,
        }
    }

    fn push_lossy_utf8(&mut self, bytes: &[u8]) {
        self.push_text(&String::from_utf8_lossy(bytes));
    }

    fn push_text(&mut self, text: &str) {
        if !text.is_empty() {
            self.saw_content = true;
        }
        for character in text.chars() {
            if character == '\n' {
                self.flush_line();
            } else {
                if character != '\r' {
                    if self.total_lines >= self.start
                        && self.lines.len() < self.limit
                        && self.current_line_len >= self.column_start
                        && self
                            .column_limit
                            .map_or(true, |limit| self.current_line_segment_len < limit)
                    {
                        self.current_line_segment.push(character);
                        self.current_line_segment_len += 1;
                    }
                    self.current_line_len += 1;
                }
            }
        }
    }

    fn finish(mut self) -> TextLineWindow {
        if self.saw_content && (self.current_line_len > 0 || self.total_lines == 0) {
            self.flush_line();
        }
        TextLineWindow {
            lines: self.lines,
            total_lines: self.total_lines,
        }
    }

    fn flush_line(&mut self) {
        if self.total_lines >= self.start && self.lines.len() < self.limit {
            let segment_len = self.current_line_segment_len;
            let column_start = self.column_start.min(self.current_line_len);
            let column_end = column_start
                .saturating_add(segment_len)
                .min(self.current_line_len);
            self.lines.push(TextWindowLine {
                text_segment: self.current_line_segment.clone(),
                text_length: self.current_line_len,
                column_start,
                column_end,
                has_more_columns: column_end < self.current_line_len,
            });
        }
        self.total_lines += 1;
        self.current_line_segment.clear();
        self.current_line_segment_len = 0;
        self.current_line_len = 0;
    }
}

#[cfg(test)]
fn text_window_from_str(text: &str, start: usize, limit: usize) -> TextLineWindow {
    let mut builder = TextLineWindowBuilder::new(WindowRequest {
        start,
        limit,
        column_start: 0,
        column_limit: None,
    });
    builder.push_text(text);
    builder.finish()
}

fn window_has_more(start: usize, count: usize, total_lines: usize) -> bool {
    start.saturating_add(count) < total_lines
}

fn text_window_has_long_columns(window: Option<&ArtifactTextWindow>) -> bool {
    window.is_some_and(|window| {
        window
            .lines
            .iter()
            .any(|line| line.has_more_columns || line.column_start > 0)
    })
}

fn lens_runtime_diff_lines(
    target: Option<&ArtifactTextWindow>,
    base: Option<&ArtifactTextWindow>,
    start: usize,
    compare_missing_base_as_insert: bool,
) -> Vec<Value> {
    match base {
        None => target
            .into_iter()
            .flat_map(|window| {
                window.lines.iter().enumerate().map(move |(index, line)| {
                    let line_number = start + index + 1;
                    if compare_missing_base_as_insert {
                        lens_diff_line_value("insert", None, Some(line_number), line)
                    } else {
                        lens_diff_line_value("equal", Some(line_number), Some(line_number), line)
                    }
                })
            })
            .collect(),
        Some(base_window) => {
            let target_lines = target
                .map(|window| window.lines.as_slice())
                .unwrap_or_default();
            let max_count = base_window.lines.len().max(target_lines.len());
            let mut lines = Vec::new();
            for index in 0..max_count {
                let old_line_number = start + index + 1;
                let new_line_number = start + index + 1;
                match (base_window.lines.get(index), target_lines.get(index)) {
                    (Some(old_line), Some(new_line))
                        if text_window_lines_match(old_line, new_line) =>
                    {
                        lines.push(lens_diff_line_value(
                            "equal",
                            Some(old_line_number),
                            Some(new_line_number),
                            new_line,
                        ));
                    }
                    (Some(old_line), Some(new_line)) => {
                        lines.push(lens_diff_line_value(
                            "delete",
                            Some(old_line_number),
                            None,
                            old_line,
                        ));
                        lines.push(lens_diff_line_value(
                            "insert",
                            None,
                            Some(new_line_number),
                            new_line,
                        ));
                    }
                    (Some(old_line), None) => {
                        lines.push(lens_diff_line_value(
                            "delete",
                            Some(old_line_number),
                            None,
                            old_line,
                        ));
                    }
                    (None, Some(new_line)) => {
                        lines.push(lens_diff_line_value(
                            "insert",
                            None,
                            Some(new_line_number),
                            new_line,
                        ));
                    }
                    (None, None) => {}
                }
            }
            lines
        }
    }
}

fn text_window_lines_match(left: &TextWindowLine, right: &TextWindowLine) -> bool {
    left.text_segment == right.text_segment
        && left.text_length == right.text_length
        && left.column_start == right.column_start
        && left.column_end == right.column_end
        && left.has_more_columns == right.has_more_columns
}

fn text_window_line_value(line: &TextWindowLine) -> Value {
    json!({
        "textSegment": line.text_segment,
        "text": line.text_segment,
        "textLength": line.text_length,
        "columnStart": line.column_start,
        "columnEnd": line.column_end,
        "hasMoreColumns": line.has_more_columns
    })
}

fn lens_diff_line_value(
    op: &str,
    old_line: Option<usize>,
    new_line: Option<usize>,
    line: &TextWindowLine,
) -> Value {
    json!({
        "op": op,
        "oldLine": old_line,
        "newLine": new_line,
        "text": line.text_segment,
        "textSegment": line.text_segment,
        "textLength": line.text_length,
        "columnStart": line.column_start,
        "columnEnd": line.column_end,
        "hasMoreColumns": line.has_more_columns
    })
}

fn preview_kind_for_path(path: &str, media_type: &str) -> &'static str {
    if is_code_path(path) {
        return "code";
    }
    if is_text_path(path) || media_type == "text/markdown" || media_type.starts_with("text/") {
        return "text";
    }
    "raw"
}

fn preview_media_type_for_path<'a>(path: &str, stored_media_type: &'a str) -> &'a str {
    if stored_media_type != "application/octet-stream" {
        return stored_media_type;
    }

    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".mdx") || lower.ends_with(".markdown") {
        return "text/markdown";
    }
    if is_code_path(path) || is_text_path(path) {
        return "text/plain";
    }
    stored_media_type
}

fn is_textual_artifact(path: &str, media_type: &str) -> bool {
    media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json"
                | "application/javascript"
                | "application/typescript"
                | "application/xml"
                | "application/x-yaml"
                | "application/yaml"
        )
        || is_code_path(path)
        || is_text_path(path)
}

fn is_code_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".css", ".html", ".js", ".json", ".jsx", ".mjs", ".cjs", ".rs", ".toml", ".ts", ".tsx",
        ".xml", ".yaml", ".yml", ".py", ".go", ".java", ".kt", ".kts", ".swift", ".c", ".h", ".cc",
        ".cpp", ".cxx", ".hpp", ".cs", ".php", ".rb", ".sh", ".bash", ".zsh", ".ps1", ".sql",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension))
}

fn is_text_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".txt", ".md", ".mdx", ".markdown", ".rst", ".log"]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

async fn access_registry_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT policy_id, layer_id,
               to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM layer_access_policies
        WHERE workspace_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("policy_id"),
                "workspaceId": workspace_id,
                "layerId": row.get::<String, _>("layer_id"),
                "rules": [],
                "updatedAt": row.get::<String, _>("updated_at")
            })
        })
        .collect())
}

async fn timeline_event_values(
    pool: &PgPool,
    workspace_id: &str,
    space_id: Option<&str>,
    layer_id: Option<&str>,
    cursor: Option<&str>,
    limit: Option<i64>,
) -> Result<Vec<Value>, ApiError> {
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let rows = sqlx::query(
        r#"
        SELECT event_id, space_id, layer_id, event_kind, title, body_json,
               to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM timeline_events
        WHERE workspace_id = $1
          AND ($2::text IS NULL OR space_id = $2)
          AND ($3::text IS NULL OR layer_id = $3)
          AND (
            $4::text IS NULL
            OR created_at > COALESCE(
                (SELECT created_at FROM timeline_events WHERE workspace_id = $1 AND event_id = $4),
                '-infinity'::timestamptz
            )
          )
        ORDER BY created_at ASC
        LIMIT $5
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(cursor)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("event_id"),
                "workspaceId": workspace_id,
                "spaceId": row.try_get::<String, _>("space_id").ok(),
                "layerId": row.try_get::<String, _>("layer_id").ok(),
                "kind": row.get::<String, _>("event_kind"),
                "title": row.get::<String, _>("title"),
                "body": redact_timeline_body(row.get::<Value, _>("body_json")),
                "createdAt": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn timeline_event_by_id(
    pool: &PgPool,
    workspace_id: &str,
    event_id: &str,
) -> Result<Value, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT event_id, space_id, layer_id, event_kind, title, body_json,
               to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM timeline_events
        WHERE workspace_id = $1 AND event_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(event_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("timeline event not found"))?;

    Ok(json!({
        "id": row.get::<String, _>("event_id"),
        "workspaceId": workspace_id,
        "spaceId": row.try_get::<String, _>("space_id").ok(),
        "layerId": row.try_get::<String, _>("layer_id").ok(),
        "kind": row.get::<String, _>("event_kind"),
        "title": row.get::<String, _>("title"),
        "body": redact_timeline_body(row.get::<Value, _>("body_json")),
        "createdAt": row.get::<String, _>("created_at")
    }))
}

async fn existing_redacted_artifact(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    path: &str,
) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM artifacts
          WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = $4 AND state = 'redacted'
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(path)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)
}

fn publish_artifact_uses_v2(body: &PublishArtifactBody) -> bool {
    body.file_object_id.is_some()
        || body.file_object_id_camel.is_some()
        || body.object_id.is_some()
        || body.object_id_camel.is_some()
        || body.tree_id.is_some()
        || body.tree_id_camel.is_some()
        || body.sha256.is_some()
        || body.content_hash.is_some()
        || body.size_bytes.is_some()
        || body.size_bytes_camel.is_some()
        || !body.chunks.is_empty()
}

async fn load_sync_batch_response(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    idempotency_key: &str,
) -> Result<Option<Value>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT response_json
        FROM sync_batches
        WHERE workspace_id = $1 AND space_id = $2 AND idempotency_key = $3 AND status = 'applied'
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)
}

async fn current_policy_epoch(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<i64, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT policy_epoch
        FROM layer_access_policies
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("layer access policy not found"))
}

#[derive(Debug)]
struct StoreObjectIndex {
    root_tree_id: Option<String>,
    file_by_path: HashMap<String, StoreFileObject>,
    deleted_paths: Vec<String>,
}

#[derive(Clone, Debug)]
struct StoreFileObject {
    file_object_id: String,
    digest: String,
    size_bytes: i64,
    media_type: Option<String>,
}

async fn load_store_file_object_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<Option<StoreFileObject>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT file_object_id, digest, size_bytes, media_type
        FROM file_objects
        WHERE workspace_id = $1 AND space_id = $2 AND file_object_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_optional(&mut **tx)
    .await?;

    Ok(row.map(|row| StoreFileObject {
        file_object_id: row.get("file_object_id"),
        digest: row.get("digest"),
        size_bytes: row.get("size_bytes"),
        media_type: row.try_get("media_type").ok(),
    }))
}

async fn upsert_store_objects_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    store_objects: Vec<PublishStoreObjectBody>,
) -> Result<StoreObjectIndex, ApiError> {
    let mut root_tree_id = None;
    let mut file_by_path = HashMap::new();
    let mut file_by_id = HashMap::new();
    let mut tree_entries_by_tree: HashMap<String, Vec<PublishTreeEntryBody>> = HashMap::new();
    let mut deleted_paths = Vec::new();

    for object in store_objects {
        let object_type = object
            .object_type
            .or(object.object_type_camel)
            .unwrap_or_else(|| "file".to_string());
        let raw_object_id = object.object_id.or(object.object_id_camel);
        match object_type.as_str() {
            "tree" => {
                let object_id =
                    validate_object_digest(raw_object_id.as_deref().ok_or_else(|| {
                        ApiError::bad_request("storeObjects.objectId is required")
                    })?)?;
                upsert_tree_object_shell_in_tx(
                    tx,
                    workspace_id,
                    space_id,
                    &object_id,
                    object.size.map(|value| value as i32).unwrap_or_default(),
                    account_id,
                )
                .await?;
                tree_entries_by_tree.insert(object_id.clone(), object.entries);
                root_tree_id = Some(object_id);
            }
            "file" => {
                let object_id =
                    validate_object_digest(raw_object_id.as_deref().ok_or_else(|| {
                        ApiError::bad_request("storeObjects.objectId is required")
                    })?)?;
                let path = object
                    .path
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(validate_publish_path)
                    .transpose()?;
                let digest = validate_object_digest(
                    object
                        .hash
                        .or(object.digest)
                        .as_deref()
                        .unwrap_or(&object_id),
                )?;
                let media_type = object.media_type.or(object.media_type_camel);
                let chunks = upsert_store_object_chunks_in_tx(
                    tx,
                    workspace_id,
                    space_id,
                    account_id,
                    object.chunks,
                )
                .await?;
                let size_bytes = object
                    .size_bytes
                    .or(object.size_bytes_camel)
                    .or_else(|| object.size.map(|value| value as i64))
                    .unwrap_or_else(|| chunks.iter().map(|chunk| chunk.size_bytes).sum());
                let file_object_id = upsert_file_object_in_tx(
                    tx,
                    workspace_id,
                    space_id,
                    Some(&object_id),
                    &digest,
                    size_bytes,
                    media_type.as_deref().unwrap_or("application/octet-stream"),
                    &chunks,
                    account_id,
                )
                .await?;
                let file = StoreFileObject {
                    file_object_id: file_object_id.clone(),
                    digest,
                    size_bytes,
                    media_type,
                };
                if let Some(path) = path {
                    file_by_path.insert(path, file.clone());
                }
                file_by_id.insert(file_object_id, file);
            }
            "tombstone" => {
                if let Some(path) = object.path {
                    deleted_paths.push(validate_publish_path(path.trim())?);
                }
            }
            _ => {
                return Err(ApiError::bad_request("unsupported store object type"));
            }
        }
    }

    for (tree_id, entries) in tree_entries_by_tree {
        for entry in entries {
            let path = validate_publish_path(entry.path.trim())?;
            let file_object_id = entry
                .file_object_id
                .or(entry.file_object_id_camel)
                .or(entry.object_id)
                .or(entry.object_id_camel)
                .ok_or_else(|| ApiError::bad_request("tree entry fileObjectId is required"))?;
            let file_object_id = validate_object_digest(&file_object_id)?;
            let file = match file_by_id.get(&file_object_id) {
                Some(file) => file.clone(),
                None => load_store_file_object_in_tx(tx, workspace_id, space_id, &file_object_id)
                    .await?
                    .ok_or_else(|| {
                        ApiError::bad_request("tree entry references missing file object")
                    })?,
            };
            sqlx::query(
                r#"
                INSERT INTO tree_entries
                    (tree_id, logical_path, entry_kind, file_object_id)
                VALUES
                    ($1, $2, 'file', $3)
                ON CONFLICT (tree_id, logical_path) DO UPDATE SET
                    file_object_id = EXCLUDED.file_object_id
                "#,
            )
            .bind(&tree_id)
            .bind(&path)
            .bind(&file.file_object_id)
            .execute(&mut **tx)
            .await?;
            file_by_path.insert(path, file);
        }
    }

    Ok(StoreObjectIndex {
        root_tree_id,
        file_by_path,
        deleted_paths,
    })
}

async fn upsert_store_object_chunks_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    _account_id: &str,
    chunks: Vec<PublishStoreObjectChunkBody>,
) -> Result<Vec<ChunkDescriptor>, ApiError> {
    let mut descriptors = Vec::new();
    let mut next_offset = 0;
    for chunk in chunks {
        let raw_chunk_id = chunk.chunk_id.or(chunk.chunk_id_camel);
        let expected_digest = chunk.digest.or(chunk.hash);
        let declared_size_bytes = chunk
            .size_bytes
            .or(chunk.size_bytes_camel)
            .or_else(|| chunk.size.map(|value| value as i64));
        let byte_offset = chunk
            .byte_offset
            .or(chunk.byte_offset_camel)
            .unwrap_or(next_offset);

        let chunk_id = validate_object_digest(
            raw_chunk_id
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("storeObjects chunkId is required"))?,
        )?;
        if let Some(expected) = expected_digest {
            if validate_object_digest(&expected)? != chunk_id {
                return Err(ApiError::bad_request("chunk digest does not match chunkId"));
            }
        }
        let row = sqlx::query(
            r#"
            SELECT digest, size_bytes
            FROM object_chunks
            WHERE workspace_id = $1
              AND space_id = $2
              AND chunk_id = $3
              AND state = 'available'
            "#,
        )
        .bind(workspace_id)
        .bind(space_id)
        .bind(&chunk_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            ApiError::bad_request("storeObjects chunk bytes must be uploaded before publish")
        })?;
        let stored_digest = row.get::<String, _>("digest");
        let actual_digest = validate_object_digest(&stored_digest)?;
        if actual_digest != chunk_id {
            return Err(ApiError::bad_request(
                "stored chunk digest does not match chunkId",
            ));
        }
        let size_bytes = row.get::<i64, _>("size_bytes");
        if let Some(expected_size) = declared_size_bytes {
            if expected_size != size_bytes {
                return Err(ApiError::bad_request(
                    "chunk size does not match uploaded bytes",
                ));
            }
        }
        descriptors.push(ChunkDescriptor {
            chunk_id,
            digest: actual_digest,
            size_bytes,
            byte_offset,
        });
        next_offset = byte_offset + size_bytes;
    }
    Ok(descriptors)
}

async fn upsert_tree_object_shell_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    tree_id: &str,
    entry_count: i32,
    account_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO tree_objects
            (tree_id, workspace_id, space_id, digest, entry_count, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (workspace_id, space_id, digest) DO UPDATE SET
            entry_count = EXCLUDED.entry_count
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(tree_id)
    .bind(entry_count)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn apply_store_object_to_artifact(
    artifact: &mut PublishArtifactBody,
    store_index: &StoreObjectIndex,
) -> Result<(), ApiError> {
    let path = required_artifact_path(artifact)?;
    if let Some(file) = store_index.file_by_path.get(&path) {
        artifact.file_object_id = Some(file.file_object_id.clone());
        artifact.sha256 = Some(file.digest.clone());
        artifact.size_bytes = Some(file.size_bytes);
        if artifact.media_type.is_none() && artifact.media_type_camel.is_none() {
            artifact.media_type = file.media_type.clone();
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ChunkDescriptor {
    chunk_id: String,
    digest: String,
    size_bytes: i64,
    byte_offset: i64,
}

async fn publish_artifact_v2_in_tx(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
    body: PublishArtifactBody,
) -> Result<(String, String, Option<String>), ApiError> {
    let logical_path = required_artifact_path(&body)?;
    let artifact_kind =
        normalize_artifact_kind(body.kind.as_deref().or(body.artifact_type.as_deref()))?;
    let provided_artifact_id = body.id.or(body.artifact_id).or(body.artifact_id_camel);
    let file_object_id = body
        .file_object_id
        .or(body.file_object_id_camel)
        .or(body.object_id)
        .or(body.object_id_camel);
    let media_type = body
        .media_type
        .or(body.media_type_camel)
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let explicit_digest = body
        .sha256
        .or(body.content_hash)
        .map(|value| validate_object_digest(&value))
        .transpose()?;
    let explicit_size = body.size_bytes.or(body.size_bytes_camel);

    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        &logical_path,
        provided_artifact_id.as_deref(),
        account_id,
    )
    .await?;
    if !decision.can_write {
        return Err(ApiError::forbidden(format!(
            "path cannot be published: {}",
            decision.reason
        )));
    }
    if existing_redacted_artifact(pool, workspace_id, space_id, layer_id, &logical_path).await? {
        return Err(ApiError::forbidden(
            "path collides with a redacted artifact and cannot be published",
        ));
    }

    let chunks = chunk_descriptors(pool, workspace_id, space_id, body.chunks).await?;
    let final_file_object_id = if chunks.is_empty() {
        let file_object_id = file_object_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ApiError::bad_request("fileObjectId or chunks are required for V2 publish")
            })?;
        ensure_file_object_in_space_in_tx(tx, workspace_id, space_id, &file_object_id).await?;
        file_object_id
    } else {
        let digest = explicit_digest
            .clone()
            .or_else(|| {
                if chunks.len() == 1 {
                    Some(chunks[0].digest.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| hash_chunk_manifest(&chunks));
        let size_bytes =
            explicit_size.unwrap_or_else(|| chunks.iter().map(|chunk| chunk.size_bytes).sum());
        upsert_file_object_in_tx(
            tx,
            workspace_id,
            space_id,
            file_object_id.as_deref(),
            &digest,
            size_bytes,
            &media_type,
            &chunks,
            account_id,
        )
        .await?
    };
    let artifact_id = upsert_artifact_metadata_v2_in_tx(
        tx,
        workspace_id,
        space_id,
        layer_id,
        provided_artifact_id.as_deref(),
        &logical_path,
        artifact_kind,
        account_id,
        Some(&final_file_object_id),
        None,
    )
    .await?;
    let event_id = insert_timeline_event_in_tx(
        tx,
        workspace_id,
        Some(space_id),
        Some(layer_id),
        "artifact.published",
        "Artifact published",
        json!({
            "artifactId": artifact_id,
            "path": logical_path,
            "kind": artifact_kind,
            "fileObjectId": final_file_object_id,
            "contentHash": explicit_digest,
            "digest": final_file_object_id,
            "mediaType": media_type,
            "chunks": chunks.iter().map(|chunk| json!({
                "chunkId": chunk.chunk_id,
                "digest": chunk.digest,
                "sizeBytes": chunk.size_bytes,
                "byteOffset": chunk.byte_offset
            })).collect::<Vec<_>>(),
            "storage": "file_object_chunks_v2"
        }),
    )
    .await?;
    Ok((artifact_id, event_id, Some(final_file_object_id)))
}

async fn delete_artifact_tombstone_in_tx(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
    logical_path: &str,
) -> Result<(String, Value, String), ApiError> {
    let existing = sqlx::query(
        r#"
        SELECT artifact_id, artifact_kind
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = $4
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(logical_path)
    .fetch_optional(pool)
    .await?;
    let existing_artifact_id = existing
        .as_ref()
        .map(|row| row.get::<String, _>("artifact_id"));
    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        logical_path,
        existing_artifact_id.as_deref(),
        account_id,
    )
    .await?;
    if !decision.can_write {
        return Err(ApiError::forbidden(format!(
            "path cannot be deleted: {}",
            decision.reason
        )));
    }
    let artifact_kind = existing
        .as_ref()
        .map(|row| row.get::<String, _>("artifact_kind"))
        .unwrap_or_else(|| "file".to_string());
    let artifact_id = existing_artifact_id.unwrap_or_else(|| prefixed_id("artifact"));
    if existing.is_some() {
        sqlx::query(
            "UPDATE artifacts SET state = 'deleted', current_file_object_id = NULL, current_tree_id = NULL, updated_at = now() WHERE artifact_id = $1",
        )
        .bind(&artifact_id)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO artifacts
                (artifact_id, workspace_id, space_id, layer_id, logical_path, artifact_kind, state, created_by_account_id)
            VALUES
                ($1, $2, $3, $4, $5, $6, 'deleted', $7)
            "#,
        )
        .bind(&artifact_id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(layer_id)
        .bind(logical_path)
        .bind(&artifact_kind)
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
    }
    let event_id = insert_timeline_event_in_tx(
        tx,
        workspace_id,
        Some(space_id),
        Some(layer_id),
        "artifact.deleted",
        "Artifact deleted",
        json!({
            "artifactId": artifact_id,
            "path": logical_path,
            "kind": artifact_kind,
            "state": "deleted",
            "contentAvailable": false,
            "storage": "artifact_state_deleted"
        }),
    )
    .await?;
    let artifact = json!({
        "id": artifact_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "name": logical_path.rsplit('/').next().unwrap_or(logical_path),
        "type": artifact_type(&artifact_kind),
        "summary": "Deleted artifact tombstone",
        "location": logical_path,
        "state": "deleted",
        "proofIds": [],
        "access": {
            "mode": "none",
            "canOpen": false,
            "isRedacted": false,
            "isDeleted": true,
            "reason": "Artifact was deleted"
        }
    });
    Ok((artifact_id, artifact, event_id))
}

async fn chunk_descriptors(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    chunks: Vec<PublishArtifactChunkBody>,
) -> Result<Vec<ChunkDescriptor>, ApiError> {
    let mut descriptors = Vec::new();
    let mut next_offset = 0;
    for chunk in chunks {
        let chunk_id = chunk
            .id
            .or(chunk.chunk_id)
            .or(chunk.chunk_id_camel)
            .or_else(|| chunk.sha256.as_ref().map(|value| value.to_string()))
            .ok_or_else(|| ApiError::bad_request("chunkId is required"))?;
        let chunk_id = validate_chunk_id(&chunk_id)?;
        let row = sqlx::query(
            r#"
            SELECT digest, size_bytes
            FROM object_chunks
            WHERE workspace_id = $1 AND space_id = $2 AND chunk_id = $3 AND state = 'available'
            "#,
        )
        .bind(workspace_id)
        .bind(space_id)
        .bind(&chunk_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::bad_request(format!("chunk {chunk_id} is not available")))?;
        let digest = row.get::<String, _>("digest");
        if let Some(expected) = chunk.sha256.as_deref() {
            if validate_object_digest(expected)? != digest {
                return Err(ApiError::bad_request(format!(
                    "chunk {chunk_id} digest does not match"
                )));
            }
        }
        let size_bytes = row.get::<i64, _>("size_bytes");
        if let Some(expected_size) = chunk.size_bytes.or(chunk.size_bytes_camel) {
            if expected_size != size_bytes {
                return Err(ApiError::bad_request(format!(
                    "chunk {chunk_id} size does not match"
                )));
            }
        }
        let byte_offset = chunk
            .byte_offset
            .or(chunk.byte_offset_camel)
            .unwrap_or(next_offset);
        descriptors.push(ChunkDescriptor {
            chunk_id,
            digest,
            size_bytes,
            byte_offset,
        });
        next_offset = byte_offset + size_bytes;
    }
    Ok(descriptors)
}

async fn upsert_file_object_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    provided_file_object_id: Option<&str>,
    digest: &str,
    size_bytes: i64,
    media_type: &str,
    chunks: &[ChunkDescriptor],
    account_id: &str,
) -> Result<String, ApiError> {
    let file_object_id = provided_file_object_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| prefixed_id("file_object"));
    let file_object_id = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO file_objects
            (file_object_id, workspace_id, space_id, digest, size_bytes, media_type, chunk_count, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (workspace_id, space_id, digest) DO UPDATE SET
            media_type = EXCLUDED.media_type,
            chunk_count = EXCLUDED.chunk_count
        RETURNING file_object_id
        "#,
    )
    .bind(&file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(digest)
    .bind(size_bytes)
    .bind(media_type)
    .bind(chunks.len() as i32)
    .bind(account_id)
    .fetch_one(&mut **tx)
    .await?;
    for (index, chunk) in chunks.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO file_object_chunks
                (file_object_id, chunk_index, chunk_id, byte_offset, size_bytes)
            VALUES
                ($1, $2, $3, $4, $5)
            ON CONFLICT (file_object_id, chunk_index) DO UPDATE SET
                chunk_id = EXCLUDED.chunk_id,
                byte_offset = EXCLUDED.byte_offset,
                size_bytes = EXCLUDED.size_bytes
            "#,
        )
        .bind(&file_object_id)
        .bind(index as i32)
        .bind(&chunk.chunk_id)
        .bind(chunk.byte_offset)
        .bind(chunk.size_bytes)
        .execute(&mut **tx)
        .await?;
    }
    Ok(file_object_id)
}

async fn ensure_file_object_in_space_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM file_objects
            WHERE workspace_id = $1 AND space_id = $2 AND file_object_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::bad_request("fileObjectId is not available"))
    }
}

async fn upsert_artifact_metadata_v2_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: Option<&str>,
    logical_path: &str,
    artifact_kind: &str,
    account_id: &str,
    current_file_object_id: Option<&str>,
    current_tree_id: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(existing_id) = sqlx::query_scalar::<_, String>(
        "SELECT artifact_id FROM artifacts WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = $4",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(logical_path)
    .fetch_optional(&mut **tx)
    .await?
    {
        sqlx::query(
            r#"
            UPDATE artifacts
            SET artifact_kind = $1,
                state = 'active',
                current_file_object_id = $2,
                current_tree_id = $3,
                updated_at = now()
            WHERE artifact_id = $4
            "#,
        )
        .bind(artifact_kind)
        .bind(current_file_object_id)
        .bind(current_tree_id)
        .bind(&existing_id)
        .execute(&mut **tx)
        .await?;
        return Ok(existing_id);
    }

    let artifact_id = artifact_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| prefixed_id("artifact"));
    sqlx::query(
        r#"
        INSERT INTO artifacts
            (artifact_id, workspace_id, space_id, layer_id, logical_path, artifact_kind,
             state, created_by_account_id, current_file_object_id, current_tree_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, 'active', $7, $8, $9)
        "#,
    )
    .bind(&artifact_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(logical_path)
    .bind(artifact_kind)
    .bind(account_id)
    .bind(current_file_object_id)
    .bind(current_tree_id)
    .execute(&mut **tx)
    .await?;
    Ok(artifact_id)
}

async fn insert_timeline_event_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: Option<&str>,
    layer_id: Option<&str>,
    event_kind: &str,
    title: &str,
    body: Value,
) -> Result<String, ApiError> {
    let event_id = prefixed_id("event");
    sqlx::query(
        r#"
        INSERT INTO timeline_events
            (event_id, workspace_id, space_id, layer_id, event_kind, title, body_json)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(&event_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(event_kind)
    .bind(title)
    .bind(body)
    .execute(&mut **tx)
    .await?;
    Ok(event_id)
}

async fn rebuild_layer_tree_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
) -> Result<Option<String>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT artifact_id, logical_path, current_file_object_id
        FROM artifacts
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = $3
          AND state = 'active'
          AND current_file_object_id IS NOT NULL
        ORDER BY logical_path ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let mut manifest = String::new();
    for row in &rows {
        manifest.push_str(row.get::<String, _>("logical_path").as_str());
        manifest.push('\0');
        manifest.push_str(row.get::<String, _>("current_file_object_id").as_str());
        manifest.push('\n');
    }
    let digest = blake3_digest_for_bytes(manifest.as_bytes());
    let tree_id = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO tree_objects
            (tree_id, workspace_id, space_id, digest, entry_count, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (workspace_id, space_id, digest) DO UPDATE SET
            entry_count = EXCLUDED.entry_count
        RETURNING tree_id
        "#,
    )
    .bind(prefixed_id("tree"))
    .bind(workspace_id)
    .bind(space_id)
    .bind(&digest)
    .bind(rows.len() as i32)
    .bind(account_id)
    .fetch_one(&mut **tx)
    .await?;
    for row in rows {
        sqlx::query(
            r#"
            INSERT INTO tree_entries
                (tree_id, logical_path, entry_kind, file_object_id, artifact_id)
            VALUES
                ($1, $2, 'file', $3, $4)
            ON CONFLICT (tree_id, logical_path) DO UPDATE SET
                file_object_id = EXCLUDED.file_object_id,
                artifact_id = EXCLUDED.artifact_id
            "#,
        )
        .bind(&tree_id)
        .bind(row.get::<String, _>("logical_path"))
        .bind(row.get::<String, _>("current_file_object_id"))
        .bind(row.get::<String, _>("artifact_id"))
        .execute(&mut **tx)
        .await?;
    }
    Ok(Some(tree_id))
}

async fn ensure_tree_in_space_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    tree_id: &str,
) -> Result<(), ApiError> {
    let tree_id = validate_object_digest(tree_id)?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM tree_objects
            WHERE workspace_id = $1 AND space_id = $2 AND tree_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(&tree_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::bad_request("rootTreeId is not available"))
    }
}

async fn insert_layer_step_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    step: Option<&SyncStepBody>,
    requested_base_tree_id: Option<&str>,
    root_tree_id: Option<&str>,
    changed_paths: &[String],
    source_client_id: Option<&str>,
    sync_batch_id: Option<&str>,
    account_id: &str,
) -> Result<String, ApiError> {
    let step_id = step
        .and_then(|step| cleaned_optional_text(step.step_id.as_deref()))
        .unwrap_or_else(|| prefixed_id("step"));
    let parent_step_id =
        match step.and_then(|step| cleaned_optional_text(step.parent_step_id.as_deref())) {
            Some(candidate) => {
                let exists: bool = sqlx::query_scalar(
                    r#"
                SELECT EXISTS(
                    SELECT 1 FROM layer_steps
                    WHERE workspace_id = $1
                      AND space_id = $2
                      AND layer_id = $3
                      AND step_id = $4
                )
                "#,
                )
                .bind(workspace_id)
                .bind(space_id)
                .bind(layer_id)
                .bind(&candidate)
                .fetch_one(&mut **tx)
                .await?;
                exists.then_some(candidate)
            }
            None => None,
        };
    let base_layer_id =
        match step.and_then(|step| cleaned_optional_text(step.base_layer_id.as_deref())) {
            Some(candidate) => {
                let exists: bool = sqlx::query_scalar(
                    r#"
                SELECT EXISTS(
                    SELECT 1 FROM layers
                    WHERE workspace_id = $1
                      AND space_id = $2
                      AND layer_id = $3
                )
                "#,
                )
                .bind(workspace_id)
                .bind(space_id)
                .bind(&candidate)
                .fetch_one(&mut **tx)
                .await?;
                if exists {
                    Some(candidate)
                } else {
                    Some(layer_id.to_string())
                }
            }
            None => Some(layer_id.to_string()),
        };
    let step_root_tree_id =
        step.and_then(|step| cleaned_optional_text(step.root_tree_id.as_deref()));
    if let (Some(step_root_tree_id), Some(root_tree_id)) =
        (step_root_tree_id.as_deref(), root_tree_id)
    {
        if validate_object_digest(step_root_tree_id)? != validate_object_digest(root_tree_id)? {
            return Err(ApiError::bad_request(
                "step rootTreeId does not match published rootTreeId",
            ));
        }
    }
    let stored_root_tree_id = optional_existing_tree_id_in_tx(
        tx,
        workspace_id,
        space_id,
        root_tree_id.or(step_root_tree_id.as_deref()),
    )
    .await?;
    let step_base_tree_id = step
        .and_then(|step| cleaned_optional_text(step.base_tree_id.as_deref()))
        .or_else(|| cleaned_optional_text(requested_base_tree_id));
    let stored_base_tree_id =
        optional_existing_tree_id_in_tx(tx, workspace_id, space_id, step_base_tree_id.as_deref())
            .await?;
    let step_changed_paths = step
        .filter(|step| !step.changed_paths.is_empty())
        .map(|step| step.changed_paths.clone())
        .unwrap_or_else(|| changed_paths.to_vec());
    let captured_at_unix = step
        .and_then(|step| step.captured_at_unix)
        .filter(|value| *value > 0)
        .map(|value| value as f64);

    sqlx::query(
        r#"
        INSERT INTO layer_steps
            (step_id, workspace_id, space_id, layer_id, parent_step_id,
             base_layer_id, base_tree_id, root_tree_id, changed_paths,
             source_client_id, sync_batch_id, created_by_account_id, captured_at)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, $9,
             $10, $11, $12, COALESCE(to_timestamp($13::double precision), now()))
        ON CONFLICT (step_id) DO UPDATE SET
            parent_step_id = COALESCE(EXCLUDED.parent_step_id, layer_steps.parent_step_id),
            base_layer_id = EXCLUDED.base_layer_id,
            base_tree_id = EXCLUDED.base_tree_id,
            root_tree_id = EXCLUDED.root_tree_id,
            changed_paths = EXCLUDED.changed_paths,
            source_client_id = EXCLUDED.source_client_id,
            sync_batch_id = EXCLUDED.sync_batch_id
        "#,
    )
    .bind(&step_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(parent_step_id.as_deref())
    .bind(base_layer_id.as_deref())
    .bind(stored_base_tree_id.as_deref())
    .bind(stored_root_tree_id.as_deref())
    .bind(&step_changed_paths)
    .bind(source_client_id)
    .bind(sync_batch_id)
    .bind(account_id)
    .bind(captured_at_unix)
    .execute(&mut **tx)
    .await?;

    Ok(step_id)
}

async fn optional_existing_tree_id_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    tree_id: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let Some(tree_id) = tree_id else {
        return Ok(None);
    };
    let tree_id = validate_object_digest(tree_id)?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM tree_objects
            WHERE workspace_id = $1 AND space_id = $2 AND tree_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(&tree_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.then_some(tree_id))
}

fn cleaned_optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn advance_layer_head_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    root_tree_id: Option<&str>,
    policy_epoch: i64,
    server_cursor: Option<&str>,
    account_id: &str,
) -> Result<String, ApiError> {
    let layer_state_id = prefixed_id("layer_state");
    sqlx::query(
        r#"
        INSERT INTO layer_states
            (layer_state_id, workspace_id, space_id, layer_id, root_tree_id, policy_epoch, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(&layer_state_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(root_tree_id)
    .bind(policy_epoch)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO layer_heads
            (workspace_id, space_id, layer_id, layer_state_id, root_tree_id,
             policy_epoch, server_cursor, updated_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (workspace_id, space_id, layer_id) DO UPDATE SET
            layer_state_id = EXCLUDED.layer_state_id,
            root_tree_id = EXCLUDED.root_tree_id,
            policy_epoch = EXCLUDED.policy_epoch,
            server_cursor = EXCLUDED.server_cursor,
            updated_by_account_id = EXCLUDED.updated_by_account_id,
            updated_at = now()
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(&layer_state_id)
    .bind(root_tree_id)
    .bind(policy_epoch)
    .bind(server_cursor)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(layer_state_id)
}

async fn insert_sync_batch_change_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sync_batch_id: &str,
    change_index: i32,
    change_kind: &str,
    artifact_id: Option<&str>,
    logical_path: Option<&str>,
    file_object_id: Option<&str>,
    tree_id: Option<&str>,
    body: Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO sync_batch_changes
            (sync_batch_change_id, sync_batch_id, change_index, change_kind,
             artifact_id, logical_path, file_object_id, tree_id, body_json)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(prefixed_id("sync_batch_change"))
    .bind(sync_batch_id)
    .bind(change_index)
    .bind(change_kind)
    .bind(artifact_id)
    .bind(logical_path)
    .bind(file_object_id)
    .bind(tree_id)
    .bind(body)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn hash_chunk_manifest(chunks: &[ChunkDescriptor]) -> String {
    let mut manifest = String::new();
    for chunk in chunks {
        manifest.push_str(&chunk.chunk_id);
        manifest.push('\0');
        manifest.push_str(&chunk.digest);
        manifest.push('\0');
        manifest.push_str(&chunk.size_bytes.to_string());
        manifest.push('\n');
    }
    blake3_digest_for_bytes(manifest.as_bytes())
}

async fn write_timeline_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: Option<&str>,
    layer_id: Option<&str>,
    event_kind: &str,
    title: &str,
    body: Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO timeline_events
            (event_id, workspace_id, space_id, layer_id, event_kind, title, body_json)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(prefixed_id("event"))
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(event_kind)
    .bind(title)
    .bind(body)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn device_values(pool: &PgPool, account_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT device_id, display_name, status,
               to_char(coalesce(last_seen_at, created_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS last_seen_at
        FROM desktop_devices
        WHERE account_id = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("device_id"),
                "accountId": account_id,
                "name": row.get::<String, _>("display_name"),
                "kind": "desktop",
                "status": row.get::<String, _>("status"),
                "lastSeenAt": row.get::<String, _>("last_seen_at")
            })
        })
        .collect())
}

async fn audit_event_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT audit_event_id, actor_account_id, action, target_kind, target_id,
               to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM audit_events
        WHERE workspace_id = $1
        ORDER BY created_at DESC
        LIMIT 50
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            let action = row.get::<String, _>("action");
            let target_kind = row.get::<String, _>("target_kind");
            let target_id = row.try_get::<String, _>("target_id").unwrap_or_default();
            json!({
                "id": row.get::<String, _>("audit_event_id"),
                "workspaceId": workspace_id,
                "actorAccountId": row.try_get::<String, _>("actor_account_id").unwrap_or_default(),
                "action": action,
                "target": if target_id.is_empty() { target_kind.clone() } else { format!("{target_kind}:{target_id}") },
                "summary": format!("{action} on {target_kind}"),
                "at": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn space_summaries_for_workspaces(
    pool: &PgPool,
    workspace_ids: &[String],
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for workspace_id in workspace_ids {
        for value in space_values(pool, workspace_id).await? {
            values.push(json!({
                "id": value["id"],
                "workspaceId": value["workspaceId"],
                "name": value["name"],
                "currentLayerId": value["currentLayerId"]
            }));
        }
    }
    Ok(values)
}

async fn layer_summaries_for_workspaces(
    pool: &PgPool,
    workspace_ids: &[String],
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for workspace_id in workspace_ids {
        let layers = layer_values(pool, workspace_id).await?;
        for layer in layers {
            let space_id = layer
                .get("spaceId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            values.push(json!({
                "id": layer["id"],
                "workspaceId": workspace_id,
                "spaceId": space_id,
                "name": layer["name"],
                "kind": layer["kind"],
                "parentLayerId": layer.get("parentId").cloned().unwrap_or(Value::Null),
                "access": "open"
            }));
        }
    }
    Ok(values)
}

async fn layer_access_policy_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Value, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT policy_id, policy_epoch,
               to_char(generated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS generated_at,
               coalesce(signature_key_id, 'server_key_local') AS signature_key_id,
               coalesce(signature_value, 'unsigned-dev') AS signature_value
        FROM layer_access_policies
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("layer access policy not found"))?;
    let policy_id = row.get::<String, _>("policy_id");
    let rule_rows = sqlx::query(
        r#"
        SELECT rule_id, path, artifact_id, mode, visibility,
               read_account_ids, read_team_ids, write_account_ids, write_team_ids, admin_account_ids, admin_team_ids
        FROM layer_access_policy_rules
        WHERE policy_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(&policy_id)
    .fetch_all(pool)
    .await?;
    let rules = rule_rows
        .iter()
        .map(|rule| {
            json!({
                "id": rule.get::<String, _>("rule_id"),
                "path": rule.get::<String, _>("path"),
                "artifact_id": rule.try_get::<String, _>("artifact_id").ok(),
                "mode": rule.get::<String, _>("mode"),
                "visibility": rule.get::<String, _>("visibility"),
                "permissions": {
                    "read": {
                        "accounts": rule.try_get::<Vec<String>, _>("read_account_ids").unwrap_or_default(),
                        "teams": rule.try_get::<Vec<String>, _>("read_team_ids").unwrap_or_default()
                    },
                    "write": {
                        "accounts": rule.try_get::<Vec<String>, _>("write_account_ids").unwrap_or_default(),
                        "teams": rule.try_get::<Vec<String>, _>("write_team_ids").unwrap_or_default()
                    },
                    "admin": {
                        "accounts": rule.try_get::<Vec<String>, _>("admin_account_ids").unwrap_or_default(),
                        "teams": rule.try_get::<Vec<String>, _>("admin_team_ids").unwrap_or_default()
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "schema": "layrs.layer_access.v1",
        "workspace_id": workspace_id,
        "space_id": space_id,
        "layer_id": layer_id,
        "policy_epoch": row.get::<i64, _>("policy_epoch"),
        "generated_at": row.get::<String, _>("generated_at"),
        "rules": rules,
        "signature": {
            "key_id": row.get::<String, _>("signature_key_id"),
            "value": row.get::<String, _>("signature_value")
        }
    }))
}

async fn load_user_by_email(pool: &PgPool, email: &str) -> Result<Option<UserPrincipal>, ApiError> {
    let row = sqlx::query(
        "SELECT account_id, email, display_name FROM accounts WHERE email = $1 AND status = 'active'",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_user(&row)))
}

async fn create_workspace_owner_only(
    pool: &PgPool,
    account_id: &str,
    workspace_id: &str,
    name: &str,
    slug: &str,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO workspaces (workspace_id, slug, name, created_by_account_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(slug)
    .bind(name)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role) VALUES ($1, $2, $3, 'owner')",
    )
    .bind(prefixed_id("membership"))
    .bind(workspace_id)
    .bind(account_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_empty_layer_policy(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: Option<&str>,
) -> Result<String, ApiError> {
    let policy_id = prefixed_id("access_policy");
    sqlx::query(
        r#"
        INSERT INTO layer_access_policies
            (policy_id, workspace_id, space_id, layer_id, registry_path, policy_epoch, updated_by_account_id, signature_key_id, signature_value)
        VALUES
            ($1, $2, $3, $4, $5, 1, $6, 'server_key_local', 'unsigned-dev')
        "#,
    )
    .bind(&policy_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(format!(".layrs/layers/{layer_id}/access.json"))
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(policy_id)
}

async fn inherit_layer_access_rules_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    parent_layer_id: &str,
    child_policy_id: &str,
) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT r.path, r.artifact_id, r.mode, r.visibility,
               r.read_account_ids, r.read_team_ids, r.write_account_ids, r.write_team_ids,
               r.admin_account_ids, r.admin_team_ids
        FROM layer_access_policy_rules r
        JOIN layer_access_policies p ON p.policy_id = r.policy_id
        WHERE p.workspace_id = $1 AND p.space_id = $2 AND p.layer_id = $3
        ORDER BY r.created_at ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(parent_layer_id)
    .fetch_all(&mut **tx)
    .await?;

    for row in rows {
        sqlx::query(
            r#"
            INSERT INTO layer_access_policy_rules
                (rule_id, policy_id, path, artifact_id, mode, visibility,
                 read_account_ids, read_team_ids, write_account_ids, write_team_ids, admin_account_ids, admin_team_ids)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(prefixed_id("access_rule"))
        .bind(child_policy_id)
        .bind(row.get::<String, _>("path"))
        .bind(row.try_get::<String, _>("artifact_id").ok())
        .bind(row.get::<String, _>("mode"))
        .bind(row.get::<String, _>("visibility"))
        .bind(row.try_get::<Vec<String>, _>("read_account_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("read_team_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("write_account_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("write_team_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("admin_account_ids").unwrap_or_default())
        .bind(row.try_get::<Vec<String>, _>("admin_team_ids").unwrap_or_default())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn upsert_layer_policy(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: Option<&str>,
) -> Result<String, ApiError> {
    let row = sqlx::query(
        r#"
        INSERT INTO layer_access_policies
            (policy_id, workspace_id, space_id, layer_id, registry_path, policy_epoch, updated_by_account_id, signature_key_id, signature_value)
        VALUES
            ($1, $2, $3, $4, $5, 1, $6, 'server_key_local', 'unsigned-dev')
        ON CONFLICT (layer_id) DO UPDATE
        SET policy_epoch = layer_access_policies.policy_epoch + 1,
            updated_by_account_id = excluded.updated_by_account_id,
            updated_at = now()
        RETURNING policy_id
        "#,
    )
    .bind(prefixed_id("access_policy"))
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(format!(".layrs/layers/{layer_id}/access.json"))
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    Ok(row.get("policy_id"))
}

async fn policy_id_for_layer(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<String, ApiError> {
    sqlx::query_scalar(
        "SELECT policy_id FROM layer_access_policies WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("layer access policy not found"))
}

async fn insert_layer_access_rule(
    pool: &PgPool,
    policy_id: &str,
    rule_id: &str,
    rule: &LayerAccessRuleBody,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    insert_layer_access_rule_in_tx(&mut tx, policy_id, rule_id, rule).await?;
    bump_policy_epoch_in_tx(&mut tx, policy_id, account_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn update_layer_access_rule_row(
    pool: &PgPool,
    policy_id: &str,
    rule_id: &str,
    rule: &LayerAccessRuleBody,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        r#"
        UPDATE layer_access_policy_rules
        SET path = $1,
            artifact_id = $2,
            mode = $3,
            visibility = $4,
            read_account_ids = $5,
            read_team_ids = $6,
            write_account_ids = $7,
            write_team_ids = $8,
            admin_account_ids = $9,
            admin_team_ids = $10
        WHERE policy_id = $11 AND rule_id = $12
        "#,
    )
    .bind(&rule.path)
    .bind(rule.artifact_id.as_deref())
    .bind(&rule.mode)
    .bind(&rule.visibility)
    .bind(&rule.permissions.read.accounts)
    .bind(&rule.permissions.read.teams)
    .bind(&rule.permissions.write.accounts)
    .bind(&rule.permissions.write.teams)
    .bind(&rule.permissions.admin.accounts)
    .bind(&rule.permissions.admin.teams)
    .bind(policy_id)
    .bind(rule_id)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("access rule not found"));
    }
    bump_policy_epoch_in_tx(&mut tx, policy_id, account_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn delete_layer_access_rule_row(
    pool: &PgPool,
    policy_id: &str,
    rule_id: &str,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let result =
        sqlx::query("DELETE FROM layer_access_policy_rules WHERE policy_id = $1 AND rule_id = $2")
            .bind(policy_id)
            .bind(rule_id)
            .execute(&mut *tx)
            .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("access rule not found"));
    }
    bump_policy_epoch_in_tx(&mut tx, policy_id, account_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_layer_access_rule_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    policy_id: &str,
    rule_id: &str,
    rule: &LayerAccessRuleBody,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO layer_access_policy_rules
            (rule_id, policy_id, path, artifact_id, mode, visibility,
             read_account_ids, read_team_ids, write_account_ids, write_team_ids, admin_account_ids, admin_team_ids)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(rule_id)
    .bind(policy_id)
    .bind(&rule.path)
    .bind(rule.artifact_id.as_deref())
    .bind(&rule.mode)
    .bind(&rule.visibility)
    .bind(&rule.permissions.read.accounts)
    .bind(&rule.permissions.read.teams)
    .bind(&rule.permissions.write.accounts)
    .bind(&rule.permissions.write.teams)
    .bind(&rule.permissions.admin.accounts)
    .bind(&rule.permissions.admin.teams)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn bump_policy_epoch_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    policy_id: &str,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE layer_access_policies
        SET policy_epoch = policy_epoch + 1,
            updated_by_account_id = $1,
            updated_at = now()
        WHERE policy_id = $2
        "#,
    )
    .bind(account_id)
    .bind(policy_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn replace_layer_policy_rules(
    pool: &PgPool,
    workspace_id: &str,
    policy_id: &str,
    body: &LayerAccessPolicyBody,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;

    if let Some(signature) = &body.signature {
        sqlx::query(
            "UPDATE layer_access_policies SET signature_key_id = $1, signature_value = $2 WHERE policy_id = $3",
        )
        .bind(&signature.key_id)
        .bind(&signature.value)
        .bind(policy_id)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("DELETE FROM layer_access_policy_rules WHERE policy_id = $1")
        .bind(policy_id)
        .execute(&mut *tx)
        .await?;

    for rule in &body.rules {
        validate_layer_access_rule(pool, workspace_id, rule).await?;
        let rule_id = rule
            .id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| prefixed_id("access_rule"));
        sqlx::query(
            r#"
            INSERT INTO layer_access_policy_rules
                (rule_id, policy_id, path, artifact_id, mode, visibility,
                 read_account_ids, read_team_ids, write_account_ids, write_team_ids, admin_account_ids, admin_team_ids)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            "#,
        )
        .bind(&rule_id)
        .bind(policy_id)
        .bind(&rule.path)
        .bind(rule.artifact_id.as_deref())
        .bind(&rule.mode)
        .bind(&rule.visibility)
        .bind(&rule.permissions.read.accounts)
        .bind(&rule.permissions.read.teams)
        .bind(&rule.permissions.write.accounts)
        .bind(&rule.permissions.write.teams)
        .bind(&rule.permissions.admin.accounts)
        .bind(&rule.permissions.admin.teams)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

async fn write_audit(
    pool: &PgPool,
    workspace_id: Option<&str>,
    actor_account_id: Option<&str>,
    action: &str,
    target_kind: &str,
    target_id: Option<&str>,
    metadata: Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events
            (audit_event_id, workspace_id, actor_account_id, action, target_kind, target_id, metadata_json)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(prefixed_id("audit"))
    .bind(workspace_id)
    .bind(actor_account_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(metadata)
    .execute(pool)
    .await?;
    Ok(())
}

fn studio_account_json(user: &UserPrincipal) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "name": user.display_name,
        "role": "owner",
        "avatarInitials": avatar_initials(&user.display_name),
        "createdAt": "2026-06-29T00:00:00Z"
    })
}

fn desktop_account_json(user: &UserPrincipal) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "displayName": user.display_name
    })
}

fn desktop_user_json(user: &UserPrincipal) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "displayName": user.display_name
    })
}

fn user_wire_json(user: &UserPrincipal) -> Value {
    json!({
        "id": user.id,
        "email": user.email,
        "display_name": user.display_name
    })
}

fn session_json(user: &UserPrincipal, active_workspace_id: Option<&str>) -> Value {
    json!({
        "id": "session-current",
        "accountId": user.id,
        "activeWorkspaceId": active_workspace_id,
        "expiresAt": "2026-07-06T00:00:00Z",
        "createdAt": "2026-06-29T00:00:00Z"
    })
}

fn route_json(route: &RouteDescriptor) -> Value {
    json!({
        "method": method_label(route.method),
        "path": route.path,
        "name": route.name,
        "auth": auth_label(route.auth)
    })
}

fn artifact_metadata_value(
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: &str,
    path: &str,
    artifact_kind: &str,
    updated_at: &str,
    file_object_id: Option<String>,
    tree_id: Option<String>,
    redacted: bool,
    reason: &str,
) -> Value {
    json!({
        "id": artifact_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "name": path.rsplit('/').next().unwrap_or(path),
        "type": artifact_type(artifact_kind),
        "summary": if redacted { "Restricted by Layer access policy" } else { "Persisted artifact" },
        "location": path,
        "updatedAt": updated_at,
        "fileObjectId": file_object_id,
        "treeId": tree_id,
        "sizeLabel": if redacted { "redacted" } else { "stored" },
        "proofIds": [],
        "access": {
            "mode": if redacted { "none" } else { "read" },
            "canOpen": !redacted,
            "isRedacted": redacted,
            "reason": if redacted { reason } else { "" }
        }
    })
}

fn latest_timeline_cursor(events: &[Value]) -> Option<String> {
    events
        .last()
        .and_then(|event| event.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn redact_timeline_body(mut body: Value) -> Value {
    if let Some(object) = body.as_object_mut() {
        if object.remove("content").is_some() {
            object.insert("contentIncluded".to_string(), Value::Bool(false));
            object.insert(
                "contentEndpoint".to_string(),
                Value::String("layer_artifact_content.get".to_string()),
            );
        }
    }
    body
}

#[derive(Debug)]
struct PreparedChunk {
    chunk_id: String,
    digest: Option<String>,
    size_bytes: Option<i64>,
    media_type: Option<String>,
}

fn prepared_chunk_from_item(item: PrepareChunkItem) -> Result<PreparedChunk, ApiError> {
    match item {
        PrepareChunkItem::Id(value) => {
            let chunk_id = validate_chunk_id(&value)?;
            Ok(PreparedChunk {
                digest: Some(chunk_id.clone()),
                chunk_id,
                size_bytes: None,
                media_type: None,
            })
        }
        PrepareChunkItem::Object {
            id,
            chunk_id,
            chunk_id_camel,
            sha256,
            size_bytes,
            size_bytes_camel,
            media_type,
            media_type_camel,
        } => {
            let raw_chunk_id = id
                .or(chunk_id)
                .or(chunk_id_camel)
                .or_else(|| sha256.as_ref().map(|value| value.to_string()))
                .ok_or_else(|| ApiError::bad_request("chunk id is required"))?;
            let chunk_id = validate_chunk_id(&raw_chunk_id)?;
            let digest = Some(validate_object_digest(
                sha256.as_deref().unwrap_or(&chunk_id),
            )?);
            Ok(PreparedChunk {
                chunk_id,
                digest,
                size_bytes: size_bytes.or(size_bytes_camel),
                media_type: media_type.or(media_type_camel),
            })
        }
    }
}

async fn object_chunk_exists(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    chunk_id: &str,
) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM object_chunks
            WHERE workspace_id = $1 AND space_id = $2 AND chunk_id = $3 AND state = 'available'
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(chunk_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)
}

fn validate_chunk_id(value: &str) -> Result<String, ApiError> {
    validate_object_digest(value)
}

fn validate_object_digest(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_lowercase();
    let Some(hex) = value.strip_prefix("blake3:") else {
        return Err(ApiError::bad_request(
            "digest must use blake3:<64 hex> format",
        ));
    };
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "digest must use blake3:<64 hex> format",
        ));
    }
    Ok(value)
}

fn blake3_digest_for_bytes(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn require_layer_id(
    layer_id: Option<String>,
    layer_id_camel: Option<String>,
) -> Result<String, ApiError> {
    layer_id
        .or(layer_id_camel)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("layer_id is required"))
}

fn normalize_deleted_paths(paths: &[String]) -> Result<Vec<String>, ApiError> {
    let mut unique = Vec::new();
    for path in paths {
        let path = validate_publish_path(path.trim())?;
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }
    Ok(unique)
}

fn artifact_requests_deletion(body: &PublishArtifactBody) -> bool {
    body.deleted.unwrap_or(false)
        || body
            .state
            .as_deref()
            .map(|value| matches!(value.trim(), "deleted" | "delete" | "tombstone"))
            .unwrap_or(false)
        || body
            .operation
            .as_deref()
            .or(body.action.as_deref())
            .map(|value| matches!(value.trim(), "delete" | "deleted" | "remove" | "tombstone"))
            .unwrap_or(false)
}

fn required_artifact_path(body: &PublishArtifactBody) -> Result<String, ApiError> {
    let path = body
        .path
        .as_deref()
        .or(body.logical_path.as_deref())
        .or(body.logical_path_camel.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("artifact path is required"))?;
    validate_publish_path(path)
}

fn validate_publish_path(path: &str) -> Result<String, ApiError> {
    validate_relative_path(path)?;
    if path == ".layrs" || path.starts_with(".layrs/") {
        return Err(ApiError::forbidden(
            "reserved .layrs paths cannot be published",
        ));
    }
    Ok(path.to_string())
}

fn normalize_artifact_kind(value: Option<&str>) -> Result<&'static str, ApiError> {
    match value.unwrap_or("file").trim() {
        "" | "file" | "code" | "layrs.code" => Ok("file"),
        "text" | "note" | "layrs.text" => Ok("note"),
        "image" | "layrs.image" => Ok("image"),
        "texture" => Ok("texture"),
        "raw" | "binary" | "layrs.raw" => Ok("binary"),
        "proof" => Ok("proof"),
        _ => Err(ApiError::bad_request("unsupported artifact kind")),
    }
}

fn hashset(values: Vec<String>) -> HashSet<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn intersects(left: &HashSet<String>, right: &HashSet<String>) -> bool {
    left.iter().any(|value| right.contains(value))
}

fn path_matches_rule(path: &str, rule_path: &str) -> bool {
    let rule_path = rule_path.trim();
    if rule_path == "*" || rule_path == "**" {
        return true;
    }
    if let Some(prefix) = rule_path.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    path == rule_path || path.starts_with(&format!("{rule_path}/"))
}

fn method_label(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    }
}

fn auth_label(auth: AuthRequirement) -> &'static str {
    match auth {
        AuthRequirement::Public => "public",
        AuthRequirement::Session => "session",
        AuthRequirement::Device => "device",
        AuthRequirement::Principal => "principal",
        AuthRequirement::WorkspaceRead => "workspace_read",
        AuthRequirement::WorkspaceWrite => "workspace_write",
        AuthRequirement::WorkspaceAdmin => "workspace_admin",
    }
}

fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| ApiError::bad_request(format!("password could not be hashed: {error}")))
}

fn verify_password(password: &str, hash: &str) -> Result<(), ApiError> {
    let parsed_hash = PasswordHash::new(hash)
        .map_err(|_| ApiError::unauthorized("email or password is incorrect"))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .map_err(|_| ApiError::unauthorized("email or password is incorrect"))
}

fn digest_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == SESSION_COOKIE_NAME).then(|| value.to_string())
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .or_else(|| {
            headers
                .get(AUTHORIZATION)?
                .to_str()
                .ok()?
                .strip_prefix("bearer ")
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn build_session_cookie(token: &str, config: &WebServerConfig) -> String {
    let secure = if config.cookie_secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_MAX_AGE_SECONDS}{secure}"
    )
}

fn expired_session_cookie(config: &WebServerConfig) -> String {
    let secure = if config.cookie_secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}")
}

fn normalize_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_ascii_lowercase();
    if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(ApiError::bad_request("email must be a valid address"));
    }
    Ok(email)
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.len() < 8 {
        return Err(ApiError::bad_request(
            "password must be at least 8 characters",
        ));
    }
    Ok(())
}

fn required_body_text(field: &'static str, value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::bad_request(format!("{field} is required")));
    }
    Ok(value.to_string())
}

fn local_layer_import_id(layer: &LocalLayerImportBody, fallback: &str) -> String {
    layer
        .local_layer_id
        .as_deref()
        .or(layer.local_layer_id_camel.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .trim()
        .to_string()
}

fn normalize_user_code(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_uppercase();
    if value.is_empty() {
        return Err(ApiError::bad_request("user_code is required"));
    }
    Ok(value)
}

fn workspace_role(value: Option<&str>) -> Result<&'static str, ApiError> {
    match value.unwrap_or("member").trim() {
        "owner" => Err(ApiError::bad_request(
            "owner cannot be assigned through invitations",
        )),
        "admin" => Ok("admin"),
        "member" | "" => Ok("member"),
        "viewer" => Ok("viewer"),
        _ => Err(ApiError::bad_request("unsupported workspace role")),
    }
}

fn team_member_role(value: Option<&str>) -> Result<&'static str, ApiError> {
    match value.unwrap_or("member").trim() {
        "maintainer" => Ok("maintainer"),
        "member" | "" => Ok("member"),
        _ => Err(ApiError::bad_request("unsupported team member role")),
    }
}

fn invitation_team_ids(mut snake: Vec<String>, camel: Vec<String>) -> Vec<String> {
    snake.extend(camel);
    unique_strings(&snake)
}

fn unique_strings(values: &[String]) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() && !unique.iter().any(|existing| existing == value) {
            unique.push(value.to_string());
        }
    }
    unique
}

async fn validate_layer_access_rule(
    pool: &PgPool,
    workspace_id: &str,
    rule: &LayerAccessRuleBody,
) -> Result<(), ApiError> {
    validate_relative_path(&rule.path)?;
    match rule.mode.as_str() {
        "restricted" => {
            if rule.permissions.read.accounts.is_empty()
                && rule.permissions.read.teams.is_empty()
                && rule.permissions.write.accounts.is_empty()
                && rule.permissions.write.teams.is_empty()
                && rule.permissions.admin.accounts.is_empty()
                && rule.permissions.admin.teams.is_empty()
            {
                return Err(ApiError::bad_request(
                    "restricted rule must define read, write or admin principals",
                ));
            }
        }
        "reserved_redacted" => {}
        _ => return Err(ApiError::bad_request("unsupported access rule mode")),
    }
    if !matches!(rule.visibility.as_str(), "full" | "stub") {
        return Err(ApiError::bad_request("unsupported access rule visibility"));
    }
    let mut account_ids = Vec::new();
    account_ids.extend(rule.permissions.read.accounts.iter().cloned());
    account_ids.extend(rule.permissions.write.accounts.iter().cloned());
    account_ids.extend(rule.permissions.admin.accounts.iter().cloned());
    ensure_accounts_in_workspace(pool, workspace_id, &account_ids).await?;
    let mut team_ids = Vec::new();
    team_ids.extend(rule.permissions.read.teams.iter().cloned());
    team_ids.extend(rule.permissions.write.teams.iter().cloned());
    team_ids.extend(rule.permissions.admin.teams.iter().cloned());
    ensure_teams_in_workspace(pool, workspace_id, &team_ids).await?;
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), ApiError> {
    if path.trim().is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains("//")
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ApiError::bad_request(
            "path must be a normalized relative path",
        ));
    }
    Ok(())
}

fn default_stub_visibility() -> String {
    "stub".to_string()
}

fn token(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for character in value.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash && !slug.is_empty() {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "workspace".to_string()
    } else {
        slug
    }
}

fn avatar_initials(value: &str) -> String {
    let mut initials = value
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .filter(|character| character.is_ascii_alphabetic())
        .take(2)
        .collect::<String>()
        .to_ascii_uppercase();
    if initials.is_empty() {
        initials = "LA".to_string();
    }
    initials
}

fn artifact_type(value: &str) -> &'static str {
    match value {
        "image" | "texture" => "image",
        "note" => "note",
        "proof" => "proof",
        _ => "file",
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn device_verification_html(
    user_code: &str,
    message: &str,
    is_error: bool,
    account: Option<&UserPrincipal>,
    can_approve: bool,
) -> String {
    let tone = if is_error { "error" } else { "notice" };
    let account_html = account
        .map(|user| {
            format!(
                r#"<p class="account">Studio account: <strong>{}</strong></p>"#,
                escape_html(&user.email)
            )
        })
        .unwrap_or_else(|| {
            r#"<p class="account">No Studio session is active in this browser.</p>"#.to_string()
        });
    let form_html = if can_approve {
        format!(
            r#"<form method="post" action="/v1/desktop/device/approve">
        <label for="user_code">Device code</label>
        <input id="user_code" name="user_code" value="{user_code}" autocomplete="one-time-code" required autofocus />
        <button type="submit">Approve device</button>
      </form>"#,
            user_code = escape_html(user_code)
        )
    } else {
        format!(
            r#"<label for="user_code">Device code</label>
      <input id="user_code" name="user_code" value="{user_code}" autocomplete="one-time-code" readonly />"#,
            user_code = escape_html(user_code)
        )
    };
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Layrs Desktop Device Verification</title>
    <style>
      :root {{
        color-scheme: light dark;
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        line-height: 1.5;
      }}
      body {{
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        background: Canvas;
        color: CanvasText;
      }}
      main {{
        width: min(460px, calc(100vw - 40px));
      }}
      h1 {{
        margin: 0 0 10px;
        font-size: 28px;
      }}
      p {{
        margin: 0 0 18px;
      }}
      .notice, .error {{
        padding: 12px 14px;
        border: 1px solid color-mix(in srgb, CanvasText 18%, Canvas 82%);
        border-radius: 8px;
        margin-bottom: 18px;
      }}
      .error {{
        border-color: #b42318;
        color: #b42318;
      }}
      .account {{
        padding: 10px 0;
        font-size: 14px;
      }}
      label {{
        display: block;
        font-weight: 700;
        margin-bottom: 8px;
      }}
      input {{
        box-sizing: border-box;
        width: 100%;
        min-height: 44px;
        padding: 10px 12px;
        border: 1px solid color-mix(in srgb, CanvasText 28%, Canvas 72%);
        border-radius: 6px;
        font: inherit;
        text-transform: uppercase;
      }}
      button {{
        margin-top: 14px;
        min-height: 44px;
        padding: 0 16px;
        border: 0;
        border-radius: 6px;
        background: CanvasText;
        color: Canvas;
        font: inherit;
        font-weight: 700;
        cursor: pointer;
      }}
    </style>
  </head>
  <body>
    <main>
      <h1>Layrs Desktop</h1>
      <p class="{tone}">{message}</p>
      {account_html}
      {form_html}
    </main>
  </body>
</html>"#,
        tone = tone,
        message = escape_html(message),
        account_html = account_html,
        form_html = form_html
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_does_not_reveal_secret() {
        let digest = digest_secret("session_secret");
        assert_ne!(digest, "session_secret");
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn slugify_keeps_workspace_slugs_stable() {
        assert_eq!(slugify("Game Prototype!"), "game-prototype");
    }

    #[test]
    fn text_window_respects_start_limit_and_has_more() {
        let window = text_window_from_str("one\ntwo\nthree\nfour", 1, 2);

        assert_eq!(
            text_segments(&window),
            vec!["two".to_string(), "three".to_string()]
        );
        assert_eq!(window.total_lines, 4);
        assert!(window_has_more(1, window.lines.len(), window.total_lines));
    }

    #[test]
    fn text_window_handles_crlf_without_trailing_empty_line() {
        let window = text_window_from_str("one\r\ntwo\r\n", 0, 10);

        assert_eq!(
            text_segments(&window),
            vec!["one".to_string(), "two".to_string()]
        );
        assert_eq!(window.total_lines, 2);
        assert!(!window_has_more(0, window.lines.len(), window.total_lines));
    }

    #[test]
    fn text_window_segments_long_lines_by_columns() {
        let mut builder = TextLineWindowBuilder::new(WindowRequest {
            start: 0,
            limit: 2,
            column_start: 3,
            column_limit: Some(4),
        });
        builder.push_text("0123456789\nabc");
        let window = builder.finish();

        assert_eq!(window.total_lines, 2);
        assert_eq!(window.lines[0].text_segment, "3456");
        assert_eq!(window.lines[0].text_length, 10);
        assert_eq!(window.lines[0].column_start, 3);
        assert_eq!(window.lines[0].column_end, 7);
        assert!(window.lines[0].has_more_columns);
        assert_eq!(window.lines[1].text_segment, "");
        assert_eq!(window.lines[1].text_length, 3);
        assert_eq!(window.lines[1].column_start, 3);
        assert_eq!(window.lines[1].column_end, 3);
        assert!(!window.lines[1].has_more_columns);
    }

    fn text_segments(window: &TextLineWindow) -> Vec<String> {
        window
            .lines
            .iter()
            .map(|line| line.text_segment.clone())
            .collect()
    }

    #[test]
    fn desktop_user_json_uses_only_camel_case_display_name() {
        let user = UserPrincipal {
            id: "account_test".to_string(),
            email: "desktop@example.com".to_string(),
            display_name: "Layrs Desktop Dev".to_string(),
        };

        let value = desktop_user_json(&user);
        let object = value.as_object().expect("desktop user json is an object");

        assert_eq!(
            value.get("displayName").and_then(Value::as_str),
            Some("Layrs Desktop Dev")
        );
        assert!(!object.contains_key("display_name"));
    }

    #[test]
    fn web_user_json_uses_only_legacy_snake_case_display_name() {
        let user = UserPrincipal {
            id: "account_test".to_string(),
            email: "web@example.com".to_string(),
            display_name: "Layrs Web Dev".to_string(),
        };

        let value = user_wire_json(&user);
        let object = value.as_object().expect("web user json is an object");

        assert_eq!(
            value.get("display_name").and_then(Value::as_str),
            Some("Layrs Web Dev")
        );
        assert!(!object.contains_key("displayName"));
    }

    #[test]
    fn device_verification_without_session_has_no_approve_form() {
        let html = device_verification_html(
            "LAYRS-123456",
            "Sign in before approving.",
            true,
            None,
            false,
        );

        assert!(html.contains("No Studio session is active"));
        assert!(!html.contains("method=\"post\""));
    }

    #[test]
    fn device_verification_with_session_names_account() {
        let user = UserPrincipal {
            id: "account_test".to_string(),
            email: "player@example.com".to_string(),
            display_name: "Player".to_string(),
        };
        let html = device_verification_html(
            "LAYRS-123456",
            "Approve this device.",
            false,
            Some(&user),
            true,
        );

        assert!(html.contains("player@example.com"));
        assert!(html.contains("method=\"post\""));
    }

    #[tokio::test]
    async fn list_lenses_endpoint_returns_contract_manifests() {
        let Json(payload) = list_lenses().await;
        let manifests = payload
            .get("items")
            .and_then(Value::as_array)
            .expect("/v1/lenses returns an items array");
        assert!(
            payload.get("errors").and_then(Value::as_array).is_some(),
            "/v1/lenses returns non-fatal manifest errors"
        );

        assert_lens_manifest_contract(manifests);
    }

    fn assert_lens_manifest_contract(manifests: &[Value]) {
        let ids = manifests
            .iter()
            .filter_map(|lens| lens.get("id").and_then(Value::as_str).map(str::to_string))
            .collect::<Vec<_>>();

        assert!(
            ids.starts_with(&[
                "layrs.code".to_string(),
                "layrs.text".to_string(),
                "layrs.image".to_string(),
                "layrs.raw".to_string()
            ]),
            "built-in lenses should be first"
        );

        for manifest in manifests {
            let object = manifest
                .as_object()
                .expect("lens manifest is a JSON object");

            assert!(object.get("id").and_then(Value::as_str).is_some());
            assert!(object.get("name").and_then(Value::as_str).is_some());
            assert!(object.get("version").and_then(Value::as_str).is_some());
            assert!(!object.contains_key("displayName"));
            assert!(!object.contains_key("serverProvided"));
            assert!(
                object
                    .get("applies_to")
                    .and_then(Value::as_object)
                    .is_some()
            );
            assert!(
                object
                    .get("capabilities")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                object
                    .get("permissions")
                    .and_then(Value::as_object)
                    .is_some()
            );

            let analyzer = object
                .get("analyzer")
                .and_then(Value::as_object)
                .expect("lens manifest includes analyzer");
            assert!(
                analyzer
                    .get("supportedMediaTypes")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                analyzer
                    .get("fileExtensions")
                    .and_then(Value::as_array)
                    .is_some()
            );
            assert!(
                analyzer
                    .get("capabilities")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );

            let viewer = object
                .get("viewer")
                .and_then(Value::as_object)
                .expect("lens manifest includes viewer");
            assert!(viewer.get("viewerId").and_then(Value::as_str).is_some());
            assert_eq!(
                viewer.get("schemaVersion").and_then(Value::as_str),
                Some("layrs.viewer.v1")
            );
            assert!(viewer.get("component").and_then(Value::as_str).is_some());
            assert!(
                viewer
                    .get("previewKinds")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                viewer
                    .get("diffKinds")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                viewer
                    .get("reconcileStatuses")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                viewer
                    .get("inspectorFields")
                    .and_then(Value::as_array)
                    .is_some()
            );
        }
    }

    #[test]
    fn timeline_body_redacts_inline_content_from_sync_payloads() {
        let redacted = redact_timeline_body(json!({
            "artifactId": "artifact_1",
            "content": { "text": "secret" }
        }));

        assert!(redacted.get("content").is_none());
        assert_eq!(
            redacted.get("contentIncluded").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn access_path_rules_match_descendants() {
        assert!(path_matches_rule("src/main.rs", "src"));
        assert!(path_matches_rule("private/image.png", "private/**"));
        assert!(!path_matches_rule("srcs/main.rs", "src"));
    }

    #[tokio::test]
    async fn create_layer_accepts_desktop_bearer_principal() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };

        let Json(payload) = create_layer(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(CreateLayerBody {
                name: "Bearer Layer".to_string(),
                parent_id: Some(fixture.layer_id.clone()),
                parent_layer_id: None,
                summary: None,
            }),
        )
        .await
        .expect("desktop bearer can create a layer");

        assert!(payload.get("id").and_then(Value::as_str).is_some());
        assert_eq!(
            payload.get("parentId").and_then(Value::as_str),
            Some(fixture.layer_id.as_str())
        );
    }

    #[tokio::test]
    async fn delete_layer_accepts_desktop_bearer_principal() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };

        let Json(created) = create_layer(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(CreateLayerBody {
                name: "Temporary Layer".to_string(),
                parent_id: Some(fixture.layer_id.clone()),
                parent_layer_id: None,
                summary: None,
            }),
        )
        .await
        .expect("desktop bearer can create a layer");
        let layer_id = created
            .get("id")
            .and_then(Value::as_str)
            .expect("created layer id")
            .to_string();

        let Json(deleted) = delete_layer(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                layer_id.clone(),
            )),
            fixture.bearer_headers(),
        )
        .await
        .expect("desktop bearer can delete a non-parent layer");

        assert_eq!(
            deleted.get("id").and_then(Value::as_str),
            Some(layer_id.as_str())
        );
        assert_eq!(deleted.get("deleted").and_then(Value::as_bool), Some(true));
    }

    #[tokio::test]
    async fn delete_space_removes_layers_artifacts_and_v2_objects() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let bytes = Bytes::from_static(b"delete me\n");
        let chunk_id = blake3_digest_for_bytes(&bytes);
        let file_object_id = blake3_digest_for_bytes(&bytes);
        let root_tree_id = blake3_digest_for_bytes(b"delete-space-tree");

        let _ = put_space_chunk(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                chunk_id.clone(),
            )),
            fixture.bearer_headers(),
            bytes.clone(),
        )
        .await
        .expect("chunk upload succeeds");

        let _ = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(
                serde_json::from_value(json!({
                    "protocol": "layrs.sync.v2",
                    "layerId": fixture.layer_id,
                    "policyEpoch": 1,
                    "idempotencyKey": format!("delete_space_{}", Uuid::new_v4().simple()),
                    "sourceClientId": "test-client",
                    "rootTreeId": root_tree_id,
                    "changedPaths": ["delete.txt"],
                    "storeObjects": {
                        "chunks": [{
                            "chunkId": chunk_id,
                            "digest": chunk_id,
                            "size": bytes.len()
                        }],
                        "fileObjects": [{
                            "fileObjectId": file_object_id,
                            "digest": file_object_id,
                            "size": bytes.len(),
                            "mediaType": "text/plain",
                            "chunks": [{
                                "chunkId": chunk_id,
                                "digest": chunk_id,
                                "size": bytes.len(),
                                "byteOffset": 0
                            }]
                        }],
                        "treeObjects": [{
                            "treeId": root_tree_id,
                            "entries": [{
                                "path": "delete.txt",
                                "fileObjectId": file_object_id,
                                "size": bytes.len()
                            }]
                        }]
                    }
                }))
                .expect("publish body is valid"),
            ),
        )
        .await
        .expect("publish succeeds");

        let Json(deleted) = delete_space(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
        )
        .await
        .expect("space delete succeeds");

        assert_eq!(
            deleted.get("id").and_then(Value::as_str),
            Some(fixture.space_id.as_str())
        );
        assert_eq!(deleted.get("deleted").and_then(Value::as_bool), Some(true));
        let spaces: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM spaces WHERE space_id = $1")
                .bind(&fixture.space_id)
                .fetch_one(&fixture.pool)
                .await
                .expect("space count");
        let artifacts: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM artifacts WHERE space_id = $1")
                .bind(&fixture.space_id)
                .fetch_one(&fixture.pool)
                .await
                .expect("artifact count");
        let chunks: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM object_chunks WHERE space_id = $1")
                .bind(&fixture.space_id)
                .fetch_one(&fixture.pool)
                .await
                .expect("chunk count");
        assert_eq!(spaces, 0);
        assert_eq!(artifacts, 0);
        assert_eq!(chunks, 0);
    }

    #[tokio::test]
    async fn publish_then_receive_returns_authorized_content() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let bytes = Bytes::from_static(b"fn main() {}\n");
        let chunk_id = blake3_digest_for_bytes(&bytes);
        let file_object_id = blake3_digest_for_bytes(&bytes);
        let root_tree_id = blake3_digest_for_bytes(b"authorized-content-tree");
        let step_id = format!("step_{}", Uuid::new_v4().simple());

        let Json(upload_payload) = put_space_chunk(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                chunk_id.clone(),
            )),
            fixture.bearer_headers(),
            bytes.clone(),
        )
        .await
        .expect("chunk upload succeeds before metadata publish");
        assert_eq!(
            upload_payload.get("chunkId").and_then(Value::as_str),
            Some(chunk_id.as_str())
        );

        let Json(publish_payload) = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(
                serde_json::from_value(json!({
                    "protocol": "layrs.sync.v2",
                    "layerId": fixture.layer_id,
                    "policyEpoch": 1,
                    "idempotencyKey": format!("idem_{}", Uuid::new_v4().simple()),
                    "sourceClientId": "test-client",
                    "rootTreeId": root_tree_id,
                    "changedPaths": ["src/main.rs"],
                    "step": {
                        "stepId": step_id,
                        "layerId": fixture.layer_id,
                        "rootTreeId": root_tree_id,
                        "changedPaths": ["src/main.rs"],
                        "capturedAtUnix": 1782910398
                    },
                    "storeObjects": {
                        "chunks": [{
                            "chunkId": chunk_id,
                            "digest": chunk_id,
                            "size": bytes.len()
                        }],
                        "fileObjects": [{
                            "fileObjectId": file_object_id,
                            "size": bytes.len(),
                            "mediaType": "text/plain",
                            "chunks": [{
                                "chunkId": chunk_id,
                                "size": bytes.len()
                            }]
                        }],
                        "treeObjects": [{
                            "treeId": root_tree_id,
                            "entries": [{
                                "path": "src/main.rs",
                                "fileObjectId": file_object_id,
                                "size": bytes.len()
                            }]
                        }]
                    }
                }))
                .expect("chunked publish body deserializes"),
            ),
        )
        .await
        .expect("chunked publish succeeds");
        assert_eq!(
            publish_payload
                .get("published")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert!(
            publish_payload
                .get("serverCursor")
                .and_then(Value::as_str)
                .is_some()
        );
        assert_eq!(
            publish_payload
                .pointer("/step/stepId")
                .and_then(Value::as_str),
            Some(step_id.as_str())
        );
        let stored_step_count: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM layer_steps WHERE step_id = $1")
                .bind(&step_id)
                .fetch_one(&fixture.pool)
                .await
                .expect("step count");
        assert_eq!(stored_step_count, 1);

        let Json(receive_payload) = receive_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(SyncReceiveBody {
                layer_id: Some(fixture.layer_id.clone()),
                layer_id_camel: None,
                cursor: None,
                limit: Some(200),
            }),
        )
        .await
        .expect("receive succeeds");

        assert!(receive_payload.get("content").is_none());
        assert!(
            receive_payload
                .get("layers")
                .and_then(Value::as_array)
                .is_some_and(|values| !values.is_empty())
        );
        assert!(
            receive_payload
                .get("accessRegistries")
                .and_then(Value::as_array)
                .is_some_and(|values| !values.is_empty())
        );
        assert_eq!(
            receive_payload
                .get("steps")
                .and_then(Value::as_array)
                .and_then(|steps| steps.first())
                .and_then(|step| step.get("stepId"))
                .and_then(Value::as_str),
            Some(step_id.as_str())
        );
        assert_eq!(
            receive_payload
                .get("contents")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        let content_objects = receive_payload
            .get("contentObjects")
            .expect("receive includes chunked store manifest");
        assert_eq!(
            content_objects
                .get("chunks")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            content_objects
                .pointer("/chunks/0/chunkId")
                .and_then(Value::as_str),
            Some(chunk_id.as_str())
        );
        assert!(
            serde_json::to_string(content_objects)
                .expect("contentObjects serializes")
                .contains("downloadUrl")
        );
        assert!(
            !serde_json::to_string(&receive_payload)
                .expect("receive serializes")
                .contains("fn main")
        );
    }

    #[tokio::test]
    async fn inline_artifact_publish_is_rejected() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };

        let error = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(SyncPublishBody {
                layer_id: Some(fixture.layer_id.clone()),
                layer_id_camel: None,
                cursor: None,
                artifacts: vec![publish_artifact(
                    "src/main.rs",
                    "code",
                    "text/plain",
                    json!("fn main() {}\n"),
                )],
                artifact: None,
                deleted_paths: Vec::new(),
                deleted_paths_camel: Vec::new(),
                policy_epoch: None,
                policy_epoch_camel: None,
                idempotency_key: None,
                idempotency_key_camel: None,
                source_client_id: None,
                source_client_id_camel: None,
                root_tree_id: None,
                root_tree_id_camel: None,
                base_tree_id: None,
                base_tree_id_camel: None,
                protocol: Some("layrs.sync.v2".to_string()),
                changed_paths: Vec::new(),
                changed_paths_camel: Vec::new(),
                store_objects: None,
                store_objects_camel: None,
                step: None,
            }),
        )
        .await
        .expect_err("inline artifact content must not publish");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "invalid_request");
        assert!(error.message.contains("inline artifact content"));
    }

    #[test]
    fn canonical_store_objects_reject_inline_chunk_data() {
        let chunk_id = blake3_digest_for_bytes(b"inline chunk");
        let payload = json!({
            "chunks": [{
                "chunkId": chunk_id,
                "digest": chunk_id,
                "size": 12,
                "encoding": "base64",
                "data": "aW5saW5lIGNodW5r"
            }],
            "fileObjects": [],
            "treeObjects": []
        });

        let error = match serde_json::from_value::<PublishStoreObjectsBody>(payload) {
            Ok(_) => panic!("inline chunk data is not part of canonical storeObjects"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unknown field"));
    }

    #[tokio::test]
    async fn v2_chunk_publish_receive_and_content_round_trips() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let bytes = Bytes::from_static(b"MERKLE_V2_CONTENT");
        let chunk_id = blake3_digest_for_bytes(&bytes);
        let file_object_id = blake3_digest_for_bytes(&bytes);
        let root_tree_id = blake3_digest_for_bytes(b"test-root-tree");

        let Json(upload_payload) = put_space_chunk(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                chunk_id.clone(),
            )),
            fixture.bearer_headers(),
            bytes.clone(),
        )
        .await
        .expect("chunk upload succeeds before metadata publish");
        assert_eq!(
            upload_payload.get("chunkId").and_then(Value::as_str),
            Some(chunk_id.as_str())
        );

        let publish_body: SyncPublishBody = serde_json::from_value(json!({
            "protocol": "layrs.sync.v2",
            "layerId": fixture.layer_id,
            "policyEpoch": 1,
            "idempotencyKey": format!("idem_{}", Uuid::new_v4().simple()),
            "sourceClientId": "test-client",
            "rootTreeId": root_tree_id,
            "changedPaths": ["assets/hero.bin"],
            "storeObjects": {
                "chunks": [{
                    "chunkId": chunk_id,
                    "digest": chunk_id,
                    "size": bytes.len()
                }],
                "fileObjects": [{
                    "fileObjectId": file_object_id,
                    "size": bytes.len(),
                    "chunks": [{
                        "chunkId": chunk_id,
                        "size": bytes.len()
                    }]
                }],
                "treeObjects": [{
                    "treeId": root_tree_id,
                    "entries": [{
                        "path": "assets/hero.bin",
                        "fileObjectId": file_object_id,
                        "size": bytes.len()
                    }]
                }],
                "tombstones": [],
                "deletedPaths": []
            }
        }))
        .expect("canonical storeObjects payload deserializes");

        let Json(publish_payload) = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(publish_body),
        )
        .await
        .expect("v2 publish succeeds");

        assert!(
            publish_payload
                .pointer("/layerHead/rootTreeId")
                .and_then(Value::as_str)
                .is_some()
        );
        let artifact_id = publish_payload
            .get("published")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|artifact| artifact.get("id"))
            .and_then(Value::as_str)
            .expect("published artifact id")
            .to_string();

        let Json(receive_payload) = receive_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(SyncReceiveBody {
                layer_id: Some(fixture.layer_id.clone()),
                layer_id_camel: None,
                cursor: None,
                limit: Some(200),
            }),
        )
        .await
        .expect("receive succeeds");
        let content_objects = receive_payload
            .get("contentObjects")
            .expect("receive has content objects");
        assert_eq!(
            content_objects
                .get("chunks")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            content_objects
                .pointer("/chunks/0/chunkId")
                .and_then(Value::as_str),
            Some(chunk_id.as_str())
        );
        assert_eq!(
            receive_payload
                .get("contents")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        assert!(
            !serde_json::to_string(&receive_payload)
                .expect("receive serializes")
                .contains(BASE64.encode(&bytes).as_str())
        );

        let Json(content_payload) = get_layer_artifact_content(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                fixture.layer_id.clone(),
                artifact_id,
            )),
            fixture.bearer_headers(),
        )
        .await
        .expect("content endpoint assembles chunks");
        assert_eq!(
            content_payload
                .pointer("/content/encoding")
                .and_then(Value::as_str),
            Some("base64")
        );
        assert_eq!(
            content_payload
                .pointer("/content/value")
                .and_then(Value::as_str),
            Some(BASE64.encode(bytes).as_str())
        );
    }

    #[tokio::test]
    async fn step_and_artifact_diff_endpoints_return_lens_window_segments() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let path = "src/main.rs";
        let base_bytes = Bytes::from_static(b"base-line-ABCDEFGHIJ\nbase-two\n");
        let target_bytes = Bytes::from_static(b"next-line-ABCDEFGHIJ\nnext-two\n");
        let base_chunk_id = blake3_digest_for_bytes(&base_bytes);
        let target_chunk_id = blake3_digest_for_bytes(&target_bytes);
        let base_file_object_id = blake3_digest_for_bytes(&base_bytes);
        let target_file_object_id = blake3_digest_for_bytes(&target_bytes);
        let base_tree_id = blake3_digest_for_bytes(b"step-diff-base-tree");
        let target_tree_id = blake3_digest_for_bytes(b"step-diff-target-tree");
        let base_step_id = format!("step_{}", Uuid::new_v4().simple());
        let target_step_id = format!("step_{}", Uuid::new_v4().simple());

        for (chunk_id, bytes) in [
            (base_chunk_id.clone(), base_bytes.clone()),
            (target_chunk_id.clone(), target_bytes.clone()),
        ] {
            let _ = put_space_chunk(
                State(test_state(fixture.pool.clone())),
                Path((
                    fixture.workspace_id.clone(),
                    fixture.space_id.clone(),
                    chunk_id,
                )),
                fixture.bearer_headers(),
                bytes,
            )
            .await
            .expect("chunk upload succeeds");
        }

        for (step_id, tree_id, file_object_id, chunk_id, bytes, base_tree) in [
            (
                base_step_id.as_str(),
                base_tree_id.as_str(),
                base_file_object_id.as_str(),
                base_chunk_id.as_str(),
                base_bytes.len(),
                None,
            ),
            (
                target_step_id.as_str(),
                target_tree_id.as_str(),
                target_file_object_id.as_str(),
                target_chunk_id.as_str(),
                target_bytes.len(),
                Some(base_tree_id.as_str()),
            ),
        ] {
            let mut body = json!({
                "protocol": "layrs.sync.v2",
                "layerId": fixture.layer_id,
                "policyEpoch": 1,
                "idempotencyKey": format!("idem_{}", Uuid::new_v4().simple()),
                "sourceClientId": "test-client",
                "rootTreeId": tree_id,
                "changedPaths": [path],
                "step": {
                    "stepId": step_id,
                    "layerId": fixture.layer_id,
                    "rootTreeId": tree_id,
                    "changedPaths": [path]
                },
                "storeObjects": {
                    "chunks": [{
                        "chunkId": chunk_id,
                        "digest": chunk_id,
                        "size": bytes
                    }],
                    "fileObjects": [{
                        "fileObjectId": file_object_id,
                        "digest": file_object_id,
                        "size": bytes,
                        "mediaType": "text/plain",
                        "chunks": [{
                            "chunkId": chunk_id,
                            "digest": chunk_id,
                            "size": bytes,
                            "byteOffset": 0
                        }]
                    }],
                    "treeObjects": [{
                        "treeId": tree_id,
                        "entries": [{
                            "path": path,
                            "fileObjectId": file_object_id,
                            "size": bytes
                        }]
                    }]
                }
            });
            if let Some(base_tree) = base_tree {
                body["baseTreeId"] = json!(base_tree);
                body["step"]["baseTreeId"] = json!(base_tree);
            }
            let _: Json<Value> = publish_local_space_sync(
                State(test_state(fixture.pool.clone())),
                Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
                fixture.bearer_headers(),
                Json(serde_json::from_value(body).expect("publish body deserializes")),
            )
            .await
            .expect("publish succeeds");
        }

        let Json(steps_payload) = list_layer_steps(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                fixture.layer_id.clone(),
            )),
            fixture.bearer_headers(),
        )
        .await
        .expect("steps list succeeds");
        assert!(
            steps_payload
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|steps| steps
                    .iter()
                    .any(|step| step.get("stepId").and_then(Value::as_str)
                        == Some(target_step_id.as_str())))
        );

        let Json(step_payload) = get_layer_step(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                fixture.layer_id.clone(),
                target_step_id.clone(),
            )),
            fixture.bearer_headers(),
        )
        .await
        .expect("step detail succeeds");
        assert_eq!(
            step_payload
                .pointer("/files/0/action")
                .and_then(Value::as_str),
            Some("modified")
        );

        let Json(step_diff) = get_layer_step_diff(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                fixture.layer_id.clone(),
                target_step_id.clone(),
            )),
            Query(StepDiffQuery {
                path: None,
                start: Some(0),
                limit: Some(1),
                column_start: Some(0),
                column_start_camel: None,
                column_limit: Some(4),
                column_limit_camel: None,
            }),
            fixture.bearer_headers(),
        )
        .await
        .expect("step diff succeeds");
        let diff_lines = step_diff
            .pointer("/diff/hunks/0/lines")
            .and_then(Value::as_array)
            .expect("step diff includes hunk lines");
        assert_eq!(
            diff_lines[0].get("op").and_then(Value::as_str),
            Some("delete")
        );
        assert_eq!(
            diff_lines[0].get("textSegment").and_then(Value::as_str),
            Some("base")
        );
        assert_eq!(
            diff_lines[1].get("op").and_then(Value::as_str),
            Some("insert")
        );
        assert_eq!(
            diff_lines[1].get("textSegment").and_then(Value::as_str),
            Some("next")
        );
        assert_eq!(
            diff_lines[1].get("textLength").and_then(Value::as_u64),
            Some(20)
        );
        assert_eq!(
            diff_lines[1].get("columnStart").and_then(Value::as_u64),
            Some(0)
        );
        assert_eq!(
            diff_lines[1].get("columnEnd").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            diff_lines[1].get("hasMoreColumns").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            step_diff
                .pointer("/source/runtime/id")
                .and_then(Value::as_str),
            Some("layrs.server.lens-runtime.text")
        );

        let Json(artifacts_payload) = list_layer_artifacts(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                fixture.layer_id.clone(),
            )),
            fixture.bearer_headers(),
        )
        .await
        .expect("artifacts list succeeds");
        let artifact_id = artifacts_payload
            .get("items")
            .and_then(Value::as_array)
            .and_then(|artifacts| artifacts.first())
            .and_then(|artifact| artifact.get("id"))
            .and_then(Value::as_str)
            .expect("artifact id exists")
            .to_string();
        let Json(artifact_diff) = get_layer_artifact_diff(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                fixture.layer_id.clone(),
                artifact_id,
            )),
            Query(ArtifactDiffQuery {
                start: Some(0),
                limit: Some(1),
                column_start: Some(0),
                column_start_camel: None,
                column_limit: Some(4),
                column_limit_camel: None,
                base_layer_id: None,
                base_layer_id_camel: None,
            }),
            fixture.bearer_headers(),
        )
        .await
        .expect("artifact diff succeeds");
        assert_eq!(
            artifact_diff
                .pointer("/diff/hunks/0/lines/0/textSegment")
                .and_then(Value::as_str),
            Some("next")
        );
        assert_eq!(
            artifact_diff
                .pointer("/diff/hunks/0/lines/0/hasMoreColumns")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[tokio::test]
    async fn receive_does_not_return_redacted_content() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let bytes = Bytes::from_static(b"TOP_SECRET");
        let chunk_id = blake3_digest_for_bytes(&bytes);
        let file_object_id = blake3_digest_for_bytes(&bytes);
        let root_tree_id = blake3_digest_for_bytes(b"secret-redacted-tree");

        let _ = put_space_chunk(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                chunk_id.clone(),
            )),
            fixture.bearer_headers(),
            bytes.clone(),
        )
        .await
        .expect("secret chunk upload succeeds");

        let publish_body: SyncPublishBody = serde_json::from_value(json!({
            "protocol": "layrs.sync.v2",
            "layerId": fixture.layer_id,
            "policyEpoch": 1,
            "idempotencyKey": format!("idem_{}", Uuid::new_v4().simple()),
            "sourceClientId": "test-client",
            "rootTreeId": root_tree_id,
            "changedPaths": ["secret/token.txt"],
            "storeObjects": {
                "chunks": [{
                    "chunkId": chunk_id,
                    "digest": chunk_id,
                    "size": bytes.len()
                }],
                "fileObjects": [{
                    "fileObjectId": file_object_id,
                    "size": bytes.len(),
                    "mediaType": "text/plain",
                    "chunks": [{
                        "chunkId": chunk_id,
                        "size": bytes.len()
                    }]
                }],
                "treeObjects": [{
                    "treeId": root_tree_id,
                    "entries": [{
                        "path": "secret/token.txt",
                        "fileObjectId": file_object_id,
                        "size": bytes.len()
                    }]
                }]
            }
        }))
        .expect("secret chunked publish body deserializes");
        let _ = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(publish_body),
        )
        .await
        .expect("secret chunked publish succeeds");

        let policy_id = policy_id_for_layer(
            &fixture.pool,
            &fixture.workspace_id,
            &fixture.space_id,
            &fixture.layer_id,
        )
        .await
        .expect("layer has policy");
        sqlx::query(
            r#"
            INSERT INTO layer_access_policy_rules
                (rule_id, policy_id, path, mode, visibility, read_account_ids)
            VALUES
                ($1, $2, 'secret/**', 'restricted', 'stub', $3)
            "#,
        )
        .bind(prefixed_id("access_rule"))
        .bind(&policy_id)
        .bind(vec!["account_somebody_else".to_string()])
        .execute(&fixture.pool)
        .await
        .expect("restricted rule inserted");

        let Json(receive_payload) = receive_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(SyncReceiveBody {
                layer_id: Some(fixture.layer_id.clone()),
                layer_id_camel: None,
                cursor: None,
                limit: Some(200),
            }),
        )
        .await
        .expect("receive succeeds");

        assert_eq!(
            receive_payload
                .get("contents")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        assert_eq!(
            receive_payload
                .get("contentObjects")
                .and_then(|objects| objects.get("chunks"))
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        let serialized = serde_json::to_string(&receive_payload).expect("receive serializes");
        assert!(!serialized.contains("TOP_SECRET"));
        assert!(
            receive_payload
                .get("artifacts")
                .and_then(Value::as_array)
                .and_then(|artifacts| artifacts.first())
                .and_then(|artifact| artifact.get("access"))
                .and_then(|access| access.get("isRedacted"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        );
    }

    struct SyncTestFixture {
        pool: PgPool,
        workspace_id: String,
        space_id: String,
        layer_id: String,
        bearer_token: String,
    }

    impl SyncTestFixture {
        async fn create() -> Option<Self> {
            let database_url = test_database_url()?;
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&database_url)
                .await
                .expect("test database URL is set but cannot be reached");
            MIGRATOR
                .run(&pool)
                .await
                .expect("test database migrations run");

            let suffix = Uuid::new_v4().simple().to_string();
            let account_id = format!("account_{suffix}");
            let workspace_id = format!("workspace_{suffix}");
            let space_id = format!("space_{suffix}");
            let layer_id = format!("layer_{suffix}");
            let device_id = format!("device_{suffix}");
            let bearer_token = token("desktop_test");

            sqlx::query(
                "INSERT INTO accounts (account_id, email, display_name) VALUES ($1, $2, 'Sync Tester')",
            )
            .bind(&account_id)
            .bind(format!("sync-{suffix}@example.com"))
            .execute(&pool)
            .await
            .expect("account inserted");
            sqlx::query(
                "INSERT INTO desktop_devices (device_id, account_id, display_name, public_key_thumbprint, last_seen_at) VALUES ($1, $2, 'Test Desktop', 'thumbprint-test', now())",
            )
            .bind(&device_id)
            .bind(&account_id)
            .execute(&pool)
            .await
            .expect("device inserted");
            sqlx::query(
                r#"
                INSERT INTO desktop_device_tokens
                    (token_id, device_id, account_id, access_token_digest, refresh_token_digest, expires_at)
                VALUES
                    ($1, $2, $3, $4, $5, now() + interval '1 day')
                "#,
            )
            .bind(prefixed_id("desktop_token"))
            .bind(&device_id)
            .bind(&account_id)
            .bind(digest_secret(&bearer_token))
            .bind(digest_secret(&token("desktop_refresh_test")))
            .execute(&pool)
            .await
            .expect("desktop token inserted");

            let mut tx = pool.begin().await.expect("test tx begins");
            sqlx::query(
                "INSERT INTO workspaces (workspace_id, slug, name, created_by_account_id) VALUES ($1, $2, 'Sync Workspace', $3)",
            )
            .bind(&workspace_id)
            .bind(format!("sync-{suffix}"))
            .bind(&account_id)
            .execute(&mut *tx)
            .await
            .expect("workspace inserted");
            sqlx::query(
                "INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role) VALUES ($1, $2, $3, 'owner')",
            )
            .bind(prefixed_id("membership"))
            .bind(&workspace_id)
            .bind(&account_id)
            .execute(&mut *tx)
            .await
            .expect("membership inserted");
            sqlx::query(
                "INSERT INTO spaces (space_id, workspace_id, slug, name, created_by_account_id) VALUES ($1, $2, 'sync-space', 'Sync Space', $3)",
            )
            .bind(&space_id)
            .bind(&workspace_id)
            .bind(&account_id)
            .execute(&mut *tx)
            .await
            .expect("space inserted");
            sqlx::query(
                "INSERT INTO layers (layer_id, workspace_id, space_id, name, created_by_account_id) VALUES ($1, $2, $3, 'Main', $4)",
            )
            .bind(&layer_id)
            .bind(&workspace_id)
            .bind(&space_id)
            .bind(&account_id)
            .execute(&mut *tx)
            .await
            .expect("layer inserted");
            insert_empty_layer_policy(
                &mut tx,
                &workspace_id,
                &space_id,
                &layer_id,
                Some(&account_id),
            )
            .await
            .expect("policy inserted");
            tx.commit().await.expect("test tx commits");

            Some(Self {
                pool,
                workspace_id,
                space_id,
                layer_id,
                bearer_token,
            })
        }

        fn bearer_headers(&self) -> HeaderMap {
            let mut headers = HeaderMap::new();
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", self.bearer_token))
                    .expect("bearer header is valid"),
            );
            headers
        }
    }

    fn test_database_url() -> Option<String> {
        std::env::var("LAYRS_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("LAYRS_DATABASE_URL"))
            .or_else(|_| std::env::var("DATABASE_URL"))
            .ok()
    }

    fn test_state(pool: PgPool) -> AppState {
        AppState {
            pool,
            config: WebServerConfig {
                addr: "127.0.0.1:0".to_string(),
                studio_url: "http://127.0.0.1:5173".to_string(),
                database_url: "postgres://test".to_string(),
                deployment_id: "test".to_string(),
                cookie_secure: false,
            },
        }
    }

    fn publish_artifact(
        path: &str,
        kind: &str,
        media_type: &str,
        content: Value,
    ) -> PublishArtifactBody {
        PublishArtifactBody {
            id: None,
            artifact_id: None,
            artifact_id_camel: None,
            path: Some(path.to_string()),
            logical_path: None,
            logical_path_camel: None,
            kind: Some(kind.to_string()),
            artifact_type: None,
            media_type: Some(media_type.to_string()),
            media_type_camel: None,
            content: Some(content),
            file_object_id: None,
            file_object_id_camel: None,
            object_id: None,
            object_id_camel: None,
            tree_id: None,
            tree_id_camel: None,
            sha256: None,
            content_hash: None,
            size_bytes: None,
            size_bytes_camel: None,
            chunks: Vec::new(),
            state: None,
            operation: None,
            action: None,
            deleted: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(transparent)]
struct PublishStoreObjectsBody(PublishCanonicalStoreObjectsBody);

impl PublishStoreObjectsBody {
    fn into_flat(self) -> Result<Vec<PublishStoreObjectBody>, ApiError> {
        self.0.into_flat()
    }
}

#[derive(Default, Deserialize)]
struct PublishCanonicalStoreObjectsBody {
    #[serde(default)]
    chunks: Vec<PublishChunkObjectBody>,
    #[serde(default)]
    file_objects: Vec<PublishFileObjectBody>,
    #[serde(default, rename = "fileObjects")]
    file_objects_camel: Vec<PublishFileObjectBody>,
    #[serde(default)]
    tree_objects: Vec<PublishTreeObjectBody>,
    #[serde(default, rename = "treeObjects")]
    tree_objects_camel: Vec<PublishTreeObjectBody>,
    #[serde(default)]
    tombstones: Vec<PublishTombstoneObjectBody>,
    #[serde(default)]
    deleted_paths: Vec<String>,
    #[serde(default, rename = "deletedPaths")]
    deleted_paths_camel: Vec<String>,
}

impl PublishCanonicalStoreObjectsBody {
    fn into_flat(mut self) -> Result<Vec<PublishStoreObjectBody>, ApiError> {
        self.file_objects.extend(self.file_objects_camel);
        self.tree_objects.extend(self.tree_objects_camel);
        self.deleted_paths.extend(self.deleted_paths_camel);

        let mut chunks_by_id = HashMap::new();
        for chunk in self.chunks {
            let chunk_id = chunk
                .chunk_id
                .clone()
                .or_else(|| chunk.chunk_id_camel.clone())
                .ok_or_else(|| {
                    ApiError::bad_request("storeObjects.chunks[].chunkId is required")
                })?;
            chunks_by_id.insert(chunk_id, chunk);
        }

        let mut objects = Vec::new();
        for tree in self.tree_objects {
            let tree_id = tree
                .tree_id
                .or(tree.tree_id_camel)
                .or(tree.object_id)
                .or(tree.object_id_camel)
                .ok_or_else(|| {
                    ApiError::bad_request("storeObjects.treeObjects[].treeId is required")
                })?;
            objects.push(PublishStoreObjectBody {
                object_type: Some("tree".to_string()),
                object_type_camel: None,
                object_id: Some(tree_id),
                object_id_camel: None,
                path: None,
                hash: None,
                digest: None,
                size: Some(tree.entries.len() as u64),
                size_bytes: None,
                size_bytes_camel: None,
                media_type: None,
                media_type_camel: None,
                chunks: Vec::new(),
                entries: tree.entries,
            });
        }

        for file in self.file_objects {
            let file_object_id = file
                .file_object_id
                .or(file.file_object_id_camel)
                .or(file.object_id)
                .or(file.object_id_camel)
                .ok_or_else(|| {
                    ApiError::bad_request("storeObjects.fileObjects[].fileObjectId is required")
                })?;
            let mut chunks = Vec::new();
            for chunk_ref in file.chunks {
                let chunk_id = chunk_ref
                    .chunk_id
                    .or(chunk_ref.chunk_id_camel)
                    .ok_or_else(|| ApiError::bad_request("file object chunkId is required"))?;
                let source = chunks_by_id.get(&chunk_id);
                chunks.push(PublishStoreObjectChunkBody {
                    chunk_id: Some(chunk_id),
                    chunk_id_camel: None,
                    digest: source.and_then(|chunk| chunk.digest.clone()),
                    hash: source.and_then(|chunk| chunk.hash.clone()),
                    size: chunk_ref
                        .size
                        .or_else(|| source.and_then(|chunk| chunk.size)),
                    size_bytes: chunk_ref
                        .size_bytes
                        .or(chunk_ref.size_bytes_camel)
                        .or_else(|| {
                            source.and_then(|chunk| chunk.size_bytes.or(chunk.size_bytes_camel))
                        }),
                    size_bytes_camel: None,
                    byte_offset: chunk_ref.byte_offset.or(chunk_ref.byte_offset_camel),
                    byte_offset_camel: None,
                });
            }
            objects.push(PublishStoreObjectBody {
                object_type: Some("file".to_string()),
                object_type_camel: None,
                object_id: Some(file_object_id),
                object_id_camel: None,
                path: file.path,
                hash: file.digest.or(file.hash),
                digest: None,
                size: file.size,
                size_bytes: file.size_bytes.or(file.size_bytes_camel),
                size_bytes_camel: None,
                media_type: file.media_type.or(file.media_type_camel),
                media_type_camel: None,
                chunks,
                entries: Vec::new(),
            });
        }

        for tombstone in self.tombstones {
            if let Some(path) = tombstone.path {
                self.deleted_paths.push(path);
            }
        }
        for path in self.deleted_paths {
            objects.push(PublishStoreObjectBody {
                object_type: Some("tombstone".to_string()),
                object_type_camel: None,
                object_id: None,
                object_id_camel: None,
                path: Some(path),
                hash: None,
                digest: None,
                size: None,
                size_bytes: None,
                size_bytes_camel: None,
                media_type: None,
                media_type_camel: None,
                chunks: Vec::new(),
                entries: Vec::new(),
            });
        }

        Ok(objects)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishChunkObjectBody {
    #[serde(default)]
    chunk_id: Option<String>,
    #[serde(default, rename = "chunkId")]
    chunk_id_camel: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
}

#[derive(Deserialize)]
struct PublishFileObjectBody {
    #[serde(default)]
    file_object_id: Option<String>,
    #[serde(default, rename = "fileObjectId")]
    file_object_id_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default, rename = "mediaType")]
    media_type_camel: Option<String>,
    #[serde(default)]
    chunks: Vec<PublishChunkRefBody>,
}

#[derive(Deserialize)]
struct PublishChunkRefBody {
    #[serde(default)]
    chunk_id: Option<String>,
    #[serde(default, rename = "chunkId")]
    chunk_id_camel: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default)]
    byte_offset: Option<i64>,
    #[serde(default, rename = "byteOffset")]
    byte_offset_camel: Option<i64>,
}

#[derive(Deserialize)]
struct PublishTreeObjectBody {
    #[serde(default)]
    tree_id: Option<String>,
    #[serde(default, rename = "treeId")]
    tree_id_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
    #[serde(default)]
    entries: Vec<PublishTreeEntryBody>,
}

#[derive(Deserialize)]
struct PublishTreeEntryBody {
    path: String,
    #[serde(default)]
    file_object_id: Option<String>,
    #[serde(default, rename = "fileObjectId")]
    file_object_id_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
}

#[derive(Deserialize)]
struct PublishTombstoneObjectBody {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
struct PublishStoreObjectBody {
    #[serde(default)]
    object_type: Option<String>,
    #[serde(default, rename = "objectType")]
    object_type_camel: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default, rename = "objectId")]
    object_id_camel: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default, rename = "mediaType")]
    media_type_camel: Option<String>,
    #[serde(default)]
    chunks: Vec<PublishStoreObjectChunkBody>,
    #[serde(default)]
    entries: Vec<PublishTreeEntryBody>,
}

#[derive(Deserialize)]
struct PublishStoreObjectChunkBody {
    #[serde(default)]
    chunk_id: Option<String>,
    #[serde(default, rename = "chunkId")]
    chunk_id_camel: Option<String>,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    size_bytes: Option<i64>,
    #[serde(default, rename = "sizeBytes")]
    size_bytes_camel: Option<i64>,
    #[serde(default)]
    byte_offset: Option<i64>,
    #[serde(default, rename = "byteOffset")]
    byte_offset_camel: Option<i64>,
}
