//! Fresh actual B Staging + C Staging checkout population only. Every
//! filesystem effect follows a positively closed journal acknowledgement.
use super::{content, ready_data::OwnedOrdinaryReadyData, reservation, reserve_plan, *};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            admitted_ordinary,
            checkout_journal::{
                self, CheckoutEntry, CheckoutJournal, CheckoutRow, ExpectedCheckout, FileState,
                RowKey,
            },
            database_io::{self, WriterCloseObserver},
            destination_ready::{self, OrdinaryReadyInputs},
            import_validation::{ImportDatabaseKind, verify_source},
            managed_ready_reopen::{
                AdmissionStep, OrdinaryCheckoutParts, OrdinaryCheckoutResources,
                OrdinaryExclusiveSource,
            },
            ordinary_transition,
            records::{
                Record,
                import::{
                    AdmittedSourceBinding, ImportSourceReferences, ProviderTransitionReceiptRecord,
                },
            },
            recovery::error::{
                ProviderBootstrapCause as Cause, Result as CauseResult, invalid as rejected,
            },
        },
        shared::{
            self,
            admitted_catalog::{
                AdmittedFamilyContext, ExportProtectionState, ExportReserveOnlyTarget,
            },
            exact_content::{ExactBinding, PublishedCasProof},
            exact_protection::ProtectionPresence,
        },
    },
};
use layrs_core::projection::{ProjectionSelection, ProjectionSnapshot};

use crate::projection::provider::bootstrap::{
    CheckoutStorage, OwnedCheckoutFile, VerifiedCheckout,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::projection::provider) enum CheckoutPopulationEffect {
    Updated,
    AlreadyComplete,
}

/// Detached DATA only. This grants no Ready, publication or admission right.
pub(in crate::projection::provider) struct OrdinaryCheckoutPopulationProgress {
    expected: ExpectedCheckout,
    journal: CheckoutJournal,
    effect: CheckoutPopulationEffect,
}
impl OrdinaryCheckoutPopulationProgress {
    pub(in crate::projection::provider) fn expected(&self) -> &ExpectedCheckout {
        &self.expected
    }
    pub(in crate::projection::provider) fn journal(&self) -> &CheckoutJournal {
        &self.journal
    }
    pub(in crate::projection::provider) fn effect(&self) -> CheckoutPopulationEffect {
        self.effect
    }
}
pub(in crate::projection::provider) struct OrdinaryCheckoutPopulationFailure {
    pub step: AdmissionStep,
    pub cause: Cause,
    pub held: OrdinaryCheckoutPopulation,
}
struct SourceObservation {
    staging: ProviderTransitionIntentRecord,
    snapshot: ProjectionSnapshot,
    selection: ProjectionSelection,
}
struct CheckoutData {
    calculated_ready: ProviderTransitionIntentRecord,
    calculated_receipt: ProviderTransitionReceiptRecord,
    expected: ExpectedCheckout,
    journal: Option<Box<CheckoutJournal>>,
    required: Vec<FileManifest>,
    binding: ExactBinding,
    family: ExportProtectionState,
}
impl CheckoutData {
    fn parts(&self) -> OrdinaryCheckoutParts<'_> {
        OrdinaryCheckoutParts {
            calculated_ready: &self.calculated_ready,
            expected: &self.expected,
            required: &self.required,
            binding: &self.binding,
        }
    }
    fn require_same_image(&self, current: &Self) -> CauseResult<()> {
        if self.calculated_ready != current.calculated_ready
            || self.calculated_receipt != current.calculated_receipt
            || self.expected.marker() != current.expected.marker()
            || self.expected.header() != current.expected.header()
            || self.expected.operation() != current.expected.operation()
            || self.expected.files() != current.expected.files()
            || self.expected.directories() != current.expected.directories()
            || self.expected.deletions() != current.expected.deletions()
            || self.expected.declared_encoded_bytes() != current.expected.declared_encoded_bytes()
            || self.expected.maximum_encoded_bytes() != current.expected.maximum_encoded_bytes()
            || self.required != current.required
            || self.binding != current.binding
            || self.family != current.family
        {
            return Err(rejected(
                "fresh checkout immutable image/funding/protection changed",
            ));
        }
        Ok(())
    }
    fn require_same_progress(&self, current: &Self) -> CauseResult<()> {
        self.require_same_image(current)?;
        match (&self.journal, &current.journal) {
            (None, None) => Ok(()),
            (Some(a), Some(b)) if a.rows() == b.rows() => Ok(()),
            _ => Err(rejected("fresh checkout full durable journal changed")),
        }
    }
    fn journal(&self) -> CauseResult<&CheckoutJournal> {
        self.journal
            .as_deref()
            .ok_or_else(|| rejected("fresh checkout journal absent"))
    }
}

