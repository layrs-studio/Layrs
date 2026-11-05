use layrs_core::scm::{ObjectId, SpaceId};
use layrs_sync::v3::{
    ClientId, FetchPlanRequest, FetchPlanResponse, IdempotencyKey, ObjectPlanRequest,
    ObjectPlanResponse, PrincipalId, RequestId, SpaceBootstrap, SpaceEventPage,
    SpaceEventPageRequest, SyncTransactionReceipt, SyncTransactionRequest, SyncV3Protocol,
    TransactionRequestType, ValidateV3,
};

use super::{error::V3HttpError, state::AuthenticatedPrincipal};

pub trait SpaceBoundWireRequest: ValidateV3 {
    fn space_id(&self) -> &SpaceId;
}

impl SpaceBoundWireRequest for ObjectPlanRequest {
    fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
}

impl SpaceBoundWireRequest for FetchPlanRequest {
    fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
}

impl SpaceBoundWireRequest for SpaceEventPageRequest {
    fn space_id(&self) -> &SpaceId {
        &self.space_id
    }
}

/// Transaction transport validation is deliberately narrower than
/// `ValidateV3`. Semantic faults (topology, duplicates, mutations and policy)
/// must reach the transaction engine so it can persist a rejected receipt and
/// AuthorizationSnapshot.
pub fn validate_transaction_transport(
    path_space_id: &str,
    request: &SyncTransactionRequest,
) -> Result<(), V3HttpError> {
    let path_space_id = parse_space_id(path_space_id)?;
    if path_space_id != request.space_id {
        return Err(V3HttpError::invalid_request("space_id_mismatch"));
    }
    if request.protocol != SyncV3Protocol::V3
        || request.message_type != TransactionRequestType::Request
    {
        return Err(V3HttpError::invalid_request("incompatible_protocol"));
    }
    let computed = request
        .computed_request_digest()
        .map_err(|_| V3HttpError::invalid_request("invalid_request_digest"))?;
    if computed != request.request_digest {
        return Err(V3HttpError::invalid_request("request_digest_mismatch"));
    }
    Ok(())
}

pub fn validate_bootstrap_response(
    principal: &AuthenticatedPrincipal,
    path_space_id: &SpaceId,
    response: &SpaceBootstrap,
) -> Result<(), V3HttpError> {
    response
        .validate_v3()
        .map_err(|_| V3HttpError::internal())?;
    correlation(
        &principal.principal_id,
        &response.principal_id,
        path_space_id,
        &response.space_id,
    )
}

pub struct PlanCorrelation {
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub request_id: RequestId,
}

impl From<&ObjectPlanRequest> for PlanCorrelation {
    fn from(request: &ObjectPlanRequest) -> Self {
        Self {
            space_id: request.space_id.clone(),
            client_id: request.client_id.clone(),
            request_id: request.request_id.clone(),
        }
    }
}

impl From<&FetchPlanRequest> for PlanCorrelation {
    fn from(request: &FetchPlanRequest) -> Self {
        Self {
            space_id: request.space_id.clone(),
            client_id: request.client_id.clone(),
            request_id: request.request_id.clone(),
        }
    }
}

pub struct TransactionCorrelation {
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: ObjectId,
}

impl From<&SyncTransactionRequest> for TransactionCorrelation {
    fn from(request: &SyncTransactionRequest) -> Self {
        Self {
            space_id: request.space_id.clone(),
            client_id: request.client_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            request_digest: request.request_digest.clone(),
        }
    }
}

pub struct EventCorrelation {
    pub space_id: SpaceId,
}

impl From<&SpaceEventPageRequest> for EventCorrelation {
    fn from(request: &SpaceEventPageRequest) -> Self {
        Self {
            space_id: request.space_id.clone(),
        }
    }
}

pub fn validate_object_plan_response(
    principal: &AuthenticatedPrincipal,
    request: &PlanCorrelation,
    response: &ObjectPlanResponse,
) -> Result<(), V3HttpError> {
    response
        .validate_v3()
        .map_err(|_| V3HttpError::internal())?;
    correlation(
        &principal.principal_id,
        &response.principal_id,
        &request.space_id,
        &response.space_id,
    )?;
    if response.client_id != request.client_id || response.request_id != request.request_id {
        return Err(V3HttpError::internal());
    }
    Ok(())
}

pub fn validate_fetch_plan_response(
    principal: &AuthenticatedPrincipal,
    request: &PlanCorrelation,
    response: &FetchPlanResponse,
) -> Result<(), V3HttpError> {
    response
        .validate_v3()
        .map_err(|_| V3HttpError::internal())?;
    correlation(
        &principal.principal_id,
        &response.principal_id,
        &request.space_id,
        &response.space_id,
    )?;
    if response.client_id != request.client_id || response.request_id != request.request_id {
        return Err(V3HttpError::internal());
    }
    Ok(())
}

pub fn validate_transaction_response(
    principal: &AuthenticatedPrincipal,
    request: &TransactionCorrelation,
    response: &SyncTransactionReceipt,
) -> Result<(), V3HttpError> {
    response
        .validate_v3()
        .map_err(|_| V3HttpError::internal())?;
    correlation(
        &principal.principal_id,
        &response.principal_id,
        &request.space_id,
        &response.space_id,
    )?;
    if response.client_id != request.client_id
        || response.idempotency_key != request.idempotency_key
        || response.request_digest != request.request_digest
    {
        return Err(V3HttpError::internal());
    }
    Ok(())
}

pub fn validate_event_response(
    principal: &AuthenticatedPrincipal,
    request: &EventCorrelation,
    response: &SpaceEventPage,
) -> Result<(), V3HttpError> {
    response
        .validate_v3()
        .map_err(|_| V3HttpError::internal())?;
    correlation(
        &principal.principal_id,
        &response.principal_id,
        &request.space_id,
        &response.space_id,
    )
}

fn correlation(
    authenticated_principal: &PrincipalId,
    response_principal: &PrincipalId,
    requested_space: &SpaceId,
    response_space: &SpaceId,
) -> Result<(), V3HttpError> {
    if authenticated_principal != response_principal || requested_space != response_space {
        Err(V3HttpError::internal())
    } else {
        Ok(())
    }
}

pub fn parse_space_id(value: &str) -> Result<SpaceId, V3HttpError> {
    SpaceId::parse(value.to_owned()).map_err(|_| V3HttpError::invalid_request("invalid_space_id"))
}

pub fn validate_wire_request<T: SpaceBoundWireRequest>(
    path_space_id: &str,
    request: &T,
) -> Result<(), V3HttpError> {
    let path_space_id = parse_space_id(path_space_id)?;
    if &path_space_id != request.space_id() {
        return Err(V3HttpError::invalid_request("space_id_mismatch"));
    }
    request
        .validate_v3()
        .map_err(|error| V3HttpError::invalid_request(error.code))
}
