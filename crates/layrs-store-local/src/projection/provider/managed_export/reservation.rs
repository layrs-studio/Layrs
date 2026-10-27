//! Explicit ReserveOnly export path. Ordinary leases remain strict while C's
//! reservation exists. Both actual Staging rows, the private source mappings,
//! current image and retained CAS are authenticated before any catalog write.
use super::{allocation::ManagedDestinationAllocation, reserve_plan, resident, *};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            admitted_ordinary,
            database_io::{self, ReadState, WriterCloseObserver},
            ordinary_destination::{staging::read_stageable, OrdinaryCopyInputs},
            records::Record,
            recovery::error::{
                invalid as cause_invalid, ProviderBootstrapCause as Cause, Result as CauseResult,
            },
        },
        shared::{
            admitted_catalog::{
                AdmittedFamilyContext, ExportReservation, ExportReservationEffect,
                ExportReserveOnlyTarget,
            },
            provider_access::provider_import::source::OrdinarySourceInputs,
        },
    },
};

impl OrdinaryManagedExport<'_> {
    pub(in crate::projection::provider) fn reserve_content(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<ExportReservationEffect> {
        let store = self.store;
        self.with_reserve_source(destination, staging, cancel, |source, closure, retained, target, manifests, cas| {
            let admitted = read_admitted_import(source, &store.marker, Some(cancel))?;
            let inputs = OrdinarySourceInputs::new(source, &store.marker, &admitted, closure, retained);
            let family = destination.family();
            let gate = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"), SessionMode::Exclusive,
            ).map_err(Cause::from)?;
            let validate = |c: &Connection| -> CauseResult<ExportReservation> {
                let state = inputs.validate_export_reserve_only(family, c, staging, target, Some(cancel))?;
                // Recheck the exact rows which authorized the already-verified
                // bytes. The Shared gate was dropped before physical CAS I/O.
                resident::require_indexed(c, manifests, cancel)?;
                Ok(state.reservation)
            };
            let result = (|| -> CauseResult<ExportReservationEffect> {
                destination.require_published(&staging.plan)?;
                cas.verify_identity(cancel)?;
                let prior = valid(database_io::read_cancellable(&family.catalog, cancel, validate)?)?;
                if matches!(prior, ExportReservation::Present { .. }) {
                    return Ok(ExportReservationEffect::AlreadyPresent);
                }
                let admission = database_io::clean_admission(&family.catalog, false, |c| {
                    validate(c)?;
                    Ok(())
                })?;
                let bare = admission.open(&family.catalog, close)?;
                let written = (|| -> CauseResult<ExportReservationEffect> {
                    admission.require_geometry(&bare)?;
                    destination.require_published(&staging.plan)?;
                    cas.verify_identity(cancel)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                    validate(c)?;
                    let effect = retained.reserve_export_only(c, family.marker_bytes(), target, Some(cancel))?;
                    validate(c)?;
                    destination.require_published(&staging.plan)?;
                    cas.verify_identity(cancel)?;
                    cancel.check()?;
                    c.execute_batch("COMMIT")?;
                    Ok(effect)
                })();
                let written = if bare.connection().is_autocommit() { written } else {
                    combine(written, bare.connection().execute_batch("ROLLBACK").map_err(Cause::from))
                };
                let effect = bare.finish_writer(written, close)?;
                let after = valid(database_io::read_cancellable(&family.catalog, cancel, validate)?)?;
                if after != (ExportReservation::Present { remaining: staging.plan.quota_bytes }) {
                    return Err(cause_invalid("ordinary export reservation readback differs"));
                }
                Ok(effect)
            })();
            // No SQL after an unsuccessful close/read acknowledgement. Keep
            // both ownership sets and CAS/lifecycle pins through this boundary.
            let post = destination.require_published(&staging.plan)
                .and_then(|()| cas.verify_identity(cancel));
            drop(gate);
            combine(result, post).map_err(ProjectionStoreError::from)
        })
    }

    /// A separate callback avoids both ordinary manifest lookup paths. All
    /// references and object pins are operation-local and cannot escape it.
    fn with_reserve_source<T>(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
        action: impl FnOnce(
            &Connection,
            &VerifiedImportClosure,
            &AdmittedFamilyContext,
            &ExportReserveOnlyTarget,
            &BTreeMap<FileId, FileManifest>,
            &resident::RetainedCas,
        ) -> Result<T>,
    ) -> Result<T> {
        check_cancel(cancel)?;
        destination.require_published(&staging.plan)?;
        let Self {
            store,
            access,
            image,
            prefix,
            mappings,
            retained,
        } = self;
        let access = access
            .as_mut()
            .ok_or_else(|| invalid("ordinary export owner already finalized"))?;
        access.with_content(|c, scope| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let result = (|| {
                let state = database::current(&tx)?;
                image.revalidate(store, &tx, scope, &state.snapshot, cancel)?;
                let (fresh, closure) = plan_admitted_import(
                    &tx,
                    &store.marker,
                    prefix.destination_identity.clone(),
                    image.digest(),
                    Some(cancel),
                )?;
                if fresh != *prefix
                    || closure.copied_content_mappings(&tx, &fresh, Some(cancel))? != *mappings
                {
                    return Err(invalid("ordinary reserve source history changed"));
                }
                let admitted = read_admitted_import(&tx, &store.marker, Some(cancel))?;
                if staging.state != TransitionState::Staging
                    || !admitted.outgoing().iter().any(|row| row == staging)
                    || staging.destination.as_ref() != Some(destination.proof())
                {
                    return Err(invalid(
                        "ordinary reserve requires exact durable source Staging",
                    ));
                }
                let actual_destination =
                    read_destination(destination, &tx, &store.marker, staging, &closure, cancel)?;
                let actual = admitted_ordinary::inspect(&tx, &store.marker, Some(cancel))?;
                let context = AdmittedFamilyContext::new(&actual)?;
                let plan = reserve_plan::derive(
                    &tx,
                    &fresh,
                    &closure,
                    image,
                    retained,
                    staging,
                    &actual_destination,
                    cancel,
                )?;
                let target = ExportReserveOnlyTarget::new(
                    &context,
                    staging,
                    &actual_destination,
                    &plan.required,
                    plan.baseline_quota,
                    plan.actual_metadata_floor,
                    Some(cancel),
                )?;
                let family = destination.family();
                let inputs =
                    OrdinarySourceInputs::new(&tx, &store.marker, &admitted, &closure, &context);
                let gate = StoreLock::acquire_existing(
                    &family.root().join("metadata/catalog.lock"),
                    SessionMode::Shared,
                )
                .map_err(Cause::from)?;
                let checked = database_io::read_cancellable(&family.catalog, cancel, |catalog| {
                    inputs.validate_export_reserve_only(
                        family,
                        catalog,
                        staging,
                        &target,
                        Some(cancel),
                    )?;
                    resident::require_indexed(catalog, retained, cancel)
                });
                // The read helper closes before returning. No byte I/O occurs
                // while a catalog connection or gate remains retained.
                let checked = checked.and_then(valid);
                drop(gate);
                checked?;
                let cas = resident::RetainedCas::verify(family, retained, cancel)?;
                image.revalidate(store, &tx, scope, &state.snapshot, cancel)?;
                scope.verify(&tx, store)?;
                if read_destination(destination, &tx, &store.marker, staging, &closure, cancel)?
                    != actual_destination
                {
                    return Err(invalid(
                        "ordinary reserve destination changed before catalog effect",
                    ));
                }
                let verified_plan = reserve_plan::derive(
                    &tx,
                    &fresh,
                    &closure,
                    image,
                    retained,
                    staging,
                    &actual_destination,
                    cancel,
                )?;
                if verified_plan.data.plan != plan.data.plan
                    || verified_plan.required != plan.required
                    || verified_plan.actual_metadata_floor != plan.actual_metadata_floor
                    || verified_plan.baseline_quota != plan.baseline_quota
                {
                    return Err(invalid("ordinary reserve rederived funding differs"));
                }
                let value = action(&tx, &closure, &context, &target, retained, &cas)?;
                // Failure above skips all further SQL. A committed full C
                // reservation is authenticated by a later explicit retry.
                image.revalidate(store, &tx, scope, &state.snapshot, cancel)?;
                scope.verify(&tx, store)?;
                read_destination(destination, &tx, &store.marker, staging, &closure, cancel)?;
                cas.verify_identity(cancel)?;
                destination.require_published(&staging.plan)?;
                check_cancel(cancel)?;
                Ok(value)
            })();
            finish_snapshot(tx, result)
        })
    }
}

