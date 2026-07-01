use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::Value;

use super::*;

pub(super) async fn create_team(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CreateTeamBody>,
) -> Result<Json<Value>, ApiError> {
    super::create_team(state, path, headers, body).await
}

pub(super) async fn list_teams(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::list_teams(state, path, headers).await
}

pub(super) async fn get_team(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::get_team(state, path, headers).await
}

pub(super) async fn list_team_members(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::list_team_members(state, path, headers).await
}

pub(super) async fn add_team_member(
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
    body: Json<AddTeamMemberBody>,
) -> Result<Json<Value>, ApiError> {
    super::add_team_member(state, path, headers, body).await
}

pub(super) async fn remove_team_member(
    state: State<AppState>,
    path: Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::remove_team_member(state, path, headers).await
}

pub(super) async fn list_workspace_members(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::list_workspace_members(state, path, headers).await
}

pub(super) async fn list_workspace_invitations(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::list_workspace_invitations(state, path, headers).await
}

pub(super) async fn create_invitation(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
    body: Json<CreateInvitationBody>,
) -> Result<Json<Value>, ApiError> {
    super::create_invitation(state, path, headers, body).await
}

pub(super) async fn list_my_invitations(
    state: State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::list_my_invitations(state, headers).await
}

pub(super) async fn accept_invitation(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::accept_invitation(state, path, headers).await
}

pub(super) async fn decline_invitation(
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::decline_invitation(state, path, headers).await
}