/// Pins newly created before Owned ACK must outlive a failed writer close.
/// No operation may adopt a Declared object on a later fresh open.
enum PendingPin {
    Directory(PinnedDirectory),
    File(OwnedCheckoutFile),
}
enum Mutation {
    Declare,
    Advance {
        previous: CheckoutRow,
        next: CheckoutRow,
    },
}
impl Mutation {
    fn require_result(&self, journal: &CheckoutJournal) -> CauseResult<()> {
        if let Self::Advance { next, .. } = self {
            if journal.row(next.key())? != next {
                return Err(rejected("fresh checkout durable next row differs"));
            }
        }
        Ok(())
    }
}

pub(in crate::projection::provider) struct OrdinaryCheckoutPopulation {
    image: OrdinaryExportImage,
    storage: CheckoutStorage,
    cas: PublishedCasProof,
    complete: Option<VerifiedCheckout>,
    pending: Option<PendingPin>,
    destination: OrdinaryCheckoutResources,
    source: OrdinaryExclusiveSource,
    observation: Box<SourceObservation>,
    data: Box<CheckoutData>,
}
impl OrdinaryCheckoutPopulation {
    pub(in crate::projection::provider) fn open(
        source_root: &Path,
        destination_root: &Path,
        transaction_id: &str,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        let source = OrdinaryExclusiveSource::open(source_root, cancel)?;
        let mut observation = None;
        database_io::clean_admission(source.database(), false, |c| {
            observation = Some(read_observation(&source, c, transaction_id, cancel)?);
            Ok(())
        })?;
        let observation =
            observation.ok_or_else(|| rejected("fresh checkout source observation absent"))?;
        let destination = source.inspect_checkout_destination(destination_root, cancel)?;
        let image = OrdinaryExportImage::build_parts(
            source.root(),
            source.marker(),
            &observation.snapshot,
            &observation.selection,
            cancel,
        )
        .map_err(Cause::from)?;
        require_image(&observation, &image)?;
        let data = Reconstruction {
            source: &source,
            destination: &destination,
            observation: &observation,
            image: &image,
            expected: None,
            cas: None,
        }
        .read_validated(cancel)?;
        // All SQL and catalog scopes have positively closed before hashes or
        // enumeration. C CAS pins are distinct from B's retained image proof.
        let cas = destination.pin_cas(&data.parts(), cancel)?;
        let storage = destination.checkout_storage(&data.parts(), cancel)?;
        let mut held = Self {
            image,
            storage,
            cas,
            complete: None,
            pending: None,
            destination,
            source,
            observation,
            data,
        };
        held.require_physical(cancel)?;
        if held
            .data
            .journal
            .as_deref()
            .is_some_and(CheckoutJournal::is_complete)
        {
            held.complete = Some(
                held.destination
                    .checkout_storage(&held.data.parts(), cancel)?
                    .verify_complete(&held.data.expected, held.data.journal()?, cancel)?,
            );
        }
        let current = held.reconstruction().read_validated(cancel)?;
        held.data.require_same_progress(&current)?;
        held.require_physical(cancel)?;
        Ok(held)
    }
    pub(in crate::projection::provider) fn populate_checkout(
        mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<OrdinaryCheckoutPopulationProgress, OrdinaryCheckoutPopulationFailure>
    {
        let mut step = AdmissionStep::Validating;
        let result = self.populate_inner(close, cancel, &mut step);
        result.map_err(|cause| OrdinaryCheckoutPopulationFailure {
            step,
            cause,
            held: self,
        })
    }
    fn reconstruction(&self) -> Reconstruction<'_> {
        Reconstruction {
            source: &self.source,
            destination: &self.destination,
            observation: &self.observation,
            image: &self.image,
            expected: Some(&self.data),
            cas: Some(&self.cas),
        }
    }
    fn require_image_and_cas(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        self.source.require_identity(cancel)?;
        self.image
            .revalidate_parts(
                self.source.root(),
                self.source.marker(),
                &self.observation.snapshot,
                &self.observation.selection,
                cancel,
            )
            .map_err(Cause::from)?;
        require_image(&self.observation, &self.image)?;
        self.destination.require_parts(&self.data.parts(), cancel)?;
        self.cas
            .require(&self.data.binding, &self.data.required, cancel)
    }
    fn require_physical(&mut self, cancel: &Cancellation) -> CauseResult<()> {
        self.require_image_and_cas(cancel)?;
        match self.data.journal.as_deref() {
            None => self
                .storage
                .require_unstarted_namespace(&self.data.expected, cancel)?,
            Some(journal) => {
                self.storage
                    .require_namespace(&self.data.expected, journal, cancel)?
            }
        }
        if let Some(complete) = &self.complete {
            complete.require(&self.data.expected, self.data.journal()?, cancel)?;
        }
        Ok(())
    }
    fn advance(
        &mut self,
        previous: CheckoutRow,
        next: CheckoutRow,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> CauseResult<()> {
        self.mutate(Mutation::Advance { previous, next }, close, cancel, step)
    }
    fn populate_inner(
        &mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> CauseResult<OrdinaryCheckoutPopulationProgress> {
        self.require_physical(cancel)?;
        let current = self.reconstruction().read_validated(cancel)?;
        self.data.require_same_progress(&current)?;
        let already = self
            .data
            .journal
            .as_deref()
            .is_some_and(CheckoutJournal::is_complete);
        if !already {
            if self.data.journal.is_none() {
                self.mutate(Mutation::Declare, close, cancel, step)?;
            }
            let (pin, next) =
                self.storage
                    .prepare_stage(&self.data.expected, self.data.journal()?, cancel)?;
            let previous = self.data.journal()?.row(RowKey::Stage)?.clone();
            self.pending = Some(PendingPin::Directory(pin));
            self.advance(previous, next, close, cancel, step)?;
            self.release_directory()?;
            for ordinal in 0..self.data.expected.directories().len() {
                let (pin, next) = self.storage.prepare_directory(
                    &self.data.expected,
                    self.data.journal()?,
                    ordinal as u32,
                    cancel,
                )?;
                let previous = self
                    .data
                    .journal()?
                    .row(RowKey::Directory(ordinal as u32))?
                    .clone();
                self.pending = Some(PendingPin::Directory(pin));
                self.advance(previous, next, close, cancel, step)?;
                self.release_directory()?;
            }
            for ordinal in 0..self.data.expected.files().len() {
                let ordinal = ordinal as u32;
                loop {
                    self.require_physical(cancel)?;
                    let previous = self.data.journal()?.row(RowKey::File(ordinal))?.clone();
                    let CheckoutEntry::File { file, state, .. } = &previous.entry else {
                        return Err(rejected("fresh checkout file row differs"));
                    };
                    let next = match state {
                        FileState::Declared => {
                            let (piece, next) = self.storage.create_temp(
                                &self.data.expected,
                                self.data.journal()?,
                                ordinal,
                                cancel,
                            )?;
                            self.pending = Some(PendingPin::File(piece));
                            self.advance(previous, next, close, cancel, step)?;
                            self.pending = None;
                            continue;
                        }
                        FileState::Owned { .. } => {
                            let piece = self.storage.open_owned_temp(
                                &self.data.expected,
                                self.data.journal()?,
                                ordinal,
                                cancel,
                            )?;
                            self.pending = Some(PendingPin::File(piece));
                            let manifest = self
                                .data
                                .required
                                .iter()
                                .find(|m| m.id() == &file.manifest_id)
                                .ok_or_else(|| {
                                    rejected("fresh checkout exact ManifestId absent from union")
                                })?
                                .clone();
                            self.require_image_and_cas(cancel)?;
                            let Some(PendingPin::File(piece)) = self.pending.as_mut() else {
                                unreachable!()
                            };
                            let output = self.storage.begin_write(
                                &self.data.expected,
                                self.data.journal()?,
                                piece,
                                cancel,
                            )?;
                            #[cfg(test)]
                            let mut observed_output =
                                super::checkout_copy_probe::Output::new(output);
                            #[cfg(test)]
                            let output = &mut observed_output;
                            self.image
                                .copy_file(&file.path, &manifest, output, cancel)
                                .map_err(Cause::from)?;
                            self.require_image_and_cas(cancel)?;
                            let Some(PendingPin::File(piece)) = self.pending.as_ref() else {
                                unreachable!()
                            };
                            let next = self.storage.verify_temp(
                                &self.data.expected,
                                self.data.journal()?,
                                piece,
                                cancel,
                            )?;
                            self.advance(previous, next, close, cancel, step)?;
                            self.pending = None;
                            continue;
                        }
                        FileState::Verified { .. } => self.storage.prepare_publish(
                            &self.data.expected,
                            self.data.journal()?,
                            ordinal,
                            cancel,
                        )?,
                        FileState::Publishing { .. } => self.storage.publish(
                            &self.data.expected,
                            self.data.journal()?,
                            ordinal,
                            cancel,
                        )?,
                        FileState::Published { .. } => break,
                    };
                    self.advance(previous, next, close, cancel, step)?;
                }
            }
        }
        self.require_physical(cancel)?;
        // A complete journal is verified physically even on no-writer replay.
        self.complete = Some(
            self.destination
                .checkout_storage(&self.data.parts(), cancel)?
                .verify_complete(&self.data.expected, self.data.journal()?, cancel)?,
        );
        let current = self.reconstruction().read_validated(cancel)?;
        self.data.require_same_progress(&current)?;
        if !current.journal()?.is_complete() {
            return Err(rejected("fresh checkout durable journal incomplete"));
        }
        self.require_physical(cancel)?;
        *step = AdmissionStep::FreshVerified;
        Ok(OrdinaryCheckoutPopulationProgress {
            expected: current.expected,
            journal: *current
                .journal
                .ok_or_else(|| rejected("fresh checkout completed journal absent"))?,
            effect: if already {
                CheckoutPopulationEffect::AlreadyComplete
            } else {
                CheckoutPopulationEffect::Updated
            },
        })
    }
    fn release_directory(&mut self) -> CauseResult<()> {
        if let Some(PendingPin::Directory(pin)) = &self.pending {
            pin.verify_path_identity()?;
        }
        self.pending = None;
        Ok(())
    }
    fn mutate(
        &mut self,
        mutation: Mutation,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> CauseResult<()> {
        // A newly created candidate is not yet in SQL. Its pin remains held,
        // but no namespace proof may invent durable ownership for it.
        self.require_image_and_cas(cancel)?;
        let reconstruction = self.reconstruction();
        let family = self.destination.family();
        let after = {
            let _catalog = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"),
                SessionMode::Shared,
            )?;
            reservation::valid(database_io::read_cancellable_strict(
                self.source.database(),
                cancel,
                |source| {
                    reservation::valid(database_io::read_cancellable_strict(
                        &family.catalog,
                        cancel,
                        |catalog| {
                            let before = reservation::valid(database_io::read_cancellable_strict(
                                self.destination.database(),
                                cancel,
                                |destination| {
                                    reconstruction.validate(source, destination, catalog, cancel)
                                },
                            )?)?;
                            self.data.require_same_progress(&before)?;
                            let already = match &mutation {
                                Mutation::Declare => before.journal.is_some(),
                                Mutation::Advance { previous, next } => {
                                    let row = before.journal()?.row(previous.key())?;
                                    if row == next {
                                        true
                                    } else if row == previous {
                                        false
                                    } else {
                                        return Err(rejected("fresh checkout predecessor differs"));
                                    }
                                }
                            };
                            if already {
                                mutation.require_result(before.journal()?)?;
                                return Ok(before);
                            }
                            let check = |destination: &Connection| {
                                let current = reconstruction.validate(
                                    source,
                                    destination,
                                    catalog,
                                    cancel,
                                )?;
                                before.require_same_progress(&current)
                            };
                            let admission = database_io::clean_admission(
                                self.destination.database(),
                                false,
                                check,
                            )?;
                            reconstruction.require_identity(cancel)?;
                            *step = AdmissionStep::WriterOpenAttempted;
                            let bare = admission.open(self.destination.database(), close)?;
                            let written = (|| {
                                admission.require_geometry(&bare)?;
                                let destination = bare.connection();
                                destination.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA synchronous=FULL; BEGIN IMMEDIATE")?;
                                check(destination)?;
                                *step = AdmissionStep::MutationAttempted;
                                match &mutation {
                                    Mutation::Declare => {
                                        checkout_journal::declare(
                                            destination,
                                            &before.expected,
                                            Some(cancel),
                                        )?;
                                    }
                                    Mutation::Advance { previous, next } => {
                                        checkout_journal::advance(
                                            destination,
                                            &before.expected,
                                            previous,
                                            next,
                                            Some(cancel),
                                        )?;
                                    }
                                }
                                let after = reconstruction.validate(
                                    source,
                                    destination,
                                    catalog,
                                    cancel,
                                )?;
                                before.require_same_image(&after)?;
                                mutation.require_result(after.journal()?)?;
                                reconstruction.require_identity(cancel)?;
                                cancel.check()?;
                                *step = AdmissionStep::CommitAttempted;
                                destination.execute_batch("COMMIT")?;
                                *step = AdmissionStep::CommitReturned;
                                reconstruction.require_identity(cancel)?;
                                Ok(after)
                            })();
                            let written = rollback_failed_action(bare.connection(), written);
                            let after = bare.finish_writer(written, close)?;
                            *step = AdmissionStep::WriterClosed;
                            Ok(after)
                        },
                    )?)
                },
            )?)?
        };
        // Mandatory nested reader closes complete before physical checks or
        // the independent full durable read. Errors above stop immediately.
        *step = AdmissionStep::ReadScopesClosed;
        self.require_image_and_cas(cancel)?;
        let current = self.reconstruction().read_validated(cancel)?;
        after.require_same_progress(&current)?;
        mutation.require_result(current.journal()?)?;
        self.data = current;
        self.require_physical(cancel)
    }
}
struct Reconstruction<'a> {
    source: &'a OrdinaryExclusiveSource,
    destination: &'a OrdinaryCheckoutResources,
    observation: &'a SourceObservation,
    image: &'a OrdinaryExportImage,
    expected: Option<&'a CheckoutData>,
    cas: Option<&'a PublishedCasProof>,
}
impl Reconstruction<'_> {
    fn require_identity(&self, cancel: &Cancellation) -> CauseResult<()> {
        self.source.require_identity(cancel)?;
        self.image.verify_identity(cancel).map_err(Cause::from)?;
        self.destination.require_identity(cancel)?;
        if let Some(data) = self.expected {
            self.destination.require_parts(&data.parts(), cancel)?;
        }
        if let Some(cas) = self.cas {
            cas.verify_identity(cancel)?;
        }
        Ok(())
    }
    fn read_validated(&self, cancel: &Cancellation) -> CauseResult<Box<CheckoutData>> {
        self.require_identity(cancel)?;
        let family = self.destination.family();
        let _catalog = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        reservation::valid(database_io::read_cancellable_strict(
            self.source.database(),
            cancel,
            |source| {
                reservation::valid(database_io::read_cancellable_strict(
                    &family.catalog,
                    cancel,
                    |catalog| {
                        reservation::valid(database_io::read_cancellable_strict(
                            self.destination.database(),
                            cancel,
                            |destination| self.validate(source, destination, catalog, cancel),
                        )?)
                    },
                )?)
            },
        )?)
    }
    fn require_observation(&self, source: &Connection, cancel: &Cancellation) -> CauseResult<()> {
        self.source.require_snapshot(source, cancel)?;
        let actual = ordinary_transition::read_publication_state(
            source,
            self.source.marker(),
            &self.observation.staging.transaction_id,
            Some(cancel),
        )?;
        if actual.intent != self.observation.staging || actual.receipt.is_some() {
            return Err(rejected(
                "fresh checkout requires actual unchanged B Staging without receipt",
            ));
        }
        let current = database::current(source).map_err(Cause::from)?;
        let selected = selection::for_scan(source, &current.snapshot).map_err(Cause::from)?;
        let version = self.observation.staging.plan.source_prefix.source.version();
        if current.generation != version.generation
            || current.digest != version.snapshot_digest
            || current.snapshot != self.observation.snapshot
            || selected != self.observation.selection
        {
            return Err(rejected(
                "fresh checkout source frontier/snapshot/selection changed",
            ));
        }
        require_image(self.observation, self.image)
    }

