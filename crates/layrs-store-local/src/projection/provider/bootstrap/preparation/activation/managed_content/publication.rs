//! Family Ready publication under retained private Ready and physical proofs.
//! Source remains Staging; ordinary admission and cleanup stay separate.
use super::ready::ManagedPrivateReady;
use super::ready_data::OwnedReadyData;
use super::*;
use crate::projection::{
    provider::destination_ready::{self, ManagedReadyInputs},
    shared::{
        exact_content::ExactBinding,
        provider_access::provider_import::publication::{
            ImportPublicationInputs, ImportPublicationMember, ImportPublicationState,
        },
    },
};

pub(in crate::projection::provider::bootstrap::preparation::activation) struct ManagedFamilyReady<
    'p,
    'b,
> {
    private: &'p ManagedPrivateReady<'b>,
}

impl<'p, 'b> ManagedFamilyReady<'p, 'b> {
    /// Both actual C Ready scopes and B's exact publication phase are checked
    /// again before this candidate escapes. Retained DATA alone grants nothing.
    pub(super) fn reopen_companion(
        lease: &ManagedImportLease<'_, '_, '_, '_, '_>,
        private: &'p ManagedPrivateReady<'b>,
        source: &super::source_protection::ManagedSourceCompanionProtection<'_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        let candidate = Self { private };
        lease.require_retained_family_ready(&candidate, source, cancel)?;
        Ok(candidate)
    }
    pub(super) fn reopen_denial(
        lease: &ManagedImportLease<'_, '_, '_, '_, '_>,
        private: &'p ManagedPrivateReady<'b>,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        let candidate = Self { private };
        lease.require_retained_denial_family_ready(&candidate, cancel)?;
        Ok(candidate)
    }
    pub(super) fn private_ready(&self) -> &ManagedPrivateReady<'_> {
        self.private
    }
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn receipt(
        &self,
    ) -> &crate::projection::provider::records::import::ProviderTransitionReceiptRecord {
        self.private.receipt()
    }
}

