//! C14 complete-checkpoint route without a companion header or source s root.
//! The real normalized rows/body and the original live source anchor remain
//! borrowed. Every SQL scope still classifies the retained document funding d.
use super::*;
use crate::projection::provider::{
    destination_ready::ManagedDenialReadyData,
    records::{denied_coverage::DerivedDeniedCoverage, import::ProviderTransitionReceiptRecord},
    transition_observation::TransitionObservationData,
    transition_reconcile::companions::OwnedCompanionData,
};

pub(super) struct DenialLeaseData<'h> {
    pub(super) anchor: &'h session::ProviderTransitionSource<'h>,
    pub(super) observation: &'h TransitionObservationData,
    pub(super) coverage: &'h DerivedDeniedCoverage,
    pub(super) rows: &'h OwnedCompanionData,
    pub(super) metadata: u64,
}

/// Same-owner retry state. There is deliberately no source companion proof:
/// all byte/checkout proofs belong to C and d is re-read from actual source SQL.
#[derive(Default)]
pub(in crate::projection::provider::bootstrap::preparation::activation) struct DenialPins {
    exact: Option<super::protection::ManagedImportProtection>,
    checkout: Option<super::checkout::ManagedCheckoutProof>,
    ready: Option<std::sync::Arc<super::ready_data::OwnedReadyData>>,
    family_ready: bool,
}

impl<'h, 'i, 'd, 'a, 'c> ManagedImportLease<'h, 'i, 'd, 'a, 'c> {
    #[allow(clippy::too_many_arguments)]
    fn open_denial(
        held: &'h outcome::PublicationPins,
        root: &'h layrs_vfs::ProviderRootGuard,
        mount: &'h ProviderActiveMountRecord,
        activation: &'h ProviderActivationRecord,
        attempt: &'h CaptureAttemptLock,
        destination: &'h managed_allocation::ManagedDestinationAllocation<'d>,
        image: &'i mut managed_image::complete::VerifiedCompleteManagedImage<'a, 'c>,
        staging: &'h ProviderTransitionIntentRecord,
        rows: &'h OwnedCompanionData,
        anchor: &'h session::ProviderTransitionSource<'h>,
        observation: &'h TransitionObservationData,
        metadata: u64,
        coverage: &'h DerivedDeniedCoverage,
        pins: &DenialPins,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        if rows.header().is_some()
            || staging.plan.companion_id.is_some()
            || staging.plan.companion_files.count != 0
            || staging.plan.companion_deletions.count != 0
        {
            return Err(invalid("plain denial opener refuses companion rows"));
        }
        coverage.record().require_intent(staging, Some(cancel))?;
        coverage.record().require_observation(
            &staging.plan.source_prefix,
            &staging.transaction_id,
            observation,
            Some(cancel),
        )?;
        rows.require_denial_plan_observation(&staging.plan.source_prefix, observation)?;
        if pins.family_ready && pins.ready.is_none()
            || pins.ready.is_some() && pins.checkout.is_none()
        {
            return Err(invalid(
                "plain denial retained phase lacks checkout or private Ready",
            ));
        }
        let mut lease = Self {
            held,
            root,
            mount,
            activation,
            attempt,
            destination,
            image: companion::ManagedImage::Complete(image),
            companion: None,
            denial: Some(DenialLeaseData {
                anchor,
                observation,
                coverage,
                rows,
                metadata,
            }),
            staging,
            manifests: rows.manifests(),
            fresh: observation.snapshot(),
        };
        lease.require_intent()?;
        lease.expected_protection(cancel)?;
        if let Some(checkout) = &pins.checkout {
            pins.exact
                .as_ref()
                .ok_or_else(|| invalid("plain denial checkout lacks C protection"))?
                .require(&lease, cancel)?;
            checkout.require_physical(&lease, cancel)?;
            let ImportSource::Provider { version, .. } = &staging.plan.source_prefix.source else {
                return Err(invalid("plain denial retained Provider prefix absent"));
            };
            // Terminal replay never rehashes B or opens its old content reader.
            // Full source/family/C Ready SQL is checked by the driver below.
            lease
                .image
                .require_retained(root, &version.effective_image_digest, cancel)?;
            return Ok(lease);
        }
        lease.rehash(cancel)?;
        let locks = held
            .family
            .locks(&held.artifacts.allocation.format.identity.worktree_id, true)?;
        lease.with_source(cancel, |c, full, closure| {
            lease.rederive(c, closure, full, cancel)?;
            Ok(())
        })?;
        drop(locks);
        // SQL/body/row rederivation and positive close precede physical pins.
        let storage = lease.pin_exact_storage()?;
        let companion::ManagedImage::Complete(image) = &mut lease.image else {
            return Err(invalid("plain denial requires complete checkpoint"));
        };
        image.pin_offline_current(storage, lease.manifests, cancel)?;
        lease.rehash(cancel)?;
        Ok(lease)
    }

