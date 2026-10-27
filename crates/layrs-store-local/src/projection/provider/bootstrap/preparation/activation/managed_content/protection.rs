//! Retained managed protection. Hash outside catalog scopes; acknowledge only
//! after positive family/source closes and complete fresh SQL readback.
use super::*;
use crate::projection::shared::{
    exact_content::{verify_published_cas, ExactBinding, PublishedCasProof},
    exact_protection::{ExactProtection, ProtectionEffect, ProtectionPresence},
    provider_access::provider_import::content::ImportProtectionState,
};

/// Keeps the byte proof and lifecycle alive. This is not a checkout or Ready
/// capability; later phases must revalidate their own complete private state.
pub(in crate::projection::provider::bootstrap::preparation::activation) struct ManagedImportProtection
{
    binding: ExactBinding,
    root: ExactProtection,
    bytes: PublishedCasProof,
}
impl ManagedImportProtection {
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn root(
        &self,
    ) -> &ExactProtection {
        &self.root
    }
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn require(
        &self,
        lease: &ManagedImportLease<'_, '_, '_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let binding = lease.exact_binding()?;
        if binding != self.binding {
            return Err(invalid("managed protection belongs to another lease"));
        }
        self.root
            .require_binding(&binding, lease.manifests, Some(cancel))?;
        self.bytes.require(&binding, lease.manifests, cancel)?;
        lease.boundary(cancel)
    }
}

impl<'h, 'd, 'a, 'c> ManagedImportLease<'h, 'd, 'a, 'c> {
    /// Reconstruct physical protection after a checkout journal has already
    /// been committed. This route is read-only and requires an existing exact
    /// root; it cannot repair an absent root or bypass the typed journal.
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn reopen_protection(
        &mut self,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedImportProtection> {
        self.rehash(cancel)?;
        let expected_checkout = self.expected_checkout(cancel)?;
        let binding = self.exact_binding()?;
        let expected = self.expected_protection(cancel)?;
        let before = self.with_checkout_authority(
            &expected_checkout,
            cancel,
            |proof, destination, floor, _, _| {
                self.require_checkout_protection(
                    &expected_checkout,
                    proof,
                    destination,
                    floor,
                    cancel,
                )?;
                self.read_protection(proof, destination, floor, &binding, &expected, cancel)
            },
        )?;
        // Hashing may invoke filesystem reads and remains outside SQL gates.
        let bytes =
            verify_published_cas(self.pin_exact_storage()?, &binding, self.manifests, cancel)?;
        bytes.require(&binding, self.manifests, cancel)?;
        self.with_checkout_authority(
            &expected_checkout,
            cancel,
            |proof, destination, floor, _, _| {
                self.require_checkout_protection(
                    &expected_checkout,
                    proof,
                    destination,
                    floor,
                    cancel,
                )?;
                let after =
                    self.read_protection(proof, destination, floor, &binding, &expected, cancel)?;
                if after != before || after.root != ProtectionPresence::Present {
                    return Err(invalid(
                        "managed checkout protection changed while reopening",
                    ));
                }
                bytes.require(&binding, self.manifests, cancel)
            },
        )?;
        let protected = ManagedImportProtection {
            binding,
            root: expected,
            bytes,
        };
        protected.require(self, cancel)?;
        Ok(protected)
    }

    pub(super) fn expected_protection(
        &self,
        cancel: &Cancellation,
    ) -> CauseResult<ExactProtection> {
        let binding = self.exact_binding()?;
        ExactProtection::new(
            &binding.family_incarnation,
            &binding.owner,
            &binding.database_identity,
            &binding.transaction_id,
            self.manifests,
            Some(cancel),
        )
    }

    pub(super) fn read_protection(
        &self,
        proof: &PrivateActiveCaptureValidation,
        destination: &ProviderTransitionIntentRecord,
        metadata_floor: u64,
        binding: &ExactBinding,
        expected: &ExactProtection,
        cancel: &Cancellation,
    ) -> CauseResult<ImportProtectionState> {
        let inputs = self.inputs(proof, destination)?;
        managed_destination::valid(database_io::read_cancellable(
            &self.held.family.catalog,
            cancel,
            |c| {
                self.held.family.validate_import_protection(
                    c,
                    &inputs,
                    binding,
                    expected,
                    metadata_floor,
                    Some(cancel),
                )
            },
        )?)
    }

    pub(in crate::projection::provider::bootstrap::preparation::activation) fn protect_exact(
        &mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedImportProtection> {
        self.rehash(cancel)?;
        let binding = self.exact_binding()?;
        let expected = self.expected_protection(cancel)?;
        let before = self.with_authority_metadata(cancel, |proof, destination, floor| {
            self.read_protection(proof, destination, floor, &binding, &expected, cancel)
        })?;
        // No SQL owner/catalog gates are retained during byte verification.
        let bytes =
            verify_published_cas(self.pin_exact_storage()?, &binding, self.manifests, cancel)?;
        bytes.require(&binding, self.manifests, cancel)?;
        self.with_authority_metadata(cancel, |proof, destination, floor| {
            let inputs = self.inputs(proof, destination)?;
            let check = |c: &Connection| {
                self.held.family.validate_import_protection(
                    c,
                    &inputs,
                    &binding,
                    &expected,
                    floor,
                    Some(cancel),
                )
            };
            let current =
                self.read_protection(proof, destination, floor, &binding, &expected, cancel)?;
            if current != before {
                return Err(invalid("managed protection predecessor changed"));
            }
            bytes.require(&binding, self.manifests, cancel)?;
            if current.root == ProtectionPresence::Present {
                return Ok(ProtectionEffect::AlreadyPresent);
            }
            let file = &self.held.family.catalog;
            database_io::validate_native_filename(file.path())?;
            let admission = database_io::clean_admission(file, false, |c| {
                if check(c)? != current {
                    return Err(invalid("managed protection admission changed"));
                }
                Ok(())
            })?;
            self.boundary(cancel)?;
            let bare = admission.open(file, close)?;
            let result = (|| {
                admission.require_geometry(&bare)?;
                let c = bare.connection();
                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                if check(c)? != current {
                    return Err(invalid("managed protection writer predecessor changed"));
                }
                bytes.require(&binding, self.manifests, cancel)?;
                let effect = self.held.family.protect_import(
                    c,
                    &inputs,
                    &binding,
                    &expected,
                    floor,
                    Some(cancel),
                )?;
                let after = check(c)?;
                if after.root != ProtectionPresence::Present
                    || after.installation != current.installation
                {
                    return Err(invalid("managed protection prospective readback differs"));
                }
                bytes.require(&binding, self.manifests, cancel)?;
                self.boundary(cancel)?;
                c.execute_batch("COMMIT")?;
                Ok(effect)
            })();
            bare.finish_writer(result, close)
        })?;
        // The outer source read close has succeeded; nothing is acknowledged
        // on a failed family or source close, even if COMMIT took effect.
        bytes.require(&binding, self.manifests, cancel)?;
        self.with_authority_metadata(cancel, |proof, destination, floor| {
            let after =
                self.read_protection(proof, destination, floor, &binding, &expected, cancel)?;
            if after.root != ProtectionPresence::Present
                || after.installation != before.installation
            {
                return Err(invalid("managed protection durable readback differs"));
            }
            Ok(())
        })?;
        let protected = ManagedImportProtection {
            binding,
            root: expected,
            bytes,
        };
        protected.require(self, cancel)?;
        Ok(protected)
    }
}
