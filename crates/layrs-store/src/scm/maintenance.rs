use super::{Result, StoreObjectId, TransactionId};
use layrs_core::scm::{OperationId, SpaceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReportSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecoveryFindingCode {
    IncompleteTechnicalTransaction,
    PublishedTransactionMissingJournalCommit,
    JournalEntryUnreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryFinding {
    code: RecoveryFindingCode,
    severity: ReportSeverity,
    transaction_id: Option<TransactionId>,
    message: String,
}

impl RecoveryFinding {
    pub fn new(
        code: RecoveryFindingCode,
        severity: ReportSeverity,
        transaction_id: Option<TransactionId>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity,
            transaction_id,
            message: message.into(),
        }
    }

    pub fn code(&self) -> RecoveryFindingCode {
        self.code
    }

    pub fn severity(&self) -> ReportSeverity {
        self.severity
    }

    pub fn transaction_id(&self) -> Option<&TransactionId> {
        self.transaction_id.as_ref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    findings: Vec<RecoveryFinding>,
}

impl RecoveryReport {
    pub fn new(findings: Vec<RecoveryFinding>) -> Self {
        Self { findings }
    }

    pub fn findings(&self) -> &[RecoveryFinding] {
        &self.findings
    }

    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FsckFindingCode {
    MissingObject,
    IdentityMismatch,
    BrokenClosure,
    InvalidAdmission,
    InvalidOperationDag,
    CurrentReferenceMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsckFinding {
    code: FsckFindingCode,
    severity: ReportSeverity,
    space_id: Option<SpaceId>,
    object_id: Option<StoreObjectId>,
    operation_id: Option<OperationId>,
    message: String,
}

impl FsckFinding {
    pub fn new(
        code: FsckFindingCode,
        severity: ReportSeverity,
        space_id: Option<SpaceId>,
        object_id: Option<StoreObjectId>,
        operation_id: Option<OperationId>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity,
            space_id,
            object_id,
            operation_id,
            message: message.into(),
        }
    }

    pub fn code(&self) -> FsckFindingCode {
        self.code
    }

    pub fn severity(&self) -> ReportSeverity {
        self.severity
    }

    pub fn space_id(&self) -> Option<&SpaceId> {
        self.space_id.as_ref()
    }

    pub fn object_id(&self) -> Option<&StoreObjectId> {
        self.object_id.as_ref()
    }

    pub fn operation_id(&self) -> Option<&OperationId> {
        self.operation_id.as_ref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FsckReport {
    findings: Vec<FsckFinding>,
}

impl FsckReport {
    pub fn new(findings: Vec<FsckFinding>) -> Self {
        Self { findings }
    }

    pub fn findings(&self) -> &[FsckFinding] {
        &self.findings
    }

    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Read-only inspection of technical recovery evidence and logical integrity.
///
/// These methods report facts only. They do not repair, delete journal data,
/// change current references, or create user-visible SCM Operations.
pub trait Maintenance: Send + Sync {
    fn recover(&self) -> Result<RecoveryReport>;
    fn fsck(&self) -> Result<FsckReport>;
}
