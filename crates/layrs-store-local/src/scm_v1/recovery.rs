//! Durable technical recovery journal for native SCM V1.
//!
//! Recovery records are deliberately separate from the immutable Operation
//! DAG. Every transition is short, idempotent, and committed under
//! `BEGIN IMMEDIATE`; user-visible filesystem effects are delegated to an
//! evidence-driven callback implemented by later phases.

use super::{NativeCas, NativeCasWriter, NativeStoreExclusive};
use layrs_core::scm::{OperationId, WorktreeId, canonical_json};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

pub type RecoveryResult<T> = std::result::Result<T, RecoveryError>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RecoveryId(String);

impl RecoveryId {
    pub fn parse(value: impl Into<String>) -> RecoveryResult<Self> {
        let value = value.into();
        if !value.starts_with("recovery:")
            || value.len() > 255
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            })
        {
            return Err(RecoveryError::InvalidInput {
                field: "recovery_id",
                reason: "expected recovery:<ASCII token>, at most 255 bytes".into(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RecoveryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryType {
    Materialize,
    Restore,
    Quarantine,
    CasStagingCleanup,
}

impl RecoveryType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Materialize => "materialize",
            Self::Restore => "restore",
            Self::Quarantine => "quarantine",
            Self::CasStagingCleanup => "cas_staging_cleanup",
        }
    }

    fn parse(value: &str) -> RecoveryResult<Self> {
        match value {
            "materialize" => Ok(Self::Materialize),
            "restore" => Ok(Self::Restore),
            "quarantine" => Ok(Self::Quarantine),
            "cas_staging_cleanup" => Ok(Self::CasStagingCleanup),
            other => Err(RecoveryError::Corruption(format!(
                "unknown recovery type {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
    Pending,
    Prepared,
    Applying,
    Completed,
    Failed,
}

impl RecoveryState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Prepared => "prepared",
            Self::Applying => "applying",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> RecoveryResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "prepared" => Ok(Self::Prepared),
            "applying" => Ok(Self::Applying),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(RecoveryError::Corruption(format!(
                "unknown recovery state {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPathRole {
    Source,
    Destination,
    Temporary,
    Quarantine,
    Materialized,
}

impl RecoveryPathRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Destination => "destination",
            Self::Temporary => "temporary",
            Self::Quarantine => "quarantine",
            Self::Materialized => "materialized",
        }
    }

    fn parse(value: &str) -> RecoveryResult<Self> {
        match value {
            "source" => Ok(Self::Source),
            "destination" => Ok(Self::Destination),
            "temporary" => Ok(Self::Temporary),
            "quarantine" => Ok(Self::Quarantine),
            "materialized" => Ok(Self::Materialized),
            other => Err(RecoveryError::Corruption(format!(
                "unknown recovery path role {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPathState {
    Pending,
    Verified,
    Applied,
    RolledBack,
    Quarantined,
}

impl RecoveryPathState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Verified => "verified",
            Self::Applied => "applied",
            Self::RolledBack => "rolled_back",
            Self::Quarantined => "quarantined",
        }
    }

    const fn is_terminal(self) -> bool {
        matches!(self, Self::Applied | Self::RolledBack | Self::Quarantined)
    }

    fn parse(value: &str) -> RecoveryResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "verified" => Ok(Self::Verified),
            "applied" => Ok(Self::Applied),
            "rolled_back" => Ok(Self::RolledBack),
            "quarantined" => Ok(Self::Quarantined),
            other => Err(RecoveryError::Corruption(format!(
                "unknown recovery path state {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PathKey {
    path: String,
    role: RecoveryPathRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPathPlan {
    path: String,
    role: RecoveryPathRole,
    expected_object_id: Option<String>,
}

impl RecoveryPathPlan {
    pub fn new(
        path: impl Into<String>,
        role: RecoveryPathRole,
        expected_object_id: Option<String>,
    ) -> RecoveryResult<Self> {
        let path = canonical_repo_path(path.into())?;
        if expected_object_id
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.contains('\0'))
        {
            return Err(RecoveryError::InvalidInput {
                field: "expected_object_id",
                reason: "must be non-empty and NUL-free".into(),
            });
        }
        Ok(Self {
            path,
            role,
            expected_object_id,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn role(&self) -> RecoveryPathRole {
        self.role
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPlan {
    id: RecoveryId,
    idempotency_key: String,
    request_digest: String,
    worktree_id: WorktreeId,
    operation_id: Option<OperationId>,
    recovery_type: RecoveryType,
    payload_json: String,
    paths: Vec<RecoveryPathPlan>,
    created_at_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryPlanDigest<'a> {
    id: &'a RecoveryId,
    idempotency_key: &'a str,
    worktree_id: &'a WorktreeId,
    operation_id: &'a Option<OperationId>,
    recovery_type: RecoveryType,
    payload: &'a Value,
    paths: &'a [RecoveryPathPlan],
    created_at_ms: i64,
}

impl RecoveryPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: RecoveryId,
        idempotency_key: impl Into<String>,
        worktree_id: WorktreeId,
        operation_id: Option<OperationId>,
        recovery_type: RecoveryType,
        payload: Value,
        mut paths: Vec<RecoveryPathPlan>,
        created_at_ms: i64,
    ) -> RecoveryResult<Self> {
        let idempotency_key = validated_token("idempotency_key", idempotency_key.into())?;
        valid_time("created_at_ms", created_at_ms)?;
        paths.sort_by(|left, right| {
            (left.path.as_str(), left.role).cmp(&(right.path.as_str(), right.role))
        });
        if paths
            .windows(2)
            .any(|pair| pair[0].path == pair[1].path && pair[0].role == pair[1].role)
        {
            return Err(RecoveryError::InvalidInput {
                field: "paths",
                reason: "duplicate path and role".into(),
            });
        }
        if recovery_type != RecoveryType::CasStagingCleanup && paths.is_empty() {
            return Err(RecoveryError::InvalidInput {
                field: "paths",
                reason: "non-CAS recovery requires at least one path".into(),
            });
        }
        let payload_json = canonical_text(&payload)?;
        let digest_payload = RecoveryPlanDigest {
            id: &id,
            idempotency_key: &idempotency_key,
            worktree_id: &worktree_id,
            operation_id: &operation_id,
            recovery_type,
            payload: &payload,
            paths: &paths,
            created_at_ms,
        };
        let digest_bytes = canonical_json(&digest_payload).map_err(json_error)?;
        let request_digest = format!("blake3:{}", blake3::hash(&digest_bytes).to_hex());
        Ok(Self {
            id,
            idempotency_key,
            request_digest,
            worktree_id,
            operation_id,
            recovery_type,
            payload_json,
            paths,
            created_at_ms,
        })
    }

    pub fn id(&self) -> &RecoveryId {
        &self.id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEvidence {
    path: String,
    role: RecoveryPathRole,
    canonical_json: String,
}

impl PathEvidence {
    pub fn new(
        path: impl Into<String>,
        role: RecoveryPathRole,
        evidence: Value,
    ) -> RecoveryResult<Self> {
        Ok(Self {
            path: canonical_repo_path(path.into())?,
            role,
            canonical_json: required_evidence(evidence)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPreparation {
    evidence_json: String,
    path_evidence: Vec<PathEvidence>,
}

impl RecoveryPreparation {
    pub fn new(evidence: Value, mut path_evidence: Vec<PathEvidence>) -> RecoveryResult<Self> {
        path_evidence.sort_by(|left, right| {
            (left.path.as_str(), left.role).cmp(&(right.path.as_str(), right.role))
        });
        if path_evidence
            .windows(2)
            .any(|pair| pair[0].path == pair[1].path && pair[0].role == pair[1].role)
        {
            return Err(RecoveryError::InvalidInput {
                field: "path_evidence",
                reason: "duplicate path and role".into(),
            });
        }
        Ok(Self {
            evidence_json: required_evidence(evidence)?,
            path_evidence,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPathRecord {
    pub path: String,
    pub role: RecoveryPathRole,
    pub expected_object_id: Option<String>,
    pub state: RecoveryPathState,
    pub evidence: Value,
}

/// Evidence-backed terminal result for one and only one planned path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPathResult {
    path: String,
    role: RecoveryPathRole,
    state: RecoveryPathState,
    evidence_json: String,
}

impl RecoveryPathResult {
    pub fn new(
        path: impl Into<String>,
        role: RecoveryPathRole,
        state: RecoveryPathState,
        evidence: Value,
    ) -> RecoveryResult<Self> {
        if !state.is_terminal() {
            return Err(RecoveryError::InvalidInput {
                field: "path_state",
                reason: "expected applied, rolled_back, or quarantined".into(),
            });
        }
        Ok(Self {
            path: canonical_repo_path(path.into())?,
            role,
            state,
            evidence_json: required_evidence(evidence)?,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn role(&self) -> RecoveryPathRole {
        self.role
    }

    pub const fn state(&self) -> RecoveryPathState {
        self.state
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRecord {
    pub id: RecoveryId,
    pub idempotency_key: String,
    pub request_digest: String,
    pub worktree_id: WorktreeId,
    pub operation_id: Option<OperationId>,
    pub recovery_type: RecoveryType,
    pub state: RecoveryState,
    pub payload: Value,
    pub evidence: Value,
    pub paths: Vec<RecoveryPathRecord>,
    pub attempt_count: u64,
    pub owner_token: Option<String>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOutcome {
    Applied,
    Replayed,
}

/// Typed convenience boundary for the non-claim state transitions.
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryTransition {
    Prepare(RecoveryPreparation),
    Complete {
        owner_token: String,
        expected_leases: Vec<ResourceLease>,
        evidence: Value,
        path_results: Vec<RecoveryPathResult>,
    },
    Fail {
        owner_token: String,
        expected_leases: Vec<ResourceLease>,
        evidence: Value,
        error: String,
        path_results: Vec<RecoveryPathResult>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLease {
    pub resource_key: String,
    pub generation: u64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimOutcome {
    pub outcome: MutationOutcome,
    pub leases: Vec<ResourceLease>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryFaultPoint {
    AfterCreateCommit,
    AfterPrepareCommit,
    AfterClaimCommit,
    BeforeHandler,
    AfterHandler,
    BeforeTerminalCommit,
    AfterTerminalCommit,
    BeforeCasStagingRecovery,
    AfterCasStagingRecovery,
}

pub trait RecoveryFaultInjector: Send + Sync {
    fn check(&self, point: RecoveryFaultPoint) -> std::result::Result<(), String>;
}

#[derive(Debug, Default)]
struct NoFaults;

impl RecoveryFaultInjector for NoFaults {
    fn check(&self, _: RecoveryFaultPoint) -> std::result::Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryDecision {
    Complete {
        evidence: Value,
        path_results: Vec<RecoveryPathResult>,
    },
    Fail {
        evidence: Value,
        error: String,
        path_results: Vec<RecoveryPathResult>,
    },
    RecoverCasStaging {
        evidence: Value,
    },
}

pub trait RecoveryHandler {
    fn recover(&mut self, record: &RecoveryRecord) -> RecoveryResult<RecoveryDecision>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryFindingCode {
    MissingEvidence,
    LeaseFenced,
    FailedNotRetryable,
    HandlerFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryFinding {
    pub recovery_id: RecoveryId,
    pub code: RecoveryFindingCode,
    pub message: String,
}

/// Maximum encoded UTF-8 size of any finding message emitted by recovery.
///
/// Handler errors are untrusted application text, so reports retain a bounded
/// prefix without ever splitting a UTF-8 code point.
pub const MAX_FINDING_MESSAGE_BYTES: usize = 4 * 1024;

/// Maximum number of individual findings retained in one recovery report.
pub const MAX_RECOVERY_FINDINGS: usize = 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub completed: u64,
    pub failed: u64,
    pub skipped: u64,
    pub findings: Vec<RecoveryFinding>,
    pub truncated_findings: u64,
}

impl RecoveryReport {
    pub fn is_clean(&self) -> bool {
        self.failed == 0 && self.findings.is_empty()
    }

    fn finding(&mut self, finding: RecoveryFinding) {
        if self.findings.len() < MAX_RECOVERY_FINDINGS {
            self.findings.push(finding);
        } else {
            self.truncated_findings = self.truncated_findings.saturating_add(1);
        }
    }
}

#[derive(Debug)]
pub enum RecoveryError {
    Sqlite(rusqlite::Error),
    Schema(super::ScmSchemaError),
    Cas(super::CasError),
    InvalidInput {
        field: &'static str,
        reason: String,
    },
    NotFound(RecoveryId),
    ReplayMismatch {
        idempotency_key: String,
    },
    InvalidTransition {
        from: RecoveryState,
        to: RecoveryState,
    },
    MissingEvidence(RecoveryId),
    LeaseFenced {
        worktree_id: WorktreeId,
        resource_key: String,
    },
    OwnerMismatch(RecoveryId),
    Corruption(String),
    InjectedFault {
        point: RecoveryFaultPoint,
        message: String,
    },
    Handler(String),
}

impl RecoveryError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "recovery_sqlite_error",
            Self::Schema(_) => "recovery_schema_error",
            Self::Cas(_) => "recovery_cas_error",
            Self::InvalidInput { .. } => "recovery_invalid_input",
            Self::NotFound(_) => "recovery_not_found",
            Self::ReplayMismatch { .. } => "recovery_replay_mismatch",
            Self::InvalidTransition { .. } => "recovery_invalid_transition",
            Self::MissingEvidence(_) => "recovery_missing_evidence",
            Self::LeaseFenced { .. } => "recovery_lease_fenced",
            Self::OwnerMismatch(_) => "recovery_owner_mismatch",
            Self::Corruption(_) => "recovery_corruption",
            Self::InjectedFault { .. } => "recovery_injected_fault",
            Self::Handler(_) => "recovery_handler_error",
        }
    }
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "recovery SQLite error: {error}"),
            Self::Schema(error) => write!(formatter, "recovery schema error: {error}"),
            Self::Cas(error) => write!(formatter, "recovery CAS error: {error}"),
            Self::InvalidInput { field, reason } => {
                write!(formatter, "invalid recovery {field}: {reason}")
            }
            Self::NotFound(id) => write!(formatter, "recovery {id} was not found"),
            Self::ReplayMismatch { idempotency_key } => write!(
                formatter,
                "recovery idempotency key {idempotency_key} was replayed with another request"
            ),
            Self::InvalidTransition { from, to } => {
                write!(formatter, "invalid recovery transition {from:?} -> {to:?}")
            }
            Self::MissingEvidence(id) => write!(formatter, "recovery {id} has no durable evidence"),
            Self::LeaseFenced {
                worktree_id,
                resource_key,
            } => write!(
                formatter,
                "recovery resource {resource_key} is actively fenced for {worktree_id}"
            ),
            Self::OwnerMismatch(id) => {
                write!(formatter, "recovery {id} is owned by another claimant")
            }
            Self::Corruption(message) => {
                write!(formatter, "recovery journal corruption: {message}")
            }
            Self::InjectedFault { point, message } => {
                write!(formatter, "injected recovery fault at {point:?}: {message}")
            }
            Self::Handler(message) => write!(formatter, "recovery handler failed: {message}"),
        }
    }
}

impl Error for RecoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Schema(error) => Some(error),
            Self::Cas(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for RecoveryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<super::ScmSchemaError> for RecoveryError {
    fn from(error: super::ScmSchemaError) -> Self {
        Self::Schema(error)
    }
}

impl From<super::CasError> for RecoveryError {
    fn from(error: super::CasError) -> Self {
        Self::Cas(error)
    }
}

pub struct NativeRecovery<'store> {
    connection: Connection,
    cas: NativeCasWriter<'store>,
    faults: Arc<dyn RecoveryFaultInjector>,
}

mod records;
mod writer;
use records::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_collection_has_a_hard_count_cap() {
        let mut report = RecoveryReport::default();
        for index in 0..(MAX_RECOVERY_FINDINGS + 7) {
            report.finding(RecoveryFinding {
                recovery_id: RecoveryId::parse(format!("recovery:finding-{index}")).unwrap(),
                code: RecoveryFindingCode::HandlerFailed,
                message: "bounded".into(),
            });
        }
        assert_eq!(report.findings.len(), MAX_RECOVERY_FINDINGS);
        assert_eq!(report.truncated_findings, 7);
    }

    #[test]
    fn finding_message_truncation_preserves_utf8() {
        let message = bounded_finding_message("界".repeat(MAX_FINDING_MESSAGE_BYTES));
        assert!(message.len() <= MAX_FINDING_MESSAGE_BYTES);
        assert!(message.is_char_boundary(message.len()));
    }
}
