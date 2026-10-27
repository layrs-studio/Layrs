//! Private checkout journal commits under the retained source, destination
//! and exact family protection. Filesystem effects occur only after this
//! adapter has positively closed and reread the destination and source.
use super::super::managed_checkout_fs::VerifiedCheckout;
use super::protection::ManagedImportProtection;
use super::*;
use crate::projection::{
    provider::checkout_journal::{self, CheckoutEntry, CheckoutRow, FileState, RowKey},
    shared::exact_protection::ProtectionPresence,
};

/// Retains immutable final-file pins. SQL progress remains DATA and must be
/// reread before Ready; this type itself does not admit an ordinary worktree.
pub(in crate::projection::provider::bootstrap::preparation::activation) struct ManagedCheckoutProof
{
    expected: ExpectedCheckout,
    journal: CheckoutJournal,
    bytes: VerifiedCheckout,
}
impl ManagedCheckoutProof {
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn expected(
        &self,
    ) -> &ExpectedCheckout {
        &self.expected
    }
    /// SQL ownership must still be the exact journal whose physical files
    /// were verified and retained. A canonical but substituted inode is not
    /// accepted merely because the inventory/header commitments match.
    pub(super) fn require_journal(&self, observed: &CheckoutJournal) -> CauseResult<()> {
        if observed.rows() != self.journal.rows() {
            return Err(invalid(
                "checkout SQL ownership differs from retained physical proof",
            ));
        }
        Ok(())
    }
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn require_physical(
        &self,
        lease: &ManagedImportLease<'_, '_, '_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        if lease.expected_checkout(cancel)?.header() != self.expected.header() {
            return Err(invalid("checkout proof belongs to another retained image"));
        }
        self.bytes.require(&self.expected, &self.journal, cancel)?;
        lease.boundary(cancel)
    }
}

enum JournalMutation<'a> {
    Declare,
    Advance {
        previous: &'a CheckoutRow,
        next: &'a CheckoutRow,
    },
}

