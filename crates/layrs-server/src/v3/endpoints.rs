use axum::{
    Json,
    body::Body,
    extract::{Path, Request, State},
    http::{
        HeaderValue, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
    },
    response::{IntoResponse, Response},
};
use futures_util::TryStreamExt;
use layrs_sync::v3::{
    FetchPlanRequest, ObjectPlanRequest, SpaceEventPageRequest, SyncTransactionRequest,
};

use super::{
    adapters::{
        EventCorrelation, PlanCorrelation, TransactionCorrelation, parse_space_id,
        validate_bootstrap_response, validate_event_response, validate_fetch_plan_response,
        validate_object_plan_response, validate_transaction_response,
        validate_transaction_transport, validate_wire_request,
    },
    auth::{bearer_token, capability_token},
    body::{limited_json, limited_upload_stream},
    error::V3HttpError,
    range::{RequestedRange, parse_range_header, resolve_range},
    state::{AuthenticatedPrincipal, DownloadResult, V3HttpState, V3ServiceError},
};

pub async fn provision_space(
    State(state): State<V3HttpState>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let body: layrs_store_server::v3::ProvisionSpaceRequest = limited_json(request, 4096).await?;
    let response = state.facade().provision_space(&principal, body).await?;
    Ok((StatusCode::CREATED, Json(response)).into_response())
}

pub async fn compare_steps(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let body = limited_json(request, 1024).await?;
    Ok(Json(
        state
            .facade()
            .compare_steps(&principal, &space_id, body)
            .await?,
    )
    .into_response())
}

pub async fn projection_access(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let body: layrs_sync::v3::ProjectionAccessRequest = limited_json(request, 1024 * 1024).await?;
    body.validate()
        .map_err(|_| V3HttpError::invalid_request("invalid_projection_paths"))?;
    let response = state
        .facade()
        .projection_access(&principal, &space_id, body.clone())
        .await?;
    if response.principal_id != principal.principal_id
        || response.space_id != space_id
        || response.step_id != body.step_id
        || response.policy_set_id != body.expected_policy_set_id
        || response.authorization_epoch != body.expected_authorization_epoch
        || response.entries.len() != body.paths.len()
        || response
            .entries
            .iter()
            .zip(&body.paths)
            .any(|(entry, path)| {
                &entry.path != path
                    || (entry.writable && !entry.content_readable)
                    || (entry.target.as_ref().is_none() && entry.executable)
            })
    {
        return Err(V3ServiceError::Internal.into());
    }
    Ok(Json(response).into_response())
}

pub async fn projection_page(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let body: layrs_sync::v3::ProjectionPageRequest = limited_json(request, 8192).await?;
    let response = state
        .facade()
        .projection_page(&principal, &space_id, body.clone())
        .await?;
    if response.principal_id != principal.principal_id
        || response.space_id != space_id
        || response.step_id != body.step_id
        || response.policy_set_id != body.expected_policy_set_id
        || response.authorization_epoch != body.expected_authorization_epoch
        || response.entries.len() > usize::from(body.limit)
        || response
            .entries
            .iter()
            .any(|entry| entry.writable && !entry.content_readable)
    {
        return Err(V3ServiceError::Internal.into());
    }
    Ok(Json(response).into_response())
}

pub async fn submit_path_patch(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let body: layrs_sync::v3::PathPatchSubmission = limited_json(request, 24 * 1024 * 1024).await?;
    if body.request.space_id != space_id {
        return Err(V3HttpError::invalid_request("space_mismatch"));
    }
    body.request
        .validate()
        .map_err(|_| V3HttpError::invalid_request("invalid_path_patch"))?;
    let digest = body.request.request_digest.clone();
    let client = body.request.client_id.clone();
    let key = body.request.idempotency_key.clone();
    let response = state.facade().submit_path_patch(&principal, body).await?;
    if response.request_digest != digest
        || response.receipt.principal_id != principal.principal_id
        || response.receipt.space_id != space_id
        || response.receipt.client_id != client
        || response.receipt.idempotency_key != key
    {
        return Err(V3ServiceError::Internal.into());
    }
    layrs_sync::v3::ValidateV3::validate_v3(&response.receipt)
        .map_err(|_| V3ServiceError::Internal)?;
    Ok(Json(response).into_response())
}

pub async fn begin_scoped_patch(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let body: layrs_sync::v3::ScopedPathPatchRequest =
        limited_json(request, 4 * 1024 * 1024).await?;
    if body.space_id != space_id {
        return Err(V3HttpError::invalid_request("space_mismatch"));
    }
    body.validate()
        .map_err(|_| V3HttpError::invalid_request("invalid_scoped_path_patch"))?;
    let result = state.facade().begin_scoped_patch(&principal, body).await?;
    Ok((StatusCode::CREATED, Json(result)).into_response())
}

pub async fn scoped_patch_progress(
    State(state): State<V3HttpState>,
    Path((space, session)): Path<(String, String)>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    Ok(Json(
        state
            .facade()
            .scoped_patch_progress(&principal, &space_id, &session)
            .await?,
    )
    .into_response())
}

