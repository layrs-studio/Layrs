//! Private C Ready under an actual retained B Staging snapshot. This driver
//! does not promote the family member, B's journal, or C's admission schema.
use super::{
    allocation::ManagedDestinationAllocation, checkout::OrdinaryCheckoutProof, content,
    protection::OrdinaryImportProtection, ready_data::OwnedOrdinaryReadyData, reservation,
    reserve_plan, resident, *,
};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            admitted_ordinary,
            checkout_journal::{self, ExpectedCheckout},
            database_io::{self, WriterCloseObserver},
            destination_ready::{self, OrdinaryReadyInputs, ReadyEffect},
            import_validation::{self, ImportDatabaseKind},
            ordinary_destination::{self, OrdinaryCopyInputs},
            records::{import::ProviderTransitionReceiptRecord, Record},
            recovery::error::{
                invalid as cause_invalid, ProviderBootstrapCause as Cause, Result as CauseResult,
            },
        },
        shared::{
            admitted_catalog::{AdmittedFamilyContext, ExportReserveOnlyTarget},
            exact_protection::ProtectionPresence,
            provider_access::provider_import::source::OrdinarySourceInputs,
        },
    },
};
use layrs_core::projection::ProjectionSnapshot;

/// Acknowledged private Ready DATA. Physical witnesses stay owned by the caller;
/// neither this result nor its records authorize family publication/admission.
pub(in crate::projection::provider) struct OrdinaryReadyOutcome {
    effect: ReadyEffect,
    ready: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
}
impl OrdinaryReadyOutcome {
    pub(in crate::projection::provider) fn effect(&self) -> ReadyEffect {
        self.effect
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.ready
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
}

struct ReadyContext<'a, 'scope> {
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
    target: &'a ExportReserveOnlyTarget,
    required: &'a [FileManifest],
    baseline_quota: u64,
    data: &'a OwnedOrdinaryReadyData,
    expected: &'a ExpectedCheckout,
    cas: &'a resident::RetainedCas,
    protection: &'a OrdinaryImportProtection,
    checkout: &'a OrdinaryCheckoutProof,
}

impl OrdinaryManagedExport<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider) fn finalize_destination_ready(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        protection: &OrdinaryImportProtection,
        checkout: &OrdinaryCheckoutProof,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<OrdinaryReadyOutcome> {
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
            .ok_or_else(|| invalid("ordinary Ready owner already finalized"))?;
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
                    return Err(invalid("ordinary Ready source history changed"));
                }
                let admitted = read_admitted_import(&tx, &store.marker, Some(cancel))?;
                if staging.role != TransitionRole::Source
                    || staging.state != TransitionState::Staging
                    || !admitted.outgoing().iter().any(|row| row == staging)
                    || staging.destination.as_ref() != Some(destination.proof())
                {
                    return Err(invalid("ordinary Ready requires exact durable B Staging"));
                }
                // Canonical predecessor DATA only. Actual C may already be Ready;
                // its complete closed reader runs before any writer is opened.
                let mut expected_staging =
                    ProviderTransitionIntentRecord::decode(&staging.encode()?)?;
                expected_staging.role = TransitionRole::Destination;
                expected_staging.local_generation = 0;
                expected_staging.validate_fields()?;
                let plan = reserve_plan::derive(
                    &tx,
                    &fresh,
                    &closure,
                    image,
                    retained,
                    staging,
                    &expected_staging,
                    cancel,
                )?;
                let actual = admitted_ordinary::inspect(&tx, &store.marker, Some(cancel))?;
                let context = AdmittedFamilyContext::new(&actual)?;
                let target = ExportReserveOnlyTarget::new(
                    &context,
                    staging,
                    &expected_staging,
                    &plan.required,
                    plan.baseline_quota,
                    plan.actual_metadata_floor,
                    Some(cancel),
                )?;
                let required = plan.required;
                let baseline_quota = plan.baseline_quota;
                protection.require_parts(target.binding(), &required, cancel)?;
                let data = OwnedOrdinaryReadyData::new(
                    plan.data,
                    staging,
                    image,
                    &required,
                    protection.root(),
                    plan.actual_metadata_floor,
                    cancel,
                )?;
                let expected = content::expected_checkout(staging, image, cancel)?;
                checkout.require_parts(&expected, checkout.journal(), cancel)?;
                let family = destination.family();
                let gate = StoreLock::acquire_existing(
                    &family.root().join("metadata/catalog.lock"),
                    SessionMode::Shared,
                )
                .map_err(Cause::from)?;
                let checked = database_io::read_cancellable(&family.catalog, cancel, |catalog| {
                    let inputs = OrdinarySourceInputs::new(
                        &tx,
                        &store.marker,
                        &admitted,
                        &closure,
                        &context,
                    );
                    let protected = inputs.validate_export_protection(
                        family,
                        catalog,
                        staging,
                        &target,
                        &required,
                        protection.root(),
                        Some(cancel),
                    )?;
                    if protected.root != ProtectionPresence::Present {
                        return Err(cause_invalid("ordinary Ready requires durable protection"));
                    }
                    require_envelope(
                        &data,
                        &expected,
                        protection,
                        protected.installation.remaining,
                    )?;
                    resident::require_indexed(catalog, retained, cancel)
                })
                .and_then(reservation::valid);
                drop(gate);
                checked?;
                let cas = resident::RetainedCas::verify(family, retained, cancel)?;
                let mut ready = ReadyContext {
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
                    target: &target,
                    required: &required,
                    baseline_quota,
                    data: &data,
                    expected: &expected,
                    cas: &cas,
                    protection,
                    checkout,
                };
                ready.revalidate_source(cancel)?;
                let effect = ready.finalize(close, cancel)?;
                ready.revalidate_source(cancel)?;
                if !ready.read_current(cancel)? {
                    return Err(invalid("ordinary Ready durable readback lost Ready"));
                }
                Ok(OrdinaryReadyOutcome {
                    effect,
                    ready: ProviderTransitionIntentRecord::decode(&data.ready().encode()?)?,
                    receipt: ProviderTransitionReceiptRecord::decode(&data.receipt().encode()?)?,
                })
            })();
            finish_snapshot(tx, result)
        })
    }
}

