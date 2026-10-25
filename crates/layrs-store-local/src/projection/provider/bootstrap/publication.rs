//! Real control publisher/callback integration. No observed DATA promotion.
use super::*;
use outcome::{ProviderControlCommitDisposition, PublicationPins};
use std::sync::Arc;

pub(super) fn publish(
    family: FamilyAccess,
    mut artifacts: Artifacts,
    retained: Option<RetainedControl>,
    preflight: Option<layrs_vfs::ControlRootPreflight>,
    ctx: &mut WriteContext,
) -> Result<ProviderControlPublicationOutcome> {
    let phase = {
        let _catalog = family.writer_catalog().map_err(|e| ctx.error(e, None))?;
        let binding = artifacts.binding().map_err(|e| ctx.error(e, None))?;
        let inspection = recovery::writer_inspect_retained_locked(
            &family,
            &artifacts,
            &binding,
            retained.as_ref(),
        )
        .map_err(|e| ctx.attach(e))?;
        outcome::publication_ready(inspection)
            .map_err(|e| ctx.error(e, None))?
            .phase
            .ok_or_else(|| ctx.error(invalid("provider phase absent"), None))?
    };
    let stage = artifacts
        .allocation
        .stage_identity()
        .map_err(|e| ctx.error(e.into(), None))?;
    let bytes = artifacts
        .allocation
        .format
        .bytes()
        .map_err(|e| ctx.error(e.into(), None))?;
    let guard = match retained {
        Some(RetainedControl::Final(guard)) => {
            if phase == ProviderBootstrapPhase::Allocating {
                return Err(ctx.error(
                    invalid("allocating plus final control cannot be promoted"),
                    None,
                ));
            }
            // Transfer first; finish brackets the phase acceptance while retaining
            // this same owner even when an already-published conclusion refuses.
            guard // Move the existing sole owner; never acquire a competing one.
        }
        other => {
            // Drop the strong FORMAT observation BEFORE fresh preflight/publisher.
            drop(other);
            let preflight = match preflight {
                Some(p) => p,
                None => layrs_vfs::ControlRootPreflight::open(stage.root())
                    .map_err(|e| ctx.error(e.into(), None))?,
            };
            if preflight.root().as_os_str() != stage.root().as_os_str()
                || preflight.filesystem_identity() != stage.root_filesystem_identity()
            {
                return Err(ctx.error(
                    invalid("fresh publication preflight differs from allocation"),
                    None,
                ));
            }
            if phase == ProviderBootstrapPhase::ControlPublished {
                return Err(ctx.error(invalid("published phase lacks final control"), None));
            }
            let expected = if phase == ProviderBootstrapPhase::ControlPrepared {
                Some(
                    artifacts
                        .prepared
                        .as_ref()
                        .ok_or_else(|| ctx.error(invalid("prepared phase lacks receipt"), None))?
                        .control_identity()
                        .map_err(|e| ctx.error(e.into(), None))?,
                )
            } else {
                None
            };
            let index = ctx
                .arm(
                    ProviderBootstrapWriteStep::ControlPublish,
                    &stage.root().join(".layrs"),
                )
                .map_err(|e| ctx.error(e, None))?;
            let mut callback_cause: Option<Arc<Cause>> = None;
            let mut callback_scope = None;
            let published = layrs_vfs::publish_control_directory_recoverable(
                &preflight,
                stage.stage(),
                &stage,
                &bytes,
                expected.as_ref(),
                |computed| {
                    let result = (|| -> CauseResult<()> {
                        artifacts.verify(&family)?;
                        if artifacts.abandoned.is_some() {
                            return Err(invalid("terminal appeared before provider callback"));
                        }
                        if let Some(p) = &artifacts.prepared {
                            p.require_allocation(&artifacts.allocation)?;
                            if p.control_identity()?.encode() != computed.encode() {
                                return Err(invalid(
                                    "publisher callback differs from authenticated existing prepared receipt",
                                ));
                            }
                        } else {
                            let directory = Path::new(&artifacts.allocation.private.metadata_path)
                                .join("provider-control");
                            let parent = artifacts.writer_control()?;
                            let pending = receipt_io::PendingReceipt::create(
                                &parent,
                                &directory,
                                &artifacts.allocation.transaction_id,
                                ProviderBootstrapReceipt::Prepared,
                                ctx,
                            )?;
                            let prepared = PreparedRecord {
                                schema_version: "layrs.projection-provider-prepared.v1".into(),
                                transaction_id: artifacts.allocation.transaction_id.clone(),
                                receipt_file_identity: pending.identity()?,
                                allocation: artifacts.allocation.data_reference()?,
                                control_identity_hex: allocation::hex(&computed.encode()),
                            };
                            prepared.require_allocation(&artifacts.allocation)?;
                            pending.publish(&parent, &prepared, ctx)?;
                            artifacts.writer_refresh_prepared(&family, &prepared)?;
                        }
                        callback_scope = Some(ProviderDatabaseScope::PrivateMetadata);
                        sql::set_phase(
                            &family,
                            &artifacts,
                            MountPhase::Allocating,
                            MountPhase::ControlPrepared,
                            ctx,
                            || Ok(()),
                        )
                    })();
                    result.map_err(|cause| {
                        let cause = Arc::new(cause);
                        #[cfg(test)]
                        tests::hooks::callback_cause(&cause);
                        callback_cause = Some(Arc::clone(&cause));
                        layrs_vfs::Error::from(std::io::Error::other(report::CallbackCause(cause)))
                    })
                },
                publisher_faults(),
            );
            match published {
                Ok(guard) => {
                    ctx.confirm(index).map_err(|e| ctx.error(e, None))?;
                    guard
                }
                Err(error) => {
                    return Err(match callback_cause {
                        Some(original) => recovery::writer_callback_failure(
                            ctx.operation,
                            error,
                            &original,
                            callback_scope,
                            ctx.report.clone(),
                        ),
                        None => ctx.error(Cause::Vfs(error), None),
                    });
                }
            }
        }
    };
    let binding = artifacts.binding().map_err(|e| ctx.error(e, None))?;
    let held = PublicationPins {
        family,
        artifacts,
        binding,
        guard,
    };
    finish(held, ctx, phase == ProviderBootstrapPhase::ControlPublished)
}

