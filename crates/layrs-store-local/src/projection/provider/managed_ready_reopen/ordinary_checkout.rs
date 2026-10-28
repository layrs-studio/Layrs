//! Dedicated fresh Exclusive C resources. No allocation adoption, SQL phase
//! interpretation, partial VerifiedCheckout or terminal capability exists here.
use super::*;
use crate::projection::{
    format,
    provider::bootstrap::CheckoutStorage,
    shared::{exact_content::ExactBinding, provider_access::FamilyAccess},
};

/// Calculation DATA only; the driver separately authenticates actual Staging.
pub(in crate::projection::provider) struct OrdinaryCheckoutParts<'a> {
    pub calculated_ready: &'a ProviderTransitionIntentRecord,
    pub expected: &'a ExpectedCheckout,
    pub required: &'a [FileManifest],
    pub binding: &'a ExactBinding,
}

pub(in crate::projection::provider) struct OrdinaryCheckoutResources {
    pins: ManagedReadyPins,
    _owner: StoreLock,
    _lifecycle: StoreLock,
}

impl OrdinaryCheckoutResources {
    pub(super) fn open_exclusive(pins: ManagedReadyPins, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        let lifecycle = pins.family.lifecycle_lease()?;
        let owner = StoreLock::acquire_existing(
            &pins.metadata_path().join("layrs.lock"),
            SessionMode::Exclusive,
        )?;
        pins.verify(cancel)?;
        Ok(Self {
            pins,
            _owner: owner,
            _lifecycle: lifecycle,
        })
    }

    pub(in crate::projection::provider) fn marker(&self) -> &format::Marker {
        &self.pins.marker
    }
    pub(in crate::projection::provider) fn database(&self) -> &database_io::DatabaseFile {
        &self.pins.database
    }
    pub(in crate::projection::provider) fn family(&self) -> &FamilyAccess {
        &self.pins.family
    }
    pub(in crate::projection::provider) fn require_identity(
        &self,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.pins.verify(cancel)
    }
    pub(in crate::projection::provider) fn require_parts(
        &self,
        parts: &OrdinaryCheckoutParts<'_>,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.pins.require_parts(
            &self.pins.marker,
            parts.calculated_ready,
            parts.expected,
            parts.binding,
            cancel,
        )
    }
    pub(in crate::projection::provider) fn checkout_storage(
        &self,
        parts: &OrdinaryCheckoutParts<'_>,
        cancel: &Cancellation,
    ) -> Result<CheckoutStorage> {
        self.require_parts(parts, cancel)?;
        CheckoutStorage::new(
            self.pins.root.try_clone()?,
            self.pins.control.try_clone()?,
            parts.expected,
            cancel,
        )
    }
    pub(in crate::projection::provider) fn pin_cas(
        &self,
        parts: &OrdinaryCheckoutParts<'_>,
        cancel: &Cancellation,
    ) -> Result<PublishedCasProof> {
        self.require_parts(parts, cancel)?;
        let storage = ExactStorage::open(
            self.pins.family.root(),
            &parts.binding.family_root_identity,
            self.pins.family.lifecycle_lease()?,
        )?;
        let cas = verify_published_cas(storage, parts.binding, parts.required, cancel)?;
        self.require_parts(parts, cancel)?;
        cas.require(parts.binding, parts.required, cancel)?;
        Ok(cas)
    }
}
