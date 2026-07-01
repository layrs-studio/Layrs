use crate::ids::{ApiChunkId, SpaceId, WorkspaceId};
use crate::validation::{ApiResult, Validate, required};
use layrs_sync::{
    ChunkObject, ChunkRef as SyncChunkRef, ContentDigest, IdempotencyKey, ObjectDigest,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReserveChunkRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: Option<SpaceId>,
    pub digest: String,
    pub byte_len: u64,
    pub media_type: Option<String>,
}

impl Validate for ReserveChunkRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        if let Some(space_id) = &self.space_id {
            space_id.validate_field("space_id")?;
        }
        required("digest", &self.digest)?;
        if ContentDigest::new(self.digest.clone()).is_err() {
            return Err(crate::validation::ValidationError::new(
                "digest",
                "must include an algorithm prefix",
            ));
        }
        if self.byte_len == 0 {
            return Err(crate::validation::ValidationError::new(
                "byte_len",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitChunkRequest {
    pub workspace_id: WorkspaceId,
    pub idempotency_key: IdempotencyKey,
    pub chunk: SyncChunkRef,
}

impl Validate for CommitChunkRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        if IdempotencyKey::validate_raw(self.idempotency_key.as_str()).is_err() {
            return Err(crate::validation::ValidationError::new(
                "idempotency_key",
                "is invalid",
            ));
        }
        if self.chunk.validate().is_err() {
            return Err(crate::validation::ValidationError::new(
                "chunk",
                "is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkResponse {
    pub workspace_id: WorkspaceId,
    pub chunk_id: ApiChunkId,
    pub digest: String,
    pub byte_len: u64,
    pub upload_required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReserveChunkV2Request {
    pub workspace_id: WorkspaceId,
    pub space_id: Option<SpaceId>,
    pub chunk_id: ObjectDigest,
    pub digest: ObjectDigest,
    pub byte_len: u64,
    pub compression: Option<String>,
    pub encryption: Option<String>,
}

impl Validate for ReserveChunkV2Request {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        if let Some(space_id) = &self.space_id {
            space_id.validate_field("space_id")?;
        }
        if ObjectDigest::new(self.chunk_id.as_str()).is_err() {
            return Err(crate::validation::ValidationError::new(
                "chunk_id",
                "must use blake3 digest format",
            ));
        }
        if ObjectDigest::new(self.digest.as_str()).is_err() {
            return Err(crate::validation::ValidationError::new(
                "digest",
                "must use blake3 digest format",
            ));
        }
        if self.chunk_id != self.digest {
            return Err(crate::validation::ValidationError::new(
                "chunk_id",
                "must match digest for raw chunk objects",
            ));
        }
        if self.byte_len == 0 {
            return Err(crate::validation::ValidationError::new(
                "byte_len",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitChunkV2Request {
    pub workspace_id: WorkspaceId,
    pub idempotency_key: IdempotencyKey,
    pub chunk: ChunkObject,
}

impl Validate for CommitChunkV2Request {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        if IdempotencyKey::validate_raw(self.idempotency_key.as_str()).is_err() {
            return Err(crate::validation::ValidationError::new(
                "idempotency_key",
                "is invalid",
            ));
        }
        if self.chunk.validate().is_err() {
            return Err(crate::validation::ValidationError::new(
                "chunk",
                "is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkV2Response {
    pub workspace_id: WorkspaceId,
    pub space_id: Option<SpaceId>,
    pub chunk_id: ObjectDigest,
    pub digest: ObjectDigest,
    pub byte_len: u64,
    pub upload_required: bool,
}
