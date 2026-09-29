//! HTTP boundary for versioned collaboration services, never legacy Web state.
use crate::identity::BearerVerifier;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use layrs_core::scm::SpaceId;
use layrs_store_server::v3::*;
use std::sync::Arc;
mod catalog;
mod invitations;
#[path = "collaboration/releases.rs"]
mod releases;
mod scoped_reviews;
mod teams;

#[derive(Clone)]
pub struct CollaborationHttpState {
    store: PgCollaborationStore,
    pool: sqlx::PgPool,
    identity: Arc<dyn BearerVerifier>,
}
impl CollaborationHttpState {
    pub fn new(repository: PgV3Repository, identity: Arc<dyn BearerVerifier>) -> Self {
        Self {
            pool: repository.pool().clone(),
            store: PgCollaborationStore::new(repository),
            identity,
        }
    }
}
pub fn router(state: CollaborationHttpState) -> Router {
    Router::new()
        .route("/v3/spaces", get(catalog::spaces))
        .route("/v3/spaces/:space/capabilities", get(catalog::capabilities))
        .route(
            "/v3/spaces/:space/policy-management",
            get(policy_management),
        )
        .route(
            "/v3/spaces/:space/teams",
            get(teams::list).post(teams::create),
        )
        .route(
            "/v3/spaces/:space/teams/eligible-members",
            get(teams::eligible),
        )
        .route(
            "/v3/spaces/:space/teams/:team",
            axum::routing::put(teams::update),
        )
        .route(
            "/v3/spaces/:space/teams/:team/archive",
            post(teams::archive),
        )
        .route("/v3/spaces/:space/teams/:team/history", get(teams::history))
        .route("/v3/invitations", get(invitations::inbox))
        .route(
            "/v3/invitations/:invitation/actions",
            post(invitations::decide),
        )
        .route(
            "/v3/spaces/:space/invitations",
            get(invitations::list).post(invitations::create),
        )
        .route("/v3/spaces/:space/releases", get(releases::list))
        .route(
            "/v3/spaces/:space/releases/:release/publications",
            get(releases::history).post(releases::publish),
        )
        .route(
            "/v3/spaces/:space/releases/:release/publications/:publication/revoke",
            post(releases::revoke),
        )
        .route("/v3/spaces/:space/reviews", get(list).post(propose))
        .route(
            "/v3/spaces/:space/scoped-reviews",
            get(scoped_reviews::list).post(scoped_reviews::propose),
        )
        .route(
            "/v3/spaces/:space/scoped-reviews/:review",
            get(scoped_reviews::detail),
        )
        .route(
            "/v3/spaces/:space/scoped-reviews/:review/actions",
            post(scoped_reviews::action),
        )
        .route(
            "/v3/spaces/:space/scoped-reviews/:review/integration-status",
            get(scoped_reviews::status),
        )
        .route("/v3/spaces/:space/reviews/:review", get(detail))
        .route("/v3/spaces/:space/reviews/:review/versions", post(version))
        .route("/v3/spaces/:space/reviews/:review/actions", post(action))
        .route(
            "/v3/spaces/:space/reviews/:review/actions/:comment/corrections",
            post(correct_comment),
        )
        .route("/v3/spaces/:space/members", post(member))
        .route(
            "/v3/spaces/:space/layers/:layer/protection",
            get(protection).put(configure),
        )
        .route("/v3/spaces/:space/runs", get(runs).post(start_run))
        .route("/v3/spaces/:space/runs/pending", post(prepare_run))
        .route("/v3/spaces/:space/runs/:run/begin", post(begin_run))
        .route("/v3/spaces/:space/runs/:run/complete", post(complete_run))
        .route("/v3/spaces/:space/runs/:run/cancel", post(cancel_run))
        .route("/v3/spaces/:space/runs/:run", get(run_detail))
        .route(
            "/v3/spaces/:space/layers/:layer/gates/evaluate",
            post(evaluate),
        )
        .route("/v3/spaces/:space/notifications", get(notifications))
        .route("/v3/spaces/:space/notifications/read", post(mark_read))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .with_state(state)
}
struct Error(V3StoreError);
async fn policy_management(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<PolicyManagementState>, Error> {
    Ok(Json(
        s.store
            .policy_management(&s.principal(&h).await?, &space(id)?)
            .await?,
    ))
}
async fn correct_comment(
    State(s): State<CollaborationHttpState>,
    Path((id, review, comment)): Path<(String, String, String)>,
    h: HeaderMap,
    Json(request): Json<CorrectCommentRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .correct_comment(
                &s.principal(&h).await?,
                &space(id)?,
                &review,
                &comment,
                request,
            )
            .await?,
    ))
}
async fn cancel_run(
    State(s): State<CollaborationHttpState>,
    Path((id, run)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<CancelRunRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .cancel_run(&s.principal(&h).await?, &space(id)?, &run, request)
            .await?,
    ))
}
impl From<V3StoreError> for Error {
    fn from(value: V3StoreError) -> Self {
        Self(value)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = match self.0 {
            V3StoreError::NotFound => StatusCode::NOT_FOUND,
            V3StoreError::InvalidCapability => StatusCode::FORBIDDEN,
            V3StoreError::AuthorizationEpochChanged | V3StoreError::CustomPolicyRootActive => {
                StatusCode::CONFLICT
            }
            V3StoreError::ReleaseStateConflict => StatusCode::CONFLICT,
            V3StoreError::Backend(_) | V3StoreError::Io(_) => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_REQUEST,
        };
        (
            status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({"error":{"code":self.0.code()}})),
        )
            .into_response()
    }
}
impl CollaborationHttpState {
    async fn principal(&self, headers: &HeaderMap) -> Result<PrincipalContext, Error> {
        if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
            return Err(Error(V3StoreError::InvalidCapability));
        }
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(Error(V3StoreError::InvalidCapability))?;
        let id = self
            .identity
            .verify_bearer(token)
            .await
            .map_err(|_| Error(V3StoreError::InvalidCapability))?;
        Ok(load_principal_context(&self.pool, &id).await?)
    }
}
fn space(value: String) -> Result<SpaceId, Error> {
    SpaceId::parse(value).map_err(|_| Error(V3StoreError::InvalidCapability))
}
async fn list(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    Query(query): Query<ReviewListQuery>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .list_reviews_page(&s.principal(&h).await?, &space(id)?, query)
            .await?,
    ))
}
async fn detail(
    State(s): State<CollaborationHttpState>,
    Path((id, review)): Path<(String, String)>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .get_review(&s.principal(&h).await?, &space(id)?, &review)
            .await?,
    ))
}
async fn propose(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<ReviewProposalRequest>,
) -> Result<Json<ReviewVersion>, Error> {
    Ok(Json(
        s.store
            .propose_review(&s.principal(&h).await?, &space(id)?, None, request)
            .await?,
    ))
}
async fn version(
    State(s): State<CollaborationHttpState>,
    Path((id, review)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<ReviewProposalRequest>,
) -> Result<Json<ReviewVersion>, Error> {
    Ok(Json(
        s.store
            .propose_review(&s.principal(&h).await?, &space(id)?, Some(&review), request)
            .await?,
    ))
}
async fn action(
    State(s): State<CollaborationHttpState>,
    Path((id, review)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<ReviewActionRequest>,
) -> Result<Json<ReviewActionDocument>, Error> {
    Ok(Json(
        s.store
            .review_action(&s.principal(&h).await?, &space(id)?, &review, request)
            .await?,
    ))
}
async fn member(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<GrantMemberRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .grant_member(&s.principal(&h).await?, &space(id)?, request)
            .await?,
    ))
}
fn layer(value: String) -> Result<layrs_core::scm::LayerId, Error> {
    layrs_core::scm::LayerId::parse(value).map_err(|_| Error(V3StoreError::InvalidCapability))
}
async fn protection(
    State(s): State<CollaborationHttpState>,
    Path((id, l)): Path<(String, String)>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .get_protection(&s.principal(&h).await?, &space(id)?, &layer(l)?)
            .await?,
    ))
}
async fn configure(
    State(s): State<CollaborationHttpState>,
    Path((id, l)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<ProtectionRequest>,
) -> Result<Json<LayerProtection>, Error> {
    Ok(Json(
        s.store
            .configure_protection(&s.principal(&h).await?, &space(id)?, &layer(l)?, request)
            .await?,
    ))
}
async fn runs(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    Query(query): Query<RunListQuery>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .list_runs_page(&s.principal(&h).await?, &space(id)?, query)
            .await?,
    ))
}
async fn start_run(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<StartRunRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .start_run(&s.principal(&h).await?, &space(id)?, request)
            .await?,
    ))
}
async fn complete_run(
    State(s): State<CollaborationHttpState>,
    Path((id, run)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<CompleteRunRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .complete_run(&s.principal(&h).await?, &space(id)?, &run, request)
            .await?,
    ))
}
async fn prepare_run(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<StartRunRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .prepare_run(&s.principal(&h).await?, &space(id)?, request)
            .await?,
    ))
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BeginRun {}
async fn begin_run(
    State(s): State<CollaborationHttpState>,
    Path((id, run)): Path<(String, String)>,
    h: HeaderMap,
    Json(_request): Json<BeginRun>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .begin_run_execution(&s.principal(&h).await?, &space(id)?, &run)
            .await?,
    ))
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Evaluate {
    step_id: layrs_core::scm::StepId,
}
async fn evaluate(
    State(s): State<CollaborationHttpState>,
    Path((id, l)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<Evaluate>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .evaluate_gate(
                &s.principal(&h).await?,
                &space(id)?,
                &layer(l)?,
                &request.step_id,
            )
            .await?,
    ))
}
async fn run_detail(
    State(s): State<CollaborationHttpState>,
    Path((id, run)): Path<(String, String)>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .get_run(&s.principal(&h).await?, &space(id)?, &run)
            .await?,
    ))
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    after: u64,
}
async fn notifications(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    axum::extract::Query(page): axum::extract::Query<Page>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .notifications(&s.principal(&h).await?, &space(id)?, page.after)
            .await?,
    ))
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    last_sequence: u64,
}
async fn mark_read(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<Read>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .mark_notifications_read(&s.principal(&h).await?, &space(id)?, request.last_sequence)
            .await?,
    ))
}
