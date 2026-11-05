//! Complete-checkpoint route. All immutable DATA is rederived from the actual
//! source transaction before any C callback. No network callback runs here.
use super::super::managed_image::{
    self, complete::VerifiedCompleteManagedImage, ManagedDeletion, ManagedFile,
};
use super::*;
use crate::projection::{
    provider::{
        denied_coverage_storage,
        destination_ready::ManagedCompanionReadyData,
        managed_budget::{self, DeniedCoverageCost},
        records::denied_coverage::DerivedDeniedCoverage,
        records::import::ImportSourceReferences,
        source_companion::SourceCompanionPublication,
        transition_observation::TransitionObservationData,
        transition_reconcile::source_sql,
        validation::{self, PrivateActivePublicationValidation},
    },
    shared::{exact_protection::ExactProtection, source_companion::SourceCompanionFamilyContext},
};
use layrs_core::scm::RepoPath;
use std::io::Write;

pub(super) enum ManagedImage<'h, 'a, 'c> {
    Legacy(&'h mut managed_image::VerifiedManagedImage<'a, 'c>),
    Complete(&'h mut VerifiedCompleteManagedImage<'a, 'c>),
}
impl<'a, 'c> ManagedImage<'_, 'a, 'c> {
    fn retained(&self) -> &managed_image::VerifiedManagedImage<'a, 'c> {
        match self {
            Self::Legacy(i) => i,
            Self::Complete(i) => i.retained(),
        }
    }
    pub(super) fn digest(&self) -> &str {
        self.retained().digest()
    }
    pub(super) fn files(&self) -> &[ManagedFile<'_>] {
        self.retained().files()
    }
    pub(super) fn deletions(&self) -> &[ManagedDeletion] {
        self.retained().deletions()
    }
    pub(super) fn require_owner(
        &self,
        root: &layrs_vfs::ProviderRootGuard,
        source: &ProviderBootstrapSource,
        activated: &ProjectionSnapshot,
    ) -> CauseResult<()> {
        self.retained().require_owner(root, source, activated)
    }
    pub(super) fn require_retained(
        &self,
        root: &layrs_vfs::ProviderRootGuard,
        digest: &str,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        match self {
            Self::Complete(i) => i.require_retained(root, digest, cancel),
            Self::Legacy(_) => Err(invalid(
                "source companion witness requires retained complete checkpoint",
            )),
        }
    }
    pub(super) fn revalidate_and_match_digest(
        &mut self,
        digest: &str,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        match self {
            Self::Legacy(i) => i.revalidate_and_match_digest(digest, cancel),
            Self::Complete(i) => {
                i.revalidate(cancel)?;
                if i.digest() != digest {
                    return Err(invalid("complete lease image digest differs"));
                }
                Ok(())
            }
        }
    }
    pub(super) fn copy_exact_chunk(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        ordinal: usize,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        match self {
            Self::Legacy(i) => i.copy_exact_chunk(path, manifest, ordinal, output, cancel),
            Self::Complete(i) => i.copy_exact_chunk(path, manifest, ordinal, output, cancel),
        }
    }
    pub(super) fn copy_exact_file(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        match self {
            Self::Legacy(i) => i.copy_exact_file(path, manifest, output, cancel),
            Self::Complete(i) => i.copy_exact_file(path, manifest, output, cancel),
        }
    }
}

