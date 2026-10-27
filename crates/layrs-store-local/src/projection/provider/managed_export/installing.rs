//! Ordinary exact installation under one retained source snapshot. Family
//! transactions close positively between engine steps and all content I/O.
//! No ExactAttempt escapes: public success waits for finish_snapshot below;
//! explicit owner.close still finalizes the persistent private connection.
use super::{allocation::ManagedDestinationAllocation, reservation, reserve_plan, resident, *};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            admitted_ordinary,
            database_io::{self, WriterCloseObserver},
            recovery::error::{
                invalid as cause_invalid, ProviderBootstrapCause as Cause, Result as CauseResult,
            },
        },
        shared::{
            admitted_catalog::{AdmittedFamilyContext, ExportReserveOnlyTarget},
            exact_content::{self, ExactBinding, ExactCatalog, ExactInstallState, ExactStorage},
            provider_access::provider_import::source::OrdinarySourceInputs,
        },
    },
};
use layrs_core::projection::ProjectionSnapshot;

struct Catalog<'a, 'scope, O: WriterCloseObserver> {
    store: &'a ProjectionStore,
    source: &'a Connection,
    scope: &'a format::ContentScope<'scope>,
    snapshot: &'a ProjectionSnapshot,
    image: &'a mut OrdinaryExportImage,
    prefix: &'a ProviderImportPrefixRecord,
    closure: &'a VerifiedImportClosure,
    retained: &'a AdmittedFamilyContext,
    manifests: &'a BTreeMap<FileId, FileManifest>,
    destination: &'a ManagedDestinationAllocation,
    staging: &'a ProviderTransitionIntentRecord,
    destination_staging: &'a ProviderTransitionIntentRecord,
    plan: &'a reserve_plan::ReservePlan,
    target: &'a ExportReserveOnlyTarget,
    cas: &'a resident::RetainedCas,
    close: &'a mut O,
}

impl OrdinaryManagedExport<'_> {
    /// Install one exact member of the complete authenticated union. This may
    /// leave valid durable Installing progress on failure; only a later explicit
    /// call may retry it. It does not populate C's checkout or grant admission.
    pub(in crate::projection::provider) fn install_content(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        id: &FileManifestId,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<FileManifest> {
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
            .ok_or_else(|| invalid("ordinary install owner already finalized"))?;
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
                    return Err(invalid("ordinary install source history changed"));
                }
                let admitted = read_admitted_import(&tx, &store.marker, Some(cancel))?;
                if staging.state != TransitionState::Staging
                    || !admitted.outgoing().iter().any(|row| row == staging)
                    || staging.destination.as_ref() != Some(destination.proof())
                {
                    return Err(invalid(
                        "ordinary install requires exact durable source Staging",
                    ));
                }
                let actual_destination = reservation::read_destination(
                    destination,
                    &tx,
                    &store.marker,
                    staging,
                    &closure,
                    cancel,
                )?;
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
                if !plan.required.iter().any(|manifest| manifest.id() == id) {
                    return Err(invalid(
                        "ordinary installation is outside complete export union",
                    ));
                }
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
                    inputs.validate_export_installing(
                        family,
                        catalog,
                        staging,
                        &target,
                        &plan.required,
                        Some(cancel),
                    )?;
                    resident::require_indexed(catalog, retained, cancel)
                })
                .and_then(reservation::valid);
                drop(gate);
                checked?;
                // No catalog connection/gate is held while immutable bytes are
                // read. The later engine scopes recheck all exact index rows.
                let cas = resident::RetainedCas::verify(family, retained, cancel)?;
                let mut catalog = Catalog {
                    store,
                    source: &tx,
                    scope,
                    snapshot: &state.snapshot,
                    image,
                    prefix: &fresh,
                    closure: &closure,
                    retained: &context,
                    manifests: retained,
                    destination,
                    staging,
                    destination_staging: &actual_destination,
                    plan: &plan,
                    target: &target,
                    cas: &cas,
                    close,
                };
                catalog.revalidate(cancel)?;
                let mut attempt = exact_content::prepare(&mut catalog, id, cancel)?;
                let manifest = attempt.manifest().clone();
                if !attempt.published() {
                    while let Some(mut piece) =
                        exact_content::prepare_piece(&mut catalog, &mut attempt, cancel)?
                    {
                        if piece.needs_produce() {
                            let ordinal = piece.ordinal();
                            piece.produce(
                                |sink| catalog.copy_chunk(&manifest, ordinal, sink, cancel),
                                cancel,
                            )?;
                        }
                        exact_content::finish_piece(&mut catalog, &mut attempt, piece, cancel)?;
                    }
                }
                exact_content::publish(&mut catalog, &mut attempt, cancel)?;
                if !attempt.published() {
                    return Err(invalid(
                        "ordinary installation acknowledgement lacks Published journal",
                    ));
                }
                catalog.revalidate(cancel)?;
                // attempt, catalog and all CAS pins are dropped inside this
                // scope; no authority can escape the observed source rollback.
                Ok(manifest)
            })();
            finish_snapshot(tx, result)
        })
    }
}

