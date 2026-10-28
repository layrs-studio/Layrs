//! Exact C byte pins plus a durable family root. No checkout or Ready authority.
use super::{
    allocation::ManagedDestinationAllocation,
    content::{self, Catalog, ContentPhase},
    *,
};
use crate::projection::{
    provider::{
        database_io::WriterCloseObserver,
        recovery::error::{invalid as cause_invalid, Result as CauseResult},
    },
    shared::{
        exact_content::{self, ExactBinding, ExactCatalog, PublishedCasProof},
        exact_protection::{ExactProtection, ProtectionPresence},
    },
};

pub(in crate::projection::provider) struct OrdinaryImportProtection {
    binding: ExactBinding,
    root: ExactProtection,
    bytes: PublishedCasProof,
}
impl OrdinaryImportProtection {
    pub(super) fn require<O: WriterCloseObserver>(
        &self,
        catalog: &Catalog<'_, '_, O>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let binding = catalog.binding()?;
        if binding != self.binding {
            return Err(cause_invalid("ordinary protection binding differs"));
        }
        self.root
            .require_binding(&binding, &catalog.plan.required, Some(cancel))?;
        self.bytes
            .require(&binding, &catalog.plan.required, cancel)?;
        catalog.boundary(cancel)
    }
}

impl OrdinaryManagedExport<'_> {
    pub(in crate::projection::provider) fn protect_content(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<OrdinaryImportProtection> {
        self.with_content_source(
            destination,
            staging,
            ContentPhase::Protection,
            close,
            cancel,
            |catalog| {
                let binding = catalog.binding()?;
                let root = content::expected_protection(catalog.target, catalog.plan, cancel)?;
                let retained = catalog.retained;
                let target = catalog.target;
                let required = &catalog.plan.required;
                let marker = catalog.destination.family().marker_bytes();
                let read = |c: &Connection| {
                    retained
                        .validate_export_protection(
                            c,
                            marker,
                            target,
                            required,
                            &root,
                            Some(cancel),
                        )
                        .map(|(_, state)| state)
                };
                let before = catalog.with_catalog(false, cancel, read)?;
                // C CAS bytes are hashed outside SQL scopes, independently of B's
                // retained CAS proof. All immutable pins survive COMMIT and close.
                let bytes = exact_content::verify_published_cas(
                    catalog.pin_storage()?,
                    &binding,
                    required,
                    cancel,
                )?;
                bytes.require(&binding, required, cancel)?;
                catalog.with_catalog(before.root == ProtectionPresence::Absent, cancel, |c| {
                    if read(c)? != before {
                        return Err(cause_invalid("ordinary protection predecessor changed"));
                    }
                    bytes.require(&binding, required, cancel)?;
                    if before.root == ProtectionPresence::Absent {
                        retained.protect_export(
                            c,
                            marker,
                            target,
                            required,
                            &root,
                            Some(cancel),
                        )?;
                    }
                    let after = read(c)?;
                    if after.root != ProtectionPresence::Present
                        || after.installation != before.installation
                    {
                        return Err(cause_invalid(
                            "ordinary protection prospective state differs",
                        ));
                    }
                    bytes.require(&binding, required, cancel)
                })?;
                // No readback is reached if COMMIT/close was not acknowledged.
                catalog.with_catalog(false, cancel, |c| {
                    let after = read(c)?;
                    if after.root != ProtectionPresence::Present
                        || after.installation != before.installation
                    {
                        return Err(cause_invalid("ordinary protection durable state differs"));
                    }
                    bytes.require(&binding, required, cancel)
                })?;
                let proof = OrdinaryImportProtection {
                    binding,
                    root,
                    bytes,
                };
                proof.require(catalog, cancel)?;
                Ok(proof)
            },
        )
    }

    /// Read-only reconstruction after a journal already exists. The common
    /// Checkout mode requires the exact committed C root and complete SQL gate.
    pub(in crate::projection::provider) fn reopen_content_protection(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<OrdinaryImportProtection> {
        self.with_content_source(
            destination,
            staging,
            ContentPhase::Checkout,
            close,
            cancel,
            |catalog| {
                let binding = catalog.binding()?;
                let root = content::expected_protection(catalog.target, catalog.plan, cancel)?;
                let retained = catalog.retained;
                let target = catalog.target;
                let required = &catalog.plan.required;
                let marker = catalog.destination.family().marker_bytes();
                let read = |c: &Connection| {
                    retained
                        .validate_export_protection(
                            c,
                            marker,
                            target,
                            required,
                            &root,
                            Some(cancel),
                        )
                        .map(|(_, state)| state)
                };
                let before = catalog.with_catalog(false, cancel, read)?;
                let bytes = exact_content::verify_published_cas(
                    catalog.pin_storage()?,
                    &binding,
                    &catalog.plan.required,
                    cancel,
                )?;
                catalog.with_catalog(false, cancel, |c| {
                    if read(c)? != before {
                        return Err(cause_invalid(
                            "ordinary checkout protection changed while reopening",
                        ));
                    }
                    bytes.require(&binding, required, cancel)
                })?;
                let proof = OrdinaryImportProtection {
                    binding,
                    root,
                    bytes,
                };
                proof.require(catalog, cancel)?;
                let expected = content::expected_checkout(catalog.staging, catalog.image, cancel)?;
                if catalog.read_checkout(&expected, &proof, cancel)?.is_none() {
                    return Err(cause_invalid(
                        "ordinary protection reopen requires an existing checkout journal",
                    ));
                }
                Ok(proof)
            },
        )
    }
}
