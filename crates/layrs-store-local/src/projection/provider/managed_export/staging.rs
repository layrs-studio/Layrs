//! Ordinary metadata Staging driver. Both durable rows precede every family
//! reservation. B's owner/image and C's independently allocated pins stay held;
//! only actual source rows authorize inspection of a destination phase gap.
use super::{allocation::ManagedDestinationAllocation, *};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            admitted_ordinary,
            database_io::{self, ReadState, WriterCloseObserver},
            ordinary_destination::{self, staging as destination_staging, OrdinaryCopyInputs},
            ordinary_transition,
            records::Record,
            recovery::error::{
                invalid as cause_invalid, ProviderBootstrapCause as Cause, Result as CauseResult,
            },
            transition_sql::SourceIntentEffect,
        },
        shared::{
            admitted_catalog::AdmittedFamilyContext,
            provider_access::{provider_import::source::OrdinarySourceInputs, ImportRegistration},
        },
    },
};

pub(in crate::projection::provider) struct StagingOutcome {
    pub intent: ProviderTransitionIntentRecord,
    pub source: SourceIntentEffect,
    pub destination: destination_staging::StageEffect,
}

impl OrdinaryManagedExport<'_> {
    pub(in crate::projection::provider) fn stage_metadata(
        &mut self,
        destination: &ManagedDestinationAllocation,
        copying: &ProviderTransitionIntentRecord,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<StagingOutcome> {
        check_cancel(cancel)?;
        if copying.destination.as_ref() != Some(destination.proof()) {
            return Err(invalid(
                "ordinary Staging differs from retained destination",
            ));
        }
        destination.require_published(&copying.plan)?;
        let staging = ordinary_transition::proposed_staging(copying)?;
        // The funded planner runs against the genuine retained source/image.
        // Its complete deterministic plan must equal the immutable durable
        // plan; an older underfunded export is refused, never silently enlarged.
        // This does not set validated_metadata_floor for a reservation writer.
        let derived = self.derive_plan(
            &copying.plan.destination_parent,
            &copying.plan.destination_parent_identity,
            &copying.plan.destination_leaf_nonce,
            &copying.transaction_id,
            cancel,
        )?;
        if derived.plan != copying.plan {
            return Err(invalid(
                "ordinary Staging plan differs from funded source image",
            ));
        }
        let store = self.store;
        let source_effect =
            self.with_source_transaction(cancel, true, |c, _, prefix, closure, _| {
                destination.require_published(&copying.plan)?;
                let admitted = read_admitted_import(c, &store.marker, Some(cancel))?;
                let actual = admitted
                    .outgoing()
                    .iter()
                    .find(|row| row.transaction_id == copying.transaction_id)
                    .ok_or_else(|| invalid("ordinary Staging source intent absent"))?;
                if actual != copying && actual != &staging {
                    return Err(invalid("ordinary Staging source predecessor differs"));
                }
                let family = destination.family();
                let gate = StoreLock::acquire_existing(
                    &family.root().join("metadata/catalog.lock"),
                    SessionMode::Shared,
                )
                .map_err(Cause::from)?;
                family_check(destination, c, &store.marker, closure, actual, cancel)?;
                let input = OrdinaryCopyInputs::new(c, &store.marker, actual, closure);
                let state = database_io::read_cancellable(
                    destination.destination_database(),
                    cancel,
                    |target| {
                        if actual.state == TransitionState::Copying {
                            ordinary_destination::read_copied(
                                target,
                                &destination.proof().format,
                                &input,
                                Some(cancel),
                            )?;
                        } else {
                            destination_staging::read_stageable(
                                target,
                                &destination.proof().format,
                                &input,
                                Some(cancel),
                            )?;
                        }
                        Ok(())
                    },
                )?;
                valid(state)?;
                let result = ordinary_transition::apply_staging(
                    c,
                    &store.marker,
                    prefix,
                    &staging,
                    Some(cancel),
                );
                // A failed update can leave the old phase; a failed nested
                // close must not be followed by another SQL probe. Revalidate
                // only physical pins on errors before the caller rolls back.
                let post = if result.is_ok() {
                    family_check(destination, c, &store.marker, closure, &staging, cancel)
                } else {
                    destination.require_published(&staging.plan)
                };
                drop(gate);
                combine(result, post).map_err(ProjectionStoreError::from)
            })?;

        let destination_effect = self.with_source(cancel, |source, _, _, closure, _| {
            destination.require_published(&staging.plan)?;
            let input = OrdinaryCopyInputs::new(source, &store.marker, &staging, closure);
            let family = destination.family();
            let gate = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"), SessionMode::Shared,
            ).map_err(Cause::from)?;
            let result = (|| -> CauseResult<destination_staging::StageEffect> {
                family_check(destination, source, &store.marker, closure, &staging, cancel)?;
                let file = destination.destination_database();
                let marker = &destination.proof().format;
                let prior = valid(database_io::read_cancellable(file, cancel, |target| {
                    destination_staging::read_stageable(target, marker, &input, Some(cancel))
                })?)?;
                if prior == TransitionState::Staging {
                    return Ok(destination_staging::StageEffect::AlreadyPresent);
                }
                let admission = database_io::clean_admission(file, false, |target| {
                    destination_staging::read_stageable(target, marker, &input, Some(cancel))?;
                    Ok(())
                })?;
                let bare = admission.open(file, close)?;
                let written = (|| -> CauseResult<destination_staging::StageEffect> {
                    admission.require_geometry(&bare)?;
                    destination.require_published(&staging.plan)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                    let effect = destination_staging::apply_staging(c, marker, &input, Some(cancel))?;
                    family_check(destination, source, &store.marker, closure, &staging, cancel)?;
                    cancel.check()?;
                    c.execute_batch("COMMIT")?;
                    Ok(effect)
                })();
                let written = if bare.connection().is_autocommit() { written } else {
                    combine(written, bare.connection().execute_batch("ROLLBACK").map_err(Cause::from))
                };
                let effect = bare.finish_writer(written, close)?;
                let after = valid(database_io::read_cancellable(file, cancel, |target| {
                    destination_staging::read_stageable(target, marker, &input, Some(cancel))
                })?)?;
                if after != TransitionState::Staging {
                    return Err(cause_invalid("ordinary destination Staging readback differs"));
                }
                Ok(effect)
            })();
            // Positive close acknowledgement is a fence for every subsequent
            // SQL open, including the family catalog. Error revalidation is
            // physical-only; a later explicit retry authenticates durable SQL.
            let post = if result.is_ok() {
                family_check(destination, source, &store.marker, closure, &staging, cancel)
            } else {
                destination.require_published(&staging.plan)
            };
            drop(gate);
            combine(result, post).map_err(ProjectionStoreError::from)
        })?;
        destination.require_published(&staging.plan)?;
        Ok(StagingOutcome {
            intent: staging,
            source: source_effect,
            destination: destination_effect,
        })
    }
}

