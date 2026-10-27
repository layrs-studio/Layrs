//! Private Ready publication with retained checkout and exact CAS proofs.
//! The family member remains Initializing. No ordinary admission or reservation
//! release follows; every acknowledged result follows all enclosing closes.
use super::*;
use super::{
    checkout::ManagedCheckoutProof, protection::ManagedImportProtection, ready_data::OwnedReadyData,
};
use crate::projection::{
    provider::destination_ready::{self, ManagedReadyInputs, ReadyEffect},
    shared::exact_protection::ProtectionPresence,
};

pub(in crate::projection::provider::bootstrap::preparation::activation) struct ManagedPrivateReady<
    'p,
> {
    data: OwnedReadyData,
    protected: &'p ManagedImportProtection,
    checkout: &'p ManagedCheckoutProof,
}

impl ManagedPrivateReady<'_> {
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn ready_intent(
        &self,
    ) -> &ProviderTransitionIntentRecord {
        self.data.ready()
    }
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn receipt(
        &self,
    ) -> &crate::projection::provider::records::import::ProviderTransitionReceiptRecord {
        self.data.receipt()
    }
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn require_physical(
        &self,
        lease: &ManagedImportLease<'_, '_, '_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        self.protected.require(lease, cancel)?;
        self.checkout.require_physical(lease, cancel)
    }
}

impl ManagedImportLease<'_, '_, '_, '_> {
    /// Unlike content/copy authority, this distinct boundary interprets both
    /// destination Staging and its exact Ready suffix, never skips validation.
    fn with_ready_context<T>(
        &self,
        protected: &ManagedImportProtection,
        checkout: &ManagedCheckoutProof,
        cancel: &Cancellation,
        action: impl FnOnce(&Connection, &Connection, &ManagedReadyInputs<'_>) -> CauseResult<T>,
    ) -> CauseResult<(T, OwnedReadyData)> {
        self.require_intent()?;
        protected.require(self, cancel)?;
        checkout.require_physical(self, cancel)?;
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.boundary(cancel)?;
        let result = managed_destination::with_source(
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
                let owned = OwnedReadyData::new(
                    derived,
                    self.staging,
                    self.image,
                    self.manifests,
                    protected.root(),
                    cancel,
                )?;
                let data = owned.as_data(self.manifests)?;
                let prepared = self
                    .held
                    .artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("Ready source prepared receipt absent"))?;
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
                };
                let content = self.inputs(proof, owned.destination_staging())?;
                let binding = self.exact_binding()?;
                let value = managed_destination::valid(database_io::read_cancellable(
                    &self.held.family.catalog,
                    cancel,
                    |family_c| {
                        let before = self.held.family.validate_import_protection(
                            family_c,
                            &content,
                            &binding,
                            protected.root(),
                            owned.metadata_floor(),
                            Some(cancel),
                        )?;
                        if before.root != ProtectionPresence::Present {
                            return Err(invalid(
                                "Ready requires exact published family protection",
                            ));
                        }
                        let metadata = owned
                            .metadata_floor()
                            .checked_add(checkout.expected().maximum_encoded_bytes())
                            .ok_or_else(|| invalid("Ready retained metadata envelope overflow"))?;
                        let remaining = before
                            .installation
                            .remaining
                            .checked_sub(checkout.expected().header().file_bytes)
                            .ok_or_else(|| {
                                invalid("Ready retained checkout envelope insufficient")
                            })?;
                        protected.root().require_envelope(remaining, metadata)?;
                        protected.require(self, cancel)?;
                        checkout.require_physical(self, cancel)?;
                        let value = action(source_c, family_c, &inputs)?;
                        let after = self.held.family.validate_import_protection(
                            family_c,
                            &content,
                            &binding,
                            protected.root(),
                            owned.metadata_floor(),
                            Some(cancel),
                        )?;
                        if after != before {
                            return Err(invalid("Ready changed family content accounting"));
                        }
                        protected.require(self, cancel)?;
                        checkout.require_physical(self, cancel)?;
                        Ok(value)
                    },
                )?)?;
                self.image.require_owner(
                    self.root,
                    &proof.active.source,
                    &proof.active.activated.snapshot,
                )?;
                self.boundary(cancel)?;
                Ok((value, owned))
            },
        )?;
        protected.require(self, cancel)?;
        checkout.require_physical(self, cancel)?;
        Ok(result)
    }

    pub(in crate::projection::provider::bootstrap::preparation::activation) fn finalize_private_ready<
        'p,
    >(
        &mut self,
        protected: &'p ManagedImportProtection,
        checkout: &'p ManagedCheckoutProof,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedPrivateReady<'p>> {
        self.rehash(cancel)?;
        let (_, published) =
            self.with_ready_context(protected, checkout, cancel, |source_c, family_c, inputs| {
                let check = |c: &Connection| {
                    ready_or_staging(source_c, c, family_c, inputs, checkout, cancel)
                };
                let file = self.destination.destination_database();
                let before = managed_destination::valid(database_io::read_cancellable(
                    file, cancel, check,
                )?)?;
                if before {
                    return Ok(ReadyEffect::AlreadyReady);
                }
                database_io::validate_native_filename(file.path())?;
                let admission = database_io::clean_admission(file, false, |c| {
                    if check(c)? {
                        return Err(invalid("Ready predecessor changed before writer"));
                    }
                    Ok(())
                })?;
                protected.require(self, cancel)?;
                checkout.require_physical(self, cancel)?;
                let bare = admission.open(file, close)?;
                let result = (|| {
                    admission.require_geometry(&bare)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                    if check(c)? {
                        return Err(invalid("Ready writer predecessor changed"));
                    }
                    protected.require(self, cancel)?;
                    checkout.require_physical(self, cancel)?;
                    let effect = destination_ready::finalize_checkout_or_replay(
                        source_c,
                        c,
                        family_c,
                        inputs,
                        checkout.expected(),
                        Some(cancel),
                    )?;
                    if !check(c)? {
                        return Err(invalid("Ready prospective readback differs"));
                    }
                    protected.require(self, cancel)?;
                    checkout.require_physical(self, cancel)?;
                    self.boundary(cancel)?;
                    c.execute_batch("COMMIT")?;
                    Ok(effect)
                })();
                bare.finish_writer(result, close)
            })?;
        // Destination writer, family read and source read have all closed.
        let (_, confirmed) =
            self.with_ready_context(protected, checkout, cancel, |source_c, family_c, inputs| {
                managed_destination::valid(database_io::read_cancellable(
                    self.destination.destination_database(),
                    cancel,
                    |c| {
                        if !ready_or_staging(source_c, c, family_c, inputs, checkout, cancel)? {
                            return Err(invalid("Ready durable private state regressed"));
                        }
                        Ok(())
                    },
                )?)?;
                Ok(())
            })?;
        if confirmed.ready() != published.ready() || confirmed.receipt() != published.receipt() {
            return Err(invalid("Ready durable receipt changed"));
        }
        protected.require(self, cancel)?;
        checkout.require_physical(self, cancel)?;
        Ok(ManagedPrivateReady {
            data: confirmed,
            protected,
            checkout,
        })
    }
}

