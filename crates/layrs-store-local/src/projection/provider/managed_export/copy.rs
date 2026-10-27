//! Physical ordinary C8 copy driver. B's actual snapshot, immutable image and
//! C's independent pins survive every destination effect and observable close.
use super::{allocation::ManagedDestinationAllocation, *};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            admitted_ordinary,
            database_io::{self, ReadState, WriterCloseObserver},
            ordinary_destination::{self, CopyEffect, OrdinaryCopyInputs},
            recovery::error::{
                invalid as cause_invalid, ProviderBootstrapCause as Cause, Result as CauseResult,
            },
        },
        shared::{
            admitted_catalog::AdmittedFamilyContext,
            provider_access::{provider_import::source::OrdinarySourceInputs, ImportRegistration},
        },
    },
};

impl OrdinaryManagedExport<'_> {
    pub(in crate::projection::provider) fn copy_metadata(
        &mut self,
        destination: &ManagedDestinationAllocation,
        copying: &ProviderTransitionIntentRecord,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<CopyEffect> {
        check_cancel(cancel)?;
        if copying.state != TransitionState::Copying
            || copying.destination.as_ref() != Some(destination.proof())
        {
            return Err(invalid("ordinary copy differs from retained destination"));
        }
        destination.require_published(&copying.plan)?;
        let store = self.store;
        self.with_source(cancel, |source_c, _, prefix, closure, _| {
            copying.require_prefix(prefix)?;
            let admitted = read_admitted_import(source_c, &store.marker, Some(cancel))?;
            let actual = admitted_ordinary::inspect(source_c, &store.marker, Some(cancel))?;
            let retained = AdmittedFamilyContext::new(&actual)?;
            let source = OrdinarySourceInputs::new(source_c, &store.marker, &admitted, closure, &retained);
            let input = OrdinaryCopyInputs::new(source_c, &store.marker, copying, closure);
            let family = destination.family();
            let gate = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"), SessionMode::Shared,
            ).map_err(Cause::from)?;
            let family_check = || -> CauseResult<()> {
                destination.require_published(&copying.plan)?;
                let state = database_io::read_cancellable(&family.catalog, cancel, |c| {
                    family.validate_ordinary_import_initializing(c, &source, copying, Some(cancel))
                })?;
                if valid(state)? != Some(ImportRegistration::Initializing) {
                    return Err(cause_invalid("ordinary copy requires exact Initializing member"));
                }
                Ok(())
            };
            let result = (|| -> CauseResult<CopyEffect> {
                family_check()?;
                let file = destination.destination_database();
                let marker = &destination.proof().format;
                database_io::validate_native_filename(file.path())?;
                let prior = database_io::read_cancellable(file, cancel, |c| {
                    ordinary_destination::read_copied(c, marker, &input, Some(cancel))
                })?;
                if valid(prior)?.is_some() {
                    family_check()?;
                    return Ok(CopyEffect::AlreadyPresent);
                }
                let admission = database_io::clean_admission(file, true, |_| {
                    Err(cause_invalid("ordinary fresh copy admission became nonempty"))
                })?;
                let bare = admission.open(file, close)?;
                let copied = (|| -> CauseResult<CopyEffect> {
                    admission.require_geometry(&bare)?;
                    destination.require_published(&copying.plan)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                    let (_, effect) = ordinary_destination::initialize_or_replay_copied(
                        c, marker, &input, Some(cancel),
                    )?;
                    admitted.require(source_c, Some(cancel))?;
                    destination.require_published(&copying.plan)?;
                    cancel.check()?;
                    c.execute_batch("COMMIT")?;
                    Ok(effect)
                })();
                // Never rely on connection destruction to acknowledge rollback.
                let copied = if bare.connection().is_autocommit() {
                    copied
                } else {
                    combine(copied, bare.connection().execute_batch("ROLLBACK").map_err(Cause::from))
                };
                let effect = bare.finish_writer(copied, close)?;
                // No readback follows an uncertain commit or close. Success
                // is checked again through a fresh read-only connection.
                let after = database_io::read_cancellable(file, cancel, |c| {
                    ordinary_destination::read_copied(c, marker, &input, Some(cancel))
                })?;
                if valid(after)?.is_none() {
                    return Err(cause_invalid("ordinary copied destination disappeared"));
                }
                family_check()?;
                Ok(effect)
            })();
            let post = destination.require_published(&copying.plan);
            drop(gate);
            combine(result, post).map_err(ProjectionStoreError::from)
        })
    }
}

fn valid<T>(state: ReadState<T>) -> CauseResult<Option<T>> {
    match state {
        ReadState::Valid(value) => Ok(Some(value)),
        ReadState::Uninitialized => Ok(None),
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
