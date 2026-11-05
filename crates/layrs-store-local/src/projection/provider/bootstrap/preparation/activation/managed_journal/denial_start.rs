//! Retained same-owner start: fund B before atomically writing body + Planned.
//! Explicit allocation scopes can then authenticate actual Planned/Copying while
//! retaining this original plan template. A failed ACK never consumes the owner.
//! Fresh-owner orphan release is a separate denial_recovery entry.
use super::*;
use crate::projection::{
    provider::{
        bootstrap::denial_receipt::{self, PublishedDeniedStartReceipt},
        import_validation::{self, ImportDatabaseKind},
        records::{denied_coverage::DerivedDeniedCoverage, import::ImportSourceReferences},
        transition_observation::TransitionObservationData,
        validation::PrivateActiveCaptureValidation,
    },
    shared::{
        exact_content::ExactStorage,
        source_denial::{SourceDeniedCoveragePresence, SourceDeniedCoverageReservation},
    },
};
use layrs_core::scm::FileManifest;

#[path = "denial_staging.rs"]
mod staging;
#[path = "denial_transition.rs"]
mod transition;

/// The same live owner, attempt lock, complete image and derived document stay
/// borrowed through every retry. No detached funding digest grants authority.
pub(in crate::projection::provider::bootstrap::preparation::activation) struct PendingDenialStart<
    'h,
    'i,
    'a,
    'c,
> {
    held: &'h outcome::PublicationPins,
    destination_parent: layrs_vfs::ExportParentGuard<'h>,
    root: &'h layrs_vfs::ProviderRootGuard,
    mount: &'h ProviderActiveMountRecord,
    activation: &'h ProviderActivationRecord,
    attempt: &'h CaptureAttemptLock,
    planned: &'h ProviderTransitionIntentRecord,
    coverage: &'h DerivedDeniedCoverage,
    observed: &'h TransitionObservationData,
    supplied: &'h [FileManifest],
    admitted: Option<super::denial_plan::AdmissibleDenialPlan>,
    receipt: Option<PublishedDeniedStartReceipt>,
    settled: bool,
    image: &'i mut managed_image::complete::VerifiedCompleteManagedImage<'a, 'c>,
    funding: SourceDeniedCoverageReservation,
}

impl<'h, 'i, 'a, 'c> PendingDenialStart<'h, 'i, 'a, 'c> {
    fn retained_plan(&self) -> CauseResult<&super::denial_plan::AdmissibleDenialPlan> {
        self.admitted
            .as_ref()
            .ok_or_else(|| invalid("denial start lacks retained plan admission"))
    }

