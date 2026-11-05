//! Journal-first ordinary checkout population. Every filesystem step follows
//! an acknowledged C writer close and a fresh complete journal readback.
use super::{
    allocation::ManagedDestinationAllocation,
    content::{self, Catalog, ContentPhase},
    protection::OrdinaryImportProtection,
    reservation, *,
};
use crate::{
    lock::StoreLock,
    projection::provider::{
        bootstrap::VerifiedCheckout,
        checkout_journal::{
            self, CheckoutEntry, CheckoutJournal, CheckoutRow, ExpectedCheckout, FileState, RowKey,
        },
        database_io::{self, WriterCloseObserver},
        recovery::error::{
            invalid as cause_invalid, ProviderBootstrapCause as Cause, Result as CauseResult,
        },
    },
    projection::shared::exact_content::ExactCatalog,
};

pub(in crate::projection::provider) struct OrdinaryCheckoutProof {
    expected: ExpectedCheckout,
    journal: CheckoutJournal,
    bytes: VerifiedCheckout,
}
impl OrdinaryCheckoutProof {
    /// The full inventory was checked outside the catalog gate. Recheck its
    /// immutable retained inodes without enumerating the checkout namespace.
    pub(super) fn require_identity(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        if expected.header() != self.expected.header() || journal.rows() != self.journal.rows() {
            return Err(cause_invalid(
                "ordinary checkout proof inventory/journal differs",
            ));
        }
        self.bytes.require_identity(expected, journal, cancel)
    }

    pub(super) fn require_parts(
        &self,
        expected: &ExpectedCheckout,
        journal: &CheckoutJournal,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        if expected.header() != self.expected.header() || journal.rows() != self.journal.rows() {
            return Err(cause_invalid(
                "ordinary checkout proof inventory/journal differs",
            ));
        }
        self.bytes.require(expected, journal, cancel)
    }

    pub(super) fn journal(&self) -> &CheckoutJournal {
        &self.journal
    }

