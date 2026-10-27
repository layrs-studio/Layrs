//! Consuming activation writer. The retained owner and native READY proof are
//! returned on every refusal, including uncertain SQL close or family commit.
mod capture;
mod capture_image;
mod managed_allocation;
mod managed_checkout_fs;
mod managed_content;
mod managed_destination;
mod managed_image;
mod managed_journal;
mod managed_plan;
mod managed_staging;
mod ranges;
mod recovery_export;
pub use capture::{ProviderCaptureAcknowledgement, ProviderCaptureFailure, ProviderCaptureStep};
mod capture_sql;
mod capture_state;
mod report;
mod session;
pub use ranges::ProviderActiveRangeFailure;
pub use recovery_export::{
    ProviderRecoveryExportCause, ProviderRecoveryExportClass, ProviderRecoveryExportDeletion,
    ProviderRecoveryExportFailure, ProviderRecoveryExportFile, ProviderRecoveryExportGap,
    ProviderRecoveryExportGapKind, ProviderRecoveryExportOutcome, ProviderRecoveryExportReport,
    ProviderRecoveryExportStep,
};
mod reopen;
mod sql;
#[cfg(test)]
pub(in crate::projection::provider::bootstrap) mod tests;

use super::report as preparation_report;
use super::*;
use crate::projection::provider::records::activation::{
    ProviderActivationRecord, ProviderActiveMountRecord,
};
use crate::projection::shared::provider_access::ActivationRegistration;
use layrs_core::projection::ProjectionSnapshot;
use rusqlite::Connection;
use std::path::Path;

pub use reopen::{ProviderActiveOpenFailure, ProviderActiveReopened};
pub use report::{
    ProviderActivationFailure, ProviderActivationFailureKind, ProviderActivationSqlPhase,
    ProviderActivationWriteEffect, ProviderActivationWriteReport,
    ProviderActivationWriteReportProvenance, ProviderActivationWriteState,
    ProviderActivationWriteStep,
};
pub use session::{
    ProviderActiveSession, ProviderActiveSessionCause, ProviderActiveSessionWithheld,
};

pub enum ProviderActivationOutcome {
    Active(ProviderActive),
    Withheld(ProviderActivationWithheld),
}

/// Retained owner of the exact generation-one provider placement.
pub struct ProviderActive {
    held: outcome::PublicationPins,
    root: layrs_vfs::ProviderRootGuard,
    record: ProviderActiveMountRecord,
    activation: ProviderActivationRecord,
    report: ProviderActivationWriteReport,
}

/// Retains the original Store pins and native READY proof after any refusal.
pub struct ProviderActivationWithheld {
    held: outcome::PublicationPins,
    root: layrs_vfs::ProviderRootGuard,
    prepared: ProviderPreparationRecord,
    failure: ProviderActivationFailure,
}

impl ProviderActive {
    pub fn root(&self) -> &Path {
        self.root.root()
    }
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.held.binding
    }
    pub fn catalog_id(&self) -> &str {
        &self.record.prepared.source.catalog_id
    }
    pub fn generation(&self) -> u64 {
        self.activation.activated_generation
    }
    pub fn digest(&self) -> &str {
        &self.activation.activated_digest
    }
    pub fn report(&self) -> &ProviderActivationWriteReport {
        &self.report
    }
    pub fn revalidate(&self) -> std::result::Result<(), ProviderActivationFailure> {
        let context = report::Context::retained(&self.report);
        let result = (|| -> std::result::Result<(), preparation_report::Failure> {
            let _locks = self
                .held
                .family
                .locks(
                    &self.held.artifacts.allocation.format.identity.worktree_id,
                    false,
                )
                .map_err(preparation_report::Failure::from)?;
            capture_state::require_exact(&self.held, &self.root, &self.record, &self.activation)?;
            Ok(())
        })();
        result.map_err(|error| context.failure(error))
    }
}