    fn validate(
        &self,
        source: &Connection,
        destination: &Connection,
        family: &Connection,
        cancel: &Cancellation,
    ) -> CauseResult<Box<CheckoutData>> {
        self.require_observation(source, cancel)?;
        let staging = &self.observation.staging;
        let prefix = &staging.plan.source_prefix;
        let admitted = admitted_ordinary::inspect(source, self.source.marker(), Some(cancel))?;
        let source_binding = AdmittedSourceBinding {
            admission_digest: admitted.admission_digest().to_owned(),
            incoming_prefix_id: admitted.prefix_id().to_owned(),
            incoming_receipt_id: admitted.receipt().digest()?,
            database_identity: admitted.binding().database_identity.clone(),
        };
        let closure = verify_source(
            source,
            self.source.marker(),
            ImportDatabaseKind::AdmittedOrdinary,
            prefix,
            ImportSourceReferences::AdmittedOrdinary {
                marker: self.source.marker(),
                admission: &source_binding,
            },
            None,
            self.image.digest(),
            Some(cancel),
        )?;
        let historical = closure.copied_content_mappings(source, prefix, Some(cancel))?;
        let mappings = include_selected_bases(source, &historical, cancel).map_err(Cause::from)?;
        let retained = exact_manifests(family, &mappings, cancel)?;
        // Calculation DATA only. Neither this canonical Staging projection nor
        // the calculated Ready record is presented as actual C SQL evidence.
        let mut destination_staging = ProviderTransitionIntentRecord::decode(&staging.encode()?)?;
        destination_staging.role = TransitionRole::Destination;
        destination_staging.local_generation = 0;
        destination_staging.validate_fields()?;
        let plan = reserve_plan::derive(
            source,
            prefix,
            &closure,
            self.image,
            &retained,
            staging,
            &destination_staging,
            cancel,
        )
        .map_err(Cause::from)?;
        let context = AdmittedFamilyContext::new(&admitted)?;
        let target = ExportReserveOnlyTarget::new(
            &context,
            staging,
            &destination_staging,
            &plan.required,
            plan.baseline_quota,
            plan.actual_metadata_floor,
            Some(cancel),
        )?;
        let protection = content::expected_protection(&target, &plan, cancel)?;
        let data = OwnedOrdinaryReadyData::new(
            plan.data,
            staging,
            self.image,
            &plan.required,
            &protection,
            plan.actual_metadata_floor,
            cancel,
        )?;
        let expected = content::expected_checkout(staging, self.image, cancel)?;
        self.source.require_ready_source(data.ready())?;
        let calculated = data.as_data(&plan.required)?;
        let inputs = OrdinaryReadyInputs {
            marker: self.destination.marker(),
            source_marker: self.source.marker(),
            source_staging: staging,
            closure: &closure,
            retained: &context,
            target: &target,
            protection: &protection,
            family_marker_bytes: self.destination.family().marker_bytes(),
            metadata_floor: data.metadata_floor(),
            data: &calculated,
        };
        // This complete actual retained+Protected walk explicitly requires C's
        // member Initializing throughout checkout population.
        let family_state = context
            .validate_export_protection(
                family,
                inputs.family_marker_bytes,
                &target,
                &plan.required,
                &protection,
                Some(cancel),
            )?
            .1;
        if family_state.root != ProtectionPresence::Present {
            return Err(rejected(
                "fresh checkout requires committed exact C protection",
            ));
        }

        let journal = destination_ready::read_ordinary_checkout_progress(
            source,
            destination,
            family,
            &inputs,
            &expected,
            Some(cancel),
        )?;
        let mut required = inputs.data.required.to_vec();
        required.sort_by(|a, b| a.id().cmp(b.id()));
        let current = Box::new(CheckoutData {
            calculated_ready: ProviderTransitionIntentRecord::decode(&inputs.data.ready.encode()?)?,
            calculated_receipt: ProviderTransitionReceiptRecord::decode(
                &inputs.data.receipt.encode()?,
            )?,
            expected,
            journal,
            required,
            binding: target.binding().clone(),
            family: family_state,
        });
        self.destination.require_parts(&current.parts(), cancel)?;
        if let Some(expected) = self.expected {
            expected.require_same_image(&current)?;
        }
        self.require_identity(cancel)?;
        Ok(current)
    }
}

