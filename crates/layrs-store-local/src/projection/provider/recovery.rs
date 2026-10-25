//! Provider bootstrap inspection and explicitly bound metadata rollback only.
pub(in crate::projection) mod error;
pub use super::artifacts::{ProviderControlLocation, ProviderFormatState};
use super::{
    artifacts::{Artifacts, ControlObservation, RetainedControl},
    database_io::{self, ReadState},
    records::*,
    validation::{self, PrivateValidation},
};
use crate::projection::shared::provider_access::{FamilyAccess, Registration};
use crate::projection::{PartialStoreFamily, ProjectionStoreError, Result};
pub use error::{
    ProviderBootstrapCause, ProviderBootstrapFailure, ProviderBootstrapFailureKind,
    ProviderDatabaseScope, ProviderRecoveryEffect, ProviderRecoveryEffectState,
    ProviderRollbackTrigger,
};
use error::{ProviderBootstrapCause as Cause, invalid};
use layrs_core::scm::WorktreeId;
use std::{path::Path, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapState {
    SqlValidationPending,
    TerminalPendingValidation,
    FilesystemUninitialized,
    UnregisteredAllocation,
    Allocating,
    ControlPrepared,
    ControlPublished,
    AbandonIncomplete,
    Abandoned,
    NeedsRepair,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBootstrapPhase {
    Allocating,
    ControlPrepared,
    ControlPublished,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderDatabaseStatus {
    Uninitialized,
    Validated,
    RollbackRequired,
    Unavailable,
}
#[derive(Debug)]
pub struct ProviderDatabaseInspection {
    pub database: ProviderDatabaseScope,
    pub status: ProviderDatabaseStatus,
    pub cause: Option<ProviderBootstrapCause>,
}
/// Inspection DATA only. A phase is a validated stored metadata phase, never
/// provider population, source authority, readiness or permission to publish.
#[derive(Debug)]
pub struct ProviderBootstrapInspection {
    pub binding: ProviderBootstrapBinding,
    pub state: ProviderBootstrapState,
    pub phase: Option<ProviderBootstrapPhase>,
    pub control_location: ProviderControlLocation,
    pub format_state: ProviderFormatState,
    pub family: ProviderDatabaseInspection,
    pub private: ProviderDatabaseInspection,
    pub effects: Vec<ProviderRecoveryEffect>,
    pub rollback_triggers: Vec<ProviderRollbackTrigger>,
    pub write_report: Option<super::bootstrap::ProviderBootstrapWriteReport>,
}

/// Retains only immutable expected data and native pins/layout evidence. Every
/// method independently acquires lifecycle -> owner -> catalog gates. A retained
/// allocated-stage strong FORMAT reader can block a future publisher; drop this
/// object before publication. No source/readiness capability is available.
pub struct ProviderBootstrapRecovery {
    family: FamilyAccess,
    artifacts: Artifacts,
    expected: ProviderBootstrapBinding,
    control: RetainedControl,
}

impl PartialStoreFamily {
    pub fn inspect_provider_bootstrap(
        family_root: impl AsRef<Path>,
        worktree: &WorktreeId,
        transaction: &str,
    ) -> Result<ProviderBootstrapInspection> {
        let mut context = Context::new("inspect provider bootstrap");
        let family =
            FamilyAccess::open(family_root.as_ref()).map_err(|e| context.error(e, None, None))?;
        let _locks = family
            .locks(worktree, false)
            .map_err(|e| context.error(e, None, None))?;
        let artifacts = Artifacts::open(&family, worktree, transaction)
            .map_err(|e| context.error(e, None, None))?;
        inspect_locked(&family, &artifacts, None, None, false, &mut context)
    }

    pub fn open_provider_bootstrap_recovery(
        family_root: impl AsRef<Path>,
        worktree: &WorktreeId,
        transaction: &str,
        expected: &ProviderBootstrapBinding,
    ) -> Result<ProviderBootstrapRecovery> {
        let mut context = Context::new("open bound provider bootstrap recovery");
        expected
            .validate_fields()
            .map_err(|e| context.error(e.into(), None, None))?;
        let family =
            FamilyAccess::open(family_root.as_ref()).map_err(|e| context.error(e, None, None))?;
        let locks = family
            .locks(worktree, false)
            .map_err(|e| context.error(e, None, None))?;
        let artifacts = Artifacts::open(&family, worktree, transaction)
            .map_err(|e| context.error(e, None, None))?;
        check_expected(&artifacts, expected, &context)?;
        // DATA discovery is compatible with a live sole owner; actual acquisition
        // may causally refuse raw32. Never promote an observation into a guard.
        let control = artifacts
            .retain_control()
            .map_err(|e| context.error(e, None, None))?;
        let _inspection = inspect_locked(
            &family,
            &artifacts,
            Some(expected),
            Some(&control),
            false,
            &mut context,
        )?;
        let encoded = expected
            .encode()
            .map_err(|e| context.error(e.into(), None, None))?;
        let expected = ProviderBootstrapBinding::decode(&encoded)
            .map_err(|e| context.error(e.into(), None, None))?;
        drop(locks); // In particular, no catalog gate/lease survives construction.
        Ok(ProviderBootstrapRecovery {
            family,
            artifacts,
            expected,
            control,
        })
    }
}

impl ProviderBootstrapRecovery {
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.expected
    }
    pub fn inspect(&self) -> Result<ProviderBootstrapInspection> {
        self.run(false)
    }
    pub fn recover_databases(&self) -> Result<ProviderBootstrapInspection> {
        self.run(true)
    }
    fn run(&self, recover: bool) -> Result<ProviderBootstrapInspection> {
        let mut context = Context::new(if recover {
            "recover provider databases"
        } else {
            "inspect retained provider bootstrap"
        });
        let _locks = self
            .family
            .locks(
                &self.artifacts.allocation.format.identity.worktree_id,
                recover,
            )
            .map_err(|e| context.error(e, None, None))?;
        inspect_locked(
            &self.family,
            &self.artifacts,
            Some(&self.expected),
            Some(&self.control),
            recover,
            &mut context,
        )
    }
}

struct Context {
    operation: &'static str,
    effects: Vec<ProviderRecoveryEffect>,
    rollback_triggers: Vec<ProviderRollbackTrigger>,
}
impl Context {
    fn new(operation: &'static str) -> Self {
        Self {
            operation,
            effects: Vec::new(),
            rollback_triggers: Vec::new(),
        }
    }
    fn error(
        &self,
        cause: Cause,
        database: Option<ProviderDatabaseScope>,
        readonly_rollback: Option<Arc<rusqlite::Error>>,
    ) -> ProjectionStoreError {
        let mut triggers = self.rollback_triggers.clone();
        if let Some(scope) = database {
            if !triggers.iter().any(|t| t.database == scope) {
                if let Some(error) = readonly_rollback
                    .as_ref()
                    .or_else(|| cause.rollback_trigger())
                {
                    triggers.push(ProviderRollbackTrigger {
                        database: scope,
                        error: Arc::clone(error),
                    });
                }
            }
        }
        ProjectionStoreError::ProviderBootstrap(Box::new(ProviderBootstrapFailure {
            kind: error::kind(&cause),
            operation: self.operation,
            database,
            effects: self.effects.clone(),
            cause,
            readonly_rollback: readonly_rollback.or_else(|| {
                let trigger = match database {
                    Some(scope) => triggers.iter().find(|t| t.database == scope),
                    None => triggers.last(),
                };
                trigger.map(|t| Arc::clone(&t.error))
            }),
            rollback_triggers: triggers,
            write_report: None,
        }))
    }
    fn remember_hot<T>(&mut self, state: &ReadState<T>, database: ProviderDatabaseScope) {
        let trigger = match state {
            ReadState::Hot(hot) => Some(&hot.trigger),
            ReadState::UnboundRollback { trigger, .. } => Some(trigger),
            _ => None,
        };
        if let Some(trigger) = trigger {
            if !self
                .rollback_triggers
                .iter()
                .any(|t| t.database == database)
            {
                self.rollback_triggers.push(ProviderRollbackTrigger {
                    database,
                    error: Arc::clone(trigger),
                });
            }
        }
    }
    fn binding_changed(&self) -> ProjectionStoreError {
        ProjectionStoreError::ProviderBootstrap(Box::new(ProviderBootstrapFailure {
            kind: ProviderBootstrapFailureKind::BindingChanged,
            operation: self.operation,
            database: None,
            effects: self.effects.clone(),
            cause: invalid("provider bootstrap binding changed; inspect again"),
            readonly_rollback: self.rollback_triggers.last().map(|t| Arc::clone(&t.error)),
            rollback_triggers: self.rollback_triggers.clone(),
            write_report: None,
        }))
    }
}
pub(super) fn profile_failure(
    e: super::process_profile::ProviderSqliteProfileError,
    operation: &'static str,
) -> ProjectionStoreError {
    Context::new(operation).error(e.into(), None, None)
}
fn check_expected(
    artifacts: &Artifacts,
    expected: &ProviderBootstrapBinding,
    context: &Context,
) -> Result<()> {
    let current = artifacts
        .binding()
        .map_err(|e| context.error(e, None, None))?;
    if &current != expected {
        return Err(context.binding_changed());
    }
    match expected
        .require_references(
            &artifacts.allocation,
            artifacts.prepared.as_ref(),
            artifacts.abandoned.as_ref(),
        )
        .map_err(|e| context.error(e.into(), None, None))?
    {
        ReceiptBindingCheck::NoTerminal | ReceiptBindingCheck::TerminalPendingValidation => {}
    }
    Ok(())
}
fn boundary(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    expected: Option<&ProviderBootstrapBinding>,
    retained: Option<&RetainedControl>,
    context: &Context,
) -> Result<ControlObservation> {
    artifacts
        .verify(family)
        .map_err(|e| context.error(e, None, None))?;
    if let Some(expected) = expected {
        check_expected(artifacts, expected, context)?;
    }
    let observation = match retained {
        Some(control) => control.revalidate(artifacts),
        None => artifacts.observe(),
    }
    .map_err(|e| context.error(e, None, None))?;
    artifacts
        .verify(family)
        .map_err(|e| context.error(e, None, None))?;
    Ok(observation)
}

fn inspect_locked(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    expected: Option<&ProviderBootstrapBinding>,
    retained: Option<&RetainedControl>,
    recover: bool,
    context: &mut Context,
) -> Result<ProviderBootstrapInspection> {
    let observation = boundary(family, artifacts, expected, retained, context)?;
    let a = &artifacts.allocation;
    let mut family_state = database_io::read(&family.catalog, |c| family.validate_catalog(c, a))
        .map_err(|e| context.error(e, Some(ProviderDatabaseScope::FamilyCatalog), None))?;
    context.remember_hot(&family_state, ProviderDatabaseScope::FamilyCatalog);
    // A binds the private path/identity independently of a hot/unavailable family.
    let mut private_state = database_io::read(&artifacts.database, |c| {
        validation::private(c, a, artifacts.prepared.as_ref())
    })
    .map_err(|e| context.error(e, Some(ProviderDatabaseScope::PrivateMetadata), None))?;
    context.remember_hot(&private_state, ProviderDatabaseScope::PrivateMetadata);
    if matches!(family_state, ReadState::Uninitialized) {
        family_state = ReadState::Rejected {
            cause: invalid("zero/uninitialized family catalog is not schema1"),
            pager_read: false,
        };
    }
    family_state = refuse_known(family_state, ProviderDatabaseScope::FamilyCatalog, context)?;
    private_state = refuse_known(
        private_state,
        ProviderDatabaseScope::PrivateMetadata,
        context,
    )?;
    check_known_terminal(artifacts, &family_state, &private_state)
        .map_err(|e| context.error(e, None, None))?;
    if recover {
        // Family first. No private writable attempt while family dependencies or
        // registration remain unavailable, even though private hot was observed.
        if let ReadState::Hot(hot) = family_state {
            boundary(family, artifacts, expected, retained, context)?;
            let admitted = database_io::admit(&family.catalog, hot).map_err(|e| {
                context.error(
                    e.cause,
                    Some(ProviderDatabaseScope::FamilyCatalog),
                    Some(e.trigger),
                )
            })?;
            boundary(family, artifacts, expected, retained, context)?;
            context.effects.push(ProviderRecoveryEffect {
                database: ProviderDatabaseScope::FamilyCatalog,
                state: ProviderRecoveryEffectState::Uncertain,
            });
            let trigger = database_io::rollback(&family.catalog, admitted).map_err(|e| {
                context.error(
                    e.cause,
                    Some(ProviderDatabaseScope::FamilyCatalog),
                    Some(e.trigger),
                )
            })?;
            family_state =
                match database_io::read(&family.catalog, |c| family.validate_catalog(c, a)) {
                    Err(e) => {
                        return Err(context.error(
                            e,
                            Some(ProviderDatabaseScope::FamilyCatalog),
                            Some(trigger),
                        ));
                    }
                    Ok(state) => accept_completed(
                        state,
                        ProviderDatabaseScope::FamilyCatalog,
                        trigger,
                        context,
                    )?,
                };
            #[cfg(test)]
            test_hooks::at(test_hooks::Point::AfterFamilyCompletion)
                .map_err(|e| context.error(e, None, None))?;
            family_state =
                refuse_known(family_state, ProviderDatabaseScope::FamilyCatalog, context)?;
            private_state = refuse_known(
                private_state,
                ProviderDatabaseScope::PrivateMetadata,
                context,
            )?;
            check_known_terminal(artifacts, &family_state, &private_state)
                .map_err(|e| context.error(e, None, None))?;
            boundary(family, artifacts, expected, retained, context)?;
        }
        if matches!(family_state, ReadState::Valid(_)) {
            if let ReadState::Hot(hot) = private_state {
                boundary(family, artifacts, expected, retained, context)?;
                let admitted = database_io::admit(&artifacts.database, hot).map_err(|e| {
                    context.error(
                        e.cause,
                        Some(ProviderDatabaseScope::PrivateMetadata),
                        Some(e.trigger),
                    )
                })?;
                boundary(family, artifacts, expected, retained, context)?;
                context.effects.push(ProviderRecoveryEffect {
                    database: ProviderDatabaseScope::PrivateMetadata,
                    state: ProviderRecoveryEffectState::Uncertain,
                });
                let trigger =
                    database_io::rollback(&artifacts.database, admitted).map_err(|e| {
                        context.error(
                            e.cause,
                            Some(ProviderDatabaseScope::PrivateMetadata),
                            Some(e.trigger),
                        )
                    })?;
                private_state = match database_io::read(&artifacts.database, |c| {
                    validation::private(c, a, artifacts.prepared.as_ref())
                }) {
                    Err(e) => {
                        return Err(context.error(
                            e,
                            Some(ProviderDatabaseScope::PrivateMetadata),
                            Some(trigger),
                        ));
                    }
                    Ok(state) => accept_completed(
                        state,
                        ProviderDatabaseScope::PrivateMetadata,
                        trigger,
                        context,
                    )?,
                };
                family_state =
                    refuse_known(family_state, ProviderDatabaseScope::FamilyCatalog, context)?;
                private_state = refuse_known(
                    private_state,
                    ProviderDatabaseScope::PrivateMetadata,
                    context,
                )?;
                check_known_terminal(artifacts, &family_state, &private_state)
                    .map_err(|e| context.error(e, None, None))?;
                boundary(family, artifacts, expected, retained, context)?;
            }
        }
    }
    let (state, phase) = classify(artifacts, &family_state, &private_state, observation)
        .map_err(|e| context.error(e, None, None))?;
    #[cfg(test)]
    test_hooks::at(test_hooks::Point::BeforeConcludingBoundary)
        .map_err(|e| context.error(e, None, None))?;
    let final_observation = boundary(family, artifacts, expected, retained, context)?;
    if observation != final_observation {
        return Err(context.binding_changed());
    }
    Ok(ProviderBootstrapInspection {
        binding: artifacts
            .binding()
            .map_err(|e| context.error(e, None, None))?,
        state,
        phase,
        control_location: observation.location,
        format_state: observation.format,
        family: summarize(family_state, ProviderDatabaseScope::FamilyCatalog),
        private: summarize(
            match private_state {
                ReadState::Valid(PrivateValidation::Uninitialized) => ReadState::Uninitialized,
                other => other,
            },
            ProviderDatabaseScope::PrivateMetadata,
        ),
        effects: context.effects.clone(),
        rollback_triggers: context.rollback_triggers.clone(),
        write_report: None,
    })
}

// Unknown/hot SQL remains pending. Conversely, a contradiction already proved
// by either independent read must not be deferred until after a writable
// attempt on the other database. This does not accept a phase or eligibility.
fn check_known_terminal(
    artifacts: &Artifacts,
    family: &ReadState<Registration>,
    private: &ReadState<PrivateValidation>,
) -> error::Result<()> {
    let Some(terminal) = &artifacts.abandoned else {
        return Ok(());
    };
    if terminal.eligibility.registry_at_decision == RegistryAtDecision::Absent
        && matches!(family, ReadState::Valid(Registration::Initializing))
    {
        return Err(invalid(
            "terminal decided absent but target registration now exists",
        ));
    }
    match private {
        ReadState::Uninitialized | ReadState::Valid(PrivateValidation::Uninitialized) => {
            terminal.require_references(&artifacts.allocation, artifacts.prepared.as_ref(), None)?
        }
        ReadState::Valid(PrivateValidation::Initialized(mount)) => terminal.require_references(
            &artifacts.allocation,
            artifacts.prepared.as_ref(),
            Some(mount),
        )?,
        _ => (),
    }
    Ok(())
}

fn refuse_known<T>(
    state: ReadState<T>,
    scope: ProviderDatabaseScope,
    context: &Context,
) -> Result<ReadState<T>> {
    match state {
        ReadState::Rejected { cause, .. } => Err(context.error(cause, Some(scope), None)),
        other => Ok(other),
    }
}

fn accept_completed<T>(
    state: ReadState<T>,
    scope: ProviderDatabaseScope,
    trigger: Arc<rusqlite::Error>,
    context: &mut Context,
) -> Result<ReadState<T>> {
    if matches!(
        state,
        ReadState::Valid(_)
            | ReadState::Rejected {
                pager_read: true,
                ..
            }
            | ReadState::ReadButUnavailable(_)
    ) {
        if let Some(effect) = context.effects.last_mut() {
            effect.state = ProviderRecoveryEffectState::Completed;
        }
        // Rejected metadata remains visible below; rollback completion does not
        // transform an unsupported schema into an accepted bootstrap phase.
        return Ok(state);
    }
    let cause = match state {
        ReadState::Hot(hot) => Cause::ReadonlyRollback(hot.trigger),
        ReadState::Unavailable(cause) | ReadState::Rejected { cause, .. } => cause,
        ReadState::UnboundRollback { trigger, cause } => {
            return Err(context.error(cause, Some(scope), Some(trigger)));
        }
        _ => invalid("fresh nonhot read did not establish rollback completion"),
    };
    Err(context.error(cause, Some(scope), Some(trigger)))
}

fn classify(
    artifacts: &Artifacts,
    family: &ReadState<Registration>,
    private: &ReadState<PrivateValidation>,
    control: ControlObservation,
) -> error::Result<(ProviderBootstrapState, Option<ProviderBootstrapPhase>)> {
    use ProviderBootstrapState as S;
    let terminal = artifacts.abandoned.as_ref();
    let registration = match family {
        ReadState::Valid(v) => *v,
        _ => {
            return Ok((
                if terminal.is_some() {
                    S::TerminalPendingValidation
                } else {
                    S::SqlValidationPending
                },
                None,
            ));
        }
    };
    let mount = match private {
        ReadState::Uninitialized | ReadState::Valid(PrivateValidation::Uninitialized) => None,
        ReadState::Valid(PrivateValidation::Initialized(mount)) => Some(mount),
        _ => {
            return Ok((
                if terminal.is_some() {
                    S::TerminalPendingValidation
                } else {
                    S::SqlValidationPending
                },
                None,
            ));
        }
    };
    if let Some(t) = terminal {
        t.require_references(&artifacts.allocation, artifacts.prepared.as_ref(), mount)?;
        if t.eligibility.registry_at_decision == RegistryAtDecision::Absent
            && registration != Registration::Absent
        {
            return Err(invalid(
                "terminal decided absent but target registration now exists",
            ));
        }
        // Static final discovery never takes a second DELETE owner and cannot
        // prove B's NOT_UNDER boundary. The retained recovery guard can do so.
        if !control.unregistered_verified {
            return Ok((S::TerminalPendingValidation, None));
        }
        return Ok((
            if registration == Registration::Absent {
                S::Abandoned
            } else {
                S::AbandonIncomplete
            },
            None,
        ));
    }
    if registration == Registration::Absent {
        return Ok((S::UnregisteredAllocation, None));
    }
    if control.format != ProviderFormatState::Complete || control.root_nonempty {
        return Ok((S::NeedsRepair, None));
    }
    let Some(mount) = mount else {
        return Ok((S::FilesystemUninitialized, None));
    };
    if (mount.phase == MountPhase::Allocating
        && control.location == ProviderControlLocation::FinalDirectory)
        || (mount.phase == MountPhase::ControlPublished
            && control.location != ProviderControlLocation::FinalDirectory)
    {
        return Ok((S::NeedsRepair, None));
    }
    let (state, phase) = match mount.phase {
        MountPhase::Allocating => (S::Allocating, ProviderBootstrapPhase::Allocating),
        MountPhase::ControlPrepared => {
            (S::ControlPrepared, ProviderBootstrapPhase::ControlPrepared)
        }
        MountPhase::ControlPublished => (
            S::ControlPublished,
            ProviderBootstrapPhase::ControlPublished,
        ),
    };
    Ok((state, Some(phase)))
}
fn summarize<T>(
    state: ReadState<T>,
    database: ProviderDatabaseScope,
) -> ProviderDatabaseInspection {
    let (status, cause) = match state {
        ReadState::Uninitialized => (ProviderDatabaseStatus::Uninitialized, None),
        ReadState::Valid(_) => (ProviderDatabaseStatus::Validated, None),
        ReadState::Hot(hot) => (
            ProviderDatabaseStatus::RollbackRequired,
            Some(Cause::ReadonlyRollback(hot.trigger)),
        ),
        ReadState::Rejected { cause: e, .. }
        | ReadState::ReadButUnavailable(e)
        | ReadState::Unavailable(e)
        | ReadState::UnboundRollback { cause: e, .. } => {
            (ProviderDatabaseStatus::Unavailable, Some(e))
        }
    };
    ProviderDatabaseInspection {
        database,
        status,
        cause,
    }
}

#[cfg(test)]
pub(super) mod test_hooks;
#[cfg(test)]
pub(super) mod tests;

// Writer adapters reuse the existing lock-held interpreter. No lock acquisition,
// writable recovery, ordinary opener or source authority is added here.
impl ProviderBootstrapRecovery {
    pub(super) fn into_writer_parts(
        self,
    ) -> (
        FamilyAccess,
        Artifacts,
        ProviderBootstrapBinding,
        RetainedControl,
    ) {
        (self.family, self.artifacts, self.expected, self.control)
    }
    pub(super) fn writer_parts_mut(
        &mut self,
    ) -> (
        &FamilyAccess,
        &mut Artifacts,
        &mut ProviderBootstrapBinding,
        &RetainedControl,
    ) {
        (
            &self.family,
            &mut self.artifacts,
            &mut self.expected,
            &self.control,
        )
    }
}
pub(super) fn writer_inspect_retained_locked(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    expected: &ProviderBootstrapBinding,
    retained: Option<&RetainedControl>,
) -> Result<ProviderBootstrapInspection> {
    inspect_locked(
        family,
        artifacts,
        Some(expected),
        retained,
        false,
        &mut Context::new("inspect writer bootstrap boundary"),
    )
}
pub(super) fn writer_inspect_locked(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    expected: &ProviderBootstrapBinding,
    guard: Option<&layrs_vfs::ControlDirectoryGuard>,
) -> Result<ProviderBootstrapInspection> {
    let context = Context::new("inspect writer publication boundary");
    if let Some(guard) = guard {
        artifacts
            .writer_require_guard(guard)
            .map_err(|e| context.error(e, None, None))?;
        guard
            .revalidate_bootstrap_root()
            .map_err(|e| context.error(e.into(), None, None))?;
    }
    // Compatible readonly DATA discovery while the caller owns the actual guard.
    // It never acquires another sole owner. Terminal/pending states are returned
    // as DATA and must be refused by the Published eligibility predicate.
    let inspection = writer_inspect_retained_locked(family, artifacts, expected, None)?;
    if let Some(guard) = guard {
        let checked = artifacts
            .writer_require_guard(guard)
            .and_then(|()| guard.revalidate_bootstrap_root().map_err(Cause::from));
        if let Err(cause) = checked {
            return Err(writer_reject_inspection(inspection, cause, None));
        }
    }
    Ok(inspection)
}
// Follow only the primary chain. Secondary close/reporting causes keep their
// own identity but cannot replace or contribute unrelated rollback history.
fn writer_nested_failure(mut cause: &Cause) -> Option<&ProviderBootstrapFailure> {
    loop {
        match cause {
            Cause::Cleanup { primary, .. } => cause = primary,
            Cause::Validation(error) => {
                return match error.as_ref() {
                    ProjectionStoreError::ProviderBootstrap(failure) => Some(failure),
                    _ => None,
                };
            }
            _ => return None,
        }
    }
}
fn writer_metadata(
    operation: &'static str,
    original: &Cause,
    database: Option<ProviderDatabaseScope>,
    explicit: Option<Arc<rusqlite::Error>>,
) -> (
    Context,
    Option<ProviderDatabaseScope>,
    Option<Arc<rusqlite::Error>>,
    ProviderBootstrapFailureKind,
) {
    let nested = writer_nested_failure(original);
    let mut context = Context::new(operation);
    if let Some(failure) = nested {
        context.effects = failure.effects.clone();
        context.rollback_triggers = failure.rollback_triggers.clone();
    }
    // Some(nested None) is deliberately unscoped, not the caller's SQL purpose.
    let scope = nested.map(|f| f.database).unwrap_or(database);
    let trigger = explicit
        .or_else(|| nested.and_then(|f| f.readonly_rollback.clone()))
        .or_else(|| original.rollback_trigger().cloned());
    let kind = if matches!(original, Cause::Cleanup { .. }) {
        error::kind(original)
    } else {
        nested
            .map(|f| f.kind)
            .unwrap_or_else(|| error::kind(original))
    };
    (context, scope, trigger, kind)
}
pub(super) fn writer_failure(
    operation: &'static str,
    cause: Cause,
    database: Option<ProviderDatabaseScope>,
    trigger: Option<Arc<rusqlite::Error>>,
    report: Option<super::bootstrap::ProviderBootstrapWriteReport>,
) -> ProjectionStoreError {
    let (context, scope, trigger, kind) = writer_metadata(operation, &cause, database, trigger);
    // Keep the entire owned outer cause, including every Cleanup layer.
    let mut error = context.error(cause, scope, trigger);
    if let ProjectionStoreError::ProviderBootstrap(failure) = &mut error {
        failure.kind = kind;
        failure.write_report = report;
    }
    error
}
pub(super) fn writer_callback_failure(
    operation: &'static str,
    outer: layrs_vfs::Error,
    original: &Cause,
    database: Option<ProviderDatabaseScope>,
    report: Option<super::bootstrap::ProviderBootstrapWriteReport>,
) -> ProjectionStoreError {
    let (context, scope, trigger, kind) = writer_metadata(operation, original, database, None);
    // The publisher retains its one-way Arc bridge; metadata is diagnostic data.
    let mut error = context.error(Cause::Vfs(outer), scope, trigger);
    if let ProjectionStoreError::ProviderBootstrap(failure) = &mut error {
        failure.kind = kind;
        failure.write_report = report;
    }
    error
}

#[cfg(test)]
pub(super) fn writer_legacy_none_controls() -> [ProjectionStoreError; 2] {
    let context = Context::new("pure legacy None control");
    [
        context.error(invalid("pure existing error constructor"), None, None),
        context.binding_changed(),
    ]
}

/// Retain the complete independently observed rollback-cause vector when a
/// writer rejects an otherwise successful DATA inspection. No new read/write.
pub(super) fn writer_reject_inspection(
    inspection: ProviderBootstrapInspection,
    cause: Cause,
    database: Option<ProviderDatabaseScope>,
) -> ProjectionStoreError {
    let mut context = Context::new("reject ineligible writer inspection");
    context.effects = inspection.effects;
    context.rollback_triggers = inspection.rollback_triggers;
    context.error(cause, database, None)
}
