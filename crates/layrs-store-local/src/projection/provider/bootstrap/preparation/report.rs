//! Invocation evidence for the post-bootstrap writer. No report grants a
//! database transition or native registration authority.
use super::*;
use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPreparationSqlPhase {
    RegistrationPending,
    ProviderPrepared,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPreparationWriteStep {
    SqlOpen(ProviderPreparationSqlPhase),
    SqlTransaction(ProviderPreparationSqlPhase),
    SqlCommit(ProviderPreparationSqlPhase),
    SqlClose(ProviderPreparationSqlPhase),
    NativePopulation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPreparationWriteState {
    Uncertain,
    Confirmed,
    NoNewEffect,
}
#[derive(Debug, Clone)]
pub struct ProviderPreparationWriteEffect {
    pub step: ProviderPreparationWriteStep,
    pub target: PathBuf,
    pub state: ProviderPreparationWriteState,
}
#[derive(Debug, Clone)]
pub struct ProviderPreparationWriteReport {
    pub provenance: ProviderPreparationWriteReportProvenance,
    pub bootstrap_issuance: ProviderBootstrapWriteReport,
    pub effects: Vec<ProviderPreparationWriteEffect>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPreparationWriteReportProvenance {
    CurrentInvocation,
    RetainedIssuance,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPreparationFailureKind {
    ProcessProfileUnavailable,
    ReadonlyRollback,
    ObservableCloseFailure,
    DatabaseBusy,
    UnsupportedJournal,
    ControlInUse,
    NativeUnavailable,
    NeedsRepair,
}
#[derive(Debug)]
pub struct ProviderPreparationFailure {
    pub kind: ProviderPreparationFailureKind,
    pub operation: &'static str,
    pub database: Option<ProviderDatabaseScope>,
    pub cause: Cause,
    pub readonly_rollback: Option<Arc<rusqlite::Error>>,
    pub rollback_triggers: Vec<recovery::ProviderRollbackTrigger>,
    pub write_report: ProviderPreparationWriteReport,
}
impl fmt::Display for ProviderPreparationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider preparation {} ({:?}): {}",
            self.operation, self.kind, self.cause
        )
    }
}
impl Error for ProviderPreparationFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

pub(super) struct Failure {
    pub(super) cause: Cause,
    pub(super) database: Option<ProviderDatabaseScope>,
    pub(super) rollback: Option<Arc<rusqlite::Error>>,
    pub(super) rollback_triggers: Vec<recovery::ProviderRollbackTrigger>,
}
impl Failure {
    pub(super) fn kind(&self) -> ProviderPreparationFailureKind {
        failure_kind(&self.cause)
    }
    pub(super) fn new(cause: Cause, database: Option<ProviderDatabaseScope>) -> Self {
        let (inherited_scope, rollback, rollback_triggers) = inherited_metadata(&cause);
        Self {
            cause,
            database: database.or(inherited_scope),
            rollback,
            rollback_triggers,
        }
    }
    pub(super) fn private(cause: Cause) -> Self {
        Self::new(cause, Some(ProviderDatabaseScope::PrivateMetadata))
    }
    pub(super) fn store(error: ProjectionStoreError) -> Self {
        Self::new(error.into(), None)
    }
}
fn inherited_metadata(
    cause: &Cause,
) -> (
    Option<ProviderDatabaseScope>,
    Option<Arc<rusqlite::Error>>,
    Vec<recovery::ProviderRollbackTrigger>,
) {
    match cause {
        Cause::ReadonlyRollback(trigger) => (None, Some(Arc::clone(trigger)), Vec::new()),
        Cause::Cleanup { primary, .. } => inherited_metadata(primary),
        Cause::Validation(error) => match error.as_ref() {
            ProjectionStoreError::ProviderBootstrap(failure) => {
                let (scope, rollback, triggers) = inherited_metadata(&failure.cause);
                let direct = if failure.rollback_triggers.is_empty() {
                    triggers
                } else {
                    failure.rollback_triggers.clone()
                };
                (
                    failure.database.or(scope),
                    failure.readonly_rollback.clone().or(rollback),
                    direct,
                )
            }
            _ => (None, None, Vec::new()),
        },
        _ => (None, None, Vec::new()),
    }
}
impl From<Cause> for Failure {
    fn from(cause: Cause) -> Self {
        Self::new(cause, None)
    }
}

pub(super) struct Context {
    pub(super) operation: &'static str,
    pub(super) report: ProviderPreparationWriteReport,
}
impl Context {
    pub(super) fn new(issuance: &ProviderBootstrapWriteReport) -> Self {
        Self {
            operation: "prepare provider root",
            report: ProviderPreparationWriteReport {
                provenance: ProviderPreparationWriteReportProvenance::CurrentInvocation,
                bootstrap_issuance: issuance.retained(),
                effects: Vec::new(),
            },
        }
    }
    pub(super) fn retained(report: &ProviderPreparationWriteReport) -> Self {
        let mut copy = report.clone();
        copy.provenance = ProviderPreparationWriteReportProvenance::RetainedIssuance;
        Self {
            operation: "revalidate prepared provider",
            report: copy,
        }
    }
    pub(super) fn arm(
        &mut self,
        step: ProviderPreparationWriteStep,
        target: &Path,
    ) -> CauseResult<usize> {
        let index = self.report.effects.len();
        self.report.effects.push(ProviderPreparationWriteEffect {
            step,
            target: target.to_owned(),
            state: ProviderPreparationWriteState::Uncertain,
        });
        #[cfg(test)]
        if let Err(cause) = tests::hooks::point(step, false) {
            if !matches!(step, ProviderPreparationWriteStep::SqlClose(_)) {
                self.no_effect(index);
            }
            return Err(cause);
        }
        Ok(index)
    }
    pub(super) fn confirm(&mut self, index: usize) -> CauseResult<()> {
        let effect = self
            .report
            .effects
            .get_mut(index)
            .ok_or_else(|| invalid("preparation effect index absent"))?;
        effect.state = ProviderPreparationWriteState::Confirmed;
        #[cfg(test)]
        tests::hooks::point(effect.step, true)?;
        Ok(())
    }
    pub(super) fn no_effect(&mut self, index: usize) {
        if let Some(effect) = self.report.effects.get_mut(index) {
            effect.state = ProviderPreparationWriteState::NoNewEffect;
        }
    }
    pub(super) fn failure(&self, error: Failure) -> ProviderPreparationFailure {
        ProviderPreparationFailure {
            kind: error.kind(),
            operation: self.operation,
            database: error.database,
            cause: error.cause,
            readonly_rollback: error.rollback,
            rollback_triggers: error.rollback_triggers,
            write_report: self.report.clone(),
        }
    }
}

// Historical rollback evidence must never replace the primary refusal.
fn failure_kind(cause: &Cause) -> ProviderPreparationFailureKind {
    match cause {
        Cause::Profile(_) => ProviderPreparationFailureKind::ProcessProfileUnavailable,
        Cause::ReadonlyRollback(_) => ProviderPreparationFailureKind::ReadonlyRollback,
        Cause::Cleanup { .. } => ProviderPreparationFailureKind::ObservableCloseFailure,
        Cause::Sqlite(rusqlite::Error::SqliteFailure(code, _))
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            ProviderPreparationFailureKind::DatabaseBusy
        }
        Cause::Journal(_) => ProviderPreparationFailureKind::UnsupportedJournal,
        Cause::Validation(store)
            if matches!(store.as_ref(), ProjectionStoreError::ProviderBootstrap(_)) =>
        {
            match store.as_ref() {
                ProjectionStoreError::ProviderBootstrap(failure) => match failure.kind {
                    recovery::ProviderBootstrapFailureKind::ProcessProfileUnavailable => {
                        ProviderPreparationFailureKind::ProcessProfileUnavailable
                    }
                    recovery::ProviderBootstrapFailureKind::DatabaseBusy => {
                        ProviderPreparationFailureKind::DatabaseBusy
                    }
                    recovery::ProviderBootstrapFailureKind::UnsupportedJournal => {
                        ProviderPreparationFailureKind::UnsupportedJournal
                    }
                    recovery::ProviderBootstrapFailureKind::ControlInUse => {
                        ProviderPreparationFailureKind::ControlInUse
                    }
                    recovery::ProviderBootstrapFailureKind::ObservableCloseFailure => {
                        ProviderPreparationFailureKind::ObservableCloseFailure
                    }
                    recovery::ProviderBootstrapFailureKind::NeedsRepair => {
                        failure_kind(&failure.cause)
                    }
                    _ => ProviderPreparationFailureKind::NeedsRepair,
                },
                _ => unreachable!(),
            }
        }
        Cause::Vfs(_) => ProviderPreparationFailureKind::NativeUnavailable,
        _ => ProviderPreparationFailureKind::NeedsRepair,
    }
}

