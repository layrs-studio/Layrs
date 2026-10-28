//! Independent inspection, publication and admission of exact managed Ready.
//! Fresh Ordinary routes require their own source/destination owner capabilities.
//! No route performs recovery or cleanup.
use super::{
    bootstrap::{verify_managed_ready_checkout, VerifiedCheckout},
    database_io::{self, ReadState},
    destination_ready::{read_managed_ready, OwnedReadySnapshot},
    records::{
        import::ProviderTransitionReceiptRecord, transition::ProviderTransitionIntentRecord,
    },
    recovery::error::{invalid, ProviderBootstrapCause as Cause, Result},
};
use crate::{
    lock::StoreLock,
    projection::{
        provider::checkout_journal::{CheckoutJournal, ExpectedCheckout},
        shared::exact_content::{verify_published_cas, ExactStorage, PublishedCasProof},
        SessionMode,
    },
};
use layrs_core::scm::FileManifest;
use layrs_vfs::Cancellation;
use std::path::Path;

mod pins;
use pins::ManagedReadyPins;
mod ordinary;
pub(in crate::projection::provider) use ordinary::{
    OrdinaryReadyInspection, OrdinaryTerminalResolution,
};
mod ordinary_source;
pub(in crate::projection::provider) use ordinary_source::{
    OrdinaryAdmissionSource, OrdinaryExclusiveSource,
};
mod ordinary_family;
pub(in crate::projection::provider) use ordinary_family::{
    OrdinaryFamilyPhysical, OrdinaryFamilyResources,
};
mod ordinary_checkout;
pub(in crate::projection::provider) use ordinary_checkout::{
    OrdinaryCheckoutParts, OrdinaryCheckoutResources,
};
mod ordinary_private_ready;
pub(in crate::projection::provider) use ordinary_private_ready::{
    OrdinaryPrivateReadyParts, OrdinaryPrivateReadyPhysical, OrdinaryPrivateReadyResources,
};
mod ordinary_admission;
pub(in crate::projection::provider) use ordinary_admission::{
    OrdinaryFreshAdmission, OrdinaryFreshAdmissionFailure,
};
mod ordinary_publication;
pub(in crate::projection::provider) use ordinary_publication::{
    OrdinaryReadyPublication, OrdinaryReadyPublicationEffect, OrdinaryReadyPublicationFailure,
    OrdinaryReadyPublicationProgress,
};
mod admission;
pub(in crate::projection::provider) use admission::{
    AdmissionFailure, AdmissionStep, ManagedReadyAdmission,
};

/// Owns fresh physical capabilities. The working files and CAS objects remain
/// immutably pinned; inspection must be dropped before normal working edits.
/// SQL DATA getters alone grant no content, admission or mutation authority.
pub(in crate::projection::provider) struct ManagedReadyInspection {
    pins: ManagedReadyPins,
    data: OwnedReadySnapshot,
    cas: PublishedCasProof,
    checkout: VerifiedCheckout,
    // No upgrade: a later admission protocol must acquire its exclusive owner
    // independently and repeat all required validation under that capability.
    _owner: StoreLock,
    _lifecycle: StoreLock,
}

