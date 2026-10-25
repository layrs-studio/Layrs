//! Terminal-first removal of proven initial intent; never delete physical data.
use super::*;

fn unregistered(control: &RetainedControl, artifacts: &Artifacts) -> CauseResult<()> {
    control.revalidate(artifacts)?;
    if let RetainedControl::Final(guard) = control {
        guard.revalidate_unregistered_root()?;
    }
    Ok(())
}
fn private_state(artifacts: &Artifacts) -> CauseResult<validation::PrivateValidation> {
    match database_io::read(&artifacts.database, |c| {
        validation::private(c, &artifacts.allocation, artifacts.prepared.as_ref())
    })? {
        database_io::ReadState::Uninitialized => Ok(validation::PrivateValidation::Uninitialized),
        state => sql::known(state, ProviderDatabaseScope::PrivateMetadata),
    }
}
fn eligible(
    registration: Registration,
    private: &validation::PrivateValidation,
    artifacts: &Artifacts,
) -> CauseResult<()> {
    if let Some(t) = &artifacts.abandoned {
        if t.eligibility.registry_at_decision == RegistryAtDecision::Absent
            && registration != Registration::Absent
        {
            return Err(invalid(
                "terminal decided absent but target registration now exists",
            ));
        }
        let mount = match private {
            validation::PrivateValidation::Uninitialized => None,
            validation::PrivateValidation::Initialized(m) => Some(m),
        };
        t.require_references(&artifacts.allocation, artifacts.prepared.as_ref(), mount)?;
    }
    Ok(())
}

