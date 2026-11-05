//! Invocation-local write evidence. None of these DATA values authorizes recovery.
use super::*;
use std::{error::Error, fmt, path::PathBuf, sync::Arc};

#[derive(Debug, Clone)]
pub struct ProviderBootstrapAttempt {
    pub family_root: PathBuf,
    pub checkout_root: PathBuf,
    pub stage_parent: PathBuf,
    pub worktree_id: WorktreeId,
    pub transaction_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapWriterOperation {
    Begin,
    ResumeControlPublication,
    Abandon,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapWriteReportProvenance {
    CurrentInvocation {
        operation: ProviderBootstrapWriterOperation,
    },
    RetainedIssuance {
        operation: ProviderBootstrapWriterOperation,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapReceipt {
    Allocation,
    Prepared,
    Abandoned,
}
impl ProviderBootstrapReceipt {
    pub(super) fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Allocation => ("allocation", "allocation.json"),
            Self::Prepared => ("prepared", "prepared.json"),
            Self::Abandoned => ("abandoned", "abandoned.json"),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapCreate {
    Member,
    Metadata,
    OwnerLock,
    ReceiptDirectory,
    PrivateDatabase,
    ControlStage,
    Format,
    ReceiptTemp(ProviderBootstrapReceipt),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapSqlWrite {
    RegisterInitializing,
    InitializePrivate,
    PrepareControl,
    PublishControl,
    RemoveInitializing,
}
impl ProviderBootstrapSqlWrite {
    pub(super) fn scope(self) -> ProviderDatabaseScope {
        match self {
            Self::RegisterInitializing | Self::RemoveInitializing => {
                ProviderDatabaseScope::FamilyCatalog
            }
            _ => ProviderDatabaseScope::PrivateMetadata,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapWriteStep {
    ExclusiveCreate(ProviderBootstrapCreate),
    ReceiptWrite(ProviderBootstrapReceipt),
    ReceiptRename(ProviderBootstrapReceipt),
    ReceiptFinalize(ProviderBootstrapReceipt),
    FormatWrite,
    SqlOpen(ProviderBootstrapSqlWrite),
    SqlTransaction(ProviderBootstrapSqlWrite),
    SqlCommit(ProviderBootstrapSqlWrite),
    SqlClose(ProviderBootstrapSqlWrite),
    ControlPublish,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapWriteState {
    NoNewEffect,
    Uncertain,
    Confirmed,
}
#[derive(Debug, Clone)]
pub struct ProviderBootstrapWriteEffect {
    pub step: ProviderBootstrapWriteStep,
    pub target: PathBuf,
    pub state: ProviderBootstrapWriteState,
}
#[derive(Debug, Clone)]
pub struct ProviderBootstrapWriteReport {
    pub provenance: ProviderBootstrapWriteReportProvenance,
    pub attempt: ProviderBootstrapAttempt,
    pub effects: Vec<ProviderBootstrapWriteEffect>,
}
impl ProviderBootstrapWriteReport {
    pub(super) fn retained(&self) -> Self {
        let operation = match self.provenance {
            ProviderBootstrapWriteReportProvenance::CurrentInvocation { operation }
            | ProviderBootstrapWriteReportProvenance::RetainedIssuance { operation } => operation,
        };
        let mut copy = self.clone();
        copy.provenance = ProviderBootstrapWriteReportProvenance::RetainedIssuance { operation };
        copy
    }
}

pub(super) struct WriteContext {
    pub(super) operation: &'static str,
    pub(super) report: Option<ProviderBootstrapWriteReport>,
}
impl WriteContext {
    pub(super) fn empty(operation: &'static str) -> Self {
        Self {
            operation,
            report: None,
        }
    }
    pub(super) fn current(
        operation: ProviderBootstrapWriterOperation,
        attempt: ProviderBootstrapAttempt,
    ) -> Self {
        Self {
            operation: match operation {
                ProviderBootstrapWriterOperation::Begin => "begin provider bootstrap",
                ProviderBootstrapWriterOperation::ResumeControlPublication => {
                    "resume provider control publication"
                }
                ProviderBootstrapWriterOperation::Abandon => "abandon provider bootstrap",
            },
            report: Some(ProviderBootstrapWriteReport {
                provenance: ProviderBootstrapWriteReportProvenance::CurrentInvocation { operation },
                attempt,
                effects: Vec::new(),
            }),
        }
    }
    pub(super) fn arm(
        &mut self,
        step: ProviderBootstrapWriteStep,
        target: &Path,
    ) -> CauseResult<usize> {
        let report = self
            .report
            .as_mut()
            .ok_or_else(|| invalid("writer attempt locator absent"))?;
        let index = report.effects.len();
        report.effects.push(ProviderBootstrapWriteEffect {
            step,
            target: target.to_owned(),
            state: ProviderBootstrapWriteState::Uncertain,
        });
        #[cfg(test)]
        tests::hooks::point(false, step, target)?;
        Ok(index)
    }
    pub(super) fn confirm(&mut self, index: usize) -> CauseResult<()> {
        let effect = self
            .report
            .as_mut()
            .and_then(|r| r.effects.get_mut(index))
            .ok_or_else(|| invalid("writer effect index absent"))?;
        effect.state = ProviderBootstrapWriteState::Confirmed;
        #[cfg(test)]
        tests::hooks::point(true, effect.step, &effect.target)?;
        Ok(())
    }
    pub(super) fn no_effect(&mut self, index: usize) -> CauseResult<()> {
        // Only an explicit no-replace collision result confirms no final-name effect.
        // Composite allocation-helper errors remain conservatively Uncertain.
        let effect = self
            .report
            .as_mut()
            .and_then(|r| r.effects.get_mut(index))
            .ok_or_else(|| invalid("writer effect index absent"))?;
        effect.state = ProviderBootstrapWriteState::NoNewEffect;
        Ok(())
    }
    pub(super) fn confirmed(&self, step: ProviderBootstrapWriteStep) -> bool {
        self.report.as_ref().is_some_and(|r| {
            r.effects
                .iter()
                .any(|e| e.step == step && e.state == ProviderBootstrapWriteState::Confirmed)
        })
    }
    pub(super) fn armed(&self, step: ProviderBootstrapWriteStep) -> bool {
        self.report
            .as_ref()
            .is_some_and(|r| r.effects.iter().any(|e| e.step == step))
    }
    pub(super) fn error(
        &self,
        cause: Cause,
        database: Option<ProviderDatabaseScope>,
    ) -> ProjectionStoreError {
        recovery::writer_failure(self.operation, cause, database, None, self.report.clone())
    }

    pub(super) fn attach(&self, error: ProjectionStoreError) -> ProjectionStoreError {
        match error {
            ProjectionStoreError::ProviderBootstrap(mut failure) => {
                failure.operation = self.operation;
                failure.write_report = self.report.clone();
                ProjectionStoreError::ProviderBootstrap(failure)
            }
            error => self.error(error.into(), None),
        }
    }
}

/// One-way ownership: this bridge owns only the original Store cause. The VFS
/// error owns the io wrapper, never the reverse; Cleanup's typed close survives.
#[derive(Debug)]
pub(super) struct CallbackCause(pub(super) Arc<Cause>);
impl fmt::Display for CallbackCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl Error for CallbackCause {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}