impl ProviderActivationWithheld {
    pub fn root(&self) -> &Path {
        self.root.root()
    }
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.held.binding
    }
    pub fn failure(&self) -> &ProviderActivationFailure {
        &self.failure
    }
    pub fn report(&self) -> &ProviderActivationWriteReport {
        &self.failure.write_report
    }
    /// Reobserves both databases without recovery. The durable SQL phase,
    /// rather than the prior report, decides the next exact transition.
    pub fn resume_activation(self) -> ProviderActivationOutcome {
        activate(
            self.held,
            self.root,
            self.prepared,
            Some(self.failure.write_report),
        )
    }
}

pub(super) fn activate(
    held: outcome::PublicationPins,
    root: layrs_vfs::ProviderRootGuard,
    prepared: ProviderPreparationRecord,
    previous: Option<ProviderActivationWriteReport>,
) -> ProviderActivationOutcome {
    let resumed = previous.is_some();
    let mut context = report::Context::new(previous);
    let result = (|| -> std::result::Result<(ProviderActiveMountRecord, ProviderActivationRecord), preparation_report::Failure> {
        let _locks = held.family.locks(&held.artifacts.allocation.format.identity.worktree_id, true)
            .map_err(preparation_report::Failure::from)?;
        boundary(&held, &root).map_err(preparation_report::Failure::from)?;
        let family = family_state(&held)?;
        let private = private_state(&held, &root, &prepared)?;
        match (private, family) {
            (PrivateState::Prepared(source), ActivationRegistration::Initializing) => {
                // A resumed v2 must still be a clean, completely validated
                // predecessor before opening either database for a write.
                super::sql::final_check(&held, &prepared, &root, root.registration().root_id())?;
                let p = held.artifacts.prepared.as_ref().ok_or_else(|| preparation_report::Failure::private(invalid("activation P absent")))?;
                let (activation, snapshot) = ProviderActivationRecord::derive(&held.artifacts.allocation, p, &prepared, &source)
                    .map_err(|error| preparation_report::Failure::private(error.into()))?;
                let mount = ProviderActiveMountRecord::derive(&prepared, &activation)
                    .map_err(|error| preparation_report::Failure::private(error.into()))?;
                sql::private_transition(&held, &root, &prepared, &activation, &mount, &snapshot, &mut context)?;
            }
            (PrivateState::Active(_, _), ActivationRegistration::Initializing) if resumed => {}
            (PrivateState::Active(_, _), ActivationRegistration::Ready) if resumed => {}
            (PrivateState::Prepared(_), ActivationRegistration::Ready) => {
                return Err(preparation_report::Failure::new(invalid("ready family has only prepared private metadata"), Some(ProviderDatabaseScope::FamilyCatalog)));
            }
            (PrivateState::Active(_, _), _) => {
                return Err(preparation_report::Failure::private(invalid("fresh activation encountered an already active private mount")));
            }
        }
        // Private activation is always durable and fully revalidated before
        // the family registry is made ready. This also handles a committed
        // private transition followed by a failed or uncertain close.
        let active = private_state(&held, &root, &prepared)?;
        let PrivateState::Active(mount, activation) = active else {
            return Err(preparation_report::Failure::private(invalid("private activation has not committed")));
        };
        if family_state(&held)? == ActivationRegistration::Initializing {
            sql::family_transition(&held, &root, &prepared, &mount, &activation, &mut context)?;
        }
        let (checked_mount, checked_activation) = final_check(&held, &root, &prepared)?;
        if checked_mount != mount || checked_activation != activation {
            return Err(preparation_report::Failure::private(invalid("activation changed after publication")));
        }
        Ok((mount, activation))
    })();
    match result {
        Ok((record, activation)) => ProviderActivationOutcome::Active(ProviderActive {
            held,
            root,
            record,
            activation,
            report: context.report,
        }),
        Err(error) => ProviderActivationOutcome::Withheld(ProviderActivationWithheld {
            held,
            root,
            prepared,
            failure: context.failure(error),
        }),
    }
}

