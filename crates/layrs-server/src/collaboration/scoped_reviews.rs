use super::*;
use layrs_sync::v3::*;

pub(super) async fn list(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    Query(query): Query<ScopedReviewListQuery>,
    h: HeaderMap,
) -> Result<Json<ScopedReviewPage>, Error> {
    Ok(Json(
        s.store
            .list_scoped_reviews(&s.principal(&h).await?, &space(id)?, query)
            .await?,
    ))
}
pub(super) async fn propose(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(request): Json<ScopedReviewProposalRequest>,
) -> Result<Json<ScopedReview>, Error> {
    Ok(Json(
        s.store
            .propose_scoped_review(&s.principal(&h).await?, &space(id)?, request)
            .await?,
    ))
}
pub(super) async fn detail(
    State(s): State<CollaborationHttpState>,
    Path((id, review)): Path<(String, String)>,
    h: HeaderMap,
) -> Result<Json<ScopedReviewDetail>, Error> {
    Ok(Json(
        s.store
            .scoped_review_detail(&s.principal(&h).await?, &space(id)?, &review)
            .await?,
    ))
}
pub(super) async fn action(
    State(s): State<CollaborationHttpState>,
    Path((id, review)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<ScopedReviewActionRequest>,
) -> Result<Json<ScopedReviewAction>, Error> {
    Ok(Json(
        s.store
            .act_on_scoped_review(&s.principal(&h).await?, &space(id)?, &review, request)
            .await?,
    ))
}
pub(super) async fn status(
    State(s): State<CollaborationHttpState>,
    Path((id, review)): Path<(String, String)>,
    h: HeaderMap,
) -> Result<Json<ScopedIntegrationStatus>, Error> {
    Ok(Json(
        s.store
            .scoped_integration_status(&s.principal(&h).await?, &space(id)?, &review)
            .await?,
    ))
}
