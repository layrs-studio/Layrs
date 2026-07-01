use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde_json::Value;

use super::*;

pub(super) async fn list_workspaces(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::list_workspaces(state, headers).await
}

pub(super) async fn create_workspace(
    state: State<AppState>,
    headers: HeaderMap,
    body: Json<CreateWorkspaceBody>,
) -> Result<Json<Value>, ApiError> {
    super::create_workspace(state, headers, body).await
}
