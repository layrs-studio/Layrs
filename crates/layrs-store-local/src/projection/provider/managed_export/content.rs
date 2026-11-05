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
            exact_content::{ExactBinding, ExactCatalog, ExactInstallState, ExactStorage},
            provider_access::provider_import::source::OrdinarySourceInputs,
        },
    },
};
use layrs_core::projection::ProjectionSnapshot;

pub(super) struct Catalog<'a, 'scope, O: WriterCloseObserver> {
    pub(super) store: &'a ProjectionStore,
    pub(super) source: &'a Connection,
    pub(super) scope: &'a format::ContentScope<'scope>,
    pub(super) snapshot: &'a ProjectionSnapshot,
    pub(super) image: &'a mut OrdinaryExportImage,
    pub(super) prefix: &'a ProviderImportPrefixRecord,
    pub(super) closure: &'a VerifiedImportClosure,
    pub(super) retained: &'a AdmittedFamilyContext,
    pub(super) manifests: &'a BTreeMap<FileId, FileManifest>,
    pub(super) destination: &'a ManagedDestinationAllocation,
    pub(super) staging: &'a ProviderTransitionIntentRecord,
    pub(super) destination_staging: &'a ProviderTransitionIntentRecord,
    pub(super) plan: &'a reserve_plan::ReservePlan,
    pub(super) target: &'a ExportReserveOnlyTarget,
    pub(super) cas: &'a resident::RetainedCas,
    pub(super) phase: ContentPhase,
    pub(super) expected: Option<ExpectedCheckout>,
    pub(super) close: &'a mut O,
}

impl OrdinaryManagedExport<'_> {
    /// Keep one immutable source snapshot through the complete operation.
    /// No public acknowledgement escapes its observed rollback.
    pub(super) fn with_content_source<T, O: WriterCloseObserver>(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        phase: ContentPhase,
        close: &mut O,
        cancel: &Cancellation,
        action: impl FnOnce(&mut Catalog<'_, '_, O>) -> CauseResult<T>,
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
                let expected = if phase == ContentPhase::Checkout {
                    Some(expected_checkout(staging, image, cancel)?)
                } else {
                    None
                };
                let actual_destination = read_destination(
                    destination,
                    &tx,
                    &store.marker,
                    staging,
                    &closure,
                    expected.as_ref(),
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
                    validate_phase(
                        &inputs,
                        family,
                        catalog,
                        staging,
                        &target,
                        &plan,
                        phase,
                        expected.as_ref(),
                        cancel,
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
                    phase,
                    expected,
                };
                catalog.revalidate(cancel)?;
                let value = action(&mut catalog)?;
                catalog.revalidate(cancel)?;
                Ok(value)
            })();
            finish_snapshot(tx, result)
        })
    }
}

