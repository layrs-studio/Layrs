use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::Value;

use super::*;

pub(super) async fn create_layer(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    body: Json<CreateLayerBody>,
) -> Result<Json<Value>, ApiError> {
    super::create_layer(state, path, headers, body).await
}

pub(super) async fn delete_layer(
    state: State<AppState>,
    path: Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::delete_layer(state, path, headers).await
}