pub(super) fn run(recovery: &mut ProviderBootstrapRecovery) -> Result<ProviderBootstrapInspection> {
    let (family, artifacts, expected, control) = recovery.writer_parts_mut();
    let attempt = attempt_from(family, artifacts)
        .map_err(|e| WriteContext::empty("abandon provider bootstrap").error(e, None))?;
    let mut ctx = WriteContext::current(ProviderBootstrapWriterOperation::Abandon, attempt);
    let result = (|| -> Result<ProviderBootstrapInspection> {
        let _lifecycle = family
            .writer_lifecycle(true)
            .map_err(|e| ctx.error(e, None))?;
        let _owner = family
            .writer_owner(&artifacts.allocation.format.identity.worktree_id)
            .map_err(|e| ctx.error(e, None))?;
        let _catalog = family.writer_catalog().map_err(|e| ctx.error(e, None))?;
        // Independently probes both databases and preserves pending causes; no
        // uninitialized inference is permitted from an unavailable/hot result.
        let mut inspection =
            recovery::writer_inspect_retained_locked(family, artifacts, expected, Some(control))
                .map_err(|e| ctx.attach(e))?;
        let refused = if inspection.family.status != ProviderDatabaseStatus::Validated {
            Some((
                inspection
                    .family
                    .cause
                    .take()
                    .unwrap_or_else(|| invalid("abandon family SQL unavailable")),
                ProviderDatabaseScope::FamilyCatalog,
            ))
        } else if !matches!(
            inspection.private.status,
            ProviderDatabaseStatus::Validated | ProviderDatabaseStatus::Uninitialized
        ) {
            Some((
                inspection
                    .private
                    .cause
                    .take()
                    .unwrap_or_else(|| invalid("abandon private SQL unavailable")),
                ProviderDatabaseScope::PrivateMetadata,
            ))
        } else {
            None
        };
        if let Some((cause, scope)) = refused {
            return Err(ctx.attach(recovery::writer_reject_inspection(
                inspection,
                cause,
                Some(scope),
            )));
        }
        unregistered(control, artifacts).map_err(|e| ctx.error(e, None))?;
        let registration = sql::known(
            database_io::read(&family.catalog, |c| {
                family.validate_catalog(c, &artifacts.allocation)
            })
            .map_err(|e| ctx.error(e, Some(ProviderDatabaseScope::FamilyCatalog)))?,
            ProviderDatabaseScope::FamilyCatalog,
        )
        .map_err(|e| ctx.error(e, Some(ProviderDatabaseScope::FamilyCatalog)))?;
        let private = private_state(artifacts)
            .map_err(|e| ctx.error(e, Some(ProviderDatabaseScope::PrivateMetadata)))?;
        eligible(registration, &private, artifacts).map_err(|e| ctx.error(e, None))?;
        if artifacts.abandoned.is_none() {
            let eligibility = EligibilityRecord {
                schema_version: "layrs.projection-provider-bootstrap-eligibility.v1".into(),
                registry_at_decision: match registration {
                    Registration::Absent => RegistryAtDecision::Absent,
                    Registration::Initializing => RegistryAtDecision::Initializing,
                },
                private_state: match &private {
                    validation::PrivateValidation::Uninitialized => PrivateState::Uninitialized,
                    validation::PrivateValidation::Initialized(m) => PrivateState::InitialSchema8 {
                        catalog_id: artifacts.allocation.source.catalog_id.clone(),
                        snapshot_digest: artifacts.allocation.source.snapshot_digest.clone(),
                        selection_digest: artifacts.allocation.source.selection_digest.clone(),
                        mount_digest: m.digest().map_err(|e| ctx.error(e.into(), None))?,
                    },
                },
            };
            artifacts.verify(family).map_err(|e| ctx.error(e, None))?;
            unregistered(control, artifacts).map_err(|e| ctx.error(e, None))?;
            let directory =
                Path::new(&artifacts.allocation.private.metadata_path).join("provider-control");
            let parent = artifacts.writer_control().map_err(|e| ctx.error(e, None))?;
            let pending = receipt_io::PendingReceipt::create(
                &parent,
                &directory,
                &artifacts.allocation.transaction_id,
                ProviderBootstrapReceipt::Abandoned,
                &mut ctx,
            )
            .map_err(|e| ctx.error(e, None))?;
            let terminal = AbandonedRecord {
                schema_version: "layrs.projection-provider-abandoned.v1".into(),
                transaction_id: artifacts.allocation.transaction_id.clone(),
                receipt_file_identity: pending.identity().map_err(|e| ctx.error(e, None))?,
                allocation: artifacts
                    .allocation
                    .data_reference()
                    .map_err(|e| ctx.error(e.into(), None))?,
                prepared: artifacts
                    .prepared
                    .as_ref()
                    .map(Receipt::data_reference)
                    .transpose()
                    .map_err(|e| ctx.error(e.into(), None))?,
                eligibility_digest: eligibility
                    .digest()
                    .map_err(|e| ctx.error(e.into(), None))?,
                eligibility,
            };
            terminal
                .require_references(
                    &artifacts.allocation,
                    artifacts.prepared.as_ref(),
                    match &private {
                        validation::PrivateValidation::Initialized(m) => Some(m),
                        _ => None,
                    },
                )
                .map_err(|e| ctx.error(e.into(), None))?;
            pending
                .publish(&parent, &terminal, &mut ctx)
                .map_err(|e| ctx.error(e, None))?;
            artifacts
                .writer_refresh_terminal(family, &terminal)
                .map_err(|e| ctx.error(e, None))?;
        }
        // Revalidate ownership/eligibility after T publication and before any
        // registry write. Unknown publication never reaches this point.
        unregistered(control, artifacts).map_err(|e| ctx.error(e, None))?;
        let now = private_state(artifacts)
            .map_err(|e| ctx.error(e, Some(ProviderDatabaseScope::PrivateMetadata)))?;
        eligible(registration, &now, artifacts).map_err(|e| ctx.error(e, None))?;
        if registration == Registration::Initializing {
            sql::remove_initializing_locked(family, artifacts, &mut ctx, || {
                unregistered(control, artifacts)?;
                let current = private_state(artifacts)?;
                eligible(Registration::Initializing, &current, artifacts)
            })
            .map_err(|e| ctx.error(e, Some(ProviderDatabaseScope::FamilyCatalog)))?;
        }
        let binding = artifacts.binding().map_err(|e| ctx.error(e, None))?;
        let mut inspection =
            recovery::writer_inspect_retained_locked(family, artifacts, &binding, Some(control))
                .map_err(|e| ctx.attach(e))?;
        if inspection.state != ProviderBootstrapState::Abandoned {
            let cause = inspection
                .family
                .cause
                .take()
                .or_else(|| inspection.private.cause.take())
                .unwrap_or_else(|| {
                    invalid("abandon conclusion did not validate terminal registry absence")
                });
            return Err(ctx.attach(recovery::writer_reject_inspection(inspection, cause, None)));
        }
        unregistered(control, artifacts).map_err(|e| ctx.error(e, None))?;
        inspection.write_report = ctx.report.clone();
        // Only a completely accepted successful call refreshes expected binding.
        *expected = duplicate_binding(&binding).map_err(|e| ctx.error(e, None))?;
        Ok(inspection)
    })();
    result.map_err(|e| ctx.attach(e))
}