pub(super) fn boundary(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
) -> CauseResult<()> {
    held.family.verify()?;
    held.artifacts.verify(&held.family)?;
    held.artifacts.writer_require_guard(&held.guard)?;
    held.guard.revalidate()?;
    root.revalidate()?;
    if root.control_directory().identity() != held.guard.identity()
        || root.registration().root_id() == [0; 16]
        || held.artifacts.binding()? != held.binding
    {
        return Err(invalid("activation retained owner/native binding changed"));
    }
    Ok(())
}

fn read_state<T>(
    state: database_io::ReadState<T>,
    scope: ProviderDatabaseScope,
) -> std::result::Result<T, preparation_report::Failure> {
    let fail = |cause| preparation_report::Failure::new(cause, Some(scope));
    match state {
        database_io::ReadState::Valid(value) => Ok(value),
        database_io::ReadState::Uninitialized => {
            Err(fail(invalid("activation database uninitialized")))
        }
        database_io::ReadState::Hot(hot) => Err(fail(Cause::ReadonlyRollback(hot.trigger))),
        database_io::ReadState::Rejected { cause, .. }
        | database_io::ReadState::ReadButUnavailable(cause)
        | database_io::ReadState::Unavailable(cause) => Err(fail(cause)),
        database_io::ReadState::UnboundRollback { trigger, cause } => {
            let mut error = fail(cause);
            error.rollback = Some(trigger);
            Err(error)
        }
    }
}

pub(super) fn family_state(
    held: &outcome::PublicationPins,
) -> std::result::Result<ActivationRegistration, preparation_report::Failure> {
    let state = database_io::read(&held.family.catalog, |c| {
        held.family
            .validate_activation_catalog(c, &held.artifacts.allocation, None)
    })
    .map_err(|error| {
        preparation_report::Failure::new(error, Some(ProviderDatabaseScope::FamilyCatalog))
    })?;
    read_state(state, ProviderDatabaseScope::FamilyCatalog)
}

enum PrivateState {
    Prepared(ProviderBootstrapSource),
    Active(ProviderActiveMountRecord, ProviderActivationRecord),
}

fn inspect_private(
    c: &Connection,
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    prepared: &ProviderPreparationRecord,
) -> CauseResult<PrivateState> {
    let p = held
        .artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("activation P absent"))?;
    match validation::private_activation_state_with_source(
        c,
        &held.artifacts.allocation,
        p,
        &held.binding,
        root.registration().root_id(),
    )? {
        validation::PrivateActivationStateValidation::Prepared(value) => {
            if value.mount != *prepared || value.source.commitment() != &prepared.source {
                return Err(invalid("activation prepared predecessor changed"));
            }
            Ok(PrivateState::Prepared(value.source))
        }
        validation::PrivateActivationStateValidation::Active(value) => {
            if value.mount.prepared != *prepared || value.source.commitment() != &prepared.source {
                return Err(invalid("activation active predecessor changed"));
            }
            Ok(PrivateState::Active(value.mount, value.activation))
        }
    }
}

fn private_state(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    prepared: &ProviderPreparationRecord,
) -> std::result::Result<PrivateState, preparation_report::Failure> {
    let state = database_io::read(&held.artifacts.database, |c| {
        inspect_private(c, held, root, prepared)
    })
    .map_err(preparation_report::Failure::private)?;
    read_state(state, ProviderDatabaseScope::PrivateMetadata)
}

pub(super) fn final_check(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    prepared: &ProviderPreparationRecord,
) -> std::result::Result<
    (ProviderActiveMountRecord, ProviderActivationRecord),
    preparation_report::Failure,
> {
    boundary(held, root).map_err(preparation_report::Failure::from)?;
    if family_state(held)? != ActivationRegistration::Ready {
        return Err(preparation_report::Failure::new(
            invalid("active provider requires ready family registration"),
            Some(ProviderDatabaseScope::FamilyCatalog),
        ));
    }
    let PrivateState::Active(mount, activation) = private_state(held, root, prepared)? else {
        return Err(preparation_report::Failure::private(invalid(
            "active provider lacks private activation",
        )));
    };
    boundary(held, root).map_err(preparation_report::Failure::from)?;
    Ok((mount, activation))
}