/// Caller holds the actual family catalog gate. Reinterpret source admission
/// at every call, including after SourceStaging changes that same transaction.
fn family_check(
    destination: &ManagedDestinationAllocation,
    source: &Connection,
    marker: &format::Marker,
    closure: &VerifiedImportClosure,
    intent: &ProviderTransitionIntentRecord,
    cancel: &Cancellation,
) -> CauseResult<()> {
    destination.require_published(&intent.plan)?;
    let admitted = read_admitted_import(source, marker, Some(cancel))?;
    let actual = admitted_ordinary::inspect(source, marker, Some(cancel))?;
    let retained = AdmittedFamilyContext::new(&actual)?;
    let inputs = OrdinarySourceInputs::new(source, marker, &admitted, closure, &retained);
    let family = destination.family();
    let state = valid(database_io::read_cancellable(
        &family.catalog,
        cancel,
        |c| match intent.state {
            TransitionState::Copying => {
                family.validate_ordinary_import_initializing(c, &inputs, intent, Some(cancel))
            }
            TransitionState::Staging => {
                family.validate_ordinary_import_unreserved_staging(c, &inputs, intent, Some(cancel))
            }
            _ => Err(cause_invalid(
                "ordinary family Staging source phase differs",
            )),
        },
    )?)?;
    if state != ImportRegistration::Initializing {
        return Err(cause_invalid(
            "ordinary Staging requires exact Initializing member",
        ));
    }
    destination.require_published(&intent.plan)?;
    cancel.check()?;
    Ok(())
}

fn valid<T>(state: ReadState<T>) -> CauseResult<T> {
    match state {
        ReadState::Valid(value) => Ok(value),
        ReadState::Uninitialized => Err(cause_invalid(
            "ordinary Staging destination is uninitialized",
        )),
        ReadState::Hot(hot) => Err(Cause::ReadonlyRollback(hot.trigger)),
        ReadState::UnboundRollback { trigger, cause } => Err(Cause::Cleanup {
            primary: Box::new(Cause::ReadonlyRollback(trigger)),
            close: Box::new(cause),
        }),
        ReadState::Rejected { cause, .. }
        | ReadState::ReadButUnavailable(cause)
        | ReadState::Unavailable(cause) => Err(cause),
    }
}

fn combine<T>(result: CauseResult<T>, post: CauseResult<()>) -> CauseResult<T> {
    match (result, post) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(close)) => Err(Cause::Cleanup {
            primary: Box::new(primary),
            close: Box::new(close),
        }),
    }
}