pub(super) struct CompanionLeaseData<'h> {
    pub(super) publication: &'h SourceCompanionPublication<'h>,
    anchor: &'h super::super::session::ProviderTransitionSource<'h>,
    pub(super) observation: &'h TransitionObservationData,
    pub(super) metadata_without_companions: u64,
    pub(super) coverage: Option<&'h DerivedDeniedCoverage>,
}
impl CompanionLeaseData<'_> {
    pub(super) fn context<'p>(
        &'p self,
        full: &'p PrivateActivePublicationValidation,
    ) -> SourceCompanionFamilyContext<'p> {
        SourceCompanionFamilyContext {
            publication: full,
            pending: Some(self.publication.protection()),
        }
    }
}
/// Caller-owned progress keeps every acquired byte/checkout pin alive even
/// when a later COMMIT/connection-close acknowledgement is withheld. There is
/// no reference to the lease or mutable image, and no self-referential Ready.
#[derive(Default)]
pub(in crate::projection::provider::bootstrap::preparation::activation) struct CompanionPins<'h> {
    exact: Option<super::protection::ManagedImportProtection>,
    source: Option<super::source_protection::ManagedSourceCompanionProtection<'h, 'h>>,
    checkout: Option<super::checkout::ManagedCheckoutProof>,
    ready: Option<std::sync::Arc<super::ready_data::OwnedReadyData>>,
    family_ready: bool,
}
impl<'h, 'i, 'd, 'a, 'c> ManagedImportLease<'h, 'i, 'd, 'a, 'c> {
    /// Trusted Store host boundary after point-in-time remote observation and
    /// exact source anchoring. The complete checkpoint stays borrowed by this
    /// lease; DATA cannot reconstruct it, source ownership or remote authority.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn open_companion(
        held: &'h outcome::PublicationPins,
        root: &'h layrs_vfs::ProviderRootGuard,
        mount: &'h ProviderActiveMountRecord,
        activation: &'h ProviderActivationRecord,
        attempt: &'h CaptureAttemptLock,
        destination: &'h managed_allocation::ManagedDestinationAllocation<'d>,
        image: &'i mut VerifiedCompleteManagedImage<'a, 'c>,
        staging: &'h ProviderTransitionIntentRecord,
        publication: &'h SourceCompanionPublication<'h>,
        anchor: &'h super::super::session::ProviderTransitionSource<'h>,
        observation: &'h TransitionObservationData,
        metadata_without_companions: u64,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        Self::open_companion_with_progress(
            held,
            root,
            mount,
            activation,
            attempt,
            destination,
            image,
            staging,
            publication,
            anchor,
            observation,
            metadata_without_companions,
            None,
            cancel,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_companion_with_progress(
        held: &'h outcome::PublicationPins,
        root: &'h layrs_vfs::ProviderRootGuard,
        mount: &'h ProviderActiveMountRecord,
        activation: &'h ProviderActivationRecord,
        attempt: &'h CaptureAttemptLock,
        destination: &'h managed_allocation::ManagedDestinationAllocation<'d>,
        image: &'i mut VerifiedCompleteManagedImage<'a, 'c>,
        staging: &'h ProviderTransitionIntentRecord,
        publication: &'h SourceCompanionPublication<'h>,
        anchor: &'h super::super::session::ProviderTransitionSource<'h>,
        observation: &'h TransitionObservationData,
        metadata_without_companions: u64,
        progress: Option<&CompanionPins<'h>>,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        Self::open_companion_in_mode(
            held,
            root,
            mount,
            activation,
            attempt,
            destination,
            image,
            staging,
            publication,
            anchor,
            observation,
            metadata_without_companions,
            None,
            progress,
            cancel,
        )
    }

