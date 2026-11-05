use super::*;
use layrs_core::scm::{ReleaseId, ReleasePublicationId};
fn release(value: String) -> Result<ReleaseId, Error> {
    ReleaseId::parse(value).map_err(|_| Error(V3StoreError::InvalidCapability))
}
pub(super) async fn list(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<serde_json::Value>, Error> {
    Ok(Json(
        s.store
            .list_releases(&s.principal(&h).await?, &space(id)?)
            .await?,
    ))
}
pub(super) async fn history(
    State(s): State<CollaborationHttpState>,
    Path((id, target)): Path<(String, String)>,
    h: HeaderMap,
) -> Result<Json<ReleasePublicationHistory>, Error> {
    Ok(Json(
        s.store
            .release_publications(&s.principal(&h).await?, &space(id)?, &release(target)?)
            .await?,
    ))
}
pub(super) async fn publish(
    State(s): State<CollaborationHttpState>,
    Path((id, target)): Path<(String, String)>,
    h: HeaderMap,
    Json(request): Json<PublishReleaseRequest>,
) -> Result<Json<ReleaseEventEntry>, Error> {
    Ok(Json(
        s.store
            .publish_release(
                &s.principal(&h).await?,
                &space(id)?,
                &release(target)?,
                request,
            )
            .await?,
    ))
}
pub(super) async fn revoke(
    State(s): State<CollaborationHttpState>,
    Path((id, target, publication)): Path<(String, String, String)>,
    h: HeaderMap,
    Json(request): Json<RevokeReleaseRequest>,
) -> Result<Json<ReleaseEventEntry>, Error> {
    let publication = ReleasePublicationId::parse(publication)
        .map_err(|_| Error(V3StoreError::InvalidCapability))?;
    Ok(Json(
        s.store
            .revoke_release(
                &s.principal(&h).await?,
                &space(id)?,
                &release(target)?,
                &publication,
                request,
            )
            .await?,
    ))
}
