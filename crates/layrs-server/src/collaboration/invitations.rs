use super::*;
pub(super) async fn inbox(
    State(s): State<CollaborationHttpState>,
    Query(query): Query<InvitationListQuery>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .list_invitations(&s.principal(&h).await?, None, query)
            .await?,
    ))
}
pub(super) async fn list(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    Query(query): Query<InvitationListQuery>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .list_invitations(&s.principal(&h).await?, Some(&space(id)?), query)
            .await?,
    ))
}
pub(super) async fn create(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<InviteMemberRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .invite_member(&s.principal(&h).await?, &space(id)?, request)
            .await?,
    ))
}
pub(super) async fn decide(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<InvitationActionRequest>,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .decide_invitation(&s.principal(&h).await?, &id, request.action)
            .await?,
    ))
}
