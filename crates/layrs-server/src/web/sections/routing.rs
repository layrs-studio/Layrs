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
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/disconnect-parent",
            post(layers::disconnect_layer_parent),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/layers/:layer_id/clear-steps",
            post(layers::clear_layer_steps),
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
            "/v1/workspaces/:workspace_id/spaces/:space_id/weave-requests",
            get(list_weave_requests).post(create_weave_request),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/weave-requests/:weave_id",
            get(get_weave_request),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/weave-requests/:weave_id/apply",
            post(apply_weave_request),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/weave-requests/:weave_id/conflicts/:conflict_id/resolve",
            post(resolve_weave_conflict),
        )
        .route(
            "/v1/workspaces/:workspace_id/spaces/:space_id/weave-requests/:weave_id/abort",
            post(abort_weave_request),
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
    #[serde(default)]
    steps: Vec<SyncStepBody>,
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
    #[serde(default, alias = "timeline_position")]
    timeline_position: Option<i64>,
    #[serde(default, alias = "origin_layer_id")]
    origin_layer_id: Option<String>,
    #[serde(default, alias = "origin_layer_name")]
    origin_layer_name: Option<String>,
    #[serde(default, alias = "origin_step_id")]
    origin_step_id: Option<String>,
    #[serde(default, alias = "step_kind")]
    step_kind: Option<String>,
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