#[cfg(test)]
#[test]
fn historical_rollback_does_not_replace_primary_rejection() {
    let original = Arc::new(rusqlite::Error::QueryReturnedNoRows);
    let error = ProjectionStoreError::ProviderBootstrap(Box::new(ProviderBootstrapFailure {
        kind: recovery::ProviderBootstrapFailureKind::NeedsRepair,
        operation: "inspect bootstrap",
        database: Some(ProviderDatabaseScope::PrivateMetadata),
        effects: Vec::new(),
        cause: invalid("private schema contradiction"),
        rollback_triggers: Vec::new(),
        readonly_rollback: Some(Arc::clone(&original)),
        write_report: None,
    }));
    let failure = Failure::store(error);
    assert_eq!(
        failure_kind(&failure.cause),
        ProviderPreparationFailureKind::NeedsRepair
    );
    assert_eq!(
        failure.database,
        Some(ProviderDatabaseScope::PrivateMetadata)
    );
    assert!(Arc::ptr_eq(failure.rollback.as_ref().unwrap(), &original));
    assert!(matches!(failure.cause, Cause::Validation(_)));
}

pub(super) struct CloseLedger<'a> {
    context: &'a mut Context,
    phase: ProviderPreparationSqlPhase,
    target: &'a Path,
    index: Option<usize>,
}
impl<'a> CloseLedger<'a> {
    pub(super) fn new(
        context: &'a mut Context,
        phase: ProviderPreparationSqlPhase,
        target: &'a Path,
    ) -> Self {
        Self {
            context,
            phase,
            target,
            index: None,
        }
    }
}
impl database_io::WriterCloseObserver for CloseLedger<'_> {
    fn before_close(&mut self, _raw: *mut rusqlite::ffi::sqlite3) -> Option<Cause> {
        let index = self.context.report.effects.len();
        let result = self.context.arm(
            ProviderPreparationWriteStep::SqlClose(self.phase),
            self.target,
        );
        self.index = self
            .context
            .report
            .effects
            .get(index)
            .filter(|effect| effect.step == ProviderPreparationWriteStep::SqlClose(self.phase))
            .map(|_| index);
        #[cfg(test)]
        {
            let hook = tests::hooks::before_close(_raw, self.phase).err();
            return match (result.err(), hook) {
                (None, cause) | (cause, None) => cause,
                (Some(primary), Some(close)) => Some(Cause::Cleanup {
                    primary: Box::new(primary),
                    close: Box::new(close),
                }),
            };
        }
        #[cfg(not(test))]
        result.err()
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        if succeeded {
            self.index
                .and_then(|index| self.context.confirm(index).err())
        } else {
            None
        }
    }
}
