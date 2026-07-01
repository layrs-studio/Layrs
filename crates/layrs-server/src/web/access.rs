use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::Value;

use super::*;

pub(super) async fn get_layer_access_policy(
    state: State<AppState>,
    path: Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::get_layer_access_policy(state, path, headers).await
}

pub(super) async fn put_layer_access_policy(
    state: State<AppState>,
    path: Path<(String, String, String)>,
    headers: HeaderMap,
    body: Json<LayerAccessPolicyBody>,
) -> Result<Json<Value>, ApiError> {
    super::put_layer_access_policy(state, path, headers, body).await
}

pub(super) async fn create_layer_access_rule(
    state: State<AppState>,
    path: Path<(String, String, String)>,
    headers: HeaderMap,
    body: Json<LayerAccessRuleBody>,
) -> Result<Json<Value>, ApiError> {
    super::create_layer_access_rule(state, path, headers, body).await
}

pub(super) async fn update_layer_access_rule(
    state: State<AppState>,
    path: Path<(String, String, String, String)>,
    headers: HeaderMap,
    body: Json<LayerAccessRuleBody>,
) -> Result<Json<Value>, ApiError> {
    super::update_layer_access_rule(state, path, headers, body).await
}

pub(super) async fn delete_layer_access_rule(
    state: State<AppState>,
    path: Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::delete_layer_access_rule(state, path, headers).await
}