impl ReadyContext<'_, '_> {
    fn boundary(&self, cancel: &Cancellation) -> CauseResult<()> {
        cancel.check()?;
        self.destination.require_published(&self.staging.plan)?;
        self.cas.verify_identity(cancel)?;
        self.image.verify_identity(cancel)?;
        self.protection
            .require_parts(self.target.binding(), self.required, cancel)?;
        self.checkout
            .require_parts(self.expected, self.checkout.journal(), cancel)
    }

    fn revalidate_source(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        self.image
            .revalidate(self.store, self.source, self.scope, self.snapshot, cancel)?;
        self.scope.verify(self.source, self.store)?;
        let fresh = reserve_plan::derive(
            self.source,
            self.prefix,
            self.closure,
            self.image,
            self.manifests,
            self.staging,
            self.data.destination_staging(),
            cancel,
        )?;
        if fresh.data.plan != self.data.ready().plan
            || fresh.required != self.required
            || fresh.baseline_quota != self.baseline_quota
            || fresh.actual_metadata_floor != self.data.actual_base_metadata_floor()
        {
            return Err(cause_invalid(
                "ordinary Ready rederived plan/funding changed",
            ));
        }
        let expected = content::expected_checkout(self.staging, self.image, cancel)?;
        self.checkout
            .require_parts(&expected, self.checkout.journal(), cancel)?;
        self.boundary(cancel)
    }

    fn validate_family(&self, c: &Connection, cancel: &Cancellation) -> CauseResult<()> {
        let admitted = read_admitted_import(self.source, &self.store.marker, Some(cancel))?;
        let inputs = OrdinarySourceInputs::new(
            self.source,
            &self.store.marker,
            &admitted,
            self.closure,
            self.retained,
        );
        let state = inputs.validate_export_protection(
            self.destination.family(),
            c,
            self.staging,
            self.target,
            self.required,
            self.protection.root(),
            Some(cancel),
        )?;
        if state.root != ProtectionPresence::Present {
            return Err(cause_invalid(
                "ordinary Ready durable protection disappeared",
            ));
        }
        require_envelope(
            self.data,
            self.expected,
            self.protection,
            state.installation.remaining,
        )?;
        resident::require_indexed(c, self.manifests, cancel)?;
        self.boundary(cancel)
    }

