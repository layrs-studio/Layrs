use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, header::CACHE_CONTROL},
    middleware,
    middleware::Next,
    response::Response,
    routing::{get, post, put},
};

use super::{endpoints, error::V3HttpError, state::V3HttpState};

pub fn router(state: V3HttpState) -> Router {
    Router::new()
        .route("/v3/workspaces/provision", post(endpoints::provision_space))
        .route("/v3/spaces/:space_id/bootstrap", get(endpoints::bootstrap))
        .route(
            "/v3/spaces/:space_id/history",
            post(endpoints::step_history),
        )
        .route(
            "/v3/spaces/:space_id/compare",
            post(endpoints::compare_steps),
        )
        .route(
            "/v3/spaces/:space_id/object-plans",
            post(endpoints::object_plan),
        )
        .route(
            "/v3/spaces/:space_id/fetch-plans",
            post(endpoints::fetch_plan),
        )
        .route(
            "/v3/spaces/:space_id/transactions",
            post(endpoints::transact),
        )
        .route("/v3/spaces/:space_id/events/page", post(endpoints::events))
        .route("/v3/transfers/uploads", put(endpoints::upload))
        .route("/v3/transfers/downloads", get(endpoints::download))
        .fallback(|| async { V3HttpError::not_found() })
        .method_not_allowed_fallback(|| async { V3HttpError::method_not_allowed() })
        .layer(middleware::from_fn(no_store))
        .with_state(state)
}

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
