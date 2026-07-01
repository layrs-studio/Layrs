use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::Value;

use super::*;

pub(super) async fn create_space(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CreateSpaceBody>,
) -> Result<Json<Value>, ApiError> {
    super::create_space(state, path, headers, body).await
}

pub(super) async fn create_space_from_local(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CreateSpaceFromLocalBody>,
) -> Result<Json<Value>, ApiError> {
    super::create_space_from_local(state, path, headers, body).await
}

pub(super) async fn delete_space(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::delete_space(state, path, headers).await
}
