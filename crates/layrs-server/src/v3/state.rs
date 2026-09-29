use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;
use layrs_core::scm::SpaceId;
use layrs_sync::v3::{
    FetchPlanRequest, FetchPlanResponse, ObjectPlanRequest, ObjectPlanResponse, PrincipalId,
    SpaceBootstrap, SpaceEventPage, SpaceEventPageRequest, SyncTransactionReceipt,
    SyncTransactionRequest, ValidateV3,
};
use serde::Serialize;
use std::{pin::Pin, sync::Arc};

use super::range::{ByteRange, RequestedRange};

pub type UploadStream = Pin<Box<dyn Stream<Item = Result<Bytes, V3ServiceError>> + Send + 'static>>;
pub type DownloadStream =
    Pin<Box<dyn Stream<Item = Result<Bytes, V3ServiceError>> + Send + 'static>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedPrincipal {
    pub principal_id: PrincipalId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UploadDisposition {
    Installed,
    AlreadyPresent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadResult {
    pub disposition: UploadDisposition,
    /// Exact encoded length authorized by the verified capability and accepted
    /// by the Store after consuming the complete request stream.
    pub encoded_len: u64,
}

pub struct StoredTransactionReceipt {
    receipt: SyncTransactionReceipt,
    canonical_jcs: Bytes,
}

impl StoredTransactionReceipt {
    pub fn new(
        receipt: SyncTransactionReceipt,
        canonical_jcs: Bytes,
    ) -> Result<Self, V3ServiceError> {
        let value = Self {
            receipt,
            canonical_jcs,
        };
        value.verify()?;
        Ok(value)
    }

    pub fn receipt(&self) -> &SyncTransactionReceipt {
        &self.receipt
    }

    pub fn verify(&self) -> Result<(), V3ServiceError> {
        self.receipt
            .validate_v3()
            .map_err(|_| V3ServiceError::Internal)?;
        let value: SyncTransactionReceipt =
            serde_json::from_slice(&self.canonical_jcs).map_err(|_| V3ServiceError::Internal)?;
        if value != self.receipt {
            return Err(V3ServiceError::Internal);
        }
        let canonical = serde_jcs::to_vec(&value).map_err(|_| V3ServiceError::Internal)?;
        if canonical.as_slice() != self.canonical_jcs.as_ref() {
            return Err(V3ServiceError::Internal);
        }
        Ok(())
    }

    pub fn into_canonical_jcs(self) -> Bytes {
        self.canonical_jcs
    }
}

pub struct DownloadResult {
    pub total_encoded_len: u64,
    pub served_range: ByteRange,
    pub content_type: &'static str,
    pub body: DownloadStream,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum V3ServiceError {
    InvalidRequest { code: &'static str },
    Unauthorized,
    Forbidden,
    AuthorizationEpochChanged,
    IdempotencyDigestMismatch,
    Conflict { code: &'static str },
    RangeNotSatisfiable { total_encoded_len: u64 },
    TransferUnavailable,
    PayloadTooLarge { limit: u64 },
    LengthMismatch,
    BodyReadFailure,
    Unavailable,
    Internal,
}

#[async_trait]
pub trait V3Facade: Send + Sync {
    async fn projection_access(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space: &SpaceId,
        _request: layrs_sync::v3::ProjectionAccessRequest,
    ) -> Result<layrs_sync::v3::ProjectionAccess, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn submit_path_patch(
        &self,
        _principal: &AuthenticatedPrincipal,
        _submission: layrs_sync::v3::PathPatchSubmission,
    ) -> Result<layrs_sync::v3::PathPatchReceipt, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn begin_scoped_patch(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: layrs_sync::v3::ScopedPathPatchRequest,
    ) -> Result<layrs_sync::v3::ScopedPatchProgress, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn scoped_patch_progress(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space: &SpaceId,
        _session_id: &str,
    ) -> Result<layrs_sync::v3::ScopedPatchProgress, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn upload_scoped_patch_chunk(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space: &SpaceId,
        _session_id: &str,
        _file_index: usize,
        _ordinal: usize,
        _bytes: Vec<u8>,
    ) -> Result<(), V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn finalize_scoped_patch(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space: &SpaceId,
        _session_id: &str,
    ) -> Result<layrs_sync::v3::ScopedPathPatchReceipt, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn integrate_scoped_review(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space: &SpaceId,
        _review_id: &str,
        _request: layrs_sync::v3::ScopedReviewIntegrationRequest,
    ) -> Result<SyncTransactionReceipt, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn projection_page(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space: &SpaceId,
        _request: layrs_sync::v3::ProjectionPageRequest,
    ) -> Result<layrs_sync::v3::ProjectionPage, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn step_history(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space: &SpaceId,
        _request: layrs_store_server::v3::StepHistoryRequest,
    ) -> Result<layrs_store_server::v3::StepHistoryResponse, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn compare_steps(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space: &SpaceId,
        _request: layrs_store_server::v3::CompareStepsRequest,
    ) -> Result<layrs_store_server::v3::CompareStepsResponse, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
    async fn provision_space(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: layrs_store_server::v3::ProvisionSpaceRequest,
    ) -> Result<layrs_store_server::v3::ProvisionSpaceResponse, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn authenticate_bearer(
        &self,
        bearer_token: &str,
    ) -> Result<AuthenticatedPrincipal, V3ServiceError>;

    async fn bootstrap(
        &self,
        principal: &AuthenticatedPrincipal,
        space_id: &SpaceId,
    ) -> Result<SpaceBootstrap, V3ServiceError>;

    async fn object_plan(
        &self,
        principal: &AuthenticatedPrincipal,
        request: ObjectPlanRequest,
    ) -> Result<ObjectPlanResponse, V3ServiceError>;

    async fn fetch_plan(
        &self,
        principal: &AuthenticatedPrincipal,
        request: FetchPlanRequest,
    ) -> Result<FetchPlanResponse, V3ServiceError>;

    async fn transact(
        &self,
        principal: &AuthenticatedPrincipal,
        request: SyncTransactionRequest,
    ) -> Result<StoredTransactionReceipt, V3ServiceError>;

    async fn events(
        &self,
        principal: &AuthenticatedPrincipal,
        request: SpaceEventPageRequest,
    ) -> Result<SpaceEventPage, V3ServiceError>;

    async fn upload(
        &self,
        capability_token: &str,
        declared_content_length: u64,
        body: UploadStream,
    ) -> Result<UploadResult, V3ServiceError>;

    async fn download(
        &self,
        capability_token: &str,
        requested_range: Option<RequestedRange>,
    ) -> Result<DownloadResult, V3ServiceError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V3HttpLimits {
    pub object_plan_json_bytes: usize,
    pub fetch_plan_json_bytes: usize,
    pub transaction_json_bytes: usize,
    pub events_json_bytes: usize,
    pub upload_encoded_bytes: u64,
}

impl Default for V3HttpLimits {
    fn default() -> Self {
        Self {
            object_plan_json_bytes: 4 * 1024 * 1024,
            fetch_plan_json_bytes: 4 * 1024 * 1024,
            transaction_json_bytes: 16 * 1024 * 1024,
            events_json_bytes: 64 * 1024,
            upload_encoded_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone)]
pub struct V3HttpState {
    facade: Arc<dyn V3Facade>,
    limits: V3HttpLimits,
}

impl V3HttpState {
    pub fn new(facade: Arc<dyn V3Facade>, limits: V3HttpLimits) -> Self {
        Self { facade, limits }
    }

    pub fn facade(&self) -> &Arc<dyn V3Facade> {
        &self.facade
    }

    pub const fn limits(&self) -> V3HttpLimits {
        self.limits
    }
}

#[derive(Debug, Default)]
pub struct UnavailableV3Facade;

#[async_trait]
impl V3Facade for UnavailableV3Facade {
    async fn authenticate_bearer(
        &self,
        _bearer_token: &str,
    ) -> Result<AuthenticatedPrincipal, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn bootstrap(
        &self,
        _principal: &AuthenticatedPrincipal,
        _space_id: &SpaceId,
    ) -> Result<SpaceBootstrap, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn object_plan(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: ObjectPlanRequest,
    ) -> Result<ObjectPlanResponse, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn fetch_plan(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: FetchPlanRequest,
    ) -> Result<FetchPlanResponse, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn transact(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: SyncTransactionRequest,
    ) -> Result<StoredTransactionReceipt, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn events(
        &self,
        _principal: &AuthenticatedPrincipal,
        _request: SpaceEventPageRequest,
    ) -> Result<SpaceEventPage, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn upload(
        &self,
        _capability_token: &str,
        _declared_content_length: u64,
        _body: UploadStream,
    ) -> Result<UploadResult, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }

    async fn download(
        &self,
        _capability_token: &str,
        _requested_range: Option<RequestedRange>,
    ) -> Result<DownloadResult, V3ServiceError> {
        Err(V3ServiceError::Unavailable)
    }
}
