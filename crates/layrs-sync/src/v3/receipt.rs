use super::{
    EventId, IdempotencyKey, LayerMutationMode, MutationId, Nullable, PrincipalId, ProductRoot,
    SyncV3Protocol, TransactionId,
};
use layrs_core::scm::{
    LayerId, ObjectId, ObservationId, RepositoryViewId, SpaceId, StepId, Timestamp,
};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

pub const RECEIPT_STORAGE_DIGEST_DOMAIN: &str = "layrs.sync.v3/receipt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionReceiptType {
    #[serde(rename = "syncTransactionReceipt")]
    Receipt,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Applied,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewOutcome {
    Advanced,
    Integrated,
    Chosen,
    Mixed,
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerMutationResult {
    pub layer_id: LayerId,
    pub mode: LayerMutationMode,
    pub previous_heads: Vec<StepId>,
    pub proposed_heads: Vec<StepId>,
    pub resulting_heads: Vec<StepId>,
    pub divergence_created: bool,
    pub decision_product_object_id: Nullable<ObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryViewResult {
    pub mutation_id: MutationId,
    pub outcome: ViewOutcome,
    pub old_repository_view_id: RepositoryViewId,
    pub proposed_repository_view_id: RepositoryViewId,
    pub new_repository_view_id: RepositoryViewId,
    pub old_generation: u64,
    pub new_generation: u64,
    pub layer_results: Vec<LayerMutationResult>,
    pub event_id: Nullable<EventId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyRootOutcome {
    Changed,
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyRootResult {
    pub mutation_id: MutationId,
    pub outcome: PolicyRootOutcome,
    pub old_policy_set_id: ObjectId,
    pub proposed_policy_set_id: ObjectId,
    pub new_policy_set_id: ObjectId,
    pub old_authorization_epoch: u64,
    pub new_authorization_epoch: u64,
    pub event_id: Nullable<EventId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyDecision {
    pub operation: String,
    pub target_kind: String,
    pub target_id: String,
    pub allowed: bool,
    pub policy_version_object_ids: Vec<ObjectId>,
    pub required_proof_object_ids: Vec<ObjectId>,
    pub required_gate_evaluation_object_ids: Vec<ObjectId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    InvalidRequest,
    MissingObject,
    MissingParent,
    MissingFileManifest,
    FileReconstructionMismatch,
    NonTopologicalSteps,
    NonTopologicalProducts,
    DuplicateIdentity,
    ImmutableObjectMismatch,
    PermissionDenied,
    PolicyRejected,
    GateRejected,
    InvalidViewMutation,
    ViewGenerationConflict,
    AuthorizationEpochChanged,
    IncompatibleVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rejection {
    pub code: RejectionCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncTransactionReceipt {
    pub protocol: SyncV3Protocol,
    #[serde(rename = "type")]
    pub message_type: TransactionReceiptType,
    pub principal_id: PrincipalId,
    pub authorization_epoch: u64,
    pub resulting_authorization_epoch: u64,
    pub authorization_snapshot_id: ObjectId,
    pub current_policy_set_id: ObjectId,
    pub space_id: SpaceId,
    pub client_id: super::ClientId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: ObjectId,
    pub transaction_id: TransactionId,
    pub status: ReceiptStatus,
    pub accepted_step_ids: Vec<StepId>,
    pub accepted_observation_ids: Vec<ObservationId>,
    pub accepted_repository_view_ids: Vec<RepositoryViewId>,
    pub accepted_policy_set_ids: Vec<ObjectId>,
    pub accepted_products: Vec<ProductRoot>,
    pub view_result: Nullable<RepositoryViewResult>,
    pub policy_root_result: Nullable<PolicyRootResult>,
    pub policy_decisions: Vec<PolicyDecision>,
    pub rejections: Vec<Rejection>,
    pub event_ids: Vec<EventId>,
    pub first_event_sequence: Nullable<u64>,
    pub last_event_sequence: Nullable<u64>,
    pub event_cursor: Nullable<super::Cursor>,
    pub stored_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayRefusal {
    Unauthenticated,
    Revoked,
    PrincipalMismatch,
    ClientMismatch,
    SpaceMismatch,
}

impl ReplayRefusal {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Revoked => "revoked",
            Self::PrincipalMismatch => "principal_mismatch",
            Self::ClientMismatch => "client_mismatch",
            Self::SpaceMismatch => "space_mismatch",
        }
    }
}

impl fmt::Display for ReplayRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl Error for ReplayRefusal {}

impl SyncTransactionReceipt {
    /// Exact canonical bytes persisted for integrity and replay. This digest is
    /// storage metadata; it is not part of the receipt wire object.
    pub fn canonical_storage_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_jcs::to_vec(self)
    }

    pub fn computed_storage_digest(&self) -> Result<ObjectId, serde_json::Error> {
        let canonical = self.canonical_storage_bytes()?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(RECEIPT_STORAGE_DIGEST_DOMAIN.as_bytes());
        hasher.update(&[0]);
        hasher.update(&canonical);
        Ok(ObjectId::parse(format!("blake3:{}", hasher.finalize()))
            .expect("BLAKE3 is a valid ObjectId"))
    }

    pub fn verify_storage_digest(&self, expected: &ObjectId) -> Result<bool, serde_json::Error> {
        Ok(&self.computed_storage_digest()? == expected)
    }

    /// Replay authorization is independent from the current epoch/PolicySet:
    /// those values changing does not rewrite or invalidate a stored receipt.
    /// Revocation and identity binding always fail closed.
    pub fn authorize_replay(
        &self,
        authenticated_principal: Option<&PrincipalId>,
        client_id: &super::ClientId,
        space_id: &SpaceId,
        revoked: bool,
    ) -> Result<(), ReplayRefusal> {
        let principal = authenticated_principal.ok_or(ReplayRefusal::Unauthenticated)?;
        if revoked {
            return Err(ReplayRefusal::Revoked);
        }
        if principal != &self.principal_id {
            return Err(ReplayRefusal::PrincipalMismatch);
        }
        if client_id != &self.client_id {
            return Err(ReplayRefusal::ClientMismatch);
        }
        if space_id != &self.space_id {
            return Err(ReplayRefusal::SpaceMismatch);
        }
        Ok(())
    }
}