fn read_destination(
    destination: &ManagedDestinationAllocation,
    source: &Connection,
    source_marker: &format::Marker,
    staging: &ProviderTransitionIntentRecord,
    closure: &VerifiedImportClosure,
    cancel: &Cancellation,
) -> CauseResult<ProviderTransitionIntentRecord> {
    destination.require_published(&staging.plan)?;
    let input = OrdinaryCopyInputs::new(source, source_marker, staging, closure);
    let result = valid(database_io::read_cancellable(
        destination.destination_database(),
        cancel,
        |c| {
            if read_stageable(c, &destination.proof().format, &input, Some(cancel))?
                != TransitionState::Staging
            {
                return Err(cause_invalid(
                    "ordinary reservation requires both durable Staging rows",
                ));
            }
            // Exact schema, singleton inventory and row payload bounds were checked
            // above; return the actual normalized row's canonical payload.
            let bytes: Vec<u8> = c.query_row(
                "SELECT payload FROM projection_provider_transitions WHERE id=?1",
                [&staging.transaction_id],
                |row| row.get(0),
            )?;
            ProviderTransitionIntentRecord::decode(&bytes).map_err(Cause::from)
        },
    )?)?;
    destination.require_published(&staging.plan)?;
    Ok(result)
}

fn valid<T>(state: ReadState<T>) -> CauseResult<T> {
    match state {
        ReadState::Valid(value) => Ok(value),
        ReadState::Uninitialized => Err(cause_invalid("ordinary reserve storage is uninitialized")),
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