pub async fn upload_scoped_patch_chunk(
    State(state): State<V3HttpState>,
    Path((space, session, file_index, ordinal)): Path<(String, String, usize, usize)>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    if request.headers().contains_key(CONTENT_RANGE) {
        return Err(V3HttpError::invalid_request("content_range_forbidden"));
    }
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let length = content_length(request.headers())?;
    if length == 0 || length > layrs_sync::v3::SCOPED_CHUNK_BYTES {
        return Err(V3HttpError::payload_too_large(
            layrs_sync::v3::SCOPED_CHUNK_BYTES,
        ));
    }
    let (body, progress) = limited_upload_stream(
        request.into_body(),
        layrs_sync::v3::SCOPED_CHUNK_BYTES,
        length,
    );
    let bytes = body
        .try_fold(
            Vec::with_capacity(length as usize),
            |mut bytes, chunk| async move {
                bytes.extend_from_slice(&chunk);
                Ok(bytes)
            },
        )
        .await?;
    if !progress.is_complete() || progress.bytes_seen() != length {
        return Err(V3ServiceError::LengthMismatch.into());
    }
    state
        .facade()
        .upload_scoped_patch_chunk(&principal, &space_id, &session, file_index, ordinal, bytes)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn finalize_scoped_patch(
    State(state): State<V3HttpState>,
    Path((space, session)): Path<(String, String)>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let result = state
        .facade()
        .finalize_scoped_patch(&principal, &space_id, &session)
        .await?;
    if result.receipt.principal_id != principal.principal_id || result.receipt.space_id != space_id
    {
        return Err(V3ServiceError::Internal.into());
    }
    layrs_sync::v3::ValidateV3::validate_v3(&result.receipt)
        .map_err(|_| V3ServiceError::Internal)?;
    Ok(Json(result).into_response())
}

pub async fn integrate_scoped_review(
    State(state): State<V3HttpState>,
    Path((space, review_id)): Path<(String, String)>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let body: layrs_sync::v3::ScopedReviewIntegrationRequest = limited_json(request, 1024).await?;
    let receipt = state
        .facade()
        .integrate_scoped_review(&principal, &space_id, &review_id, body)
        .await?;
    if receipt.principal_id != principal.principal_id || receipt.space_id != space_id {
        return Err(V3ServiceError::Internal.into());
    }
    layrs_sync::v3::ValidateV3::validate_v3(&receipt).map_err(|_| V3ServiceError::Internal)?;
    Ok(Json(receipt).into_response())
}

pub async fn step_history(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let body = limited_json(request, 2048).await?;
    Ok(Json(
        state
            .facade()
            .step_history(&principal, &space_id, body)
            .await?,
    )
    .into_response())
}

pub async fn bootstrap(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let space_id = parse_space_id(&space)?;
    let response = state.facade().bootstrap(&principal, &space_id).await?;
    validate_bootstrap_response(&principal, &space_id, &response)?;
    Ok(Json(response).into_response())
}

pub async fn object_plan(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let limit = state.limits().object_plan_json_bytes;
    let body: ObjectPlanRequest = limited_json(request, limit).await?;
    validate_wire_request(&space, &body)?;
    let correlation = PlanCorrelation::from(&body);
    let response = state.facade().object_plan(&principal, body).await?;
    validate_object_plan_response(&principal, &correlation, &response)?;
    Ok(Json(response).into_response())
}

pub async fn fetch_plan(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let limit = state.limits().fetch_plan_json_bytes;
    let body: FetchPlanRequest = limited_json(request, limit).await?;
    validate_wire_request(&space, &body)?;
    let correlation = PlanCorrelation::from(&body);
    let response = state.facade().fetch_plan(&principal, body).await?;
    validate_fetch_plan_response(&principal, &correlation, &response)?;
    Ok(Json(response).into_response())
}

pub async fn transact(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let limit = state.limits().transaction_json_bytes;
    let body: SyncTransactionRequest = limited_json(request, limit).await?;
    validate_transaction_transport(&space, &body)?;
    let correlation = TransactionCorrelation::from(&body);
    let response = state.facade().transact(&principal, body).await?;
    response.verify()?;
    validate_transaction_response(&principal, &correlation, response.receipt())?;
    let canonical_jcs = response.into_canonical_jcs();
    let length = canonical_jcs.len();
    let mut response = Response::new(Body::from(canonical_jcs));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).map_err(|_| V3ServiceError::Internal)?,
    );
    Ok(response)
}

pub async fn events(
    State(state): State<V3HttpState>,
    Path(space): Path<String>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    require_json(request.headers())?;
    let principal = authenticate(&state, principal_token(&request)?).await?;
    let limit = state.limits().events_json_bytes;
    let body: SpaceEventPageRequest = limited_json(request, limit).await?;
    validate_wire_request(&space, &body)?;
    let correlation = EventCorrelation::from(&body);
    let response = state.facade().events(&principal, body).await?;
    validate_event_response(&principal, &correlation, &response)?;
    Ok(Json(response).into_response())
}

