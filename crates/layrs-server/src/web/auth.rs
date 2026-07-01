use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::Value;

use super::*;

pub(super) async fn signup(
    state: State<AppState>,
    body: Json<SignupRequest>,
) -> Result<Response, ApiError> {
    super::signup(state, body).await
}

pub(super) async fn login(
    state: State<AppState>,
    body: Json<LoginRequest>,
) -> Result<Response, ApiError> {
    super::login(state, body).await
}

pub(super) async fn logout(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    super::logout(state, headers).await
}

pub(super) async fn auth_session(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::auth_session(state, headers).await
}

pub(super) async fn me(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::me(state, headers).await
}
