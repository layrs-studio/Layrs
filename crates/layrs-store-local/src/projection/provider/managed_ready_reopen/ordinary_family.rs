//! Preliminary C resources for the fresh actual-B-backed family publisher.
//! This is neither terminal Ready inspection nor admission/publication authority.
//! Only the exclusive B source factory can acquire these shared C resources.
use super::*;
use crate::projection::{format, shared::provider_access::FamilyAccess};

pub(in crate::projection::provider) struct OrdinaryFamilyResources {
    pins: ManagedReadyPins,
    _owner: StoreLock,
    _lifecycle: StoreLock,
}

/// Mechanical immutable byte proofs only. Supplying detached SQL DATA cannot
/// authorize a catalog writer or construct a terminal/admission holder.
pub(in crate::projection::provider) struct OrdinaryFamilyPhysical {
    cas: PublishedCasProof,
    checkout: VerifiedCheckout,
}

impl OrdinaryFamilyResources {
    // The B factory checks distinct checkout/private DB and the exact family
    // before calling this constructor and attempting C's shared owner.
    pub(super) fn open_shared(pins: ManagedReadyPins, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        let lifecycle = pins.family.lifecycle_lease()?;
        let owner = StoreLock::acquire_existing(
            &pins.metadata_path().join("layrs.lock"),
            SessionMode::Shared,
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
        data: &super::super::destination_ready::OrdinaryPrivateReadySnapshot,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.pins.require_parts(
            data.marker(),
            data.ready(),
            data.expected(),
            data.binding(),
            cancel,
        )
    }
    pub(in crate::projection::provider) fn require_data(
        &self,
        data: &OwnedReadySnapshot,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.pins.require_data(data, cancel)
    }

    /// Caller first completes the actual-B/private/whole-family reader and
    /// positively closes all SQL scopes. Only then may it hash/pin C bytes.
    /// The caller must repeat that full read after these filesystem effects.
    pub(in crate::projection::provider) fn pin_ready(
        &self,
        data: &OwnedReadySnapshot,
        cancel: &Cancellation,
    ) -> Result<OrdinaryFamilyPhysical> {
        self.pins.require_data(data, cancel)?;
        let storage = ExactStorage::open(
            self.pins.family.root(),
            &data.binding.family_root_identity,
            self.pins.family.lifecycle_lease()?,
        )?;
        let cas = verify_published_cas(storage, &data.binding, &data.required, cancel)?;
        self.pins.require_data(data, cancel)?;
        let checkout = verify_managed_ready_checkout(
            self.pins.root.try_clone()?,
            self.pins.control.try_clone()?,
            &data.expected,
            &data.journal,
            cancel,
        )?;
        cas.require(&data.binding, &data.required, cancel)?;
        checkout.require(&data.expected, &data.journal, cancel)?;
        Ok(OrdinaryFamilyPhysical { cas, checkout })
    }
}

impl OrdinaryFamilyPhysical {
    pub(in crate::projection::provider) fn require_identity(
        &self,
        data: &OwnedReadySnapshot,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.cas.verify_identity(cancel)?;
        self.checkout
            .require_identity(&data.expected, &data.journal, cancel)
    }

    pub(in crate::projection::provider) fn require_physical(
        &self,
        data: &OwnedReadySnapshot,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.cas.require(&data.binding, &data.required, cancel)?;
        self.checkout.require(&data.expected, &data.journal, cancel)
    }
}
