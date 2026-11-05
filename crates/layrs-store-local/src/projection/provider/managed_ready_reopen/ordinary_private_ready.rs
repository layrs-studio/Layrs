//! Preliminary exclusive C resources and mechanical checkout/CAS proofs only.
//! No SQL Staging/Ready interpretation, allocation adoption or terminal/admission
//! capability is constructed here. The fresh B factory is the only opener.
use super::*;
use crate::projection::{
    format,
    shared::{exact_content::ExactBinding, provider_access::FamilyAccess},
};

/// Descriptive physical inventory, deliberately independent of actual C phase.
/// `ready` is calculation DATA until the driver has observed actual Ready.
pub(in crate::projection::provider) struct OrdinaryPrivateReadyParts<'a> {
    pub ready: &'a ProviderTransitionIntentRecord,
    pub expected: &'a ExpectedCheckout,
    pub journal: &'a CheckoutJournal,
    pub required: &'a [FileManifest],
    pub binding: &'a ExactBinding,
}

pub(in crate::projection::provider) struct OrdinaryPrivateReadyResources {
    pins: ManagedReadyPins,
    _owner: StoreLock,
    _lifecycle: StoreLock,
}

pub(in crate::projection::provider) struct OrdinaryPrivateReadyPhysical {
    cas: PublishedCasProof,
    checkout: VerifiedCheckout,
}

impl OrdinaryPrivateReadyResources {
    // Caller already holds B Exclusive and has checked distinct B/C and their
    // exact shared family. C is acquired Exclusive immediately, never upgraded.
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
        parts: &OrdinaryPrivateReadyParts<'_>,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.pins.require_parts(
            &self.pins.marker,
            parts.ready,
            parts.expected,
            parts.binding,
            cancel,
        )
    }

    /// The driver must first finish actual-B/C/full-family SQL validation and
    /// positively close all read scopes, then repeat that read after pinning.
    /// A complete published checkout journal can be verified while C is still
    /// Staging; these physical proofs do not claim that a Ready suffix exists.
    pub(in crate::projection::provider) fn pin_checkout(
        &self,
        parts: &OrdinaryPrivateReadyParts<'_>,
        cancel: &Cancellation,
    ) -> Result<OrdinaryPrivateReadyPhysical> {
        self.require_parts(parts, cancel)?;
        let storage = ExactStorage::open(
            self.pins.family.root(),
            &parts.binding.family_root_identity,
            self.pins.family.lifecycle_lease()?,
        )?;
        let cas = verify_published_cas(storage, parts.binding, parts.required, cancel)?;
        self.require_parts(parts, cancel)?;
        let checkout = verify_managed_ready_checkout(
            self.pins.root.try_clone()?,
            self.pins.control.try_clone()?,
            parts.expected,
            parts.journal,
            cancel,
        )?;
        cas.require(parts.binding, parts.required, cancel)?;
        checkout.require(parts.expected, parts.journal, cancel)?;
        Ok(OrdinaryPrivateReadyPhysical { cas, checkout })
    }
}

impl OrdinaryPrivateReadyPhysical {
    pub(in crate::projection::provider) fn require_identity(
        &self,
        parts: &OrdinaryPrivateReadyParts<'_>,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.cas.verify_identity(cancel)?;
        self.checkout
            .require_identity(parts.expected, parts.journal, cancel)
    }

    pub(in crate::projection::provider) fn require_physical(
        &self,
        parts: &OrdinaryPrivateReadyParts<'_>,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.cas.require(parts.binding, parts.required, cancel)?;
        self.checkout.require(parts.expected, parts.journal, cancel)
    }
}