    pub(in crate::projection::provider::bootstrap::preparation::activation) fn prospective_companion_cost(
        &self,
    ) -> CauseResult<Option<&crate::projection::provider::source_companion::ProspectiveCompanionCost>>
    {
        Ok(self.retained_plan()?.companion_cost())
    }
    /// Prepare only against a strict, unreserved source. Required manifests
    /// are the caller's complete derived union, retained for later phases;
    /// pinning them confers no installation or checkout permission.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn prepare(
        held: &'h outcome::PublicationPins,
        destination_parent: layrs_vfs::ExportParentGuard<'h>,
        root: &'h layrs_vfs::ProviderRootGuard,
        mount: &'h ProviderActiveMountRecord,
        activation: &'h ProviderActivationRecord,
        attempt: &'h CaptureAttemptLock,
        planned: &'h ProviderTransitionIntentRecord,
        coverage: &'h DerivedDeniedCoverage,
        observed: &'h TransitionObservationData,
        image: &'i mut managed_image::complete::VerifiedCompleteManagedImage<'a, 'c>,
        supplied: &'h [FileManifest],
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        if planned.role != TransitionRole::Source
            || planned.state != TransitionState::Planned
            || planned.destination.is_some()
            || planned.reservation.is_some()
            || planned.receipt_id.is_some()
            || planned.plan.denial_coverage.is_none()
        {
            return Err(invalid(
                "denial start requires unallocated v2 source Planned",
            ));
        }
        let funding = SourceDeniedCoverageReservation::new(
            &held.artifacts.allocation,
            planned,
            coverage.record(),
            Some(cancel),
        )?;
        image.revalidate(cancel)?;
        let mut context = Self {
            held,
            destination_parent,
            root,
            mount,
            activation,
            attempt,
            planned,
            coverage,
            observed,
            supplied,
            admitted: None,
            receipt: None,
            settled: false,
            image,
            funding,
        };
        {
            let _locks = held.family.locks(
                &held.artifacts.allocation.format.identity.worktree_id,
                false,
            )?;
            let admitted = valid(database_io::read_cancellable_strict(
                &held.artifacts.database,
                cancel,
                |c| {
                    let (state, admitted) = context.inspect_plan(c, cancel)?;
                    if context.already_planned(&state)? {
                        return Err(invalid("denial start context must precede Planned"));
                    }
                    // No prospective exception while constructing a new context.
                    context.read_actual_family(&state, cancel)?;
                    context.read_future_capacity(&state, &admitted, cancel)?;
                    context.boundary(&state, cancel)?;
                    Ok(admitted)
                },
            )?)?;
            context.admitted = Some(admitted);
        }
        held.family.require_allocation(&held.artifacts.allocation)?;
        let storage = ExactStorage::open(
            held.family.root(),
            &held.artifacts.allocation.family.root_identity,
            held.family.writer_lifecycle(false)?,
        )?;
        let manifests = context
            .admitted
            .as_ref()
            .ok_or_else(|| invalid("denial start lacks admissible C plan"))?
            .manifests();
        context
            .image
            .pin_offline_current(storage, manifests, cancel)?;
        held.family.verify()?;
        Ok(context)
    }

    fn already_planned(&self, state: &PrivateActiveTransitionValidation) -> CauseResult<bool> {
        match state
            .transitions
            .iter()
            .find(|row| row.transaction_id == self.planned.transaction_id)
        {
            Some(row) if row == self.planned => {
                if !state.capture.denial_reservations().contains(&self.funding) {
                    return Err(invalid(
                        "actual Planned document funding differs from retained start",
                    ));
                }
                Ok(true)
            }
            Some(_) => Err(invalid("denial start durable source phase or plan differs")),
            None => {
                if state
                    .transitions
                    .iter()
                    .any(|row| row.state != TransitionState::Cancelled)
                {
                    return Err(invalid("denial start source has another live transition"));
                }
                Ok(false)
            }
        }
    }

    fn boundary(
        &self,
        state: &PrivateActiveTransitionValidation,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        cancel.check()?;
        if self.destination_parent.root()
            != std::path::Path::new(&self.planned.plan.destination_parent)
            || self.destination_parent.filesystem_identity()
                != self.planned.plan.destination_parent_identity
        {
            return Err(invalid("denial start retained destination parent differs"));
        }
        self.destination_parent.require_source(self.root)?;
        self.destination_parent.revalidate()?;
        self.attempt
            .verify(&self.held.artifacts, &self.held.family)?;
        super::super::boundary(self.held, self.root)?;
        if let Some(receipt) = &self.receipt {
            receipt.verify(Some(cancel))?;
        }
        self.image.require_owner(
            self.root,
            &state.active.source,
            &state.active.activated.snapshot,
        )?;
        let ImportSource::Provider { version, .. } = &self.planned.plan.source_prefix.source else {
            return Err(invalid("denial start requires Provider source"));
        };
        self.image
            .require_retained(self.root, &version.effective_image_digest, cancel)
    }

    /// Full private/prefix interpretation from the caller's actual connection.
    /// Supplied coverage never repairs the retained SQL inventory.
    fn inspect(
        &self,
        c: &Connection,
        cancel: &Cancellation,
    ) -> CauseResult<PrivateActiveTransitionValidation> {
        let (state, admitted) = self.inspect_plan(c, cancel)?;
        self.admitted
            .as_ref()
            .ok_or_else(|| invalid("denial start lacks retained plan admission"))?
            .require_same(&admitted)?;
        Ok(state)
    }

    fn inspect_plan(
        &self,
        c: &Connection,
        cancel: &Cancellation,
    ) -> CauseResult<(
        PrivateActiveTransitionValidation,
        super::denial_plan::AdmissibleDenialPlan,
    )> {
        let state = private(c, self.held, self.root, self.mount, self.activation, cancel)?;
        if !self.already_planned(&state)? {
            validation::preflight_denial_start(
                c,
                &self.held.artifacts.allocation,
                self.held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("denial source P absent"))?,
                &state,
                self.planned,
                self.coverage,
                Some(cancel),
            )?;
        }
        self.rederive_from_state(c, state, cancel)
    }

    /// The template remains Planned while actual SQL advances. Callers must
    /// authenticate the actual durable phase separately before this derivation.
    fn rederive_from_state(
        &self,
        c: &Connection,
        state: PrivateActiveTransitionValidation,
        cancel: &Cancellation,
    ) -> CauseResult<(
        PrivateActiveTransitionValidation,
        super::denial_plan::AdmissibleDenialPlan,
    )> {
        self.boundary(&state, cancel)?;
        let proof = PrivateActiveCaptureValidation {
            active: state.active,
            capture: state.capture,
        };
        let ImportSource::Provider { version, .. } = &self.planned.plan.source_prefix.source else {
            return Err(invalid("denial start requires Provider source"));
        };
        let closure = import_validation::verify_source(
            c,
            &self.held.artifacts.allocation.format,
            ImportDatabaseKind::Provider,
            &self.planned.plan.source_prefix,
            ImportSourceReferences::Provider {
                allocation: &self.held.artifacts.allocation,
                prepared: self
                    .held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("denial source P absent"))?,
                activation: &proof.active.activation,
            },
            Some(&proof),
            &version.effective_image_digest,
            Some(cancel),
        )?;
        let admitted = super::denial_plan::rederive(
            c,
            &self.held.artifacts.allocation,
            &closure,
            &proof,
            self.image,
            self.observed,
            self.planned,
            self.coverage,
            self.supplied,
            cancel,
        )?;
        Ok((
            PrivateActiveTransitionValidation {
                active: proof.active,
                capture: proof.capture,
                transitions: state.transitions,
            },
            admitted,
        ))
    }

    fn family_on(
        &self,
        c: &Connection,
        state: &PrivateActiveTransitionValidation,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let registration = if self.already_planned(state)? {
            self.held.family.validate_capture_catalog(
                c,
                &self.held.artifacts.allocation,
                &state.capture,
                Some(cancel),
            )?
        } else {
            self.held
                .family
                .validate_capture_catalog_with_pending_denial(
                    c,
                    &self.held.artifacts.allocation,
                    &state.capture,
                    &self.funding,
                    Some(cancel),
                )?
        };
        if registration != ActivationRegistration::Ready {
            return Err(invalid("denial start family owner is not Ready"));
        }
        Ok(())
    }

    fn read_family(
        &self,
        state: &PrivateActiveTransitionValidation,
        cancel: &Cancellation,
    ) -> CauseResult<SourceDeniedCoveragePresence> {
        valid(database_io::read_cancellable_strict(
            &self.held.family.catalog,
            cancel,
            |c| {
                self.family_on(c, state, cancel)?;
                self.funding.read_presence(c, Some(cancel))
            },
        )?)
    }

    fn read_actual_family(
        &self,
        state: &PrivateActiveTransitionValidation,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let registration = valid(database_io::read_cancellable_strict(
            &self.held.family.catalog,
            cancel,
            |c| {
                self.held.family.validate_capture_catalog(
                    c,
                    &self.held.artifacts.allocation,
                    &state.capture,
                    Some(cancel),
                )
            },
        )?)?;
        if registration != ActivationRegistration::Ready {
            return Err(invalid("denial start family owner is not Ready"));
        }
        Ok(())
    }

    /// Prospective document-B plus C quota, under the actual complete family
    /// snapshot, including the separately derived companion-preservation B
    /// bound. No capacity or authority survives release of these gates.
    fn future_capacity_on(
        &self,
        c: &Connection,
        state: &PrivateActiveTransitionValidation,
        admitted: &super::denial_plan::AdmissibleDenialPlan,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        self.family_on(c, state, cancel)?;
        let need = self
            .funding
            .prospective_destination_capacity_with_companion(
                c,
                self.planned,
                admitted.companion_cost(),
                Some(cancel),
            )?;
        self.boundary(state, cancel)?;
        // C is not allocated yet. Its retained parent supplies the actual
        // destination volume. Require the whole envelope on each volume;
        // comparing twice on one volume does not double-charge it.
        if fs2::available_space(self.held.family.root())? < need
            || fs2::available_space(self.destination_parent.root())? < need
        {
            return Err(invalid(
                "insufficient physical space for denial source and future destination",
            ));
        }
        self.boundary(state, cancel)
    }

    fn read_future_capacity(
        &self,
        state: &PrivateActiveTransitionValidation,
        admitted: &super::denial_plan::AdmissibleDenialPlan,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        valid(database_io::read_cancellable_strict(
            &self.held.family.catalog,
            cancel,
            |c| self.future_capacity_on(c, state, admitted, cancel),
        )?)
    }

    /// Retain a strict source snapshot across the family COMMIT/close. A
    /// callback failure closes existing scopes and returns before new probes.
    fn reserve_locked(
        &mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<bool> {
        let held = self.held;
        valid(database_io::read_cancellable_strict(
            &held.artifacts.database,
            cancel,
            |source| {
                let state = self.inspect(source, cancel)?;
                let present = self.already_planned(&state)?;
                if !present {
                    // Refuse a known-unfundable C before even publishing the
                    // compact recovery receipt, then repeat under the writer.
                    self.read_future_capacity(&state, self.retained_plan()?, cancel)?;
                    self.ensure_receipt(cancel)?;
                }
                if self.read_family(&state, cancel)? == SourceDeniedCoveragePresence::Reserved {
                    // A fresh complete read with a positive close proves durable
                    // replay; neither a family nor source writer is needed here.
                    self.boundary(&state, cancel)?;
                    return Ok(present);
                }
                let admission =
                    database_io::clean_admission(&self.held.family.catalog, false, |c| {
                        self.family_on(c, &state, cancel)
                    })?;
                self.boundary(&state, cancel)?;
                let bare = admission.open(&self.held.family.catalog, close)?;
                let result = (|| -> CauseResult<()> {
                    admission.require_geometry(&bare)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                    self.family_on(c, &state, cancel)?;
                    self.future_capacity_on(c, &state, self.retained_plan()?, cancel)?;
                    self.funding.reserve(c, Some(cancel))?;
                    self.family_on(c, &state, cancel)?;
                    self.funding.require_present(c, Some(cancel))?;
                    self.boundary(&state, cancel)?;
                    c.execute_batch("COMMIT")?;
                    Ok(())
                })();
                let result = managed_destination::rollback(bare.connection(), result);
                bare.finish_writer(result, close)?;
                if self.read_family(&state, cancel)? != SourceDeniedCoveragePresence::Reserved {
                    return Err(invalid("denial funding durable readback absent"));
                }
                self.boundary(&state, cancel)?;
                Ok(present)
            },
        )?)
    }

    /// Call again with this same context after an uncertain acknowledgement.
    /// Existing exact reservations and exact Planned bodies replay read-only.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn begin(
        &mut self,
        family_close: &mut impl WriterCloseObserver,
        source_close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<SourceIntentEffect> {
        let result = self.begin_inner(family_close, source_close, cancel);
        if result.is_ok() {
            // A caller must observe completion of Planned and receipt cleanup
            // before advancing this owner to any destination effects.
            self.settled = true;
        }
        result
    }

    fn begin_inner(
        &mut self,
        family_close: &mut impl WriterCloseObserver,
        source_close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<SourceIntentEffect> {
        self.image.revalidate(cancel)?;
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        if self.reserve_locked(family_close, cancel)? {
            self.settle_receipt(cancel)?;
            return Ok(SourceIntentEffect::AlreadyPresent);
        }
        let admission = database_io::clean_admission(&self.held.artifacts.database, false, |c| {
            let state = self.inspect(c, cancel)?;
            if self.read_family(&state, cancel)? != SourceDeniedCoveragePresence::Reserved {
                return Err(invalid("denial funding absent before private writer"));
            }
            self.boundary(&state, cancel)
        })?;
        let bare = admission.open(&self.held.artifacts.database, source_close)?;
        let result = (|| -> CauseResult<SourceIntentEffect> {
            admission.require_geometry(&bare)?;
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
            let before = self.inspect(c, cancel)?;
            if self.read_family(&before, cancel)? != SourceDeniedCoveragePresence::Reserved {
                return Err(invalid("denial funding absent on private writer"));
            }
            let effect = transition_sql::apply_source_intent_with_coverage(
                c,
                &self.held.artifacts.allocation,
                self.held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("denial source P absent"))?,
                &self.held.binding,
                self.root.registration().root_id(),
                None,
                self.planned,
                Some(self.coverage),
                Some(cancel),
            )?;
            let after = self.inspect(c, cancel)?;
            require_next(&after, self.planned)?;
            // After insertion only SQL-derived funding is accepted.
            self.read_actual_family(&after, cancel)?;
            self.boundary(&after, cancel)?;
            c.execute_batch("COMMIT")?;
            Ok(effect)
        })();
        let result = managed_destination::rollback(bare.connection(), result);
        let effect = bare.finish_writer(result, source_close)?;
        valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |c| {
                let state = self.inspect(c, cancel)?;
                require_next(&state, self.planned)?;
                self.read_actual_family(&state, cancel)?;
                self.boundary(&state, cancel)
            },
        )?)?;
        self.settle_receipt(cancel)?;
        Ok(effect)
    }

    fn ensure_receipt(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        if let Some(receipt) = &self.receipt {
            receipt.verify(Some(cancel))?;
            receipt.record().require_reservation(&self.funding)?;
        } else {
            self.receipt = Some(denial_receipt::publish(
                &self.held.artifacts.allocation,
                self.held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("denial start P absent"))?,
                &self.funding,
                Some(cancel),
            )?);
        }
        Ok(())
    }

    /// Only after full actual Planned/family readback and positive SQL closes.
    /// If a previous unlink completed but its acknowledgement failed, reopen
    /// observes absence; never replace the receipt merely to replay Planned.
    fn settle_receipt(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        let receipt = match self.receipt.take() {
            Some(receipt) => Some(receipt),
            None => denial_receipt::reopen(
                &self.held.artifacts.allocation,
                self.held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("denial start P absent"))?,
                Some(cancel),
            )?,
        };
        if let Some(receipt) = receipt {
            receipt.record().require_reservation(&self.funding)?;
            receipt.cleanup(Some(cancel))?;
        }
        Ok(())
    }
}