impl<O: WriterCloseObserver> Catalog<'_, '_, O> {
    /// Source image hashes and C SQL reads happen only outside family scopes.
    fn revalidate(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        self.image
            .revalidate(self.store, self.source, self.scope, self.snapshot, cancel)?;
        self.scope.verify(self.source, self.store)?;
        if reservation::read_destination(
            self.destination,
            self.source,
            &self.store.marker,
            self.staging,
            self.closure,
            cancel,
        )? != *self.destination_staging
        {
            return Err(cause_invalid(
                "ordinary install destination Staging changed",
            ));
        }
        let plan = reserve_plan::derive(
            self.source,
            self.prefix,
            self.closure,
            self.image,
            self.manifests,
            self.staging,
            self.destination_staging,
            cancel,
        )?;
        if plan.data.plan != self.plan.data.plan
            || plan.required != self.plan.required
            || plan.actual_metadata_floor != self.plan.actual_metadata_floor
            || plan.baseline_quota != self.plan.baseline_quota
        {
            return Err(cause_invalid("ordinary install immutable funding changed"));
        }
        self.boundary(cancel)
    }

    fn validate_catalog(
        &self,
        c: &Connection,
        cancel: &Cancellation,
    ) -> CauseResult<ExactInstallState> {
        let admitted = read_admitted_import(self.source, &self.store.marker, Some(cancel))?;
        let inputs = OrdinarySourceInputs::new(
            self.source,
            &self.store.marker,
            &admitted,
            self.closure,
            self.retained,
        );
        let state = inputs.validate_export_installing(
            self.destination.family(),
            c,
            self.staging,
            self.target,
            &self.plan.required,
            Some(cancel),
        )?;
        resident::require_indexed(c, self.manifests, cancel)?;
        Ok(state)
    }

    fn copy_chunk(
        &mut self,
        manifest: &FileManifest,
        ordinal: usize,
        sink: &mut dyn Write,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        self.revalidate(cancel)?;
        let path = self
            .image
            .files()
            .iter()
            .find(|file| file.manifest == *manifest)
            .map(|file| file.path.clone());
        if let Some(path) = path {
            self.image
                .copy_chunk(&path, manifest, ordinal, sink, cancel)?;
        } else {
            if self.manifests.get(manifest.file_id()) != Some(manifest) {
                return Err(cause_invalid(
                    "ordinary install producer has no exact retained representation",
                ));
            }
            self.cas.copy_chunk(manifest, ordinal, sink, cancel)?;
        }
        self.revalidate(cancel)
    }
}

impl<O: WriterCloseObserver> ExactCatalog for Catalog<'_, '_, O> {
    fn expected_manifest(&self, id: &FileManifestId) -> CauseResult<FileManifest> {
        self.plan
            .required
            .iter()
            .find(|manifest| manifest.id() == id)
            .cloned()
            .ok_or_else(|| cause_invalid("ordinary exact manifest is outside authenticated union"))
    }

    fn binding(&self) -> CauseResult<ExactBinding> {
        Ok(self.target.binding().clone())
    }

    fn pin_storage(&self) -> CauseResult<ExactStorage> {
        let family = self.destination.family();
        self.destination.require_published(&self.staging.plan)?;
        let storage = ExactStorage::open(
            family.root(),
            &self.target.binding().family_root_identity,
            family.writer_lifecycle(false)?,
        )?;
        family.verify()?;
        Ok(storage)
    }

    fn boundary(&self, cancel: &Cancellation) -> CauseResult<()> {
        // Physical checks only, also safe after failed writer/close callbacks.
        cancel.check()?;
        self.destination.require_published(&self.staging.plan)?;
        self.cas.verify_identity(cancel)?;
        self.image.verify_identity(cancel)?;
        Ok(())
    }

    fn with_catalog<T>(
        &mut self,
        write: bool,
        cancel: &Cancellation,
        action: impl FnOnce(&Connection) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.revalidate(cancel)?;
        let family = self.destination.family();
        let gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            if write {
                SessionMode::Exclusive
            } else {
                SessionMode::Shared
            },
        )
        .map_err(Cause::from)?;
        let result = (|| {
            if !write {
                return reservation::valid(database_io::read_cancellable(
                    &family.catalog,
                    cancel,
                    |c| {
                        self.validate_catalog(c, cancel)?;
                        let value = action(c)?;
                        self.validate_catalog(c, cancel)?;
                        self.boundary(cancel)?;
                        Ok(value)
                    },
                )?);
            }
            let admission = database_io::clean_admission(&family.catalog, false, |c| {
                self.validate_catalog(c, cancel)?;
                Ok(())
            })?;
            let bare = admission.open(&family.catalog, self.close)?;
            let written = (|| {
                admission.require_geometry(&bare)?;
                self.boundary(cancel)?;
                let c = bare.connection();
                c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                self.validate_catalog(c, cancel)?;
                let value = action(c)?;
                self.validate_catalog(c, cancel)?;
                self.boundary(cancel)?;
                c.execute_batch("COMMIT")?;
                Ok(value)
            })();
            let written = if bare.connection().is_autocommit() {
                written
            } else {
                reservation::combine(
                    written,
                    bare.connection()
                        .execute_batch("ROLLBACK")
                        .map_err(Cause::from),
                )
            };
            let value = bare.finish_writer(written, self.close)?;
            // An error above immediately aborts; no SQL after uncertain close.
            reservation::valid(database_io::read_cancellable(
                &family.catalog,
                cancel,
                |c| {
                    self.validate_catalog(c, cancel)?;
                    self.boundary(cancel)
                },
            )?)?;
            Ok(value)
        })();
        let post = self.boundary(cancel);
        drop(gate);
        let value = reservation::combine(result, post)?;
        self.revalidate(cancel)?;
        Ok(value)
    }
}
