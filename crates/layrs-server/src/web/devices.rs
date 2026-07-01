use axum::Json;
use axum::extract::{Form, Query, State};
use axum::http::HeaderMap;
use axum::response::Html;
use axum::response::Response;
use serde_json::Value;

use super::*;

pub(super) async fn list_devices(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::list_devices(state, headers).await
}

pub(super) async fn start_device_flow(state: State<AppState>) -> Result<Json<Value>, ApiError> {
    super::start_device_flow(state).await
}

pub(super) async fn device_verification_page(
    state: State<AppState>,
    headers: HeaderMap,
    query: Query<DevicePageQuery>,
) -> Html<String> {
    super::device_verification_page(state, headers, query).await
}

pub(super) async fn approve_device_flow(
    state: State<AppState>,
    headers: HeaderMap,
    body: Form<DeviceApproveForm>,
) -> Result<Response, ApiError> {
    super::approve_device_flow(state, headers, body).await
}

pub(super) async fn poll_device_flow(
    state: State<AppState>,
    body: Json<DevicePollRequest>,
) -> Result<Response, ApiError> {
    super::poll_device_flow(state, body).await
}

pub(super) async fn desktop_bootstrap(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::desktop_bootstrap(state, headers).await
}