    pub(super) fn ready_denial<'p>(
        &'p self,
        full: &'p PrivateActivePublicationValidation,
    ) -> Option<ManagedDenialReadyData<'p>> {
        self.denial.as_ref().map(|data| ManagedDenialReadyData {
            rows: data.rows,
            observation: data.observation,
            coverage: data.coverage,
            source_context:
                crate::projection::shared::source_companion::SourceCompanionFamilyContext {
                    publication: full,
                    pending: None,
                },
        })
    }

    fn finish_denial(
        &mut self,
        pins: &mut DenialPins,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ProviderTransitionReceiptRecord> {
        if self.denial.is_none() || self.companion.is_some() {
            return Err(invalid("plain denial driver requires its closed mode"));
        }
        if pins.ready.is_none() && pins.checkout.is_none() && pins.exact.is_none() {
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
        let protected = pins
            .exact
            .as_ref()
            .ok_or_else(|| invalid("plain denial retained C protection absent"))?;
        protected.require(self, cancel)?;
        if pins.checkout.is_none() {
            pins.checkout = Some(self.copy_checkout(protected, close, cancel)?);
        }
        let checkout = pins
            .checkout
            .as_ref()
            .ok_or_else(|| invalid("plain denial retained checkout absent"))?;
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
            super::publication::ManagedFamilyReady::reopen_denial(self, &private, cancel)?
        } else {
            let family = self.publish_family_ready(&private, close, cancel)?;
            // Retain private/family progress before the first B writer. An ACK
            // refusal after B Ready/Published leaves this retry carrier intact.
            pins.family_ready = true;
            family
        };
        self.publish_source_denial(&family, close, cancel)?;
        Ok(ProviderTransitionReceiptRecord::decode(
            &family.receipt().encode()?,
        )?)
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider::bootstrap::preparation::activation) fn commit_denial<
    'h,
    'i,
    'd,
    'a,
    'c,
>(
    held: &'h outcome::PublicationPins,
    root: &'h layrs_vfs::ProviderRootGuard,
    mount: &'h ProviderActiveMountRecord,
    activation: &'h ProviderActivationRecord,
    attempt: &'h CaptureAttemptLock,
    destination: &'h managed_allocation::ManagedDestinationAllocation<'d>,
    image: &'i mut managed_image::complete::VerifiedCompleteManagedImage<'a, 'c>,
    staging: &'h ProviderTransitionIntentRecord,
    rows: &'h OwnedCompanionData,
    anchor: &'h session::ProviderTransitionSource<'h>,
    observation: &'h TransitionObservationData,
    metadata: u64,
    coverage: &'h DerivedDeniedCoverage,
    pins: &mut DenialPins,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<ProviderTransitionReceiptRecord> {
    let mut lease = ManagedImportLease::open_denial(
        held,
        root,
        mount,
        activation,
        attempt,
        destination,
        image,
        staging,
        rows,
        anchor,
        observation,
        metadata,
        coverage,
        pins,
        cancel,
    )?;
    lease.finish_denial(pins, close, cancel)
}