    pub(super) fn require_physical<O: WriterCloseObserver>(
        &self,
        catalog: &Catalog<'_, '_, O>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let expected = content::expected_checkout(catalog.staging, catalog.image, cancel)?;
        if expected.header() != self.expected.header() {
            return Err(cause_invalid("ordinary checkout proof image differs"));
        }
        self.bytes.require(&self.expected, &self.journal, cancel)?;
        catalog.boundary(cancel)
    }
}
enum JournalMutation<'a> {
    Declare,
    Advance {
        previous: &'a CheckoutRow,
        next: &'a CheckoutRow,
    },
}
impl OrdinaryManagedExport<'_> {
    pub(in crate::projection::provider) fn copy_checkout(
        &mut self,
        destination: &ManagedDestinationAllocation,
        staging: &ProviderTransitionIntentRecord,
        protected: &OrdinaryImportProtection,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> Result<OrdinaryCheckoutProof> {
        self.with_content_source(
            destination,
            staging,
            ContentPhase::Checkout,
            close,
            cancel,
            |catalog| catalog.populate_checkout(protected, cancel),
        )
    }
}
impl<O: WriterCloseObserver> Catalog<'_, '_, O> {
    fn populate_checkout(
        &mut self,
        protected: &OrdinaryImportProtection,
        cancel: &Cancellation,
    ) -> CauseResult<OrdinaryCheckoutProof> {
        protected.require(self, cancel)?;
        let expected = content::expected_checkout(self.staging, self.image, cancel)?;
        let storage = self
            .destination
            .checkout_storage(&self.staging.plan, &expected, cancel)?;
        let mut journal =
            self.checkout_mutation(&expected, protected, JournalMutation::Declare, cancel)?;
        storage.require_namespace(&expected, &journal, cancel)?;
        let (stage_pin, next) = storage.prepare_stage(&expected, &journal, cancel)?;
        journal = self.checkout_mutation(
            &expected,
            protected,
            JournalMutation::Advance {
                previous: journal.row(RowKey::Stage)?,
                next: &next,
            },
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
                    return Err(cause_invalid("checkout inventory file row differs"));
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
                            cancel,
                        )?;
                        drop(piece);
                        continue;
                    }
                    FileState::Owned { .. } => {
                        let mut piece =
                            storage.open_owned_temp(&expected, &journal, ordinal, cancel)?;
                        let manifest = self
                            .plan
                            .required
                            .iter()
                            .find(|m| m.id() == &file.manifest_id)
                            .ok_or_else(|| {
                                cause_invalid("checkout exact manifest absent from protected union")
                            })?
                            .clone();
                        // An interrupted owned TEMP restarts from zero. This
                        // path can never rewrite Verified or final files.
                        self.revalidate(cancel)?;
                        let output =
                            storage.begin_write(&expected, &journal, &mut piece, cancel)?;
                        self.image
                            .copy_file(&file.path, &manifest, output, cancel)?;
                        self.revalidate(cancel)?;
                        let next = storage.verify_temp(&expected, &journal, &piece, cancel)?;
                        journal = self.checkout_mutation(
                            &expected,
                            protected,
                            JournalMutation::Advance {
                                previous: &previous,
                                next: &next,
                            },
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
                    cancel,
                )?;
            }
        }
        protected.require(self, cancel)?;
        let bytes = storage.verify_complete(&expected, &journal, cancel)?;
        let durable = self
            .read_checkout(&expected, protected, cancel)?
            .ok_or_else(|| cause_invalid("completed checkout journal disappeared"))?;
        if !durable.is_complete() || durable.rows() != journal.rows() {
            return Err(cause_invalid("completed checkout durable progress differs"));
        }
        bytes.require(&expected, &durable, cancel)?;
        let proof = OrdinaryCheckoutProof {
            expected,
            journal: durable,
            bytes,
        };
        proof.require_physical(self, cancel)?;
        Ok(proof)
    }

    fn read_checkout_connection(
        &self,
        c: &Connection,
        expected: &ExpectedCheckout,
        cancel: &Cancellation,
    ) -> CauseResult<Option<CheckoutJournal>> {
        content::read_checkout_destination(
            c,
            self.destination,
            self.source,
            &self.store.marker,
            self.staging,
            self.closure,
            expected,
            cancel,
        )
    }

    pub(super) fn read_checkout(
        &mut self,
        expected: &ExpectedCheckout,
        protected: &OrdinaryImportProtection,
        cancel: &Cancellation,
    ) -> CauseResult<Option<CheckoutJournal>> {
        self.revalidate(cancel)?;
        protected.require(self, cancel)?;
        let family = self.destination.family();
        let gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )
        .map_err(Cause::from)?;
        let result: CauseResult<Option<CheckoutJournal>> = (|| {
            reservation::valid(database_io::read_cancellable_strict(
                &family.catalog,
                cancel,
                |c| {
                    self.validate_catalog(c, cancel)?;
                    protected.verify_identity(cancel)
                },
            )?)?;
            let journal = reservation::valid(database_io::read_cancellable_strict(
                self.destination.destination_database(),
                cancel,
                |c| self.read_checkout_connection(c, expected, cancel),
            )?)?;
            protected.verify_identity(cancel)?;
            Ok(journal)
        })();
        drop(gate);
        let journal = result?;
        self.revalidate(cancel)?;
        protected.require(self, cancel)?;
        Ok(journal)
    }

    fn checkout_mutation(
        &mut self,
        expected: &ExpectedCheckout,
        protected: &OrdinaryImportProtection,
        mutation: JournalMutation<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<CheckoutJournal> {
        self.revalidate(cancel)?;
        protected.require(self, cancel)?;
        let family = self.destination.family();
        let gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )
        .map_err(Cause::from)?;
        let result = (|| {
            reservation::valid(database_io::read_cancellable_strict(
                &family.catalog,
                cancel,
                |c| {
                    self.validate_catalog(c, cancel)?;
                    protected.verify_identity(cancel)
                },
            )?)?;
            let file = self.destination.destination_database();
            let before =
                reservation::valid(database_io::read_cancellable_strict(file, cancel, |c| {
                    self.read_checkout_connection(c, expected, cancel)
                })?)?;
            let already = match &mutation {
                JournalMutation::Declare => before.is_some(),
                JournalMutation::Advance { previous, next } => {
                    let journal = before
                        .as_ref()
                        .ok_or_else(|| cause_invalid("ordinary checkout journal absent"))?;
                    let row = journal.row(previous.key())?;
                    if row == *next {
                        true
                    } else if row == *previous {
                        false
                    } else {
                        return Err(cause_invalid("ordinary checkout predecessor differs"));
                    }
                }
            };
            if !already {
                // Capture independent references so the writer observer remains
                // exclusively borrowed only for the actual open/close calls.
                let destination = self.destination;
                let source = self.source;
                let marker = &self.store.marker;
                let staging = self.staging;
                let closure = self.closure;
                let check = |c: &Connection| {
                    let current = content::read_checkout_destination(
                        c,
                        destination,
                        source,
                        marker,
                        staging,
                        closure,
                        expected,
                        cancel,
                    )?;
                    match (&before, &current) {
                        (None, None) => Ok(()),
                        (Some(a), Some(b)) if a.rows() == b.rows() => Ok(()),
                        _ => Err(cause_invalid(
                            "ordinary checkout full journal changed before writer",
                        )),
                    }
                };
                let admission = database_io::clean_admission(file, false, check)?;
                let bare = admission.open(file, self.close)?;
                let written = (|| {
                    admission.require_geometry(&bare)?;
                    protected.verify_identity(cancel)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                    check(c)?;
                    match &mutation {
                        JournalMutation::Declare => {
                            checkout_journal::declare(c, expected, Some(cancel))?;
                        }
                        JournalMutation::Advance { previous, next } => {
                            checkout_journal::advance(c, expected, previous, next, Some(cancel))?;
                        }
                    }
                    let after = self
                        .read_checkout_connection(c, expected, cancel)?
                        .ok_or_else(|| {
                            cause_invalid("ordinary checkout prospective journal absent")
                        })?;
                    require_result(&mutation, &after)?;
                    reservation::valid(database_io::read_cancellable_strict(
                        &family.catalog,
                        cancel,
                        |catalog| {
                            self.validate_catalog(catalog, cancel)?;
                            protected.verify_identity(cancel)
                        },
                    )?)?;
                    self.boundary(cancel)?;
                    c.execute_batch("COMMIT")?;
                    Ok(())
                })();
                let written = if bare.connection().is_autocommit() {
                    written
                } else {
                    reservation::combine(
                        written,
                        bare.connection()
                            .execute_batch("ROLLBACK")
                            .map_err(Cause::from),
                    )
                };
                bare.finish_writer(written, self.close)?;
            }
            // No SQL is reached on an unsuccessful write/rollback/close.
            let after =
                reservation::valid(database_io::read_cancellable_strict(file, cancel, |c| {
                    self.read_checkout_connection(c, expected, cancel)
                })?)?
                .ok_or_else(|| cause_invalid("ordinary checkout durable journal absent"))?;
            require_result(&mutation, &after)?;
            reservation::valid(database_io::read_cancellable_strict(
                &family.catalog,
                cancel,
                |c| {
                    self.validate_catalog(c, cancel)?;
                    protected.verify_identity(cancel)
                },
            )?)?;
            Ok(after)
        })();
        drop(gate);
        let after = result?;
        self.revalidate(cancel)?;
        protected.require(self, cancel)?;
        Ok(after)
    }
}
fn require_result(mutation: &JournalMutation<'_>, journal: &CheckoutJournal) -> CauseResult<()> {
    if let JournalMutation::Advance { next, .. } = mutation {
        if journal.row(next.key())? != *next {
            return Err(cause_invalid("ordinary checkout durable next row differs"));
        }
    }
    Ok(())
}
