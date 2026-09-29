//! Navigation contracts for the UI: the authorized Space catalog and the
//! per-Space capability hint. Both are evaluated live for every request and
//! answered with `Cache-Control: no-store`; see
//! `docs/contracts/ui-catalog-and-capabilities.md`.
use super::*;
use axum::http::Uri;

pub(super) async fn spaces(
    State(s): State<CollaborationHttpState>,
    uri: Uri,
    h: HeaderMap,
) -> Result<Response, Error> {
    let principal = s.principal(&h).await?;
    let Query(query) = Query::<SpaceCatalogQuery>::try_from_uri(&uri).map_err(|_| {
        Error(V3StoreError::InvalidDescriptor(
            "invalid_catalog_query".into(),
        ))
    })?;
    Ok(no_store(Json(
        s.store.space_catalog(&principal, query).await?,
    )))
}

pub(super) async fn capabilities(
    State(s): State<CollaborationHttpState>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Response, Error> {
    let principal = s.principal(&h).await?;
    Ok(no_store(Json(
        s.store.space_capabilities(&principal, &space(id)?).await?,
    )))
}

fn no_store(body: impl IntoResponse) -> Response {
    ([(header::CACHE_CONTROL, "no-store")], body).into_response()
}
