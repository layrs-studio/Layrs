//! Consuming source-free provider preparation. A failed attempt retains the
//! original Store pins and any completed native proof in a withheld holder.
pub(super) mod activation;
mod ranges;
mod reopen;
mod report;
mod sql;
#[cfg(test)]
pub(super) mod tests;

use super::*;
use crate::projection::provider::records::preparation::{
    PreparationState, ProviderPreparationRecord,
};
use std::{path::Path, sync::Arc};

pub use activation::{
    ProviderActivationFailure, ProviderActivationFailureKind, ProviderActivationOutcome,
    ProviderActivationSqlPhase, ProviderActivationWithheld, ProviderActivationWriteEffect,
    ProviderActivationWriteReport, ProviderActivationWriteReportProvenance,
    ProviderActivationWriteState, ProviderActivationWriteStep, ProviderActive,
    ProviderActiveOpenFailure, ProviderActiveRangeFailure, ProviderActiveReopened,
    ProviderActiveSession, ProviderActiveSessionCause, ProviderActiveSessionWithheld,
    ProviderCaptureAcknowledgement, ProviderCaptureFailure, ProviderCaptureStep,
    ProviderRecoveryExportCause, ProviderRecoveryExportClass, ProviderRecoveryExportDeletion,
    ProviderRecoveryExportFailure, ProviderRecoveryExportFile, ProviderRecoveryExportGap,
    ProviderRecoveryExportGapKind, ProviderRecoveryExportOutcome, ProviderRecoveryExportReport,
    ProviderRecoveryExportStep, ProviderTransitionSource,
};
pub use ranges::ProviderRangeFailure;
pub use reopen::{ProviderPreparedOpenFailure, ProviderPreparedReopened};
pub use report::{
    ProviderPreparationFailure, ProviderPreparationFailureKind, ProviderPreparationSqlPhase,
    ProviderPreparationWriteEffect, ProviderPreparationWriteReport,
    ProviderPreparationWriteReportProvenance, ProviderPreparationWriteState,
    ProviderPreparationWriteStep,
};

pub enum ProviderPreparationOutcome {
    Prepared(ProviderPrepared),
    Withheld(ProviderPreparationWithheld),
}

/// The returned root proof belongs to this authenticated Store holder. This
/// does not expose a source, local content lease, activation map or recovery.
pub struct ProviderPrepared {
    held: outcome::PublicationPins,
    root: layrs_vfs::ProviderRootGuard,
    record: ProviderPreparationRecord,
    report: ProviderPreparationWriteReport,
}

/// Retains the sole Store owner after any refused or uncertain attempt. A
/// native proof may exist even when the later SQL commit or close was refused.
pub struct ProviderPreparationWithheld {
    held: outcome::PublicationPins,
    root: Option<layrs_vfs::ProviderRootGuard>,
    pending_candidate: Option<ProviderPreparationRecord>,
    prepared_candidate: Option<ProviderPreparationRecord>,
    failure: ProviderPreparationFailure,
}

