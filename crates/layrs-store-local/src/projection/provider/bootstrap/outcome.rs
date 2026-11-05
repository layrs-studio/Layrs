//! Owned layout proof. No guard accessor, registration, serving or readiness.
use super::*;

pub enum ProviderControlPublicationOutcome {
    Published(ProviderControlPublished),
    CommittedCapabilityWithheld(ProviderControlWithheld),
    CommitOutcomeUnknown(ProviderControlWithheld),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderControlCommitDisposition {
    Committed,
    Unknown,
}

pub(super) struct PublicationPins {
    pub(super) family: FamilyAccess,
    pub(super) artifacts: Artifacts,
    pub(super) binding: ProviderBootstrapBinding,
    pub(super) guard: layrs_vfs::ControlDirectoryGuard,
}
pub struct ProviderControlPublished {
    pub(super) held: PublicationPins,
    pub(super) inspection: ProviderBootstrapInspection,
    pub(super) issuance: ProviderBootstrapWriteReport,
}
pub struct ProviderControlWithheld {
    pub(super) held: PublicationPins,
    pub(super) inspection: Option<ProviderBootstrapInspection>,
    pub(super) disposition: ProviderControlCommitDisposition,
    pub(super) failure: ProviderBootstrapFailure,
}
impl ProviderControlPublished {
    pub fn root(&self) -> &Path {
        self.held.guard.root()
    }
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.held.binding
    }
    /// Issuance DATA, not a fresh observation or authority to register/mount.
    pub fn inspection(&self) -> &ProviderBootstrapInspection {
        &self.inspection
    }
    pub fn revalidate(&self) -> Result<()> {
        let report = &self.issuance;
        let context = WriteContext {
            operation: "revalidate provider control publication",
            report: Some(report.retained()),
        };
        let result = (|| -> Result<()> {
            let _locks = self
                .held
                .family
                .locks(
                    &self.held.artifacts.allocation.format.identity.worktree_id,
                    false,
                )
                .map_err(|e| context.error(e, None))?;
            self.held
                .guard
                .revalidate_bootstrap_root()
                .map_err(|e| context.error(e.into(), None))?;
            let inspection = recovery::writer_inspect_locked(
                &self.held.family,
                &self.held.artifacts,
                &self.held.binding,
                Some(&self.held.guard),
            )?;
            require_published(inspection).map_err(|e| context.error(e, None))?;
            self.held
                .guard
                .revalidate_bootstrap_root()
                .map_err(|e| context.error(e.into(), None))?;
            self.held
                .artifacts
                .verify(&self.held.family)
                .map_err(|e| context.error(e, None))?;
            Ok(())
        })();
        result.map_err(|e| context.attach(e))
    }
}
impl ProviderControlWithheld {
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.held.binding
    }
    pub fn last_inspection(&self) -> Option<&ProviderBootstrapInspection> {
        self.inspection.as_ref()
    }
    pub fn commit_disposition(&self) -> ProviderControlCommitDisposition {
        self.disposition
    }
    pub fn cause(&self) -> &ProviderBootstrapCause {
        &self.failure.cause
    }
    pub fn failure(&self) -> &ProviderBootstrapFailure {
        &self.failure
    }
}

fn database_refusal(
    inspection: &mut ProviderBootstrapInspection,
) -> Option<(Cause, ProviderDatabaseScope)> {
    for database in [&mut inspection.family, &mut inspection.private] {
        if database.status != ProviderDatabaseStatus::Validated {
            return Some((
                database.cause.take().unwrap_or_else(|| {
                    invalid("provider publication requires validated initialized databases")
                }),
                database.database,
            ));
        }
    }
    None
}
fn reject(
    inspection: ProviderBootstrapInspection,
    cause: Cause,
    scope: Option<ProviderDatabaseScope>,
) -> Cause {
    Cause::Validation(Box::new(recovery::writer_reject_inspection(
        inspection, cause, scope,
    )))
}
pub(super) fn require_published(
    mut inspection: ProviderBootstrapInspection,
) -> CauseResult<ProviderBootstrapInspection> {
    if let Some((cause, scope)) = database_refusal(&mut inspection) {
        return Err(reject(inspection, cause, Some(scope)));
    }
    if inspection.state != ProviderBootstrapState::ControlPublished
        || inspection.phase != Some(ProviderBootstrapPhase::ControlPublished)
        || inspection.binding.abandoned.is_some()
        || inspection.control_location != ProviderControlLocation::FinalDirectory
        || inspection.format_state != ProviderFormatState::Complete
    {
        return Err(reject(
            inspection,
            invalid("inspection is not an eligible published provider bootstrap"),
            None,
        ));
    }
    Ok(inspection)
}
pub(super) fn publication_ready(
    mut inspection: ProviderBootstrapInspection,
) -> CauseResult<ProviderBootstrapInspection> {
    if let Some((cause, scope)) = database_refusal(&mut inspection) {
        return Err(reject(inspection, cause, Some(scope)));
    }
    if !matches!(
        inspection.state,
        ProviderBootstrapState::Allocating
            | ProviderBootstrapState::ControlPrepared
            | ProviderBootstrapState::ControlPublished
    ) || inspection.binding.abandoned.is_some()
        || inspection.phase.is_none()
        || inspection.format_state != ProviderFormatState::Complete
    {
        return Err(reject(
            inspection,
            invalid("provider bootstrap is not eligible for publication-only resume"),
            None,
        ));
    }
    Ok(inspection)
}