fn finish(
    held: PublicationPins,
    ctx: &mut WriteContext,
    already: bool,
) -> Result<ProviderControlPublicationOutcome> {
    let commit = ProviderBootstrapWriteStep::SqlCommit(ProviderBootstrapSqlWrite::PublishControl);
    let mut before = None;
    let result = (|| -> Result<(ProviderBootstrapInspection, ProviderBootstrapWriteReport)> {
        {
            let _catalog = held
                .family
                .writer_catalog()
                .map_err(|e| ctx.error(e, None))?;
            held.guard
                .revalidate_bootstrap_root()
                .map_err(|e| ctx.error(e.into(), None))?;
            let observation = recovery::writer_inspect_locked(
                &held.family,
                &held.artifacts,
                &held.binding,
                Some(&held.guard),
            )
            .map_err(|e| ctx.attach(e))?;
            let observation =
                outcome::publication_ready(observation).map_err(|e| ctx.error(e, None))?;
            if (observation.phase == Some(ProviderBootstrapPhase::ControlPublished)) != already {
                return Err(ctx.error(
                    invalid("provider phase changed before publication commit"),
                    None,
                ));
            }
            before = Some(observation);
        }
        if !already {
            sql::set_phase(
                &held.family,
                &held.artifacts,
                MountPhase::ControlPrepared,
                MountPhase::ControlPublished,
                ctx,
                || held.guard.revalidate_bootstrap_root().map_err(Cause::from),
            )
            .map_err(|e| ctx.error(e, Some(ProviderDatabaseScope::PrivateMetadata)))?;
        }
        let _catalog = held
            .family
            .writer_catalog()
            .map_err(|e| ctx.error(e, None))?;
        held.guard
            .revalidate_bootstrap_root()
            .map_err(|e| ctx.error(e.into(), None))?;
        let inspection = recovery::writer_inspect_locked(
            &held.family,
            &held.artifacts,
            &held.binding,
            Some(&held.guard),
        )
        .map_err(|e| ctx.attach(e))?;
        let mut inspection =
            outcome::require_published(inspection).map_err(|e| ctx.error(e, None))?;
        held.artifacts
            .verify(&held.family)
            .map_err(|e| ctx.error(e, None))?;
        held.guard
            .revalidate_bootstrap_root()
            .map_err(|e| ctx.error(e.into(), None))?;
        let issuance = ctx
            .report
            .clone()
            .ok_or_else(|| ctx.error(invalid("publication issuance report absent"), None))?;
        inspection.write_report = Some(issuance.clone());
        Ok((inspection, issuance))
    })();
    match result {
        Ok((inspection, issuance)) => Ok(ProviderControlPublicationOutcome::Published(
            ProviderControlPublished {
                held,
                inspection,
                issuance,
            },
        )),
        Err(error) => {
            let error = ctx.attach(error);
            if already || ctx.confirmed(commit) || ctx.armed(commit) {
                let disposition = if already || ctx.confirmed(commit) {
                    ProviderControlCommitDisposition::Committed
                } else {
                    ProviderControlCommitDisposition::Unknown
                };
                let failure = match error {
                    ProjectionStoreError::ProviderBootstrap(f) => *f,
                    other => return Err(other),
                };
                let withheld = ProviderControlWithheld {
                    held,
                    inspection: before,
                    disposition,
                    failure,
                };
                Ok(match disposition {
                    ProviderControlCommitDisposition::Committed => {
                        ProviderControlPublicationOutcome::CommittedCapabilityWithheld(withheld)
                    }
                    ProviderControlCommitDisposition::Unknown => {
                        ProviderControlPublicationOutcome::CommitOutcomeUnknown(withheld)
                    }
                })
            } else {
                Err(error)
            }
        }
    }
}
fn publisher_faults() -> &'static dyn layrs_vfs::ControlDirectoryPublishFaults {
    #[cfg(test)]
    {
        &tests::hooks::PUBLISHER
    }
    #[cfg(not(test))]
    {
        &layrs_vfs::NoControlDirectoryPublishFaults
    }
}