fn read_observation(
    source: &OrdinaryExclusiveSource,
    c: &Connection,
    transaction_id: &str,
    cancel: &Cancellation,
) -> CauseResult<Box<SourceObservation>> {
    source.require_snapshot(c, cancel)?;
    let state = ordinary_transition::read_publication_state(
        c,
        source.marker(),
        transaction_id,
        Some(cancel),
    )?;
    if state.intent.state != TransitionState::Staging
        || state.intent.role != TransitionRole::Source
        || state.intent.receipt_id.is_some()
        || state.receipt.is_some()
    {
        return Err(rejected(
            "fresh checkout constructor requires actual B Staging without receipt",
        ));
    }
    let current = database::current(c).map_err(Cause::from)?;
    if current.snapshot.path_semantics != layrs_core::projection::PathSemantics::PortableWindows {
        return Err(rejected(
            "fresh checkout export requires portable Windows path semantics",
        ));
    }
    let selected = selection::for_scan(c, &current.snapshot).map_err(Cause::from)?;
    let version = state.intent.plan.source_prefix.source.version();
    if current.generation != version.generation || current.digest != version.snapshot_digest {
        return Err(rejected("fresh checkout actual source frontier differs"));
    }
    Ok(Box::new(SourceObservation {
        staging: state.intent,
        snapshot: current.snapshot,
        selection: selected,
    }))
}

