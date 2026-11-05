//! Connect genuine ordinary source ownership to the retained destination pins.
//! Source Copying and C Initializing are separate durable, exactly retryable
//! boundaries. Neither state admits C or authorizes content reservations.
use super::allocation::ManagedDestinationAllocation;
use super::*;
use crate::projection::{
    provider::{
        admitted_catalog_io, admitted_ordinary, ordinary_transition,
        records::Record,
        recovery::error::{ProviderBootstrapCause as Cause, Result as CauseResult},
        transition_sql::SourceIntentEffect,
    },
    shared::{
        admitted_catalog::AdmittedFamilyContext,
        provider_access::provider_import::{
            ImportRegistrationEffect, source::OrdinarySourceInputs,
        },
    },
};

impl OrdinaryManagedExport<'_> {
    /// The caller retains `destination` even if commit or positive source close
    /// is uncertain. Its actual pins, not the copied proof DATA, authorize this
    /// source journal advance. Repeating this exact transition is read-only.
    pub(in crate::projection::provider) fn commit_allocation(
        &mut self,
        planned: &ProviderTransitionIntentRecord,
        destination: &ManagedDestinationAllocation,
        cancel: &Cancellation,
    ) -> Result<(ProviderTransitionIntentRecord, SourceIntentEffect)> {
        check_cancel(cancel)?;
        if planned.state != TransitionState::Planned {
            return Err(invalid(
                "allocation commit requires the original Planned intent",
            ));
        }
        destination.verify(&planned.plan)?;
        let mut copying = ProviderTransitionIntentRecord::decode(&planned.encode()?)?;
        copying.state = TransitionState::Copying;
        copying.destination = Some(
            serde_json::from_slice(
                &canonical_json(destination.proof()).map_err(|error| invalid(error.to_string()))?,
            )
            .map_err(|error| invalid(error.to_string()))?,
        );
        copying.require_transition(planned)?;
        let store = self.store;
        self.with_source_transaction(cancel, true, |c, _, prefix, _, _| {
            destination.verify(&planned.plan)?;
            let effect = ordinary_transition::apply_copying(
                c,
                &store.marker,
                prefix,
                &copying,
                Some(cancel),
            )?;
            destination.verify(&planned.plan)?;
            Ok((copying, effect))
        })
    }

    /// Register only the exact pinned destination after same-inode FORMAT
    /// publication. The family writer owns its own Immediate transaction and
    /// observable close while the source snapshot and destination pins remain.
    pub(in crate::projection::provider) fn register_allocation(
        &mut self,
        destination: &ManagedDestinationAllocation,
        copying: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> Result<ImportRegistrationEffect> {
        check_cancel(cancel)?;
        if copying.state != TransitionState::Copying
            || copying.destination.as_ref() != Some(destination.proof())
        {
            return Err(invalid("ordinary registration differs from pinned Copying"));
        }
        destination.require_published(&copying.plan)?;
        let store = self.store;
        self.with_source(cancel, |c, _, prefix, closure, _| {
            copying.require_prefix(prefix)?;
            let admitted = read_admitted_import(c, &store.marker, Some(cancel))?;
            let actual = admitted_ordinary::inspect(c, &store.marker, Some(cancel))?;
            let retained = AdmittedFamilyContext::new(&actual)?;
            let source = OrdinarySourceInputs::new(c, &store.marker, &admitted, closure, &retained);
            let family = destination.family();
            let gate = family.writer_catalog()?;
            destination.require_published(&copying.plan)?;
            let result = admitted_catalog_io::with_catalog(
                &family.catalog,
                true,
                |catalog| {
                    destination.require_published(&copying.plan)?;
                    family.validate_ordinary_import_initializing(
                        catalog,
                        &source,
                        copying,
                        Some(cancel),
                    )?;
                    Ok(())
                },
                |catalog| {
                    let tx = catalog.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let result: CauseResult<ImportRegistrationEffect> = (|| {
                        destination.require_published(&copying.plan)?;
                        let effect = family.register_ordinary_import_initializing(
                            &tx,
                            &source,
                            copying,
                            Some(cancel),
                        )?;
                        admitted.require(c, Some(cancel))?;
                        destination.require_published(&copying.plan)?;
                        cancel.check()?;
                        Ok(effect)
                    })();
                    finish_writer(tx, result.map_err(ProjectionStoreError::from))
                        .map_err(Cause::from)
                },
            );
            let recheck = destination.require_published(&copying.plan);
            drop(gate);
            match (result, recheck) {
                (Ok(effect), Ok(())) => Ok(effect),
                (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error.into()),
                (Err(primary), Err(close)) => Err(Cause::Cleanup {
                    primary: Box::new(primary),
                    close: Box::new(close),
                }
                .into()),
            }
        })
    }
}
