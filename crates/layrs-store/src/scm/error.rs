use super::{EntityKind, StoreObjectKind, TransactionId};
use layrs_core::scm::{OperationId, SpaceId};
use std::error::Error;
use std::fmt;

pub type Result<T> = std::result::Result<T, StoreError>;

/// Stable error boundary shared by every native SCM store backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    InvalidTransactionId {
        reason: String,
    },
    EmptyBatch,
    DuplicateObject {
        kind: StoreObjectKind,
        id: String,
    },
    DuplicateEntity {
        kind: EntityKind,
        id: String,
    },
    DuplicateAdmission {
        space_id: String,
        object_id: String,
    },
    DuplicateMutableTarget {
        target: String,
    },
    NonCanonicalSet {
        field: &'static str,
    },
    InvalidCas {
        target: String,
        reason: String,
    },
    OperationHeadsCasMissingOperation {
        space_id: SpaceId,
        operation_id: OperationId,
    },
    OperationHeadsCasSpaceMismatch {
        operation_id: OperationId,
        expected_space_id: SpaceId,
        actual_space_id: SpaceId,
    },
    InvalidOperationHeadsTransition {
        operation_id: OperationId,
        reason: String,
    },
    OperationPublicationMissingViewCas {
        space_id: SpaceId,
    },
    RepositoryViewCasMissingOperationPublication {
        space_id: SpaceId,
    },
    OperationPublicationViewMismatch {
        operation_id: OperationId,
        published_view_id: String,
        cas_view_id: String,
    },
    InvalidConflictDocument {
        reason: String,
    },
    NonCanonicalConflict,
    ConflictIdentityMismatch {
        claimed: String,
        computed: String,
    },
    Serialization {
        message: String,
    },
    NotFound {
        kind: &'static str,
        id: String,
    },
    AdmissionRejected {
        kind: &'static str,
        id: String,
    },
    AdmissionBudgetExceeded {
        resource: &'static str,
        limit: usize,
    },
    CasMismatch {
        target: String,
    },
    TransactionReplayMismatch {
        transaction_id: TransactionId,
    },
    Corruption {
        message: String,
    },
    Backend {
        message: String,
    },
}

impl StoreError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidTransactionId { .. } => "invalid_transaction_id",
            Self::EmptyBatch => "empty_batch",
            Self::DuplicateObject { .. } => "duplicate_object",
            Self::DuplicateEntity { .. } => "duplicate_entity",
            Self::DuplicateAdmission { .. } => "duplicate_admission",
            Self::DuplicateMutableTarget { .. } => "duplicate_mutable_target",
            Self::NonCanonicalSet { .. } => "non_canonical_set",
            Self::InvalidCas { .. } => "invalid_cas",
            Self::OperationHeadsCasMissingOperation { .. } => {
                "operation_heads_cas_missing_operation"
            }
            Self::OperationHeadsCasSpaceMismatch { .. } => "operation_heads_cas_space_mismatch",
            Self::InvalidOperationHeadsTransition { .. } => "invalid_operation_heads_transition",
            Self::OperationPublicationMissingViewCas { .. } => {
                "operation_publication_missing_view_cas"
            }
            Self::RepositoryViewCasMissingOperationPublication { .. } => {
                "repository_view_cas_missing_operation_publication"
            }
            Self::OperationPublicationViewMismatch { .. } => "operation_publication_view_mismatch",
            Self::InvalidConflictDocument { .. } => "invalid_conflict_document",
            Self::NonCanonicalConflict => "non_canonical_conflict",
            Self::ConflictIdentityMismatch { .. } => "conflict_identity_mismatch",
            Self::Serialization { .. } => "serialization_error",
            Self::NotFound { .. } => "not_found",
            Self::AdmissionRejected { .. } => "admission_rejected",
            Self::AdmissionBudgetExceeded { .. } => "admission_budget_exceeded",
            Self::CasMismatch { .. } => "cas_mismatch",
            Self::TransactionReplayMismatch { .. } => "transaction_replay_mismatch",
            Self::Corruption { .. } => "corruption",
            Self::Backend { .. } => "backend_error",
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTransactionId { reason } => {
                write!(formatter, "invalid transaction id: {reason}")
            }
            Self::EmptyBatch => formatter.write_str("a WriteBatch cannot be empty"),
            Self::DuplicateObject { kind, id } => write!(formatter, "duplicate {kind} object {id}"),
            Self::DuplicateEntity { kind, id } => write!(formatter, "duplicate {kind} entity {id}"),
            Self::DuplicateAdmission {
                space_id,
                object_id,
            } => write!(
                formatter,
                "duplicate admission of {object_id} to {space_id}"
            ),
            Self::DuplicateMutableTarget { target } => {
                write!(formatter, "mutable target {target} appears more than once")
            }
            Self::NonCanonicalSet { field } => write!(
                formatter,
                "{field} must be strictly sorted and duplicate-free"
            ),
            Self::InvalidCas { target, reason } => {
                write!(formatter, "invalid CAS for {target}: {reason}")
            }
            Self::OperationHeadsCasMissingOperation {
                space_id,
                operation_id,
            } => write!(
                formatter,
                "Operation heads CAS for {space_id} names missing batch Operation {operation_id}"
            ),
            Self::OperationHeadsCasSpaceMismatch {
                operation_id,
                expected_space_id,
                actual_space_id,
            } => write!(
                formatter,
                "Operation {operation_id} belongs to {actual_space_id}, not CAS Space {expected_space_id}"
            ),
            Self::InvalidOperationHeadsTransition {
                operation_id,
                reason,
            } => write!(
                formatter,
                "invalid Operation head transition for {operation_id}: {reason}"
            ),
            Self::OperationPublicationMissingViewCas { space_id } => write!(
                formatter,
                "Operation publication for {space_id} requires exactly one RepositoryView CAS"
            ),
            Self::RepositoryViewCasMissingOperationPublication { space_id } => write!(
                formatter,
                "RepositoryView CAS for {space_id} requires exactly one Operation publication"
            ),
            Self::OperationPublicationViewMismatch {
                operation_id,
                published_view_id,
                cas_view_id,
            } => write!(
                formatter,
                "Operation {operation_id} publishes RepositoryView {published_view_id}, not CAS view {cas_view_id}"
            ),
            Self::InvalidConflictDocument { reason } => {
                write!(formatter, "invalid canonical Conflict document: {reason}")
            }
            Self::NonCanonicalConflict => formatter.write_str("Conflict document is not exact JCS"),
            Self::ConflictIdentityMismatch { claimed, computed } => write!(
                formatter,
                "Conflict identity mismatch: claimed {claimed}, computed {computed}"
            ),
            Self::Serialization { message } => {
                write!(formatter, "batch serialization failed: {message}")
            }
            Self::NotFound { kind, id } => write!(formatter, "{kind} {id} was not found"),
            Self::AdmissionRejected { kind, id } => {
                write!(formatter, "admission rejected for {kind} {id}")
            }
            Self::AdmissionBudgetExceeded { resource, limit } => {
                write!(formatter, "admission {resource} exceeds limit {limit}")
            }
            Self::CasMismatch { target } => {
                write!(formatter, "compare-and-swap mismatch for {target}")
            }
            Self::TransactionReplayMismatch { transaction_id } => write!(
                formatter,
                "transaction {transaction_id} was replayed with different request bytes"
            ),
            Self::Corruption { message } => {
                write!(formatter, "store corruption detected: {message}")
            }
            Self::Backend { message } => write!(formatter, "store backend failed: {message}"),
        }
    }
}

impl Error for StoreError {}
