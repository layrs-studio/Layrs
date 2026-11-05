//! Explicit denial-aware source scopes for fresh physical allocation. Actual
//! SQL phases are authenticated independently of the immutable Planned template.
//! This owner carries local observation DATA; it is not a remote authority lease.
use super::*;
use crate::projection::shared::provider_access::ImportRegistration;

impl PendingDenialStart<'_, '_, '_, '_> {
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn require_allocation_session(
        &self,
        session: &layrs_vfs::Session,
    ) -> CauseResult<()> {
        session.require_provider_root(self.root)?;
        Ok(())
    }

    /// Rehash before the caller takes exclusive gates. This alone supplies no
    /// SQL authority; every registration/copy step must enter the scope below.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn prepare_destination_copy(
        &mut self,
        held: &outcome::PublicationPins,
        copying: &ProviderTransitionIntentRecord,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        self.require_allocation_owner(held, copying)?;
        if copying.state != TransitionState::Copying
            || copying.destination.as_ref() != Some(destination.proof())
        {
            return Err(invalid("denial copy requires exact retained Copying proof"));
        }
        self.image.revalidate(cancel)?;
        destination.require_published(&copying.plan)
    }

    /// The caller holds family/source gates. Source SQL, native owner and actual
    /// current coverage remain retained throughout the nested family/C writer,
    /// its COMMIT and positively observed close. No scope data escape as proof.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn with_copying_destination_source<
        T,
    >(
        &self,
        held: &outcome::PublicationPins,
        copying: &ProviderTransitionIntentRecord,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        registering: bool,
        cancel: &Cancellation,
        action: impl FnOnce(
            &Connection,
            &PrivateActiveCaptureValidation,
            &import_validation::VerifiedImportClosure,
            &crate::projection::provider::destination_coverage::SourceCurrentDeniedCoverage<'_>,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.require_allocation_owner(held, copying)?;
        if copying.state != TransitionState::Copying
            || copying.destination.as_ref() != Some(destination.proof())
        {
            return Err(invalid("denial copy source scope differs from allocation"));
        }
        managed_destination::with_source(
            self.held,
            self.root,
            self.mount,
            self.activation,
            self.attempt,
            copying,
            destination,
            cancel,
            |source, proof, closure| {
                let state = self.inspect_allocating(source, copying, cancel)?;
                let require_family = |allow_absent: bool| -> CauseResult<()> {
                    valid(database_io::read_cancellable_strict(
                        &self.held.family.catalog,
                        cancel,
                        |catalog| {
                            // This full reader accepts only absent or exact C
                            // Initializing and classifies actual B funding.
                            let registration = self.held.family.validate_import_initializing(
                                catalog,
                                &self.held.artifacts.allocation,
                                self.held
                                    .artifacts
                                    .prepared
                                    .as_ref()
                                    .ok_or_else(|| invalid("denial copy source P absent"))?,
                                &state.active.activation,
                                &state.capture,
                                copying,
                                Some(cancel),
                            )?;
                            if !allow_absent && registration != ImportRegistration::Initializing {
                                return Err(invalid(
                                    "denial copy requires registered Initializing destination",
                                ));
                            }
                            Ok(())
                        },
                    )?)
                };
                require_family(registering)?;
                self.boundary(&state, cancel)?;
                destination.require_published(&copying.plan)?;
                let current = crate::projection::provider::destination_coverage::read_source(
                    source,
                    &self.held.artifacts.allocation,
                    self.held
                        .artifacts
                        .prepared
                        .as_ref()
                        .ok_or_else(|| invalid("denial copy source P absent"))?,
                    &self.held.binding,
                    self.root.registration().root_id(),
                    copying,
                    Some(cancel),
                )?;
                let value = action(source, proof, closure, &current)?;
                // A nested writer/read failure returns above: no family or FS
                // observation follows an unacknowledged close.
                // Only the registration step may start with an absent member.
                // Its success and every copy/readback scope require actual C.
                require_family(false)?;
                self.boundary(&state, cancel)?;
                destination.require_published(&copying.plan)?;
                Ok(value)
            },
        )
    }

    fn require_allocation_owner(
        &self,
        held: &outcome::PublicationPins,
        intent: &ProviderTransitionIntentRecord,
    ) -> CauseResult<()> {
        if !self.settled {
            return Err(invalid(
                "denial allocation requires acknowledged settled Planned",
            ));
        }
        intent.validate_fields()?;
        if !std::ptr::eq(held, self.held)
            || intent.plan != self.planned.plan
            || intent.transaction_id != self.planned.transaction_id
            || intent.role != TransitionRole::Source
            || intent.reservation.is_some()
            || intent.receipt_id.is_some()
        {
            return Err(invalid("denial allocation owner or immutable plan differs"));
        }
        match intent.state {
            TransitionState::Planned if intent == self.planned => Ok(()),
            TransitionState::Copying if intent.destination.is_some() => {
                intent.require_transition(self.planned)?;
                Ok(())
            }
            _ => Err(invalid(
                "denial allocation requires exact Planned or Copying",
            )),
        }
    }

    fn inspect_allocating(
        &self,
        c: &Connection,
        expected: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<PrivateActiveTransitionValidation> {
        self.require_allocation_owner(self.held, expected)?;
        let state = private(c, self.held, self.root, self.mount, self.activation, cancel)?;
        require_next(&state, expected)?;
        if !state.capture.denial_reservations().contains(&self.funding) {
            return Err(invalid(
                "denial allocation requires actual document funding",
            ));
        }
        let (state, admitted) = self.rederive_from_state(c, state, cancel)?;
        self.admitted
            .as_ref()
            .ok_or_else(|| invalid("denial allocation retained plan absent"))?
            .require_same(&admitted)?;
        Ok(state)
    }

    fn require_unregistered_family(
        &self,
        state: &PrivateActiveTransitionValidation,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        valid(database_io::read_cancellable_strict(
            &self.held.family.catalog,
            cancel,
            |c| {
                if self.held.family.validate_capture_catalog(
                    c,
                    &self.held.artifacts.allocation,
                    &state.capture,
                    Some(cancel),
                )? != ActivationRegistration::Ready
                {
                    return Err(invalid("denial allocation source owner is not Ready"));
                }
                let exists: bool = c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1)",
                    [self
                        .planned
                        .plan
                        .source_prefix
                        .destination_identity
                        .worktree_id
                        .as_str()],
                    |row| row.get(0),
                )?;
                if exists {
                    return Err(invalid("denial allocation destination already registered"));
                }
                Ok(())
            },
        )?)
    }

    pub(in crate::projection::provider::bootstrap::preparation::activation) fn observe_planned_allocation(
        &mut self,
        held: &outcome::PublicationPins,
        planned: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        self.require_allocation_owner(held, planned)?;
        if planned != self.planned {
            return Err(invalid("denial fresh allocation requires original Planned"));
        }
        self.observe_allocating(planned, None, cancel)
    }

    pub(in crate::projection::provider::bootstrap::preparation::activation) fn observe_copying_allocation(
        &mut self,
        held: &outcome::PublicationPins,
        copying: &ProviderTransitionIntentRecord,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        self.require_allocation_owner(held, copying)?;
        if copying.state != TransitionState::Copying
            || copying.destination.as_ref() != Some(destination.proof())
        {
            return Err(invalid(
                "denial FORMAT requires exact retained Copying proof",
            ));
        }
        self.observe_allocating(copying, Some(destination), cancel)
    }

    fn observe_allocating(
        &mut self,
        expected: &ProviderTransitionIntentRecord,
        destination: Option<&managed_allocation::ManagedDestinationAllocation<'_>>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        // Offline pins were acquired before B. No ordinary source range-reader
        // is allowed here, and no family gate is held while hashing native files.
        self.image.revalidate(cancel)?;
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            false,
        )?;
        valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |c| {
                let state = self.inspect_allocating(c, expected, cancel)?;
                self.require_unregistered_family(&state, cancel)?;
                if let Some(destination) = destination {
                    destination.verify(&expected.plan)?;
                }
                self.boundary(&state, cancel)
            },
        )?)
    }

    /// Bind the actual fresh allocation. A durable Copying replay is read-only;
    /// any failed writer close ends the attempt before catalog or FS readback.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn bind_denial_allocation(
        &mut self,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<(ProviderTransitionIntentRecord, SourceIntentEffect)> {
        let mut copying = ProviderTransitionIntentRecord::decode(&self.planned.encode()?)?;
        copying.state = TransitionState::Copying;
        copying.destination = Some(
            serde_json::from_slice(
                &serde_json::to_vec(destination.proof()).map_err(|e| invalid(e.to_string()))?,
            )
            .map_err(|e| invalid(e.to_string()))?,
        );
        if copying.destination.as_ref() != Some(destination.proof()) {
            return Err(invalid(
                "denial allocation copied proof differs from native allocation",
            ));
        }
        self.require_allocation_owner(self.held, &copying)?;
        self.image.revalidate(cancel)?;
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        let inspect = |c: &Connection| -> CauseResult<bool> {
            let state = private(c, self.held, self.root, self.mount, self.activation, cancel)?;
            let actual = state
                .transitions
                .iter()
                .find(|row| row.transaction_id == self.planned.transaction_id)
                .ok_or_else(|| invalid("denial allocation Planned absent"))?;
            let replay = actual == &copying;
            if !replay && actual != self.planned {
                return Err(invalid("denial allocation source predecessor differs"));
            }
            let state =
                self.inspect_allocating(c, if replay { &copying } else { self.planned }, cancel)?;
            self.require_unregistered_family(&state, cancel)?;
            destination.verify(&copying.plan)?;
            self.boundary(&state, cancel)?;
            Ok(replay)
        };
        let replay = valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            inspect,
        )?)?;
        if replay {
            return Ok((copying, SourceIntentEffect::AlreadyPresent));
        }
        let admission = database_io::clean_admission(&self.held.artifacts.database, false, |c| {
            inspect(c).map(|_| ())
        })?;
        let bare = admission.open(&self.held.artifacts.database, close)?;
        let result = (|| -> CauseResult<SourceIntentEffect> {
            admission.require_geometry(&bare)?;
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
            inspect(c)?;
            let effect = transition_sql::apply_source_intent(
                c,
                &self.held.artifacts.allocation,
                self.held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("denial source P absent"))?,
                &self.held.binding,
                self.root.registration().root_id(),
                Some(self.planned),
                &copying,
                Some(cancel),
            )?;
            if !inspect(c)? {
                return Err(invalid("denial allocation Copying write not observed"));
            }
            c.execute_batch("COMMIT")?;
            Ok(effect)
        })();
        let result = managed_destination::rollback(bare.connection(), result);
        let effect = bare.finish_writer(result, close)?;
        let actual = valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            inspect,
        )?)?;
        if !actual {
            return Err(invalid("denial allocation Copying readback differs"));
        }
        Ok((copying, effect))
    }
}