impl ManagedImportLease<'_, '_, '_, '_, '_> {
    fn with_family_publication_source<T>(
        &self,
        private: &ManagedPrivateReady<'_>,
        cancel: &Cancellation,
        action: impl FnOnce(
            &Connection,
            &Connection,
            &ManagedReadyInputs<'_>,
            &ImportPublicationInputs<'_>,
            &ExactBinding,
            u64,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.require_intent()?;
        private.require_physical(self, cancel)?;
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.boundary(cancel)?;
        let value =
            self.with_source(cancel, |source_c, proof, closure| {
                self.image.require_owner(
                    self.root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                let derived = self.rederive(source_c, closure, proof, cancel)?;
                let (protected, _) = private.physical_parts();
                let owned = self.ready_data(derived, protected.root(), cancel)?;
                if owned.ready() != private.ready_intent() || owned.receipt() != private.receipt() {
                    return Err(invalid("family publication private Ready proof differs"));
                }
                let data = owned.as_data(self.manifests)?;
                let prepared =
                    self.held.artifacts.prepared.as_ref().ok_or_else(|| {
                        invalid("family publication source prepared receipt absent")
                    })?;
                let inputs = ManagedReadyInputs {
                    marker: &self.destination.proof().format,
                    source: &proof.active.source,
                    allocation: &self.held.artifacts.allocation,
                    prepared,
                    activation: &proof.active.activation,
                    prefix: &self.staging.plan.source_prefix,
                    staging: owned.destination_staging(),
                    closure,
                    family_marker_bytes: self.held.family.marker_bytes(),
                    data: &data,
                    companion: self.ready_companion(proof),
                    denial: self.ready_denial(proof),
                };
                let publication = ImportPublicationInputs {
                    content: self.inputs(proof, owned.destination_staging())?,
                    destination_ready: owned.ready(),
                    receipt: owned.receipt(),
                };
                let binding = self.exact_binding()?;
                private.require_physical(self, cancel)?;
                // Retain the destination read transaction through the family
                // COMMIT. Its positive close is part of acknowledgement.
                let result = managed_destination::valid(database_io::read_cancellable_strict(
                    self.destination.destination_database(),
                    cancel,
                    |destination_c| {
                        action(
                            source_c,
                            destination_c,
                            &inputs,
                            &publication,
                            &binding,
                            owned.metadata_floor(),
                        )
                    },
                )?)?;
                self.image.require_owner(
                    self.root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                private.require_physical(self, cancel)?;
                self.boundary(cancel)?;
                Ok(result)
            })?;
        private.require_physical(self, cancel)?;
        Ok(value)
    }

    /// Reads the complete family protection and private Ready under the same
    /// family snapshot. Its member phase selects a distinct strict reader.
    pub(super) fn read_family_publication(
        &self,
        source_c: &Connection,
        destination_c: &Connection,
        family_c: &Connection,
        inputs: &ManagedReadyInputs<'_>,
        publication: &ImportPublicationInputs<'_>,
        binding: &ExactBinding,
        metadata_floor: u64,
        private: &ManagedPrivateReady<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<ImportPublicationState> {
        let (protected, checkout) = private.physical_parts();
        private.require_physical(self, cancel)?;
        let state = self.held.family.read_import_publication(
            family_c,
            publication,
            binding,
            protected.root(),
            metadata_floor,
            Some(cancel),
        )?;
        let metadata = metadata_floor
            .checked_add(checkout.expected().maximum_encoded_bytes())
            .ok_or_else(|| invalid("family publication metadata envelope overflow"))?;
        let remaining = state
            .protection
            .installation
            .remaining
            .checked_sub(checkout.expected().header().file_bytes)
            .ok_or_else(|| invalid("family publication checkout envelope insufficient"))?;
        protected.root().require_envelope(remaining, metadata)?;
        match state.member {
            ImportPublicationMember::Initializing => destination_ready::read_checkout_ready(
                source_c,
                destination_c,
                family_c,
                inputs,
                checkout.expected(),
                Some(cancel),
            )?,
            ImportPublicationMember::Ready => destination_ready::read_checkout_family_ready(
                source_c,
                destination_c,
                family_c,
                inputs,
                checkout.expected(),
                Some(cancel),
            )?,
        };
        let journal = crate::projection::provider::checkout_journal::read(
            destination_c,
            checkout.expected(),
            Some(cancel),
        )?;
        checkout.require_journal(&journal)?;
        private.require_physical(self, cancel)?;
        Ok(state)
    }

    pub(in crate::projection::provider::bootstrap::preparation::activation) fn publish_family_ready<
        'p,
        'b,
    >(
        &mut self,
        private: &'p ManagedPrivateReady<'b>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedFamilyReady<'p, 'b>> {
        self.rehash(cancel)?;
        self.with_family_publication_source(
            private,
            cancel,
            |source_c, destination_c, inputs, publication, binding, floor| {
                let check = |c: &Connection| {
                    self.read_family_publication(
                        source_c,
                        destination_c,
                        c,
                        inputs,
                        publication,
                        binding,
                        floor,
                        private,
                        cancel,
                    )
                };
                let file = &self.held.family.catalog;
                let before = managed_destination::valid(database_io::read_cancellable_strict(
                    file, cancel, check,
                )?)?;
                if before.member == ImportPublicationMember::Ready {
                    return Ok(());
                }
                database_io::validate_native_filename(file.path())?;
                let admission = database_io::clean_admission(file, false, |c| {
                    if check(c)? != before {
                        return Err(invalid("family publication predecessor changed"));
                    }
                    Ok(())
                })?;
                private.require_physical(self, cancel)?;
                let bare = admission.open(file, close)?;
                let result = (|| {
                    admission.require_geometry(&bare)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                    if check(c)? != before {
                        return Err(invalid("family publication writer predecessor changed"));
                    }
                    private.require_physical(self, cancel)?;
                    let (protected, _) = private.physical_parts();
                    self.held.family.publish_import_ready(
                        c,
                        publication,
                        binding,
                        protected.root(),
                        floor,
                        Some(cancel),
                    )?;
                    let after = check(c)?;
                    if after.member != ImportPublicationMember::Ready
                        || after.protection != before.protection
                    {
                        return Err(invalid("family publication prospective state differs"));
                    }
                    private.require_physical(self, cancel)?;
                    self.boundary(cancel)?;
                    c.execute_batch("COMMIT")?;
                    Ok(())
                })();
                let result = super::source_protection::rollback(bare.connection(), result);
                bare.finish_writer(result, close)
            },
        )?;
        // Family writer and enclosing source read have closed positively.
        self.with_family_publication_source(
            private,
            cancel,
            |source_c, destination_c, inputs, publication, binding, floor| {
                let state = managed_destination::valid(database_io::read_cancellable_strict(
                    &self.held.family.catalog,
                    cancel,
                    |c| {
                        self.read_family_publication(
                            source_c,
                            destination_c,
                            c,
                            inputs,
                            publication,
                            binding,
                            floor,
                            private,
                            cancel,
                        )
                    },
                )?)?;
                if state.member != ImportPublicationMember::Ready {
                    return Err(invalid("family publication durable member regressed"));
                }
                Ok(())
            },
        )?;
        private.require_physical(self, cancel)?;
        Ok(ManagedFamilyReady { private })
    }
}