impl ManagedImportLease<'_, '_, '_, '_> {
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn copy_checkout(
        &mut self,
        protected: &ManagedImportProtection,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedCheckoutProof> {
        self.rehash(cancel)?;
        protected.require(self, cancel)?;
        let expected = self.expected_checkout(cancel)?;
        let storage = self
            .destination
            .checkout_storage(&self.staging.plan, &expected, cancel)?;
        let mut journal = self.checkout_mutation(
            &expected,
            protected,
            JournalMutation::Declare,
            close,
            cancel,
        )?;
        storage.require_namespace(&expected, &journal, cancel)?;
        let (stage_pin, next) = storage.prepare_stage(&expected, &journal, cancel)?;
        journal = self.checkout_mutation(
            &expected,
            protected,
            JournalMutation::Advance {
                previous: journal.row(RowKey::Stage)?,
                next: &next,
            },
            close,
            cancel,
        )?;
        stage_pin.verify_path_identity()?;
        for ordinal in 0..expected.directories().len() {
            protected.require(self, cancel)?;
            let key = RowKey::Directory(ordinal as u32);
            let (pin, next) =
                storage.prepare_directory(&expected, &journal, ordinal as u32, cancel)?;
            journal = self.checkout_mutation(
                &expected,
                protected,
                JournalMutation::Advance {
                    previous: journal.row(key)?,
                    next: &next,
                },
                close,
                cancel,
            )?;
            pin.verify_path_identity()?;
        }
        for ordinal in 0..expected.files().len() {
            let ordinal = ordinal as u32;
            let key = RowKey::File(ordinal);
            loop {
                cancel.check()?;
                protected.require(self, cancel)?;
                let previous = journal.row(key)?.clone();
                let CheckoutEntry::File { file, state, .. } = &previous.entry else {
                    return Err(invalid("checkout inventory file row differs"));
                };
                let next = match state {
                    FileState::Declared => {
                        // Keep the exclusively created inode alive through the
                        // first durable ownership acknowledgement. No bytes are
                        // written until a fresh journal reads that exact inode.
                        let (piece, next) =
                            storage.create_temp(&expected, &journal, ordinal, cancel)?;
                        journal = self.checkout_mutation(
                            &expected,
                            protected,
                            JournalMutation::Advance {
                                previous: &previous,
                                next: &next,
                            },
                            close,
                            cancel,
                        )?;
                        drop(piece);
                        continue;
                    }
                    FileState::Owned { .. } => {
                        let mut piece =
                            storage.open_owned_temp(&expected, &journal, ordinal, cancel)?;
                        let manifest = self
                            .manifests
                            .iter()
                            .find(|m| m.id() == &file.manifest_id)
                            .ok_or_else(|| {
                                invalid("checkout exact manifest absent from protected union")
                            })?
                            .clone();
                        // An interrupted owned TEMP restarts from zero. This
                        // path can never rewrite Verified or final files.
                        let output =
                            storage.begin_write(&expected, &journal, &mut piece, cancel)?;
                        self.image
                            .copy_exact_file(&file.path, &manifest, output, cancel)?;
                        let next = storage.verify_temp(&expected, &journal, &piece, cancel)?;
                        journal = self.checkout_mutation(
                            &expected,
                            protected,
                            JournalMutation::Advance {
                                previous: &previous,
                                next: &next,
                            },
                            close,
                            cancel,
                        )?;
                        drop(piece);
                        continue;
                    }
                    FileState::Verified { .. } => {
                        storage.prepare_publish(&expected, &journal, ordinal, cancel)?
                    }
                    FileState::Publishing { .. } => {
                        storage.publish(&expected, &journal, ordinal, cancel)?
                    }
                    FileState::Published { .. } => break,
                };
                journal = self.checkout_mutation(
                    &expected,
                    protected,
                    JournalMutation::Advance {
                        previous: &previous,
                        next: &next,
                    },
                    close,
                    cancel,
                )?;
            }
        }
        protected.require(self, cancel)?;
        let bytes = storage.verify_complete(&expected, &journal, cancel)?;
        let durable = self
            .read_checkout(&expected, protected, cancel)?
            .ok_or_else(|| invalid("completed checkout journal disappeared"))?;
        if !durable.is_complete() || durable.rows() != journal.rows() {
            return Err(invalid("completed checkout durable progress differs"));
        }
        bytes.require(&expected, &durable, cancel)?;
        let proof = ManagedCheckoutProof {
            expected,
            journal: durable,
            bytes,
        };
        proof.require_physical(self, cancel)?;
        Ok(proof)
    }

    pub(super) fn require_checkout_protection(
        &self,
        expected: &ExpectedCheckout,
        proof: &PrivateActiveCaptureValidation,
        destination: &ProviderTransitionIntentRecord,
        metadata_floor: u64,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let binding = self.exact_binding()?;
        let protection = self.expected_protection(cancel)?;
        let state = self.read_protection(
            proof,
            destination,
            metadata_floor,
            &binding,
            &protection,
            cancel,
        )?;
        if state.root != ProtectionPresence::Present {
            return Err(invalid(
                "checkout requires committed exact family protection",
            ));
        }
        // The original fixed reservation retains the checkout bytes and all
        // future journal states. No reserve growth or release is performed.
        let metadata = metadata_floor
            .checked_add(expected.maximum_encoded_bytes())
            .ok_or_else(|| invalid("checkout metadata envelope overflow"))?;
        let after_checkout = state
            .installation
            .remaining
            .checked_sub(expected.header().file_bytes)
            .ok_or_else(|| invalid("checkout byte reservation is insufficient"))?;
        protection.require_envelope(after_checkout, metadata)?;
        Ok(())
    }

    fn checkout_destination(
        &self,
        c: &Connection,
        proof: &PrivateActiveCaptureValidation,
        destination: &ProviderTransitionIntentRecord,
        closure: &VerifiedImportClosure,
        expected: &ExpectedCheckout,
        cancel: &Cancellation,
    ) -> CauseResult<Option<CheckoutJournal>> {
        crate::projection::provider::import_validation::require_schema(
            c,
            &self.destination.proof().format,
            crate::projection::provider::import_validation::ImportDatabaseKind::Provider,
            Some(cancel),
        )?;
        let prepared = self
            .held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("checkout source prepared receipt absent"))?;
        let present: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_prejournal)",
            [],
            |row| row.get(0),
        )?;
        if present {
            return destination_validation::read_checkout_staging(
                c,
                &self.destination.proof().format,
                &proof.active.source,
                &self.held.artifacts.allocation,
                prepared,
                &proof.active.activation,
                &self.staging.plan.source_prefix,
                destination,
                closure,
                expected,
                Some(cancel),
            )
            .map(Some);
        }
        destination_validation::read_staging(
            c,
            &self.destination.proof().format,
            &proof.active.source,
            &self.held.artifacts.allocation,
            prepared,
            &proof.active.activation,
            &self.staging.plan.source_prefix,
            destination,
            closure,
            Some(cancel),
        )?;
        Ok(None)
    }

    fn read_checkout(
        &self,
        expected: &ExpectedCheckout,
        protected: &ManagedImportProtection,
        cancel: &Cancellation,
    ) -> CauseResult<Option<CheckoutJournal>> {
        protected.require(self, cancel)?;
        let result = self.with_checkout_authority(
            expected,
            cancel,
            |proof, destination, floor, journal, _| {
                self.require_checkout_protection(expected, proof, destination, floor, cancel)?;
                protected.require(self, cancel)?;
                Ok(journal)
            },
        )?;
        protected.require(self, cancel)?;
        Ok(result)
    }

    fn checkout_mutation(
        &self,
        expected: &ExpectedCheckout,
        protected: &ManagedImportProtection,
        mutation: JournalMutation<'_>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<CheckoutJournal> {
        protected.require(self, cancel)?;
        self.with_checkout_authority(
            expected,
            cancel,
            |proof, destination, floor, before, closure| {
                self.require_checkout_protection(expected, proof, destination, floor, cancel)?;
                let already = match &mutation {
                    JournalMutation::Declare => before.is_some(),
                    JournalMutation::Advance { previous, next } => {
                        let journal = before
                            .as_ref()
                            .ok_or_else(|| invalid("checkout journal absent"))?;
                        let row = journal.row(previous.key())?;
                        if row == *next {
                            true
                        } else if row == *previous {
                            false
                        } else {
                            return Err(invalid("checkout predecessor changed before mutation"));
                        }
                    }
                };
                if already {
                    return Ok(());
                }
                let check = |c: &Connection| -> CauseResult<Option<CheckoutJournal>> {
                    let current = self.checkout_destination(
                        c,
                        proof,
                        destination,
                        closure,
                        expected,
                        cancel,
                    )?;
                    match (&before, &current) {
                        (None, None) => (),
                        (Some(a), Some(b)) if a.rows() == b.rows() => (),
                        _ => return Err(invalid("checkout journal changed before writer")),
                    }
                    Ok(current)
                };
                let file = self.destination.destination_database();
                database_io::validate_native_filename(file.path())?;
                let admission = database_io::clean_admission(file, false, |c| {
                    check(c)?;
                    Ok(())
                })?;
                protected.require(self, cancel)?;
                let bare = admission.open(file, close)?;
                let result = (|| -> CauseResult<()> {
                    admission.require_geometry(&bare)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                    check(c)?;
                    self.require_checkout_protection(expected, proof, destination, floor, cancel)?;
                    protected.require(self, cancel)?;
                    match &mutation {
                        JournalMutation::Declare => {
                            checkout_journal::declare(c, expected, Some(cancel))?;
                        }
                        JournalMutation::Advance { previous, next } => {
                            checkout_journal::advance(c, expected, previous, next, Some(cancel))?;
                        }
                    }
                    let after = self
                        .checkout_destination(c, proof, destination, closure, expected, cancel)?
                        .ok_or_else(|| invalid("checkout prospective journal absent"))?;
                    require_result(&mutation, &after)?;
                    protected.require(self, cancel)?;
                    self.boundary(cancel)?;
                    c.execute_batch("COMMIT")?;
                    Ok(())
                })();
                bare.finish_writer(result, close)
            },
        )?;
        // No filesystem operation or success capability escapes a failed
        // destination writer close or enclosing source read close.
        let after = self
            .read_checkout(expected, protected, cancel)?
            .ok_or_else(|| invalid("checkout durable journal absent"))?;
        require_result(&mutation, &after)?;
        Ok(after)
    }
}

fn require_result(mutation: &JournalMutation<'_>, journal: &CheckoutJournal) -> CauseResult<()> {
    if let JournalMutation::Advance { next, .. } = mutation {
        if journal.row(next.key())? != *next {
            return Err(invalid("checkout durable next row differs"));
        }
    }
    Ok(())
}
