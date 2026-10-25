use super::*;

pub(super) async fn list(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .list_organisation_teams(&s.principal(&h).await?, &space(id)?)
            .await?,
    ))
}

pub(super) async fn create(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<OrganisationTeamRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .change_organisation_team(&s.principal(&h).await?, &space(id)?, None, request)
            .await?,
    ))
}

pub(super) async fn update(
    State(s): State<CollaborationHttpState>,
    Path((id, team)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<OrganisationTeamRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .change_organisation_team(&s.principal(&h).await?, &space(id)?, Some(&team), request)
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
            .archive_organisation_team(&s.principal(&h).await?, &space(id)?, &team, request)
            .await?,
    ))
}

pub(super) async fn assignment(
    State(s): State<CollaborationHttpState>,
    Path((id, team)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<OrganisationTeamAssignmentRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .assign_organisation_team(&s.principal(&h).await?, &space(id)?, &team, request)
            .await?,
    ))
}
