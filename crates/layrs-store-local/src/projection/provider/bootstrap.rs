//! Stage1b writer: control_published with family initializing, no readiness.
mod abandon;
mod allocation;
mod catalog;
mod outcome;
mod preparation;
mod publication;
mod receipt_io;
mod report;
mod sql;
#[cfg(test)]
mod tests;

use super::recovery::error::{Result as CauseResult, invalid};
use super::recovery::{
    ProviderBootstrapCause, ProviderBootstrapCause as Cause, ProviderBootstrapFailure,
    ProviderBootstrapInspection, ProviderBootstrapPhase, ProviderBootstrapRecovery,
    ProviderBootstrapState, ProviderControlLocation, ProviderDatabaseScope, ProviderDatabaseStatus,
    ProviderFormatState,
};
use super::{
    artifacts::{Artifacts, RetainedControl},
    database_io,
    records::{self, *},
    recovery, validation,
};
use crate::projection::shared::provider_access::{FamilyAccess, Registration};
use crate::projection::{PartialStoreFamily, ProjectionStoreError, Result};
use layrs_core::scm::WorktreeId;
pub use catalog::{ProviderBootstrapCatalog, ProviderCatalogDescriptor};
pub use outcome::{
    ProviderControlCommitDisposition, ProviderControlPublicationOutcome, ProviderControlPublished,
    ProviderControlWithheld,
};
pub use preparation::{
    ProviderCaptureAcknowledgement, ProviderCaptureFailure, ProviderCaptureStep,
    ProviderActivationFailure, ProviderActivationFailureKind, ProviderActivationOutcome,
    ProviderActivationSqlPhase, ProviderActivationWithheld, ProviderActivationWriteEffect,
    ProviderActivationWriteReport, ProviderActivationWriteReportProvenance,
    ProviderActivationWriteState, ProviderActivationWriteStep, ProviderActive,
    ProviderActiveOpenFailure, ProviderActiveReopened, ProviderActiveRangeFailure,
    ProviderActiveSession, ProviderActiveSessionCause, ProviderActiveSessionWithheld,
    ProviderRecoveryExportCause, ProviderRecoveryExportClass, ProviderRecoveryExportDeletion,
    ProviderRecoveryExportFailure, ProviderRecoveryExportFile, ProviderRecoveryExportGap,
    ProviderRecoveryExportGapKind, ProviderRecoveryExportOutcome, ProviderRecoveryExportReport,
    ProviderRecoveryExportStep,
    ProviderPreparedOpenFailure, ProviderPreparedReopened, ProviderRangeFailure,
    ProviderPreparationFailure, ProviderPreparationFailureKind,
    ProviderPreparationOutcome, ProviderPreparationSqlPhase,
    ProviderPreparationWithheld, ProviderPreparationWriteEffect,
    ProviderPreparationWriteReport, ProviderPreparationWriteState,
    ProviderPreparationWriteStep, ProviderPreparationWriteReportProvenance,
    ProviderPrepared,
};
use report::WriteContext;
pub use report::{
    ProviderBootstrapAttempt, ProviderBootstrapCreate, ProviderBootstrapReceipt,
    ProviderBootstrapSqlWrite, ProviderBootstrapWriteEffect, ProviderBootstrapWriteReport,
    ProviderBootstrapWriteReportProvenance, ProviderBootstrapWriteState,
    ProviderBootstrapWriteStep, ProviderBootstrapWriterOperation,
};
use std::path::{Path, PathBuf};

impl PartialStoreFamily {
    pub fn begin_provider_bootstrap(
        &self,
        root: &Path,
        stage_parent: &Path,
        worktree: &WorktreeId,
        transaction_id: &str,
        source: ProviderBootstrapSource,
    ) -> Result<ProviderControlPublicationOutcome> {
        let mut context = WriteContext::empty("begin provider bootstrap");
        let initial = allocation::begin(
            self,
            root,
            stage_parent,
            worktree,
            transaction_id,
            &source,
            &mut context,
        )
        .map_err(|e| context.error(e, None))?;
        sql::register(&initial.family, &initial.artifacts, &mut context)
            .map_err(|e| context.error(e, Some(ProviderDatabaseScope::FamilyCatalog)))?;
        sql::initialize(&initial.family, &initial.artifacts, &source, &mut context)
            .map_err(|e| context.error(e, Some(ProviderDatabaseScope::PrivateMetadata)))?;
        let (family, artifacts, preflight, _owner, _lifecycle) = initial
            .write_format(&mut context)
            .map_err(|e| context.error(e, None))?;
        publication::publish(family, artifacts, None, Some(preflight), &mut context)
            .map_err(|e| context.attach(e))
    }
}
impl ProviderBootstrapRecovery {
    pub fn resume_control_publication(self) -> Result<ProviderControlPublicationOutcome> {
        let (family, artifacts, expected, control) = self.into_writer_parts();
        let attempt = attempt_from(&family, &artifacts).map_err(|e| {
            WriteContext::empty("resume provider control publication").error(e, None)
        })?;
        let mut context = WriteContext::current(
            ProviderBootstrapWriterOperation::ResumeControlPublication,
            attempt,
        );
        let _lifecycle = family
            .writer_lifecycle(false)
            .map_err(|e| context.error(e, None))?;
        let _owner = family
            .writer_owner(&artifacts.allocation.format.identity.worktree_id)
            .map_err(|e| context.error(e, None))?;
        {
            let _catalog = family
                .writer_catalog()
                .map_err(|e| context.error(e, None))?;
            let inspection = recovery::writer_inspect_retained_locked(
                &family,
                &artifacts,
                &expected,
                Some(&control),
            )
            .map_err(|e| context.attach(e))?;
            outcome::publication_ready(inspection).map_err(|e| context.error(e, None))?;
        }
        publication::publish(family, artifacts, Some(control), None, &mut context)
            .map_err(|e| context.attach(e))
    }
    pub fn abandon(&mut self) -> Result<ProviderBootstrapInspection> {
        abandon::run(self)
    }
}
fn attempt_from(
    family: &FamilyAccess,
    artifacts: &Artifacts,
) -> CauseResult<ProviderBootstrapAttempt> {
    let stage = artifacts.allocation.stage_identity()?;
    Ok(ProviderBootstrapAttempt {
        family_root: family.root().to_owned(),
        checkout_root: stage.root().to_owned(),
        stage_parent: stage.stage_parent().to_owned(),
        worktree_id: artifacts.allocation.format.identity.worktree_id.clone(),
        transaction_id: artifacts.allocation.transaction_id.clone(),
    })
}
fn duplicate_binding(binding: &ProviderBootstrapBinding) -> CauseResult<ProviderBootstrapBinding> {
    Ok(ProviderBootstrapBinding::decode(&binding.encode()?)?)
}
