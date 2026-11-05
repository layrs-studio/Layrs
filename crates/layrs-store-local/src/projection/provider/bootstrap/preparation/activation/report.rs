//! Invocation evidence for the two ordered activation database transitions.
use super::super::report as preparation_report;
use super::*;
use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderActivationSqlPhase {
    PrivateActivation,
    FamilyReady,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderActivationWriteStep {
    SqlOpen(ProviderActivationSqlPhase),
    SqlTransaction(ProviderActivationSqlPhase),
    SqlCommit(ProviderActivationSqlPhase),
    SqlClose(ProviderActivationSqlPhase),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderActivationWriteState {
    Uncertain,
    Confirmed,
    NoNewEffect,
}
#[derive(Debug, Clone)]
pub struct ProviderActivationWriteEffect {
    pub step: ProviderActivationWriteStep,
    pub target: PathBuf,
    pub state: ProviderActivationWriteState,
}
#[derive(Debug, Clone)]
pub struct ProviderActivationWriteReport {
    pub provenance: ProviderActivationWriteReportProvenance,
    pub resumed: bool,
    pub previous_effects: Vec<ProviderActivationWriteEffect>,
    pub effects: Vec<ProviderActivationWriteEffect>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderActivationWriteReportProvenance {
    CurrentInvocation,
    ResumedInvocation,
    RetainedIssuance,
}
#[derive(Debug)]
pub struct ProviderActivationFailure {
    pub kind: ProviderActivationFailureKind,
    pub operation: &'static str,
    pub database: Option<ProviderDatabaseScope>,
    pub cause: Cause,
    pub readonly_rollback: Option<Arc<rusqlite::Error>>,
    pub rollback_triggers: Vec<recovery::ProviderRollbackTrigger>,
    pub write_report: ProviderActivationWriteReport,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderActivationFailureKind {
    ProcessProfileUnavailable,
    ReadonlyRollback,
    ObservableCloseFailure,
    DatabaseBusy,
    UnsupportedJournal,
    ControlInUse,
    NativeUnavailable,
    NeedsRepair,
}
impl From<ProviderPreparationFailureKind> for ProviderActivationFailureKind {
    fn from(kind: ProviderPreparationFailureKind) -> Self {
        match kind {
            ProviderPreparationFailureKind::ProcessProfileUnavailable => {
                Self::ProcessProfileUnavailable
            }
            ProviderPreparationFailureKind::ReadonlyRollback => Self::ReadonlyRollback,
            ProviderPreparationFailureKind::ObservableCloseFailure => Self::ObservableCloseFailure,
            ProviderPreparationFailureKind::DatabaseBusy => Self::DatabaseBusy,
            ProviderPreparationFailureKind::UnsupportedJournal => Self::UnsupportedJournal,
            ProviderPreparationFailureKind::ControlInUse => Self::ControlInUse,
            ProviderPreparationFailureKind::NativeUnavailable => Self::NativeUnavailable,
            ProviderPreparationFailureKind::NeedsRepair => Self::NeedsRepair,
        }
    }
}
impl fmt::Display for ProviderActivationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider activation {} ({:?}): {}",
            self.operation, self.kind, self.cause
        )
    }
}
impl Error for ProviderActivationFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}

pub(super) struct Context {
    pub(super) report: ProviderActivationWriteReport,
    operation: &'static str,
}
impl Context {
    pub(super) fn new(previous: Option<ProviderActivationWriteReport>) -> Self {
        match previous {
            None => Self {
                operation: "activate prepared provider",
                report: ProviderActivationWriteReport {
                    provenance: ProviderActivationWriteReportProvenance::CurrentInvocation,
                    resumed: false,
                    previous_effects: Vec::new(),
                    effects: Vec::new(),
                },
            },
            Some(previous) => {
                let mut effects = previous.previous_effects;
                effects.extend(previous.effects);
                Self {
                    operation: "resume provider activation",
                    report: ProviderActivationWriteReport {
                        provenance: ProviderActivationWriteReportProvenance::ResumedInvocation,
                        resumed: true,
                        previous_effects: effects,
                        effects: Vec::new(),
                    },
                }
            }
        }
    }
    pub(super) fn retained(prior: &ProviderActivationWriteReport) -> Self {
        let mut report = prior.clone();
        report.provenance = ProviderActivationWriteReportProvenance::RetainedIssuance;
        Self {
            operation: "revalidate active provider",
            report,
        }
    }
    pub(super) fn arm(
        &mut self,
        step: ProviderActivationWriteStep,
        target: &Path,
    ) -> CauseResult<usize> {
        let index = self.report.effects.len();
        self.report.effects.push(ProviderActivationWriteEffect {
            step,
            target: target.to_owned(),
            state: ProviderActivationWriteState::Uncertain,
        });
        #[cfg(test)]
        if let Err(cause) = super::tests::hooks::point(step, false) {
            if !matches!(step, ProviderActivationWriteStep::SqlClose(_)) {
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
            .ok_or_else(|| invalid("activation effect index absent"))?;
        effect.state = ProviderActivationWriteState::Confirmed;
        #[cfg(test)]
        super::tests::hooks::point(effect.step, true)?;
        Ok(())
    }
    pub(super) fn no_effect(&mut self, index: usize) {
        if let Some(effect) = self.report.effects.get_mut(index) {
            effect.state = ProviderActivationWriteState::NoNewEffect;
        }
    }
    pub(super) fn failure(&self, error: preparation_report::Failure) -> ProviderActivationFailure {
        ProviderActivationFailure {
            kind: error.kind().into(),
            operation: self.operation,
            database: error.database,
            cause: error.cause,
            readonly_rollback: error.rollback,
            rollback_triggers: error.rollback_triggers,
            write_report: self.report.clone(),
        }
    }
}

pub(super) struct CloseLedger<'a> {
    context: &'a mut Context,
    phase: ProviderActivationSqlPhase,
    target: &'a Path,
    index: Option<usize>,
}
impl<'a> CloseLedger<'a> {
    pub(super) fn new(
        context: &'a mut Context,
        phase: ProviderActivationSqlPhase,
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
            ProviderActivationWriteStep::SqlClose(self.phase),
            self.target,
        );
        self.index = self
            .context
            .report
            .effects
            .get(index)
            .filter(|effect| effect.step == ProviderActivationWriteStep::SqlClose(self.phase))
            .map(|_| index);
        #[cfg(test)]
        {
            let hook = super::tests::hooks::before_close(_raw, self.phase).err();
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