impl ProviderPrepared {
    /// Consumes the prepared owner while advancing the fixed source to generation one.
    pub fn activate(self) -> ProviderActivationOutcome {
        activation::activate(self.held, self.root, self.record, None)
    }
    pub fn root(&self) -> &Path {
        self.root.root()
    }
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.held.binding
    }
    pub fn catalog_id(&self) -> &str {
        &self.record.source.catalog_id
    }
    pub fn report(&self) -> &ProviderPreparationWriteReport {
        &self.report
    }
    pub fn revalidate(&self) -> std::result::Result<(), ProviderPreparationFailure> {
        let context = report::Context::retained(&self.report);
        let result = (|| -> std::result::Result<(), report::Failure> {
            let _locks = self
                .held
                .family
                .locks(
                    &self.held.artifacts.allocation.format.identity.worktree_id,
                    false,
                )
                .map_err(report::Failure::from)?;
            sql::final_check(
                &self.held,
                &self.record,
                &self.root,
                self.root.registration().root_id(),
            )
        })();
        result.map_err(|e| context.failure(e))
    }
    /// Reads only already resident, fully verified bytes from the fixed source.
    /// The callback buffer remains unchanged if the read cannot be completed.
    pub fn read_resident_range(
        &self,
        file_id: &layrs_core::scm::FileId,
        offset: u64,
        buffer: &mut [u8],
        cancel: &layrs_vfs::Cancellation,
    ) -> std::result::Result<(), ProviderRangeFailure> {
        ranges::read(
            &self.held,
            &self.record,
            &self.root,
            file_id,
            offset,
            buffer,
            cancel,
        )
    }
}
impl ProviderPreparationWithheld {
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.held.binding
    }
    pub fn failure(&self) -> &ProviderPreparationFailure {
        &self.failure
    }
    pub fn retained_native_ready_proof(&self) -> bool {
        self.root.is_some()
    }
    pub fn pending_candidate_retained(&self) -> bool {
        self.pending_candidate.is_some()
    }
    pub fn prepared_candidate_retained(&self) -> bool {
        self.prepared_candidate.is_some()
    }
    #[cfg(test)]
    pub(in crate::projection::provider::bootstrap) fn test_revalidate_unregistered_root(
        &self,
    ) -> layrs_vfs::Result<()> {
        self.held.guard.revalidate_bootstrap_root()
    }
}