impl<O: WriterCloseObserver> Catalog<'_, '_, O> {
    /// Source image hashes and C SQL reads happen only outside family scopes.
    pub(super) fn revalidate(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        self.image
            .revalidate(self.store, self.source, self.scope, self.snapshot, cancel)?;
        self.scope.verify(self.source, self.store)?;
        if read_destination(
            self.destination,
            self.source,
            &self.store.marker,
            self.staging,
            self.closure,
            self.expected.as_ref(),
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

    pub(super) fn validate_catalog(
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
        let state = validate_phase(
            &inputs,
            self.destination.family(),
            c,
            self.staging,
            self.target,
            self.plan,
            self.phase,
            self.expected.as_ref(),
            cancel,
        )?;
        resident::require_indexed(c, self.manifests, cancel)?;
        Ok(state)
    }

    pub(super) fn copy_chunk(
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

use crate::projection::{
    provider::{
        checkout_journal::{CheckoutFile, ExpectedCheckout},
        import_validation::{self, ImportDatabaseKind},
        ordinary_destination::{self, OrdinaryCopyInputs},
        records::Record,
    },
    shared::{
        exact_protection::{ExactProtection, ProtectionPresence},
        provider_access::FamilyAccess,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ContentPhase {
    Installing,
    Protection,
    Checkout,
}

pub(super) fn expected_protection(
    target: &ExportReserveOnlyTarget,
    plan: &reserve_plan::ReservePlan,
    cancel: &Cancellation,
) -> CauseResult<ExactProtection> {
    let binding = target.binding();
    ExactProtection::new(
        &binding.family_incarnation,
        &binding.owner,
        &binding.database_identity,
        &binding.transaction_id,
        &plan.required,
        Some(cancel),
    )
}

fn validate_phase(
    inputs: &OrdinarySourceInputs<'_, '_>,
    family: &FamilyAccess,
    c: &Connection,
    staging: &ProviderTransitionIntentRecord,
    target: &ExportReserveOnlyTarget,
    plan: &reserve_plan::ReservePlan,
    phase: ContentPhase,
    expected: Option<&ExpectedCheckout>,
    cancel: &Cancellation,
) -> CauseResult<ExactInstallState> {
    if phase == ContentPhase::Installing {
        return inputs.validate_export_installing(
            family,
            c,
            staging,
            target,
            &plan.required,
            Some(cancel),
        );
    }
    let protection = expected_protection(target, plan, cancel)?;
    let state = inputs.validate_export_protection(
        family,
        c,
        staging,
        target,
        &plan.required,
        &protection,
        Some(cancel),
    )?;
    if phase == ContentPhase::Checkout {
        let expected = expected.ok_or_else(|| cause_invalid("checkout inventory absent"))?;
        if state.root != ProtectionPresence::Present {
            return Err(cause_invalid(
                "ordinary checkout requires durable exact protection",
            ));
        }
        let floor = plan
            .actual_metadata_floor
            .checked_add(expected.maximum_encoded_bytes())
            .ok_or_else(|| cause_invalid("ordinary checkout metadata overflow"))?;
        let remaining = state
            .installation
            .remaining
            .checked_sub(expected.header().file_bytes)
            .ok_or_else(|| cause_invalid("ordinary checkout bytes exceed reservation"))?;
        protection.require_envelope(remaining, floor)?;
    }
    Ok(state.installation)
}

pub(super) fn expected_checkout(
    staging: &ProviderTransitionIntentRecord,
    image: &OrdinaryExportImage,
    cancel: &Cancellation,
) -> CauseResult<ExpectedCheckout> {
    let files = image
        .files()
        .iter()
        .map(|f| CheckoutFile {
            path: f.path.clone(),
            manifest_id: f.manifest.id().clone(),
            file_id: f.manifest.file_id().clone(),
            bytes: f.manifest.byte_length(),
            executable: f.executable,
        })
        .collect::<Vec<_>>();
    let deletions = image
        .deletions()
        .iter()
        .map(|f| f.path.clone())
        .collect::<Vec<_>>();
    ExpectedCheckout::ordinary(staging, &files, &deletions, image.digest(), Some(cancel))
}

fn read_destination(
    destination: &ManagedDestinationAllocation,
    source: &Connection,
    marker: &format::Marker,
    staging: &ProviderTransitionIntentRecord,
    closure: &VerifiedImportClosure,
    expected: Option<&ExpectedCheckout>,
    cancel: &Cancellation,
) -> CauseResult<ProviderTransitionIntentRecord> {
    let Some(expected) = expected else {
        return reservation::read_destination(
            destination,
            source,
            marker,
            staging,
            closure,
            cancel,
        );
    };
    destination.require_published(&staging.plan)?;
    let result = reservation::valid(database_io::read_cancellable(
        destination.destination_database(),
        cancel,
        |c| {
            read_checkout_destination(
                c,
                destination,
                source,
                marker,
                staging,
                closure,
                expected,
                cancel,
            )?;
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

pub(super) fn read_checkout_destination(
    c: &Connection,
    destination: &ManagedDestinationAllocation,
    source: &Connection,
    marker: &format::Marker,
    staging: &ProviderTransitionIntentRecord,
    closure: &VerifiedImportClosure,
    expected: &ExpectedCheckout,
    cancel: &Cancellation,
) -> CauseResult<Option<crate::projection::provider::checkout_journal::CheckoutJournal>> {
    let marker_c = &destination.proof().format;
    import_validation::require_schema(c, marker_c, ImportDatabaseKind::Provider, Some(cancel))?;
    let input = OrdinaryCopyInputs::new(source, marker, staging, closure);
    let present: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_prejournal)",
        [],
        |r| r.get(0),
    )?;
    if present {
        ordinary_destination::read_checkout_staging(c, marker_c, &input, expected, Some(cancel))
            .map(Some)
    } else {
        if ordinary_destination::staging::read_stageable(c, marker_c, &input, Some(cancel))?
            != TransitionState::Staging
        {
            return Err(cause_invalid(
                "ordinary checkout requires destination Staging",
            ));
        }
        Ok(None)
    }
}