fn require_image(observation: &SourceObservation, image: &OrdinaryExportImage) -> CauseResult<()> {
    if image.digest()
        != observation
            .staging
            .plan
            .source_prefix
            .source
            .version()
            .effective_image_digest
    {
        return Err(rejected(
            "fresh Ordinary checkout source image digest changed",
        ));
    }
    Ok(())
}

fn exact_manifests(
    family: &Connection,
    mappings: &Mappings,
    cancel: &Cancellation,
) -> CauseResult<BTreeMap<FileId, FileManifest>> {
    if mappings.len() > 100_000 {
        return Err(rejected("fresh checkout exact mapping count exceeds bound"));
    }
    let mut retained = BTreeMap::new();
    let mut bytes = 0usize;
    for (file, (id, length)) in mappings {
        cancel.check()?;
        let manifest = shared::read_manifest_exact(family, id, Some(cancel))
            .map_err(Cause::from)?
            .ok_or_else(|| rejected("fresh checkout exact family ManifestId missing"))?;
        if manifest.id() != id || manifest.file_id() != file || manifest.byte_length() != *length {
            return Err(rejected(
                "fresh checkout exact family manifest mapping differs",
            ));
        }
        bytes = bytes
            .checked_add(
                canonical_json(&manifest)
                    .map_err(|e| rejected(e.to_string()))?
                    .len(),
            )
            .filter(|n| *n <= 32 * 1024 * 1024)
            .ok_or_else(|| rejected("fresh checkout retained canonical manifests exceed bound"))?;
        cancel.check()?;
        retained.insert(file.clone(), manifest);
    }
    Ok(retained)
}

fn rollback_failed_action<T>(c: &Connection, result: CauseResult<T>) -> CauseResult<T> {
    if c.is_autocommit() {
        return result;
    }
    reservation::combine(result, c.execute_batch("ROLLBACK").map_err(Cause::from))
}
