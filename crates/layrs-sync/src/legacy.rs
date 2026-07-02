use crate::requests::IdempotencyKey;
use crate::validation::{SyncResult, SyncValidationError, validate_digest, validate_non_empty};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChunkId(pub String);

impl ChunkId {
    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        validate_non_empty("chunk_id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ContentDigest(pub String);

impl ContentDigest {
    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        validate_digest(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkRef {
    pub chunk_id: ChunkId,
    pub digest: ContentDigest,
    pub byte_len: u64,
    pub media_type: Option<String>,
    pub compression: Option<String>,
    pub encryption: Option<String>,
}

impl ChunkRef {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("chunk_id", self.chunk_id.as_str())?;
        validate_digest(self.digest.as_str())?;
        if self.byte_len == 0 {
            return Err(SyncValidationError::new(
                "byte_len",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncManifest {
    pub manifest_id: String,
    pub workspace_id: String,
    pub space_id: Option<String>,
    pub source_client_id: String,
    pub base_cursor: Option<String>,
    pub capability_epoch: u64,
    pub generated_at: String,
    pub chunks: Vec<ChunkRef>,
    pub operations: Vec<SyncOperationRef>,
}

impl SyncManifest {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("manifest_id", &self.manifest_id)?;
        validate_non_empty("workspace_id", &self.workspace_id)?;
        validate_non_empty("source_client_id", &self.source_client_id)?;
        validate_non_empty("generated_at", &self.generated_at)?;
        if let Some(space_id) = &self.space_id {
            validate_non_empty("space_id", space_id)?;
        }
        for chunk in &self.chunks {
            chunk.validate()?;
        }
        for operation in &self.operations {
            operation.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncOperationRef {
    pub operation_id: String,
    pub entity_kind: SyncEntityKind,
    pub entity_id: String,
    pub operation_kind: SyncOperationKind,
    pub client_sequence: u64,
    pub base_version: Option<String>,
    pub resulting_version: String,
    pub chunks: Vec<ChunkId>,
}

impl SyncOperationRef {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("operation_id", &self.operation_id)?;
        validate_non_empty("entity_id", &self.entity_id)?;
        validate_non_empty("resulting_version", &self.resulting_version)?;
        if self.client_sequence == 0 {
            return Err(SyncValidationError::new(
                "client_sequence",
                "must be greater than zero",
            ));
        }
        for chunk_id in &self.chunks {
            validate_non_empty("chunk_id", chunk_id.as_str())?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SyncEntityKind {
    Workspace,
    Team,
    Space,
    Layer,
    Artifact,
    Weave,
    Proof,
    Policy,
    TimelineEvent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SyncOperationKind {
    Create,
    Update,
    Delete,
    AttachChunk,
    DetachChunk,
    Reorder,
    Decision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishRequest {
    pub idempotency_key: IdempotencyKey,
    pub manifest: SyncManifest,
    pub expected_server_cursor: Option<String>,
    pub dry_run: bool,
}

impl PublishRequest {
    pub fn validate(&self) -> SyncResult<()> {
        IdempotencyKey::validate_raw(self.idempotency_key.as_str())?;
        self.manifest.validate()?;
        if let Some(cursor) = &self.expected_server_cursor {
            validate_non_empty("expected_server_cursor", cursor)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiveRequest {
    pub workspace_id: String,
    pub space_id: Option<String>,
    pub since_cursor: Option<String>,
    pub client_id: String,
    pub max_chunks: u32,
}

impl ReceiveRequest {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("workspace_id", &self.workspace_id)?;
        validate_non_empty("client_id", &self.client_id)?;
        if let Some(space_id) = &self.space_id {
            validate_non_empty("space_id", space_id)?;
        }
        if let Some(cursor) = &self.since_cursor {
            validate_non_empty("since_cursor", cursor)?;
        }
        if self.max_chunks == 0 {
            return Err(SyncValidationError::new(
                "max_chunks",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncDecision {
    Accepted {
        server_cursor: String,
        accepted_operations: usize,
        missing_chunks: Vec<ChunkId>,
    },
    AlreadyApplied {
        server_cursor: String,
    },
    Conflict {
        server_cursor: String,
        conflicts: Vec<SyncConflict>,
    },
    Rejected {
        reason: SyncRejectionReason,
        message: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncConflict {
    pub entity_kind: SyncEntityKind,
    pub entity_id: String,
    pub client_operation_id: String,
    pub server_version: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SyncRejectionReason {
    PermissionDenied,
    PolicyRejected,
    MissingBase,
    MissingChunk,
    StaleCursor,
    InvalidManifest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishReceipt {
    pub idempotency_key: IdempotencyKey,
    pub decision: SyncDecision,
    pub received_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiveResponse {
    pub manifest: SyncManifest,
    pub server_cursor: String,
    pub has_more: bool,
}