fn ready_or_staging(
    source_c: &Connection,
    destination: &Connection,
    family_c: &Connection,
    inputs: &ManagedReadyInputs<'_>,
    checkout: &ManagedCheckoutProof,
    cancel: &Cancellation,
) -> CauseResult<bool> {
    let expected = checkout.expected();
    crate::projection::provider::import_validation::require_schema(
        destination,
        inputs.marker,
        crate::projection::provider::import_validation::ImportDatabaseKind::Provider,
        Some(cancel),
    )?;
    let ready: bool = destination.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE state='ready')",
        [],
        |r| r.get(0),
    )?;
    if ready {
        destination_ready::read_checkout_ready(
            source_c,
            destination,
            family_c,
            inputs,
            expected,
            Some(cancel),
        )?;
        let journal = crate::projection::provider::checkout_journal::read(
            destination,
            expected,
            Some(cancel),
        )?;
        checkout.require_journal(&journal)?;
    } else {
        let journal = destination_validation::read_checkout_staging(
            destination,
            inputs.marker,
            inputs.source,
            inputs.allocation,
            inputs.prepared,
            inputs.activation,
            inputs.prefix,
            inputs.staging,
            inputs.closure,
            expected,
            Some(cancel),
        )?;
        if !journal.is_complete() {
            return Err(invalid("Ready requires complete physical checkout journal"));
        }
        checkout.require_journal(&journal)?;
    }
    Ok(ready)
}