impl ProviderControlPublished {
    /// Consumes the authenticated Published holder. The caller must quiesce
    /// namespace, metadata and registration mutations across native audit/READY, as
    /// required by the VFS factory. Store gates serialize cooperating writers.
    /// All refusal paths return an owner holding the original Store pins.
    pub fn prepare_provider(self, root_id: [u8; 16]) -> ProviderPreparationOutcome {
        let mut context = report::Context::new(&self.issuance);
        let held = self.held;
        let mut root = None;
        let mut pending_candidate = None;
        let mut prepared_candidate = None;
        let result = (|| -> std::result::Result<(), report::Failure> {
            let _locks = held
                .family
                .locks(&held.artifacts.allocation.format.identity.worktree_id, true)
                .map_err(report::Failure::from)?;
            if root_id == [0; 16] {
                return Err(invalid("provider root ID is zero").into());
            }
            held.guard
                .revalidate_bootstrap_root()
                .map_err(|e| report::Failure::new(e.into(), None))?;
            held.artifacts
                .verify(&held.family)
                .map_err(report::Failure::from)?;
            held.artifacts
                .writer_require_guard(&held.guard)
                .map_err(report::Failure::from)?;
            if held.artifacts.binding().map_err(report::Failure::from)? != held.binding {
                return Err(invalid("published preparation binding changed").into());
            }
            let inspection = recovery::writer_inspect_locked(
                &held.family,
                &held.artifacts,
                &held.binding,
                Some(&held.guard),
            )
            .map_err(report::Failure::store)?;
            outcome::require_published(inspection).map_err(report::Failure::from)?;
            let initial = database_io::read(&held.artifacts.database, |c| {
                validation::private_with_source(
                    c,
                    &held.artifacts.allocation,
                    held.artifacts.prepared.as_ref(),
                )
            })
            .map_err(report::Failure::private)?;
            let (mount, source) = match sql::read_state(initial)? {
                validation::PrivateSourceValidation::Initialized { mount, source }
                    if mount.phase == MountPhase::ControlPublished =>
                {
                    (mount, source)
                }
                _ => {
                    return Err(report::Failure::private(invalid(
                        "preparation requires published private mount",
                    )));
                }
            };
            source
                .require_commitment(&held.artifacts.allocation.source)
                .map_err(|e| report::Failure::private(e.into()))?;
            held.guard
                .revalidate_bootstrap_root()
                .map_err(|e| report::Failure::new(e.into(), None))?;
            held.artifacts
                .verify(&held.family)
                .map_err(report::Failure::from)?;
            let catalog = Arc::new(source.vfs_catalog().clone());
            let index = context
                .arm(
                    report::ProviderPreparationWriteStep::NativePopulation,
                    held.guard.root(),
                )
                .map_err(report::Failure::from)?;
            let mut callback_error = None;
            let created = layrs_vfs::ProviderRootGuard::create_with_control_directory(
                &held.guard,
                root_id,
                catalog,
                |registration| {
                    let pending = make_pending(&held, &mount, &source, root_id, registration)
                        .map_err(|e| report::Failure::private(e.into()));
                    let result = pending.and_then(|pending| {
                        pending_candidate = Some(pending);
                        sql::transition(
                            &held,
                            sql::Predecessor::Published(&mount),
                            pending_candidate.as_ref().expect("candidate retained"),
                            root_id,
                            None,
                            &mut context,
                        )
                    });
                    result.map_err(|error| {
                        callback_error = Some(error);
                        layrs_vfs::Error::Unsupported(
                            "Store registration ticket persistence refused",
                        )
                    })
                },
            );
            match created {
                Ok(proof) => {
                    root = Some(proof);
                    context.confirm(index).map_err(report::Failure::from)?;
                }
                Err(error) => {
                    if let Some(original) = callback_error {
                        context.no_effect(index); // callback precedes first native registration
                        return Err(original);
                    }
                    return Err(report::Failure::new(Cause::Vfs(error), None));
                }
            }
            let pending = pending_candidate.as_ref().ok_or_else(|| {
                report::Failure::private(invalid("native READY lacks retained pending ticket"))
            })?;
            let proof = root
                .as_ref()
                .ok_or_else(|| report::Failure::new(invalid("native READY proof absent"), None))?;
            if pending.registration_ticket_hex != allocation::hex(&proof.registration().encode()) {
                return Err(report::Failure::private(invalid(
                    "native READY ticket differs from pending",
                )));
            }
            let mut prepared = ProviderPreparationRecord::decode(
                &pending
                    .encode()
                    .map_err(|e| report::Failure::private(e.into()))?,
            )
            .map_err(|e| report::Failure::private(e.into()))?;
            prepared.state = PreparationState::ProviderPrepared {
                pending_digest: pending
                    .digest()
                    .map_err(|e| report::Failure::private(e.into()))?,
            };
            prepared
                .require_pending(pending)
                .map_err(|e| report::Failure::private(e.into()))?;
            prepared_candidate = Some(prepared);
            sql::transition(
                &held,
                sql::Predecessor::Pending(pending),
                prepared_candidate.as_ref().expect("candidate retained"),
                root_id,
                Some(proof),
                &mut context,
            )?;
            sql::final_check(
                &held,
                prepared_candidate.as_ref().expect("candidate retained"),
                proof,
                root_id,
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => ProviderPreparationOutcome::Prepared(ProviderPrepared {
                held,
                root: root.expect("READY proof retained on successful preparation"),
                record: prepared_candidate.expect("prepared record retained after final check"),
                report: context.report,
            }),
            Err(error) => ProviderPreparationOutcome::Withheld(ProviderPreparationWithheld {
                held,
                root,
                pending_candidate,
                prepared_candidate,
                failure: context.failure(error),
            }),
        }
    }
}

fn make_pending(
    held: &outcome::PublicationPins,
    mount: &MountRecord,
    source: &ProviderBootstrapSource,
    root_id: [u8; 16],
    ticket: &layrs_vfs::Registration,
) -> crate::projection::Result<ProviderPreparationRecord> {
    let record = ProviderPreparationRecord {
        schema_version: "layrs.projection-provider-mount.v2".into(),
        bootstrap: MountRecord::decode(&mount.encode()?)?,
        binding: ProviderBootstrapBinding::decode(&held.binding.encode()?)?,
        source: serde_json::from_slice(
            &serde_json::to_vec(source.commitment()).map_err(crate::projection::invalid)?,
        )
        .map_err(crate::projection::invalid)?,
        root_id_hex: allocation::hex(&root_id),
        registration_ticket_hex: allocation::hex(&ticket.encode()),
        state: PreparationState::RegistrationPending,
    };
    record.require_references(
        &held.artifacts.allocation,
        held.artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| crate::projection::invalid("preparation P absent"))?,
        source,
        root_id,
    )?;
    record.encode()?;
    Ok(record)
}
