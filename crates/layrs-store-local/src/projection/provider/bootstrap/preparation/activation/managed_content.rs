//! Borrow-only managed content admission and exact reservation publication.
//! The source attempt, checkpoint and both physical owners stay with the caller.
//! This lease exposes no general catalog, cleanup, installation or protection API.
use super::*;
mod checkout;
mod exact;
mod protection;
mod ready;
mod ready_data;
use crate::projection::{
    provider::{
        capture_lock::CaptureAttemptLock,
        checkout_journal::{CheckoutFile, CheckoutJournal, ExpectedCheckout},
        database_io::{self, WriterCloseObserver},
        destination_validation,
        import_validation::VerifiedImportClosure,
        records::{
            import::{ImportSource, ProviderImportKind},
            transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
            Record,
        },
        validation::PrivateActiveCaptureValidation,
    },
    shared::provider_access::provider_import::content::{
        ImportContentInputs, ImportContentReservation, ImportContentState, ImportReservationEffect,
    },
};
use layrs_core::scm::FileManifest;
use layrs_vfs::Cancellation;

/// Mutable image access cannot escape the retained checkpoint. All other
/// fields are fixed inputs; every operation reinterprets their private SQL and
/// family state under short owner/catalog gates before admitting any effect.
pub(super) struct ManagedImportLease<'h, 'd, 'a, 'c> {
    held: &'h outcome::PublicationPins,
    root: &'h layrs_vfs::ProviderRootGuard,
    mount: &'h ProviderActiveMountRecord,
    activation: &'h ProviderActivationRecord,
    attempt: &'h CaptureAttemptLock,
    destination: &'h managed_allocation::ManagedDestinationAllocation<'d>,
    image: &'h mut managed_image::VerifiedManagedImage<'a, 'c>,
    staging: &'h ProviderTransitionIntentRecord,
    manifests: &'h [FileManifest],
    fresh: &'h ProjectionSnapshot,
}