    /// C14 uses the same live anchor acquired before Planned and a sealed body.
    /// No caller can replace either with compact plan commitments alone.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn open_denial_companion_with_progress(
        held: &'h outcome::PublicationPins,
        root: &'h layrs_vfs::ProviderRootGuard,
        mount: &'h ProviderActiveMountRecord,
        activation: &'h ProviderActivationRecord,
        attempt: &'h CaptureAttemptLock,
        destination: &'h managed_allocation::ManagedDestinationAllocation<'d>,
        image: &'i mut VerifiedCompleteManagedImage<'a, 'c>,
        staging: &'h ProviderTransitionIntentRecord,
        publication: &'h SourceCompanionPublication<'h>,
        anchor: &'h super::super::session::ProviderTransitionSource<'h>,
        observation: &'h TransitionObservationData,
        metadata_without_companions: u64,
        coverage: &'h DerivedDeniedCoverage,
        prospective: &crate::projection::provider::source_companion::ProspectiveCompanionCost,
        progress: Option<&CompanionPins<'h>>,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        prospective.require_actual(publication, Some(cancel))?;
        Self::open_companion_in_mode(
            held,
            root,
            mount,
            activation,
            attempt,
            destination,
            image,
            staging,
            publication,
            anchor,
            observation,
            metadata_without_companions,
            Some(coverage),
            progress,
            cancel,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_companion_in_mode(
        held: &'h outcome::PublicationPins,
        root: &'h layrs_vfs::ProviderRootGuard,
        mount: &'h ProviderActiveMountRecord,
        activation: &'h ProviderActivationRecord,
        attempt: &'h CaptureAttemptLock,
        destination: &'h managed_allocation::ManagedDestinationAllocation<'d>,
        image: &'i mut VerifiedCompleteManagedImage<'a, 'c>,
        staging: &'h ProviderTransitionIntentRecord,
        publication: &'h SourceCompanionPublication<'h>,
        anchor: &'h super::super::session::ProviderTransitionSource<'h>,
        observation: &'h TransitionObservationData,
        metadata_without_companions: u64,
        coverage: Option<&'h DerivedDeniedCoverage>,
        progress: Option<&CompanionPins<'h>>,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        if staging.plan.denial_coverage.is_some() != coverage.is_some()
            || publication.coverage().map(|body| body.record())
                != coverage.map(|body| body.record())
        {
            return Err(invalid("companion opener denial coverage mode differs"));
        }
        if let Some(body) = coverage {
            body.record().require_intent(staging, Some(cancel))?;
            body.record().require_observation(
                &staging.plan.source_prefix,
                &staging.transaction_id,
                observation,
                Some(cancel),
            )?;
        } else if observation.binding().denied_path_count != 0 {
            return Err(invalid(
                "C14 durable denied-absence contract required before effects",
            ));
        }
        let mut lease = Self {
            held,
            root,
            mount,
            activation,
            attempt,
            destination,
            image: ManagedImage::Complete(image),
            companion: Some(CompanionLeaseData {
                publication,
                anchor,
                observation,
                metadata_without_companions,
                coverage,
            }),
            denial: None,
            staging,
            manifests: publication.rows().manifests(),
            fresh: observation.snapshot(),
        };
        lease.expected_protection(cancel)?;
        // Once checkout is retained, retries consume only the SAME immutable
        // CAS/checkpoint/checkout pins. Source Ready no longer admits a fresh
        // image opener. The driver validates complete SQL before any effect
        // or result, including both actual C Ready scopes on terminal replay.
        if let Some(progress) = progress.filter(|p| p.checkout.is_some()) {
            progress
                .exact
                .as_ref()
                .ok_or_else(|| invalid("retained checkout lacks C protection"))?
                .require(&lease, cancel)?;
            progress
                .checkout
                .as_ref()
                .ok_or_else(|| invalid("retained checkout absent"))?
                .require_physical(&lease, cancel)?;
            if let Some(source) = &progress.source {
                source.require(&lease, cancel)?;
            } else {
                return Err(invalid("retained checkout lacks B protection"));
            }
            lease
                .image
                .require_retained(root, lease.image.digest(), cancel)?;
            return Ok(lease);
        }
        lease.rehash(cancel)?;
        // Opening rederives every row/representation/observation but permits an
        // absent B reservation. reserve_source_companion is the FIRST effect.
        let locks = held
            .family
            .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
        lease.with_source(cancel, |c, full, closure| {
            lease.rederive(c, closure, full, cancel)?;
            publication.require(
                &held.artifacts.allocation,
                staging,
                publication.receipt(),
                Some(cancel),
            )?;
            Ok(())
        })?;
        drop(locks);
        // Do not bind the mutable image's retained byte set to a rejected
        // publication. Full source/plan rederivation and positive SQL close
        // precede these physical pins; the first writer revalidates again.
        let offline_storage = lease.pin_exact_storage()?;
        if let ManagedImage::Complete(image) = &mut lease.image {
            image.pin_offline_current(offline_storage, lease.manifests, cancel)?;
        }
        lease.rehash(cancel)?;
        Ok(lease)
    }

    /// Transition-specific original-prefix validation. This deliberately does
    /// not invoke transition_source()'s no-pending gate once a plan is pending.
    pub(super) fn with_source<T>(
        &self,
        cancel: &Cancellation,
        action: impl FnOnce(
            &Connection,
            &PrivateActivePublicationValidation,
            &VerifiedImportClosure,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.boundary(cancel)?;
        managed_destination::valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |c| {
                let prepared = self
                    .held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("complete lease prepared source absent"))?;
                let full = validation::private_active_publication_with_source(
                    c,
                    &self.held.artifacts.allocation,
                    prepared,
                    &self.held.binding,
                    Some(self.root.registration().root_id()),
                    Some(cancel),
                )?;
                if full.active.mount != *self.mount
                    || full.active.activation != *self.activation
                    || full
                        .transitions
                        .iter()
                        .find(|i| i.transaction_id == self.staging.transaction_id)
                        != Some(self.staging)
                {
                    return Err(invalid("complete lease original source Staging changed"));
                }
                self.denial_cost(c, &full, cancel)?;
                let ImportSource::Provider { version, .. } =
                    &self.staging.plan.source_prefix.source
                else {
                    return Err(invalid("complete lease provider prefix absent"));
                };
                let closure =
                    crate::projection::provider::import_validation::verify_source_publication(
                        c,
                        &self.held.artifacts.allocation.format,
                        &self.staging.plan.source_prefix,
                        ImportSourceReferences::Provider {
                            allocation: &self.held.artifacts.allocation,
                            prepared,
                            activation: &full.active.activation,
                        },
                        &full,
                        &version.effective_image_digest,
                        Some(cancel),
                    )?;
                self.image.require_owner(
                    self.root,
                    &full.active.source,
                    &full.active.activated.snapshot,
                )?;
                let result = action(c, &full, &closure)?;
                self.boundary(cancel)?;
                Ok(result)
            },
        )?)
    }

    /// Read the complete actual current document in the caller's retained
    /// source snapshot. Only its exact column cost is returned, never authority.
    fn denial_cost(
        &self,
        c: &Connection,
        full: &PrivateActivePublicationValidation,
        cancel: &Cancellation,
    ) -> CauseResult<Option<DeniedCoverageCost>> {
        let coverage = self.retained_coverage();
        if self.staging.plan.denial_coverage.is_some() != coverage.is_some() {
            return Err(invalid("content denial coverage mode differs"));
        }
        let Some(retained) = coverage else {
            return Ok(None);
        };
        let actual = full
            .transitions
            .iter()
            .find(|row| row.transaction_id == self.staging.transaction_id)
            .ok_or_else(|| invalid("content denial source intent absent"))?;
        if actual.plan != self.staging.plan {
            return Err(invalid("content denial source plan differs"));
        }
        let reference = actual
            .plan
            .denial_coverage
            .as_ref()
            .ok_or_else(|| invalid("content denial current reference absent"))?;
        let body = denied_coverage_storage::read_required(c, reference, Some(cancel))?;
        body.require_intent(actual, Some(cancel))?;
        if body != *retained.record() {
            return Err(invalid(
                "content denial actual body differs from retained body",
            ));
        }
        Ok(Some(DeniedCoverageCost::new(actual, &body, Some(cancel))?))
    }

    pub(super) fn rederive(
        &self,
        c: &Connection,
        closure: &VerifiedImportClosure,
        full: &PrivateActivePublicationValidation,
        cancel: &Cancellation,
    ) -> CauseResult<managed_plan::ManagedPlanData> {
        let (anchor, observation, retained_rows, metadata, coverage) =
            match (&self.companion, &self.denial) {
                (Some(data), None) => (
                    data.anchor,
                    data.observation,
                    data.publication.rows(),
                    data.metadata_without_companions,
                    data.coverage,
                ),
                (None, Some(data)) => (
                    data.anchor,
                    data.observation,
                    data.rows,
                    data.metadata,
                    Some(data.coverage),
                ),
                (None, None) => {
                    return managed_plan::rederive_publication_reservation(
                        c,
                        closure,
                        self.image.retained(),
                        full,
                        self.fresh,
                        self.staging,
                        self.manifests,
                        cancel,
                    )
                }
                _ => return Err(invalid("content lease modes overlap")),
            };
        anchor.require_transition(self.held, self.root, full, self.staging, cancel)?;
        let coverage_cost = self.denial_cost(c, full, cancel)?;
        let ManagedImage::Complete(image) = &self.image else {
            return Err(invalid("companion lease complete checkpoint absent"));
        };
        let rows = source_sql::with_source(
            c,
            &self.held.artifacts.allocation.format,
            &self.staging.plan.source_prefix,
            closure,
            full,
            observation,
            Some(cancel),
            |source| {
                if let Some(coverage) = coverage {
                    image.reconcile_denial_publication_retained(
                        source,
                        observation,
                        full,
                        self.staging,
                        coverage,
                        self.manifests,
                        metadata,
                        cancel,
                    )
                } else {
                    image.reconcile_publication_retained(
                        source,
                        observation,
                        full,
                        self.manifests,
                        metadata,
                        cancel,
                    )
                }
            },
        )?;
        if &rows != retained_rows {
            return Err(invalid(
                "complete lease reconcile rows, sticky mappings or full union changed",
            ));
        }
        let p = &self.staging.plan;
        let mut derived = if let Some(coverage) = coverage {
            managed_plan::derive_reconciled_denial_data(
                &p.source_prefix,
                &rows,
                observation,
                p.source_prefix.destination_identity.clone(),
                &p.destination_parent,
                &p.destination_parent_identity,
                &p.destination_leaf_nonce,
                &self.staging.transaction_id,
                coverage,
            )?
        } else {
            managed_plan::derive_reconciled_data(
                &p.source_prefix,
                &rows,
                observation,
                p.source_prefix.destination_identity.clone(),
                &p.destination_parent,
                &p.destination_parent_identity,
                &p.destination_leaf_nonce,
                &self.staging.transaction_id,
            )?
        };
        if derived.plan != *p || derived.required_manifests.len() != self.manifests.len() {
            return Err(invalid(
                "complete lease immutable plan differs from source rederivation",
            ));
        }
        let destination = managed_destination::destination_intent(self.staging)?;
        let minimum = managed_budget::base_metadata_floor_with_coverage(
            closure.metadata_bytes(c, &p.source_prefix, Some(cancel))?,
            &derived.observation,
            &derived.placement,
            &derived.capture,
            p,
            observation.selection(),
            crate::projection::provider::managed_budget::intent_size(self.staging)?,
            crate::projection::provider::managed_budget::intent_size(&destination)?,
            coverage_cost.as_ref(),
        )?;
        if rows.metadata_bytes() < minimum {
            return Err(invalid(
                "companion lease source-rederived metadata floor insufficient",
            ));
        }
        derived.validated_metadata_floor = Some(
            minimum
                .checked_add(rows.source_row_bytes()?)
                .ok_or_else(|| invalid("companion Ready metadata overflow"))?,
        );
        Ok(derived)
    }
    pub(super) fn ready_data(
        &self,
        derived: managed_plan::ManagedPlanData,
        root: &ExactProtection,
        cancel: &Cancellation,
    ) -> CauseResult<super::ready_data::OwnedReadyData> {
        if let Some(data) = &self.denial {
            return super::ready_data::OwnedReadyData::new_denial(
                derived,
                self.staging,
                self.image.digest(),
                self.manifests,
                root,
                data.rows,
                data.coverage,
                cancel,
            );
        }
        match &self.companion {
            None => super::ready_data::OwnedReadyData::new(
                derived,
                self.staging,
                self.image.retained(),
                self.manifests,
                root,
                cancel,
            ),
            Some(data) if data.coverage.is_some() => {
                super::ready_data::OwnedReadyData::new_companion_with_coverage(
                    derived,
                    self.staging,
                    self.image.digest(),
                    self.manifests,
                    root,
                    data.publication.rows(),
                    data.coverage
                        .ok_or_else(|| invalid("C14 Ready body absent"))?,
                    cancel,
                )
            }
            Some(data) => super::ready_data::OwnedReadyData::new_companion(
                derived,
                self.staging,
                self.image.digest(),
                self.manifests,
                root,
                data.publication.rows(),
                cancel,
            ),
        }
    }
    pub(super) fn ready_companion<'p>(
        &'p self,
        full: &'p PrivateActivePublicationValidation,
    ) -> Option<ManagedCompanionReadyData<'p>> {
        self.companion
            .as_ref()
            .map(|data| ManagedCompanionReadyData {
                rows: data.publication.rows(),
                observation: data.observation,
                source_context: data.context(full),
            })
    }

    /// Actual production Store driver: B reserve -> C exact/install/protect ->
    /// B protect -> authorized checkout -> genuine private/family Ready -> B
    /// Ready atom -> separate Published. Every scope observes positive closes.
    /// The callback can request same-owner terminal replay while all proofs
    /// and the SAME mutable checkpoint are still retained, without self-reference.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn with_published_companion<
        T,
    >(
        &mut self,
        pins: &mut CompanionPins<'h>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        action: impl FnOnce(
            &mut Self,
            &super::publication::ManagedFamilyReady<'_, '_>,
            &super::source_protection::ManagedSourceCompanionProtection<'_, '_>,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        let data = self
            .companion
            .as_ref()
            .ok_or_else(|| invalid("complete companion route absent"))?
            .publication;
        if pins.ready.is_none() && pins.checkout.is_none() {
            self.reserve_source_companion(data, close, cancel)?;
            if pins.exact.is_none() {
                let expected = self.expected_checkout(cancel)?;
                let phase = self.with_checkout_authority(
                    &expected,
                    cancel,
                    |proof, destination, floor, _, _| {
                        self.read_admission(proof, destination, floor, cancel)
                    },
                )?;
                if phase == super::exact::AdmissionPhase::Protected {
                    pins.exact = Some(self.reopen_protection(cancel)?);
                } else {
                    if phase == super::exact::AdmissionPhase::Reservation {
                        self.reserve_plan(close, cancel)?;
                    }
                    for index in 0..self.manifests.len() {
                        let id = self.manifests[index].id().clone();
                        self.install_exact(&id, close, cancel)?;
                    }
                    pins.exact = Some(self.protect_exact(close, cancel)?);
                }
            }
        }
        if pins.source.is_none() {
            pins.source = Some(self.protect_source_companion(data, close, cancel)?);
        }
        let protected = pins
            .exact
            .as_ref()
            .ok_or_else(|| invalid("retained C protection absent"))?;
        protected.require(self, cancel)?;
        if pins.checkout.is_none() {
            pins.checkout = Some(self.copy_checkout(protected, close, cancel)?);
        }
        let source = pins
            .source
            .as_ref()
            .ok_or_else(|| invalid("retained B protection absent"))?;
        source.require(self, cancel)?;
        let checkout = pins
            .checkout
            .as_ref()
            .ok_or_else(|| invalid("retained C checkout absent"))?;
        checkout.require_physical(self, cancel)?;
        let private = if let Some(ready) = &pins.ready {
            super::ready::ManagedPrivateReady::from_retained(
                std::sync::Arc::clone(ready),
                protected,
                checkout,
            )
        } else {
            let private = self.finalize_private_ready(protected, checkout, close, cancel)?;
            pins.ready = Some(private.retained_data());
            private
        };
        let family = if pins.family_ready {
            super::publication::ManagedFamilyReady::reopen_companion(
                self, &private, source, cancel,
            )?
        } else {
            let family = self.publish_family_ready(&private, close, cancel)?;
            // Set before B's first writer: a failed B COMMIT acknowledgement
            // keeps the exact Ready DATA and all physical pins caller-owned.
            pins.family_ready = true;
            family
        };
        self.publish_source_companion(&family, source, close, cancel)?;
        action(self, &family, source)
    }
}
