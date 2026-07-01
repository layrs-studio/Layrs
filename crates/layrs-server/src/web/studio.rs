use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use serde_json::Value;

use super::*;

pub(super) async fn studio_snapshot(
    state: State<AppState>,
    query: Query<SnapshotQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::studio_snapshot(state, query, headers).await
}
