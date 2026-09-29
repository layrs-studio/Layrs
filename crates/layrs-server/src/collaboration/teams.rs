use super::*;
pub(super) async fn eligible(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .eligible_team_members(&s.principal(&h).await?, &space(id)?)
            .await?,
    ))
}
pub(super) async fn list(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .list_teams(&s.principal(&h).await?, &space(id)?)
            .await?,
    ))
}
pub(super) async fn create(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<TeamRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .change_team(&s.principal(&h).await?, &space(id)?, None, request)
            .await?,
    ))
}
pub(super) async fn update(
    State(s): State<CollaborationHttpState>,
    Path((id, team)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<TeamRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .change_team(&s.principal(&h).await?, &space(id)?, Some(&team), request)
            .await?,
    ))
}
pub(super) async fn archive(
    State(s): State<CollaborationHttpState>,
    Path((id, team)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<ArchiveTeamRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .archive_team(&s.principal(&h).await?, &space(id)?, &team, request)
            .await?,
    ))
}
pub(super) async fn history(
    State(s): State<CollaborationHttpState>,
    Path((id, team)): Path<(String, String)>,
    h: HeaderMap,
    Query(query): Query<TeamHistoryQuery>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .team_history(&s.principal(&h).await?, &space(id)?, &team, query)
            .await?,
    ))
}