impl ManagedReadyInspection {
    pub(in crate::projection::provider) fn open(
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<Self> {
        Self::open_with_mode(root, SessionMode::Shared, cancel)
    }

    fn open_with_mode(root: &Path, mode: SessionMode, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        let pins = ManagedReadyPins::open(root, cancel)?;
        let lifecycle = pins.family.lifecycle_lease()?;
        let owner = StoreLock::acquire_existing(&pins.metadata_path().join("layrs.lock"), mode)?;
        pins.verify(cancel)?;
        let data = read_snapshot(&pins, cancel)?;
        pins.require_data(&data, cancel)?;
        // Both read transactions and their catalog gate have positively closed.
        // Full byte verification occurs exclusively outside catalog scopes.
        let storage = ExactStorage::open(
            pins.family.root(),
            &data.binding.family_root_identity,
            pins.family.lifecycle_lease()?,
        )?;
        let cas = verify_published_cas(storage, &data.binding, &data.required, cancel)?;
        pins.require_data(&data, cancel)?;
        let checkout = verify_managed_ready_checkout(
            pins.root.try_clone()?,
            pins.control.try_clone()?,
            &data.expected,
            &data.journal,
            cancel,
        )?;
        cas.require(&data.binding, &data.required, cancel)?;
        checkout.require(&data.expected, &data.journal, cancel)?;
        let current = read_snapshot(&pins, cancel)?;
        data.require_same(&current)?;
        pins.require_data(&data, cancel)?;
        cas.require(&data.binding, &data.required, cancel)?;
        checkout.require(&data.expected, &data.journal, cancel)?;
        cancel.check()?;
        Ok(Self {
            pins,
            data,
            cas,
            checkout,
            _owner: owner,
            _lifecycle: lifecycle,
        })
    }

    /// Revalidate physical identities, every exact retained journal and the two
    /// fresh SQL snapshots. Immutable pins make another full hash unnecessary.
    pub(in crate::projection::provider) fn require(&self, cancel: &Cancellation) -> Result<()> {
        self.pins.require_data(&self.data, cancel)?;
        self.cas
            .require(&self.data.binding, &self.data.required, cancel)?;
        self.checkout
            .require(&self.data.expected, &self.data.journal, cancel)?;
        let current = read_snapshot(&self.pins, cancel)?;
        self.data.require_same(&current)?;
        self.pins.require_data(&self.data, cancel)?;
        self.cas
            .require(&self.data.binding, &self.data.required, cancel)?;
        self.checkout
            .require(&self.data.expected, &self.data.journal, cancel)?;
        cancel.check()?;
        Ok(())
    }

    pub(in crate::projection::provider) fn root(&self) -> &Path {
        self.pins.checkout_path()
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.data.receipt
    }
    pub(in crate::projection::provider) fn ready(&self) -> &ProviderTransitionIntentRecord {
        &self.data.ready
    }
    pub(in crate::projection::provider) fn expected(&self) -> &ExpectedCheckout {
        &self.data.expected
    }
    pub(in crate::projection::provider) fn journal(&self) -> &CheckoutJournal {
        &self.data.journal
    }
    pub(in crate::projection::provider) fn required(&self) -> &[FileManifest] {
        &self.data.required
    }
}

fn read_snapshot(pins: &ManagedReadyPins, cancel: &Cancellation) -> Result<OwnedReadySnapshot> {
    pins.verify(cancel)?;
    let result = {
        // Lock order is lifecycle -> private owner -> short family catalog.
        let _catalog = StoreLock::acquire_existing(
            &pins.family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        pins.verify(cancel)?;
        let result = valid(database_io::read_cancellable(
            &pins.database,
            cancel,
            |destination| {
                valid(database_io::read_cancellable(
                    &pins.family.catalog,
                    cancel,
                    |family| {
                        let data =
                            read_managed_ready(destination, &pins.marker, family, Some(cancel))?
                                .into_snapshot()?;
                        pins.require_data(&data, cancel)?;
                        Ok(data)
                    },
                )?)
            },
        )?)?;
        // Family and then private database positively close before DATA escapes.
        pins.verify(cancel)?;
        result
    };
    pins.require_data(&result, cancel)?;
    Ok(result)
}

fn valid<T>(state: ReadState<T>) -> Result<T> {
    match state {
        ReadState::Valid(value) => Ok(value),
        ReadState::Uninitialized => Err(invalid("managed Ready database is uninitialized")),
        ReadState::Hot(hot) => Err(Cause::ReadonlyRollback(hot.trigger)),
        ReadState::UnboundRollback { trigger, cause } => Err(Cause::Cleanup {
            primary: Box::new(Cause::ReadonlyRollback(trigger)),
            close: Box::new(cause),
        }),
        ReadState::Rejected { cause, .. }
        | ReadState::ReadButUnavailable(cause)
        | ReadState::Unavailable(cause) => Err(cause),
    }
}