impl<'h, 'd, 'a, 'c> ManagedImportLease<'h, 'd, 'a, 'c> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open(
        held: &'h outcome::PublicationPins,
        root: &'h layrs_vfs::ProviderRootGuard,
        mount: &'h ProviderActiveMountRecord,
        activation: &'h ProviderActivationRecord,
        attempt: &'h CaptureAttemptLock,
        destination: &'h managed_allocation::ManagedDestinationAllocation<'d>,
        image: &'h mut managed_image::VerifiedManagedImage<'a, 'c>,
        staging: &'h ProviderTransitionIntentRecord,
        manifests: &'h [FileManifest],
        fresh: &'h ProjectionSnapshot,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        let mut lease = Self {
            held,
            root,
            mount,
            activation,
            attempt,
            destination,
            image,
            staging,
            manifests,
            fresh,
        };
        lease.rehash(cancel)?;
        let expected = lease.expected_checkout(cancel)?;
        lease.with_checkout_authority(
            &expected,
            cancel,
            |proof, destination_staging, metadata_floor, journal, _| {
                lease.read_admission(proof, destination_staging, metadata_floor, cancel)?;
                if journal.is_some() {
                    lease.require_checkout_protection(
                        &expected,
                        proof,
                        destination_staging,
                        metadata_floor,
                        cancel,
                    )?;
                }
                Ok(())
            },
        )?;
        Ok(lease)
    }

    fn require_intent(&self) -> CauseResult<()> {
        self.staging.validate_fields()?;
        if self.staging.role != TransitionRole::Source
            || self.staging.state != TransitionState::Staging
            || self.staging.plan.kind != ProviderImportKind::ManagedOrdinary
            || self.staging.receipt_id.is_some()
            || self.staging.reservation.as_deref()
                != Some(self.staging.reservation_operation()?.as_str())
            || self.staging.destination.as_ref() != Some(self.destination.proof())
        {
            return Err(invalid(
                "managed content lease requires exact source Staging",
            ));
        }
        Ok(())
    }

    /// Immutable reads can call the provider and reacquire its gates. Complete
    /// this full image pass before the source owner/catalog locks are acquired.
    fn rehash(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        self.require_intent()?;
        let ImportSource::Provider { version, .. } = &self.staging.plan.source_prefix.source else {
            return Err(invalid("managed content lease requires provider source"));
        };
        self.image
            .revalidate_and_match_digest(&version.effective_image_digest, cancel)
    }

    fn inputs<'p>(
        &'p self,
        proof: &'p PrivateActiveCaptureValidation,
        destination_staging: &'p ProviderTransitionIntentRecord,
    ) -> CauseResult<ImportContentInputs<'p>> {
        Ok(ImportContentInputs {
            allocation: &self.held.artifacts.allocation,
            prepared: self
                .held
                .artifacts
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("managed content source prepared receipt absent"))?,
            activation: &proof.active.activation,
            source: &proof.active.source,
            capture: &proof.capture,
            source_staging: self.staging,
            destination_staging,
            required: self.manifests,
        })
    }

    fn read_family(
        &self,
        proof: &PrivateActiveCaptureValidation,
        destination_staging: &ProviderTransitionIntentRecord,
        cancel: &Cancellation,
    ) -> CauseResult<ImportContentState> {
        let inputs = self.inputs(proof, destination_staging)?;
        managed_destination::valid(database_io::read_cancellable(
            &self.held.family.catalog,
            cancel,
            |c| {
                self.held
                    .family
                    .validate_import_content(c, &inputs, Some(cancel))
            },
        )?)
    }

    fn boundary(&self, cancel: &Cancellation) -> CauseResult<()> {
        managed_destination::boundary(
            self.held,
            self.root,
            self.attempt,
            self.staging,
            self.destination,
            cancel,
        )
    }

    /// The generic callback is private and receives no SQL connection. The
    /// exact verified import closure remains inside its retained source RO
    /// transaction; destination RO closes positively before the callback runs.
    fn with_authority<T>(
        &self,
        cancel: &Cancellation,
        action: impl FnOnce(
            &PrivateActiveCaptureValidation,
            &ProviderTransitionIntentRecord,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.with_authority_metadata(cancel, |proof, destination, _| action(proof, destination))
    }

    fn with_authority_metadata<T>(
        &self,
        cancel: &Cancellation,
        action: impl FnOnce(
            &PrivateActiveCaptureValidation,
            &ProviderTransitionIntentRecord,
            u64,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.with_destination_authority(None, cancel, |proof, destination, floor, _, _| {
            action(proof, destination, floor)
        })
    }

    /// Inventory is derived only from the retained verified image; callers
    /// cannot nominate arbitrary filesystem paths for checkout writes.
    fn expected_checkout(&self, cancel: &Cancellation) -> CauseResult<ExpectedCheckout> {
        cancel.check()?;
        let files: Vec<_> = self
            .image
            .files()
            .iter()
            .map(|file| {
                let manifest = file.manifest.manifest();
                CheckoutFile {
                    path: file.path.clone(),
                    manifest_id: manifest.id().clone(),
                    file_id: manifest.file_id().clone(),
                    bytes: manifest.byte_length(),
                    executable: file.executable,
                }
            })
            .collect();
        let deletions: Vec<_> = self
            .image
            .deletions()
            .iter()
            .map(|d| d.path.clone())
            .collect();
        ExpectedCheckout::new(
            self.staging,
            &files,
            &deletions,
            self.image.digest(),
            Some(cancel),
        )
    }

    fn with_checkout_authority<T>(
        &self,
        expected: &ExpectedCheckout,
        cancel: &Cancellation,
        action: impl FnOnce(
            &PrivateActiveCaptureValidation,
            &ProviderTransitionIntentRecord,
            u64,
            Option<CheckoutJournal>,
            &VerifiedImportClosure,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        if self.expected_checkout(cancel)?.header() != expected.header() {
            return Err(invalid(
                "managed checkout inventory differs from retained image",
            ));
        }
        self.with_destination_authority(Some(expected), cancel, action)
    }

    fn with_destination_authority<T>(
        &self,
        checkout: Option<&ExpectedCheckout>,
        cancel: &Cancellation,
        action: impl FnOnce(
            &PrivateActiveCaptureValidation,
            &ProviderTransitionIntentRecord,
            u64,
            Option<CheckoutJournal>,
            &VerifiedImportClosure,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.require_intent()?;
        // The allocation already retains the destination owner. Reacquiring
        // that lock here would be a reentrant error, not additional authority.
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.boundary(cancel)?;
        managed_destination::with_source(
            self.held,
            self.root,
            self.mount,
            self.activation,
            self.attempt,
            self.staging,
            self.destination,
            cancel,
            |source_c, proof, closure| {
                self.image.require_owner(
                    self.root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                // Do not treat a matching union digest as proof that the
                // caller supplied the whole source/history/current-image set.
                let derived = managed_plan::rederive_reservation(
                    source_c,
                    closure,
                    self.image,
                    proof,
                    self.fresh,
                    self.staging,
                    self.manifests,
                    cancel,
                )?;
                let destination_staging = managed_destination::destination_intent(self.staging)?;
                let prepared = self
                    .held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("managed content source prepared receipt absent"))?;
                let journal = managed_destination::valid(database_io::read_cancellable(
                    self.destination.destination_database(),
                    cancel,
                    |c| {
                        if let Some(expected) = checkout {
                            crate::projection::provider::import_validation::require_schema(
                                c, &self.destination.proof().format,
                                crate::projection::provider::import_validation::ImportDatabaseKind::Provider,
                                Some(cancel),
                            )?;
                            let present: bool = c.query_row(
                                "SELECT EXISTS(SELECT 1 FROM projection_prejournal)",
                                [],
                                |row| row.get(0),
                            )?;
                            if present {
                                return destination_validation::read_checkout_staging(
                                    c,
                                    &self.destination.proof().format,
                                    &proof.active.source,
                                    &self.held.artifacts.allocation,
                                    prepared,
                                    &proof.active.activation,
                                    &self.staging.plan.source_prefix,
                                    &destination_staging,
                                    closure,
                                    expected,
                                    Some(cancel),
                                )
                                .map(Some);
                            }
                        }
                        destination_validation::read_staging(
                            c,
                            &self.destination.proof().format,
                            &proof.active.source,
                            &self.held.artifacts.allocation,
                            prepared,
                            &proof.active.activation,
                            &self.staging.plan.source_prefix,
                            &destination_staging,
                            closure,
                            Some(cancel),
                        )?;
                        Ok(None)
                    },
                )?)?;
                self.boundary(cancel)?;
                let metadata_floor = derived
                    .validated_metadata_floor
                    .ok_or_else(|| invalid("managed metadata floor not rederived"))?;
                let value = action(
                    proof,
                    &destination_staging,
                    metadata_floor,
                    journal,
                    closure,
                )?;
                self.image.require_owner(
                    self.root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                self.boundary(cancel)?;
                Ok(value)
            },
        )
    }

    fn require_space(&self, state: &ImportContentState, cancel: &Cancellation) -> CauseResult<()> {
        let additional = match state.reservation {
            ImportContentReservation::Absent => self.staging.plan.quota_bytes,
            ImportContentReservation::Present { .. } => 0,
        };
        let need = state
            .usage
            .reserved_bytes
            .checked_add(additional)
            .ok_or_else(|| invalid("managed physical reservation budget overflow"))?;
        self.boundary(cancel)?;
        // This conservative envelope includes both shared representations and
        // the ordinary image. On separate volumes require the full envelope
        // on each; on a common volume these probes do not add the charge twice.
        if fs2::available_space(self.held.family.root())? < need
            || fs2::available_space(self.destination.root())? < need
        {
            return Err(invalid(
                "insufficient physical space for managed reservation",
            ));
        }
        self.boundary(cancel)
    }

    fn reserve_under_authority(
        &self,
        proof: &PrivateActiveCaptureValidation,
        destination_staging: &ProviderTransitionIntentRecord,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ImportReservationEffect> {
        let inputs = self.inputs(proof, destination_staging)?;
        let check = |c: &Connection| {
            self.held
                .family
                .validate_import_content(c, &inputs, Some(cancel))
        };
        let before = self.read_family(proof, destination_staging, cancel)?;
        self.require_space(&before, cancel)?;
        if matches!(before.reservation, ImportContentReservation::Present { .. }) {
            return Ok(ImportReservationEffect::AlreadyPresent);
        }
        let file = &self.held.family.catalog;
        database_io::validate_native_filename(file.path())?;
        let admission = database_io::clean_admission(file, false, |c| {
            check(c)?;
            Ok(())
        })?;
        self.boundary(cancel)?;
        let bare = admission.open(file, close)?;
        let result = (|| -> CauseResult<ImportReservationEffect> {
            admission.require_geometry(&bare)?;
            let c = bare.connection();
            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
            let current = check(c)?;
            if current != before {
                return Err(invalid(
                    "managed reservation predecessor changed before writer",
                ));
            }
            self.require_space(&current, cancel)?;
            let effect = self
                .held
                .family
                .reserve_import_content(c, &inputs, Some(cancel))?;
            let after = check(c)?;
            if after.reservation
                != (ImportContentReservation::Present {
                    remaining: self.staging.plan.quota_bytes,
                })
            {
                return Err(invalid("managed reservation prospective readback differs"));
            }
            self.boundary(cancel)?;
            c.execute_batch("COMMIT")?;
            Ok(effect)
        })();
        bare.finish_writer(result, close)
    }

    /// Reserve only this retained plan's exact identifier, owner and charge.
    /// Failure retains all caller-owned holders and performs no cleanup. Any
    /// uncertain writer or source close stops before the fresh readback.
    pub(super) fn reserve_plan(
        &mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ImportReservationEffect> {
        self.rehash(cancel)?;
        let effect = self.with_authority(cancel, |proof, destination_staging| {
            self.reserve_under_authority(proof, destination_staging, close, cancel)
        })?;
        self.with_authority(cancel, |proof, destination_staging| {
            let durable = self.read_family(proof, destination_staging, cancel)?;
            if durable.reservation
                != (ImportContentReservation::Present {
                    remaining: self.staging.plan.quota_bytes,
                })
            {
                return Err(invalid("managed reservation durable readback differs"));
            }
            Ok(())
        })?;
        self.boundary(cancel)?;
        Ok(effect)
    }
}