pub async fn upload(
    State(state): State<V3HttpState>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    if request.headers().contains_key(CONTENT_RANGE) {
        return Err(V3HttpError::invalid_request("content_range_forbidden"));
    }
    let capability = capability_token(request.headers())?.to_owned();
    let content_length = content_length(request.headers())?;
    let limit = state.limits().upload_encoded_bytes;
    if content_length > limit {
        return Err(V3HttpError::payload_too_large(limit));
    }
    let (body, progress) = limited_upload_stream(request.into_body(), limit, content_length);
    let result = state
        .facade()
        .upload(&capability, content_length, body)
        .await?;
    if !progress.is_complete() {
        return Err(V3HttpError::internal());
    }
    if progress.bytes_seen() != content_length {
        return Err(V3ServiceError::LengthMismatch.into());
    }
    if result.encoded_len != content_length {
        return Err(V3HttpError::internal());
    }
    Ok((StatusCode::OK, Json(result)).into_response())
}

pub async fn download(
    State(state): State<V3HttpState>,
    request: Request,
) -> Result<Response, V3HttpError> {
    reject_query(&request)?;
    let capability = capability_token(request.headers())?.to_owned();
    let requested_range = requested_range(request.headers())?;
    let result = state
        .facade()
        .download(&capability, requested_range)
        .await?;
    download_response(result, requested_range)
}

async fn authenticate(
    state: &V3HttpState,
    token: String,
) -> Result<AuthenticatedPrincipal, V3HttpError> {
    state
        .facade()
        .authenticate_bearer(&token)
        .await
        .map_err(Into::into)
}

fn principal_token(request: &Request) -> Result<String, V3HttpError> {
    bearer_token(request.headers()).map(str::to_owned)
}

fn reject_query(request: &Request) -> Result<(), V3HttpError> {
    if request.uri().query().is_some() {
        Err(V3HttpError::invalid_request("query_forbidden"))
    } else {
        Ok(())
    }
}

fn content_length(headers: &axum::http::HeaderMap) -> Result<u64, V3HttpError> {
    let Some(value) = headers.get(CONTENT_LENGTH) else {
        return Err(V3HttpError::length_required());
    };
    value
        .to_str()
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| V3HttpError::invalid_request("invalid_content_length"))
}

fn require_json(headers: &axum::http::HeaderMap) -> Result<(), V3HttpError> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let value = values
        .next()
        .ok_or_else(V3HttpError::unsupported_media_type)?;
    if values.next().is_some() {
        return Err(V3HttpError::unsupported_media_type());
    }
    let media_type = value
        .to_str()
        .map_err(|_| V3HttpError::unsupported_media_type())?
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default();
    if media_type.eq_ignore_ascii_case("application/json") {
        Ok(())
    } else {
        Err(V3HttpError::unsupported_media_type())
    }
}

fn requested_range(headers: &axum::http::HeaderMap) -> Result<Option<RequestedRange>, V3HttpError> {
    let mut values = headers.get_all(RANGE).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(V3HttpError::invalid_request("invalid_range"));
    }
    let value = value
        .to_str()
        .map_err(|_| V3HttpError::invalid_request("invalid_range"))?;
    parse_range_header(value)
        .map(Some)
        .map_err(|_| V3HttpError::invalid_request("invalid_range"))
}

fn download_response(
    result: DownloadResult,
    requested_range: Option<RequestedRange>,
) -> Result<Response, V3HttpError> {
    let expected_range = match requested_range {
        Some(requested) => resolve_range(requested, result.total_encoded_len)
            .ok_or_else(|| V3HttpError::range_not_satisfiable(result.total_encoded_len))?,
        None => super::range::ByteRange::new(0, result.total_encoded_len)
            .ok_or(V3ServiceError::Internal)?,
    };
    if result.served_range.end_exclusive > result.total_encoded_len
        || result.served_range.start > result.served_range.end_exclusive
        || result.served_range != expected_range
        || (requested_range.is_some() && result.served_range.is_empty())
    {
        return Err(V3ServiceError::Internal.into());
    }
    let status = if requested_range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let length = result.served_range.len();
    let body = Body::from_stream(
        result
            .body
            .map_err(|_| std::io::Error::other("download stream failed")),
    );
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(result.content_type).map_err(|_| V3ServiceError::Internal)?,
    );
    response.headers_mut().insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).map_err(|_| V3ServiceError::Internal)?,
    );
    if requested_range.is_some() {
        let end_inclusive = result
            .served_range
            .end_exclusive
            .checked_sub(1)
            .ok_or(V3ServiceError::Internal)?;
        let value = format!(
            "bytes {}-{}/{}",
            result.served_range.start, end_inclusive, result.total_encoded_len
        );
        response.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&value).map_err(|_| V3ServiceError::Internal)?,
        );
    }
    Ok(response)
}
