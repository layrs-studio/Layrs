//! Physical ordinary B SourceReady/SourcePublished under the original owner.
//! Each phase has a distinct B transaction. Independent C and family snapshots
//! span its COMMIT and close positively before fresh B readback. Progress is
//! DATA: the retained B connection still needs a separate positive owner close.
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
            database_io,
            destination_ready::{self, OrdinaryReadyInputs},
            ordinary_publication::OrdinaryPublicationSource,
            ordinary_transition::{self, PublicationCapacity, PublicationState},
            records::{Record, import::ProviderTransitionReceiptRecord},
            recovery::error::{
                ProviderBootstrapCause as Cause, Result as CauseResult, invalid as cause_invalid,
            },
            transition_sql::SourceIntentEffect,
        },
        shared::{
            admitted_catalog::{
                AdmittedFamilyContext, ExportPublicationMember, ExportReserveOnlyTarget,
            },
            provider_access::provider_import::source::OrdinarySourceInputs,
        },
    },
};

/// The requested phase committed (or was fully validated as exact replay).
/// This does not acknowledge native B closure or authorize C9 admission.
pub(in crate::projection::provider) struct OrdinarySourcePublicationProgress {
    effect: SourceIntentEffect,
    intent: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
}
impl OrdinarySourcePublicationProgress {
    pub(in crate::projection::provider) fn effect(&self) -> SourceIntentEffect {
        self.effect
    }
    pub(in crate::projection::provider) fn intent(&self) -> &ProviderTransitionIntentRecord {
        &self.intent
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
}

#[derive(Clone, Copy)]
enum Phase {
    Ready,
    Published,
}
impl Phase {
    fn state(self) -> TransitionState {
        match self {
            Self::Ready => TransitionState::Ready,
            Self::Published => TransitionState::Published,
        }
    }
}

struct PublicationContext<'a, 'scope> {
    store: &'a ProjectionStore,
    source: &'a Connection,
    scope: &'a format::ContentScope<'scope>,
    image: &'a OrdinaryExportImage,
    closure: &'a VerifiedImportClosure,
    retained: &'a AdmittedFamilyContext,
    manifests: &'a BTreeMap<FileId, FileManifest>,
    destination: &'a ManagedDestinationAllocation,
    staging: &'a ProviderTransitionIntentRecord,
    target: &'a ExportReserveOnlyTarget,
    required: &'a [FileManifest],
    data: &'a OwnedOrdinaryReadyData,
    expected: &'a ExpectedCheckout,
    protection: &'a OrdinaryImportProtection,
    checkout: &'a OrdinaryCheckoutProof,
    cas: Option<&'a resident::RetainedCas>,
}

impl OrdinaryManagedExport<'_> {
    /// Promote actual B Staging only after complete actual C8 and family Ready.
    /// Exact Ready replay validates all witnesses and performs no SQL mutation.
    /// Keep this owner on failure; no post-error SQL validation is performed.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider) fn publish_source_ready(
        &mut self,
        destination: &ManagedDestinationAllocation,
        original_staging: &ProviderTransitionIntentRecord,
        protection: &OrdinaryImportProtection,
        checkout: &OrdinaryCheckoutProof,
        expected_c_ready: &ProviderTransitionIntentRecord,
        expected_c_receipt: &ProviderTransitionReceiptRecord,
        cancel: &Cancellation,
    ) -> Result<OrdinarySourcePublicationProgress> {
        self.publish_source_phase(
            destination,
            original_staging,
            protection,
            checkout,
            expected_c_ready,
            expected_c_receipt,
            Phase::Ready,
            cancel,
        )
    }

    /// A separate transaction promotes actual B Ready to Published. The same
    /// original Staging DATA binds replay to this owner's unchanged frontier.
    /// A later historical Published handoff requires its own independent reader.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider) fn publish_source_published(
        &mut self,
        destination: &ManagedDestinationAllocation,
        original_staging: &ProviderTransitionIntentRecord,
        protection: &OrdinaryImportProtection,
        checkout: &OrdinaryCheckoutProof,
        expected_c_ready: &ProviderTransitionIntentRecord,
        expected_c_receipt: &ProviderTransitionReceiptRecord,
        cancel: &Cancellation,
    ) -> Result<OrdinarySourcePublicationProgress> {
        self.publish_source_phase(
            destination,
            original_staging,
            protection,
            checkout,
            expected_c_ready,
            expected_c_receipt,
            Phase::Published,
            cancel,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_source_phase(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        protection: &OrdinaryImportProtection,
        checkout: &OrdinaryCheckoutProof,
        expected_c_ready: &ProviderTransitionIntentRecord,
        expected_c_receipt: &ProviderTransitionReceiptRecord,
        phase: Phase,
        cancel: &Cancellation,
    ) -> Result<OrdinarySourcePublicationProgress> {
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
            .ok_or_else(|| invalid("ordinary publication owner already finalized"))?;
        // Deliberately bypass with_source_transaction/with_ready_source: both
        // have Staging assumptions and post-action validation on error paths.
        access.with_content(|source, scope| {
            if !source.is_autocommit() {
                return Err(invalid(
                    "ordinary publication source transaction already open",
                ));
            }
            source.execute_batch("BEGIN IMMEDIATE")?;
            let mut rollback_attempted = false;
            let result = (|| -> CauseResult<OrdinarySourcePublicationProgress> {
                // Authenticate the actual phase before using any reconstructed
                // Staging DATA in the pure planning/Ready constructors.
                let (fresh, closure) = plan_admitted_import(
                    source,
                    &store.marker,
                    prefix.destination_identity.clone(),
                    image.digest(),
                    Some(cancel),
                )?;
                if fresh != *prefix
                    || closure.copied_content_mappings(source, &fresh, Some(cancel))? != *mappings
                {
                    return Err(cause_invalid("ordinary publication source history changed"));
                }
                let publication = OrdinaryPublicationSource::new(
                    source,
                    &store.marker,
                    &closure,
                    staging,
                    expected_c_ready,
                    expected_c_receipt,
                    Some(cancel),
                )?;
                let before = publication.revalidate(Some(cancel))?;
                if staging.destination.as_ref() != Some(destination.proof())
                    || (matches!(phase, Phase::Ready) && before.state == TransitionState::Published)
                    || (matches!(phase, Phase::Published)
                        && before.state == TransitionState::Staging)
                {
                    return Err(cause_invalid(
                        "ordinary publication predecessor/proof differs",
                    ));
                }
                let current = database::current(source)?;
                image.revalidate(store, source, scope, &current.snapshot, cancel)?;
                scope.verify(source, store)?;
                let mut destination_staging =
                    ProviderTransitionIntentRecord::decode(&staging.encode()?)?;
                destination_staging.role = TransitionRole::Destination;
                destination_staging.local_generation = 0;
                destination_staging.validate_fields()?;
                let plan = reserve_plan::derive(
                    source,
                    &fresh,
                    &closure,
                    image,
                    retained,
                    staging,
                    &destination_staging,
                    cancel,
                )?;
                let actual = admitted_ordinary::inspect(source, &store.marker, Some(cancel))?;
                let context = AdmittedFamilyContext::new(&actual)?;
                let target = ExportReserveOnlyTarget::new(
                    &context,
                    staging,
                    &destination_staging,
                    &plan.required,
                    plan.baseline_quota,
                    plan.actual_metadata_floor,
                    Some(cancel),
                )?;
                let data = OwnedOrdinaryReadyData::new(
                    plan.data,
                    staging,
                    image,
                    &plan.required,
                    protection.root(),
                    plan.actual_metadata_floor,
                    cancel,
                )?;
                if data.ready() != expected_c_ready || data.receipt() != expected_c_receipt {
                    return Err(cause_invalid(
                        "ordinary publication expected C Ready DATA changed",
                    ));
                }
                let expected = content::expected_checkout(staging, image, cancel)?;
                protection.require_parts(target.binding(), &plan.required, cancel)?;
                checkout.require_parts(&expected, checkout.journal(), cancel)?;
                // Authenticate the complete exact family/index before opening
                // retained physical CAS handles. Its scopes close positively
                // before the byte walk, with no catalog gate held for I/O.
                let preflight = PublicationContext {
                    store,
                    source,
                    scope,
                    image,
                    closure: &closure,
                    retained: &context,
                    manifests: retained,
                    destination,
                    staging,
                    target: &target,
                    required: &plan.required,
                    data: &data,
                    expected: &expected,
                    protection,
                    checkout,
                    cas: None,
                };
                preflight.read_current(&publication, cancel)?;
                drop(preflight);
                // All physical byte verification and complete plan derivation
                // occurs outside the family catalog gate. Later boundaries
                // retain these handles and check identities only.
                let cas = resident::RetainedCas::verify(destination.family(), retained, cancel)?;
                let rederived = reserve_plan::derive(
                    source,
                    &fresh,
                    &closure,
                    image,
                    retained,
                    staging,
                    &destination_staging,
                    cancel,
                )?;
                if rederived.data.plan != data.ready().plan
                    || rederived.required != plan.required
                    || rederived.baseline_quota != plan.baseline_quota
                    || rederived.actual_metadata_floor != data.actual_base_metadata_floor()
                {
                    return Err(cause_invalid(
                        "ordinary publication rederived plan/funding changed",
                    ));
                }
                image.revalidate(store, source, scope, &current.snapshot, cancel)?;
                scope.verify(source, store)?;
                let driver = PublicationContext {
                    store,
                    source,
                    scope,
                    image,
                    closure: &closure,
                    retained: &context,
                    manifests: retained,
                    destination,
                    staging,
                    target: &target,
                    required: &plan.required,
                    data: &data,
                    expected: &expected,
                    protection,
                    checkout,
                    cas: Some(&cas),
                };
                let effect = driver.commit_phase(phase, &publication, cancel)?;
                // Both independent RO connections have positively closed. No
                // SQL follows their failure; only the outer rollback may run.
                driver.boundary(cancel)?;
                source.execute_batch("BEGIN DEFERRED")?;
                let readback = (|| -> CauseResult<PublicationState> {
                    publication.revalidate(Some(cancel))?;
                    let state = ordinary_transition::read_publication_state(
                        source,
                        &store.marker,
                        &staging.transaction_id,
                        Some(cancel),
                    )?;
                    if state.intent.state != phase.state()
                        || state.receipt.as_ref() != Some(expected_c_receipt)
                    {
                        return Err(cause_invalid("ordinary publication durable phase differs"));
                    }
                    // Fresh B plus independently reopened C/family snapshots
                    // authenticate the complete durable phase after the prior
                    // scopes' positive closes. Failed close stops later SQL.
                    driver.read_current(&publication, cancel)?;
                    Ok(state)
                })();
                drop(driver);
                let readback = readback.and_then(|state| {
                    image.revalidate(store, source, scope, &current.snapshot, cancel)?;
                    scope.verify(source, store)?;
                    let fresh_plan = reserve_plan::derive(
                        source,
                        &fresh,
                        &closure,
                        image,
                        retained,
                        staging,
                        &destination_staging,
                        cancel,
                    )?;
                    if fresh_plan.data.plan != data.ready().plan
                        || fresh_plan.required != plan.required
                        || fresh_plan.baseline_quota != plan.baseline_quota
                        || fresh_plan.actual_metadata_floor != data.actual_base_metadata_floor()
                    {
                        return Err(cause_invalid(
                            "ordinary publication durable plan/funding changed",
                        ));
                    }
                    Ok(state)
                });
                rollback_attempted = true;
                let state = reservation::combine(
                    readback,
                    scope.rollback_transaction(source).map_err(Cause::from),
                )?;
                destination.require_published(&staging.plan)?;
                scope.verify_identity(store)?;
                image.verify_identity(cancel)?;
                cas.verify_identity(cancel)?;
                protection.require_parts(target.binding(), &plan.required, cancel)?;
                checkout.require_parts(&expected, checkout.journal(), cancel)?;
                Ok(OrdinarySourcePublicationProgress {
                    effect,
                    intent: state.intent,
                    receipt: state
                        .receipt
                        .ok_or_else(|| cause_invalid("ordinary publication receipt absent"))?,
                })
            })();
            // Rollback is mandatory cleanup, never a new validation/readback.
            // The source owner/connection is retained even after close failure
            // in one of the independently opened RO scopes.
            let result = if source.is_autocommit() || rollback_attempted {
                result
            } else {
                reservation::combine(
                    result,
                    scope.rollback_transaction(source).map_err(Cause::from),
                )
            };
            result.map_err(Into::into)
        })
    }
}

impl PublicationContext<'_, '_> {
    fn boundary(&self, cancel: &Cancellation) -> CauseResult<()> {
        cancel.check()?;
        self.scope.verify_identity(self.store)?;
        self.destination.require_published(&self.staging.plan)?;
        self.image.verify_identity(cancel)?;
        if let Some(cas) = self.cas {
            cas.verify_identity(cancel)?;
        }
        self.protection
            .require_parts(self.target.binding(), self.required, cancel)?;
        self.checkout
            .require_parts(self.expected, self.checkout.journal(), cancel)
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

    /// Actual C decoded rows are returned, and are the only rows passed into
    /// the B writers. Expected records remain comparison/calculation DATA.
    fn validate(
        &self,
        destination: &Connection,
        catalog: &Connection,
        publication: &OrdinaryPublicationSource<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<(
        ProviderTransitionIntentRecord,
        ProviderTransitionReceiptRecord,
        PublicationCapacity,
    )> {
        publication.revalidate(Some(cancel))?;
        let admitted = read_admitted_import(self.source, &self.store.marker, Some(cancel))?;
        let source = OrdinarySourceInputs::new(
            self.source,
            &self.store.marker,
            &admitted,
            self.closure,
            self.retained,
        );
        let (ready, receipt, state) = self.with_inputs(|inputs| {
            let ready = destination_ready::read_ordinary_ready_for_source_publication(
                self.source,
                destination,
                catalog,
                publication,
                inputs,
                self.expected,
                Some(cancel),
            )?;
            let state = source.read_export_source_publication(
                self.destination.family(),
                catalog,
                publication,
                &inputs.publication_inputs(self.expected),
                Some(cancel),
            )?;
            Ok((
                ProviderTransitionIntentRecord::decode(&ready.ready().encode()?)?,
                ProviderTransitionReceiptRecord::decode(&ready.receipt().encode()?)?,
                state,
            ))
        })?;
        if state.member != ExportPublicationMember::Ready {
            return Err(cause_invalid(
                "ordinary source publication requires family Ready",
            ));
        }
        let journal = checkout_journal::read(destination, self.expected, Some(cancel))?;
        self.checkout
            .require_parts(self.expected, &journal, cancel)?;
        let remaining = state
            .protection
            .installation
            .remaining
            .checked_sub(self.expected.header().file_bytes)
            .ok_or_else(|| cause_invalid("ordinary publication checkout exceeds reservation"))?;
        let ready_floor = self
            .data
            .metadata_floor()
            .checked_add(self.expected.maximum_encoded_bytes())
            .ok_or_else(|| cause_invalid("ordinary publication metadata overflow"))?;
        let capacity = PublicationCapacity::new(&ready, &receipt, ready_floor, remaining)?;
        self.protection
            .root()
            .require_envelope(remaining, capacity.metadata_floor())?;
        resident::require_indexed(catalog, self.manifests, cancel)?;
        self.boundary(cancel)?;
        Ok((ready, receipt, capacity))
    }

    fn commit_phase(
        &self,
        phase: Phase,
        publication: &OrdinaryPublicationSource<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<SourceIntentEffect> {
        if self.cas.is_none() {
            return Err(cause_invalid(
                "ordinary publication requires retained CAS pins",
            ));
        }
        self.boundary(cancel)?;
        let family = self.destination.family();
        let gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )
        .map_err(Cause::from)?;
        let result = database_io::read_cancellable(
            self.destination.destination_database(),
            cancel,
            |destination| {
                reservation::valid(database_io::read_cancellable(
                    &family.catalog,
                    cancel,
                    |catalog| {
                        let (ready, receipt, capacity) =
                            self.validate(destination, catalog, publication, cancel)?;
                        let effect = match phase {
                            Phase::Ready => ordinary_transition::apply_ready(
                                self.source,
                                &self.store.marker,
                                self.staging,
                                &ready,
                                &receipt,
                                &capacity,
                                Some(cancel),
                            )?,
                            Phase::Published => {
                                let mut previous =
                                    ProviderTransitionIntentRecord::decode(&ready.encode()?)?;
                                previous.role = TransitionRole::Source;
                                previous.local_generation = self.staging.local_generation;
                                ordinary_transition::apply_published(
                                    self.source,
                                    &self.store.marker,
                                    &previous,
                                    &ready,
                                    &receipt,
                                    &capacity,
                                    Some(cancel),
                                )?
                            }
                        };
                        let after = publication.revalidate(Some(cancel))?;
                        if after.state != phase.state() {
                            return Err(cause_invalid(
                                "ordinary publication prospective phase differs",
                            ));
                        }
                        self.validate(destination, catalog, publication, cancel)?;
                        self.boundary(cancel)?;
                        // COMMIT must remain INSIDE both retained independent RO
                        // callbacks; finish_writer outside would be too late.
                        self.source.execute_batch("COMMIT")?;
                        self.boundary(cancel)?;
                        Ok(effect)
                    },
                )?)
            },
        )
        .and_then(reservation::valid);
        // This is physical identity-only even on an uncertain enclosing close.
        let physical = self.boundary(cancel);
        drop(gate);
        reservation::combine(result, physical)
    }

    fn read_current(
        &self,
        publication: &OrdinaryPublicationSource<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        self.boundary(cancel)?;
        let family = self.destination.family();
        let gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )
        .map_err(Cause::from)?;
        let result = database_io::read_cancellable(
            self.destination.destination_database(),
            cancel,
            |destination| {
                reservation::valid(database_io::read_cancellable(
                    &family.catalog,
                    cancel,
                    |catalog| {
                        self.validate(destination, catalog, publication, cancel)?;
                        Ok(())
                    },
                )?)
            },
        )
        .and_then(reservation::valid);
        let physical = self.boundary(cancel);
        drop(gate);
        reservation::combine(result, physical)
    }
}