    fn with_inputs<T>(
        &self,
        action: impl FnOnce(&OrdinaryReadyInputs<'_>) -> CauseResult<T>,
    ) -> CauseResult<T> {
        let data = self.data.as_data(self.required)?;
        action(&OrdinaryReadyInputs {
            marker: &self.destination.proof().format,
            source_marker: &self.store.marker,
            source_staging: self.staging,
            closure: self.closure,
            retained: self.retained,
            target: self.target,
            protection: self.protection.root(),
            family_marker_bytes: self.destination.family().marker_bytes(),
            metadata_floor: self.data.metadata_floor(),
            data: &data,
        })
    }

    // The discriminant is only a dispatch hint. Each arm validates the complete
    // actual C inventory, prefix, binding and normalized journal before returning.
    fn read_destination(
        &self,
        c: &Connection,
        family: &Connection,
        cancel: &Cancellation,
    ) -> CauseResult<bool> {
        import_validation::require_schema(
            c,
            &self.destination.proof().format,
            ImportDatabaseKind::Provider,
            Some(cancel),
        )?;
        let count: u64 = c.query_row(
            "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 2)",
            [],
            |row| row.get(0),
        )?;
        if count != 1 {
            return Err(cause_invalid("ordinary Ready intent cardinality differs"));
        }
        let phase: String = c.query_row(
            "SELECT state FROM projection_provider_transitions WHERE length(state)<=16",
            [],
            |row| row.get(0),
        )?;
        let journal = match phase.as_str() {
            "staging" => {
                let inputs = OrdinaryCopyInputs::new(
                    self.source,
                    &self.store.marker,
                    self.staging,
                    self.closure,
                );
                ordinary_destination::read_checkout_staging(
                    c,
                    &self.destination.proof().format,
                    &inputs,
                    self.expected,
                    Some(cancel),
                )?
            }
            "ready" => {
                self.with_inputs(|inputs| {
                    destination_ready::read_ordinary_checkout_ready(
                        self.source,
                        c,
                        family,
                        inputs,
                        self.expected,
                        Some(cancel),
                    )
                    .map(|_| ())
                })?;
                checkout_journal::read(c, self.expected, Some(cancel))?
            }
            _ => return Err(cause_invalid("ordinary Ready predecessor phase differs")),
        };
        self.checkout
            .require_parts(self.expected, &journal, cancel)?;
        self.boundary(cancel)?;
        Ok(phase == "ready")
    }

    fn read_current(&self, cancel: &Cancellation) -> CauseResult<bool> {
        self.boundary(cancel)?;
        let family = self.destination.family();
        let gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )
        .map_err(Cause::from)?;
        let result = reservation::valid(database_io::read_cancellable(
            &family.catalog,
            cancel,
            |catalog| {
                self.validate_family(catalog, cancel)?;
                let ready = reservation::valid(database_io::read_cancellable(
                    self.destination.destination_database(),
                    cancel,
                    |c| self.read_destination(c, catalog, cancel),
                )?)?;
                self.validate_family(catalog, cancel)?;
                Ok(ready)
            },
        )?);
        let post = self.boundary(cancel);
        drop(gate);
        reservation::combine(result, post)
    }

    fn finalize(
        &self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ReadyEffect> {
        self.boundary(cancel)?;
        let family = self.destination.family();
        let gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )
        .map_err(Cause::from)?;
        let result = reservation::valid(database_io::read_cancellable(
            &family.catalog,
            cancel,
            |catalog| {
                self.validate_family(catalog, cancel)?;
                let file = self.destination.destination_database();
                let already =
                    reservation::valid(database_io::read_cancellable(file, cancel, |c| {
                        self.read_destination(c, catalog, cancel)
                    })?)?;
                if already {
                    self.validate_family(catalog, cancel)?;
                    return Ok(ReadyEffect::AlreadyReady);
                }
                let admission = database_io::clean_admission(file, false, |c| {
                    if self.read_destination(c, catalog, cancel)? {
                        return Err(cause_invalid(
                            "ordinary Ready predecessor changed before writer",
                        ));
                    }
                    Ok(())
                })?;
                let bare = admission.open(file, close)?;
                let written = (|| {
                    admission.require_geometry(&bare)?;
                    self.boundary(cancel)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                    if self.read_destination(c, catalog, cancel)? {
                        return Err(cause_invalid(
                            "ordinary Ready predecessor changed inside writer",
                        ));
                    }
                    let effect = self.with_inputs(|inputs| {
                        destination_ready::finalize_ordinary_checkout_or_replay(
                            self.source,
                            c,
                            catalog,
                            inputs,
                            self.expected,
                            Some(cancel),
                        )
                    })?;
                    if effect != ReadyEffect::Finalized
                        || !self.read_destination(c, catalog, cancel)?
                    {
                        return Err(cause_invalid("ordinary Ready prospective suffix differs"));
                    }
                    self.validate_family(catalog, cancel)?;
                    self.boundary(cancel)?;
                    c.execute_batch("COMMIT")?;
                    Ok(effect)
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
                let effect = bare.finish_writer(written, close)?;
                // No SQL probe follows an uncertain COMMIT/rollback/writer close.
                let ready =
                    reservation::valid(database_io::read_cancellable(file, cancel, |c| {
                        self.read_destination(c, catalog, cancel)
                    })?)?;
                if !ready {
                    return Err(cause_invalid("ordinary Ready durable suffix absent"));
                }
                self.validate_family(catalog, cancel)?;
                Ok(effect)
            },
        )?);
        let post = self.boundary(cancel);
        drop(gate);
        reservation::combine(result, post)
    }
}

fn require_envelope(
    data: &OwnedOrdinaryReadyData,
    expected: &ExpectedCheckout,
    protection: &OrdinaryImportProtection,
    remaining: u64,
) -> CauseResult<()> {
    let remaining = remaining
        .checked_sub(expected.header().file_bytes)
        .ok_or_else(|| cause_invalid("ordinary Ready checkout exceeds reservation"))?;
    let floor = data
        .metadata_floor()
        .checked_add(expected.maximum_encoded_bytes())
        .ok_or_else(|| cause_invalid("ordinary Ready metadata overflow"))?;
    protection.root().require_envelope(remaining, floor)
}
