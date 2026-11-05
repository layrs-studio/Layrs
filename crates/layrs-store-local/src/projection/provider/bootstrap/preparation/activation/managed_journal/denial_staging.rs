//! Same-owner denial Copying/Copied -> Staging. Only existing intent payloads
//! change: no document repair, C reservation, installation or Ready authority.
use super::super::super::managed_staging::{self, DestinationPhase, ManagedStagingEffect};
use super::*;
use crate::projection::{
    provider::{
        denied_coverage_storage,
        destination_sql::{self, CopiedStagingEffect},
        import_validation::VerifiedImportClosure,
    },
    shared::provider_access::ImportRegistration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Copying,
    Staging,
}

impl PendingDenialStart<'_, '_, '_, '_> {
    /// Derive both phases from the immutable retained template and actual C
    /// pins. A retry after B's positive close finishes only C; an exact dual
    /// Staging replay opens no writer. A failed close ends this attempt.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn stage_denial(
        &mut self,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<(ProviderTransitionIntentRecord, ManagedStagingEffect)> {
        let (copying, staging) = self.denial_staging_intents(destination, cancel)?;
        // Hash outside the catalog/member gates. Scopes below still require
        // the same retained native image, source root and offline pins.
        self.image.revalidate(cancel)?;
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        let phase = valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |c| {
                self.with_denial_staging_source(
                    c,
                    &copying,
                    &staging,
                    destination,
                    cancel,
                    |source, proof, closure, phase| {
                        let current = self.read_denial_staging_destination(
                            destination,
                            proof,
                            closure,
                            &copying,
                            &staging,
                            cancel,
                        )?;
                        if phase == Phase::Copying {
                            if current != DestinationPhase::Copying {
                                return Err(invalid(
                                    "denial source Copying requires destination CopiedPrefix",
                                ));
                            }
                            validation::preflight_source_transition_update(
                                source,
                                &copying,
                                &staging,
                                Some(cancel),
                            )?;
                            self.read_denial_staging_capacity(
                                proof, phase, &copying, &staging, cancel,
                            )?;
                        }
                        Ok(phase)
                    },
                )
            },
        )?)?;
        let source_effect = if phase == Phase::Copying {
            self.write_denial_source_staging(&copying, &staging, destination, close, cancel)?
        } else {
            SourceIntentEffect::AlreadyPresent
        };
        // The source read scope encloses C's transaction, COMMIT, observable
        // close and full readback. Nothing probes C after a failed B close.
        let destination_effect = valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |source| {
                self.with_denial_staging_source(
                    source,
                    &copying,
                    &staging,
                    destination,
                    cancel,
                    |source, proof, closure, phase| {
                        if phase != Phase::Staging {
                            return Err(invalid(
                                "denial destination staging requires source Staging",
                            ));
                        }
                        self.write_denial_destination_staging(
                            source,
                            proof,
                            closure,
                            &copying,
                            &staging,
                            destination,
                            close,
                            cancel,
                        )
                    },
                )
            },
        )?)?;
        Ok((
            staging,
            ManagedStagingEffect {
                source: source_effect,
                destination: destination_effect,
            },
        ))
    }

    fn denial_staging_intents(
        &self,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<(
        ProviderTransitionIntentRecord,
        ProviderTransitionIntentRecord,
    )> {
        cancel.check()?;
        if !self.settled || self.planned.plan.denial_coverage.is_none() {
            return Err(invalid(
                "denial staging requires acknowledged retained Planned",
            ));
        }
        self.planned.validate_fields()?;
        let mut copying = ProviderTransitionIntentRecord::decode(&self.planned.encode()?)?;
        copying.state = TransitionState::Copying;
        copying.destination = Some(
            serde_json::from_slice(
                &serde_json::to_vec(destination.proof()).map_err(|e| invalid(e.to_string()))?,
            )
            .map_err(|e| invalid(e.to_string()))?,
        );
        copying.require_transition(self.planned)?;
        let mut staging = ProviderTransitionIntentRecord::decode(&copying.encode()?)?;
        staging.state = TransitionState::Staging;
        staging.reservation = Some(staging.reservation_operation()?);
        staging.require_transition(&copying)?;
        if copying.destination.as_ref() != Some(destination.proof())
            || staging.destination.as_ref() != Some(destination.proof())
            || copying.role != TransitionRole::Source
            || copying.reservation.is_some()
            || copying.receipt_id.is_some()
            || staging.receipt_id.is_some()
        {
            return Err(invalid(
                "denial staging retained plan or physical proof differs",
            ));
        }
        // Both source and destination codecs must fit before the first writer.
        staging.encode()?;
        let (destination_copying, destination_staging) =
            managed_staging::destination_intents(&copying, &staging)?;
        destination_copying.encode()?;
        destination_staging.encode()?;
        destination.require_published(&copying.plan)?;
        Ok((copying, staging))
    }

    /// The allocation-only scopes intentionally refuse Staging. This reader
    /// instead authenticates either exact derived phase, actual current body
    /// and B funding, then rederives the plan from the original Planned template.
    fn inspect_denial_staging(
        &self,
        c: &Connection,
        copying: &ProviderTransitionIntentRecord,
        staging: &ProviderTransitionIntentRecord,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<(PrivateActiveTransitionValidation, Phase)> {
        let state = private(c, self.held, self.root, self.mount, self.activation, cancel)?;
        let actual = state
            .transitions
            .iter()
            .find(|row| row.transaction_id == self.planned.transaction_id)
            .ok_or_else(|| invalid("denial staging source intent absent"))?;
        let phase = if actual == copying {
            Phase::Copying
        } else if actual == staging {
            Phase::Staging
        } else {
            return Err(invalid("denial staging source phase or exact plan differs"));
        };
        if !state.capture.denial_reservations().contains(&self.funding) {
            return Err(invalid("denial staging actual document funding differs"));
        }
        let reference = actual
            .plan
            .denial_coverage
            .as_ref()
            .ok_or_else(|| invalid("denial staging current reference absent"))?;
        let body = denied_coverage_storage::read_required(c, reference, Some(cancel))?;
        body.require_intent(actual, Some(cancel))?;
        if body != *self.coverage.record() {
            return Err(invalid("denial staging retained current body differs"));
        }
        let (state, admitted) = self.rederive_from_state(c, state, cancel)?;
        self.admitted
            .as_ref()
            .ok_or_else(|| invalid("denial staging retained plan admission absent"))?
            .require_same(&admitted)?;
        self.boundary(&state, cancel)?;
        destination.require_published(&copying.plan)?;
        Ok((state, phase))
    }

    /// Retain the actual source snapshot and real closure through the action.
    /// An action failure returns before any new family, filesystem or SQL probe.
    #[allow(clippy::too_many_arguments)]
    fn with_denial_staging_source<T>(
        &self,
        source: &Connection,
        copying: &ProviderTransitionIntentRecord,
        staging: &ProviderTransitionIntentRecord,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        cancel: &Cancellation,
        action: impl FnOnce(
            &Connection,
            &PrivateActiveCaptureValidation,
            &VerifiedImportClosure,
            Phase,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        let (state, phase) =
            self.inspect_denial_staging(source, copying, staging, destination, cancel)?;
        let proof = PrivateActiveCaptureValidation {
            active: state.active,
            capture: state.capture,
        };
        self.read_denial_staging_family(&proof, phase, copying, staging, cancel)?;
        let ImportSource::Provider { version, .. } = &self.planned.plan.source_prefix.source else {
            return Err(invalid("denial staging requires Provider source"));
        };
        let closure = import_validation::verify_source(
            source,
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
                    .ok_or_else(|| invalid("denial staging source P absent"))?,
                activation: &proof.active.activation,
            },
            Some(&proof),
            &version.effective_image_digest,
            Some(cancel),
        )?;
        let value = action(source, &proof, &closure, phase)?;
        let (after, phase) =
            self.inspect_denial_staging(source, copying, staging, destination, cancel)?;
        let after = PrivateActiveCaptureValidation {
            active: after.active,
            capture: after.capture,
        };
        self.read_denial_staging_family(&after, phase, copying, staging, cancel)?;
        Ok(value)
    }

    fn denial_staging_family_on(
        &self,
        c: &Connection,
        proof: &PrivateActiveCaptureValidation,
        phase: Phase,
        copying: &ProviderTransitionIntentRecord,
        staging: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let a = &self.held.artifacts.allocation;
        let p = self
            .held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("denial staging source P absent"))?;
        match phase {
            Phase::Copying => {
                if self.held.family.validate_import_initializing(
                    c,
                    a,
                    p,
                    &proof.active.activation,
                    &proof.capture,
                    copying,
                    Some(cancel),
                )? != ImportRegistration::Initializing
                {
                    return Err(invalid("denial staging requires family Initializing"));
                }
            }
            Phase::Staging => self.held.family.validate_import_staging_unreserved(
                c,
                a,
                p,
                &proof.active.activation,
                &proof.capture,
                staging,
                Some(cancel),
            )?,
        }
        self.funding.require_retained(c, Some(cancel))?;
        Ok(())
    }

    fn read_denial_staging_family(
        &self,
        proof: &PrivateActiveCaptureValidation,
        phase: Phase,
        copying: &ProviderTransitionIntentRecord,
        staging: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        valid(database_io::read_cancellable_strict(
            &self.held.family.catalog,
            cancel,
            |c| self.denial_staging_family_on(c, proof, phase, copying, staging, cancel),
        )?)
    }

    /// Use the phase-appropriate complete family reader, never family_on's
    /// Planned-only shortcut. The original template binds the future q and d;
    /// this checks capacity without creating either a C or companion reservation.
    fn read_denial_staging_capacity(
        &self,
        proof: &PrivateActiveCaptureValidation,
        phase: Phase,
        copying: &ProviderTransitionIntentRecord,
        staging: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        valid(database_io::read_cancellable_strict(
            &self.held.family.catalog,
            cancel,
            |c| {
                self.denial_staging_family_on(c, proof, phase, copying, staging, cancel)?;
                let need = self
                    .funding
                    .prospective_destination_capacity_with_companion(
                        c,
                        self.planned,
                        self.retained_plan()?.companion_cost(),
                        Some(cancel),
                    )?;
                self.destination_parent.revalidate()?;
                if fs2::available_space(self.held.family.root())? < need
                    || fs2::available_space(self.destination_parent.root())? < need
                {
                    return Err(invalid(
                        "insufficient physical space for denial source and future destination",
                    ));
                }
                self.destination_parent.revalidate()?;
                cancel.check()?;
                Ok(())
            },
        )?)
    }

    fn read_denial_staging_destination(
        &self,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        proof: &PrivateActiveCaptureValidation,
        closure: &VerifiedImportClosure,
        copying: &ProviderTransitionIntentRecord,
        staging: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<DestinationPhase> {
        valid(database_io::read_cancellable_strict(
            destination.destination_database(),
            cancel,
            |c| {
                managed_staging::read_destination(
                    c, self.held, proof, closure, copying, staging, cancel,
                )
            },
        )?)
    }

    fn write_denial_source_staging(
        &self,
        copying: &ProviderTransitionIntentRecord,
        staging: &ProviderTransitionIntentRecord,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<SourceIntentEffect> {
        let check = |c: &Connection| {
            self.with_denial_staging_source(
                c,
                copying,
                staging,
                destination,
                cancel,
                |source, proof, closure, phase| {
                    if phase != Phase::Copying {
                        return Err(invalid("denial source staging predecessor changed"));
                    }
                    if self.read_denial_staging_destination(
                        destination,
                        proof,
                        closure,
                        copying,
                        staging,
                        cancel,
                    )? != DestinationPhase::Copying
                    {
                        return Err(invalid(
                            "denial source Copying requires destination CopiedPrefix",
                        ));
                    }
                    validation::preflight_source_transition_update(
                        source,
                        copying,
                        staging,
                        Some(cancel),
                    )?;
                    self.read_denial_staging_capacity(proof, phase, copying, staging, cancel)
                },
            )
        };
        let admission = database_io::clean_admission(&self.held.artifacts.database, false, check)?;
        let bare = admission.open(&self.held.artifacts.database, close)?;
        let result = (|| -> CauseResult<SourceIntentEffect> {
            admission.require_geometry(&bare)?;
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
            check(c)?;
            let effect = transition_sql::apply_source_intent(
                c,
                &self.held.artifacts.allocation,
                self.held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("denial staging source P absent"))?,
                &self.held.binding,
                self.root.registration().root_id(),
                Some(copying),
                staging,
                Some(cancel),
            )?;
            // Read the actual advanced row/body; never supply coverage as repair.
            self.with_denial_staging_source(
                c,
                copying,
                staging,
                destination,
                cancel,
                |_c, _proof, _closure, phase| {
                    if phase != Phase::Staging {
                        return Err(invalid("denial source Staging write not observed"));
                    }
                    Ok(())
                },
            )?;
            c.execute_batch("COMMIT")?;
            Ok(effect)
        })();
        let result = managed_destination::rollback(bare.connection(), result);
        let effect = bare.finish_writer(result, close)?;
        valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |c| {
                self.with_denial_staging_source(
                    c,
                    copying,
                    staging,
                    destination,
                    cancel,
                    |_c, _proof, _closure, phase| {
                        if phase != Phase::Staging {
                            return Err(invalid("denial source Staging durable readback differs"));
                        }
                        Ok(())
                    },
                )
            },
        )?)?;
        Ok(effect)
    }

    #[allow(clippy::too_many_arguments)]
    fn write_denial_destination_staging(
        &self,
        source: &Connection,
        proof: &PrivateActiveCaptureValidation,
        closure: &VerifiedImportClosure,
        copying: &ProviderTransitionIntentRecord,
        staging: &ProviderTransitionIntentRecord,
        destination: &managed_allocation::ManagedDestinationAllocation<'_>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<CopiedStagingEffect> {
        if self.read_denial_staging_destination(
            destination,
            proof,
            closure,
            copying,
            staging,
            cancel,
        )? == DestinationPhase::Staging
        {
            return Ok(CopiedStagingEffect::AlreadyPresent);
        }
        // B Staging is already durable and C's complete copied rows/body are
        // authenticated above. Finishing this intent CAS adds no reservation
        // or content obligation. Retain actual d/family validation through
        // the enclosing source scope, without re-admitting the future q.
        let file = destination.destination_database();
        database_io::validate_native_filename(file.path())?;
        let check = |c: &Connection| -> CauseResult<()> {
            if managed_staging::read_destination(
                c, self.held, proof, closure, copying, staging, cancel,
            )? != DestinationPhase::Copying
            {
                return Err(invalid("denial destination staging predecessor changed"));
            }
            Ok(())
        };
        let admission = database_io::clean_admission(file, false, check)?;
        let bare = admission.open(file, close)?;
        let result = (|| -> CauseResult<CopiedStagingEffect> {
            admission.require_geometry(&bare)?;
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
            check(c)?;
            let (destination_copying, destination_staging) =
                managed_staging::destination_intents(copying, staging)?;
            let effect = destination_sql::stage_copied(
                source,
                c,
                &destination.proof().format,
                &proof.active.source,
                &self.held.artifacts.allocation,
                self.held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("denial staging source P absent"))?,
                &proof.active.activation,
                &copying.plan.source_prefix,
                &destination_copying,
                &destination_staging,
                closure,
                Some(cancel),
            )?;
            c.execute_batch("COMMIT")?;
            Ok(effect)
        })();
        let result = managed_destination::rollback(bare.connection(), result);
        let effect = bare.finish_writer(result, close)?;
        if self.read_denial_staging_destination(
            destination,
            proof,
            closure,
            copying,
            staging,
            cancel,
        )? != DestinationPhase::Staging
        {
            return Err(invalid(
                "denial destination Staging durable readback differs",
            ));
        }
        Ok(effect)
    }
}
