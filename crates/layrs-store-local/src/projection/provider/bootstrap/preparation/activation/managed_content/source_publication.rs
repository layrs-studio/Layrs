//! Publish only the SAME image already retained by private/family Ready.
//! This phase never reads source content or constructs a fresh observation:
//! immutable protected CAS and verified checkout pins carry the committed
//! bytes. The image/checkpoint owner, namespace, quiescence, source head,
//! private receipt and complete family protection remain revalidated.
//!
//! Source Ready and Published have distinct commits. Destination/family RO
//! snapshots span each source COMMIT and close positively before source writer
//! acknowledgement. Any error stops; there is no cleanup, release or ordinary
//! admission, and no new file/path can be nominated through the result.
use super::{
    publication::ManagedFamilyReady, ready::ManagedPrivateReady, ready_data::OwnedReadyData, *,
};
use crate::projection::{
    provider::{
        destination_ready::ManagedReadyInputs,
        import_validation,
        records::import::{ImportSourceReferences, ProviderTransitionReceiptRecord},
        transition_sql,
        validation::{self, PrivateActivePublicationValidation},
    },
    shared::{
        exact_content::ExactBinding,
        provider_access::provider_import::publication::{
            ImportPublicationInputs, ImportPublicationMember,
        },
    },
};

const MAX_METADATA: u64 = 1024 * 1024 * 1024;

/// Keeps all earlier physical witnesses borrowed. `source` is terminal DATA,
/// not a capability to admit an ordinary worktree or authorize another image.
pub(in crate::projection::provider::bootstrap::preparation::activation) struct ManagedPublished<
    'p,
    'r,
    'b,
> {
    family: &'p ManagedFamilyReady<'r, 'b>,
    source: ProviderTransitionIntentRecord,
}
impl ManagedPublished<'_, '_, '_> {
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn source_published(
        &self,
    ) -> &ProviderTransitionIntentRecord {
        &self.source
    }

    pub(in crate::projection::provider::bootstrap::preparation::activation) fn receipt(
        &self,
    ) -> &ProviderTransitionReceiptRecord {
        self.family.receipt()
    }
}

#[derive(PartialEq, Eq)]
struct SourceState {
    phase: TransitionState,
    ready: ProviderTransitionIntentRecord,
    published: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
}

fn copy_intent(
    intent: &ProviderTransitionIntentRecord,
) -> CauseResult<ProviderTransitionIntentRecord> {
    Ok(ProviderTransitionIntentRecord::decode(&intent.encode()?)?)
}

impl ManagedImportLease<'_, '_, '_, '_> {
    /// All SQL handles supplied here belong to the caller's continuously held
    /// source snapshot. Complete publication proof remains alive through the
    /// action; no CaptureValidation conversion is performed.
    fn with_source_publication_inputs<T>(
        &self,
        source_c: &Connection,
        family: &ManagedFamilyReady<'_, '_>,
        cancel: &Cancellation,
        action: impl FnOnce(
            &ManagedReadyInputs<'_>,
            &ImportPublicationInputs<'_>,
            &ExactBinding,
            u64,
            SourceState,
        ) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.require_intent()?;
        let private = family.private_ready();
        private.require_physical(self, cancel)?;
        self.boundary(cancel)?;
        let prepared = self
            .held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("source publication prepared receipt absent"))?;
        let proof: PrivateActivePublicationValidation =
            validation::private_active_publication_with_source(
                source_c,
                &self.held.artifacts.allocation,
                prepared,
                &self.held.binding,
                Some(self.root.registration().root_id()),
                Some(cancel),
            )?;
        if proof.active.mount != *self.mount || proof.active.activation != *self.activation {
            return Err(invalid("source publication Active owner differs"));
        }
        self.image.require_owner(
            self.root,
            &proof.active.source,
            &proof.active.activated.snapshot,
        )?;
        let ImportSource::Provider { version, .. } = &self.staging.plan.source_prefix.source else {
            return Err(invalid("source publication requires provider prefix"));
        };
        let closure = import_validation::verify_source_publication(
            source_c,
            &self.held.artifacts.allocation.format,
            &self.staging.plan.source_prefix,
            ImportSourceReferences::Provider {
                allocation: &self.held.artifacts.allocation,
                prepared,
                activation: &proof.active.activation,
            },
            &proof,
            &version.effective_image_digest,
            Some(cancel),
        )?;
        let derived = managed_plan::rederive_publication_reservation(
            source_c,
            &closure,
            self.image,
            &proof,
            self.fresh,
            self.staging,
            self.manifests,
            cancel,
        )?;
        let (protected, checkout) = private.physical_parts();
        let owned = OwnedReadyData::new(
            derived,
            self.staging,
            self.image,
            self.manifests,
            protected.root(),
            cancel,
        )?;
        if owned.ready() != private.ready_intent()
            || owned.receipt() != private.receipt()
            || owned.receipt() != family.receipt()
        {
            return Err(invalid("source publication retained Ready receipt differs"));
        }
        let mut ready = copy_intent(self.staging)?;
        ready.state = TransitionState::Ready;
        ready.receipt_id = Some(owned.receipt().digest()?);
        ready.require_transition(self.staging)?;
        ready.require_receipt(owned.receipt())?;
        let mut published = copy_intent(&ready)?;
        published.state = TransitionState::Published;
        published.require_transition(&ready)?;
        published.require_receipt(owned.receipt())?;
        let current = proof
            .transitions
            .iter()
            .find(|intent| intent.transaction_id == self.staging.transaction_id)
            .ok_or_else(|| invalid("source publication intent absent"))?;
        let expected = match current.state {
            TransitionState::Staging => self.staging,
            TransitionState::Ready => &ready,
            TransitionState::Published => &published,
            _ => return Err(invalid("source publication phase differs")),
        };
        if current != expected {
            return Err(invalid("source publication exact intent differs"));
        }
        let phase = current.state;
        let floor =
            source_metadata_floor(owned.metadata_floor(), &ready, &published, owned.receipt())?;
        // The separate retained checkout journal is also included before any
        // writer opens. No physical SQLite page/index geometry is asserted.
        add_metadata(floor, checkout.expected().maximum_encoded_bytes())?;
        let data = owned.as_data(self.manifests)?;
        let inputs = ManagedReadyInputs {
            marker: &self.destination.proof().format,
            source: &proof.active.source,
            allocation: &self.held.artifacts.allocation,
            prepared,
            activation: &proof.active.activation,
            prefix: &self.staging.plan.source_prefix,
            staging: owned.destination_staging(),
            closure: &closure,
            family_marker_bytes: self.held.family.marker_bytes(),
            data: &data,
        };
        let publication = ImportPublicationInputs {
            content: ImportContentInputs {
                allocation: &self.held.artifacts.allocation,
                prepared,
                activation: &proof.active.activation,
                source: &proof.active.source,
                capture: &proof.capture,
                source_staging: self.staging,
                destination_staging: owned.destination_staging(),
                required: self.manifests,
            },
            destination_ready: owned.ready(),
            receipt: owned.receipt(),
        };
        let binding = self.exact_binding()?;
        private.require_physical(self, cancel)?;
        let result = action(
            &inputs,
            &publication,
            &binding,
            floor,
            SourceState {
                phase,
                ready,
                published,
                receipt: ProviderTransitionReceiptRecord::decode(&owned.receipt().encode()?)?,
            },
        )?;
        // The action may have COMMITted source. These checks must remain
        // namespace/pin-only: no SQL or source content callback after COMMIT.
        self.image.require_owner(
            self.root,
            &proof.active.source,
            &proof.active.activated.snapshot,
        )?;
        private.require_physical(self, cancel)?;
        self.boundary(cancel)?;
        Ok(result)
    }

    fn check_source_publication(
        &self,
        source_c: &Connection,
        destination_c: &Connection,
        family_c: &Connection,
        family: &ManagedFamilyReady<'_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<SourceState> {
        self.with_source_publication_inputs(
            source_c,
            family,
            cancel,
            |inputs, publication, binding, floor, source| {
                self.require_source_publication_family(
                    source_c,
                    destination_c,
                    family_c,
                    inputs,
                    publication,
                    binding,
                    floor,
                    family.private_ready(),
                    cancel,
                )?;
                Ok(source)
            },
        )
    }

    fn require_source_publication_family(
        &self,
        source_c: &Connection,
        destination_c: &Connection,
        family_c: &Connection,
        inputs: &ManagedReadyInputs<'_>,
        publication: &ImportPublicationInputs<'_>,
        binding: &ExactBinding,
        floor: u64,
        private: &ManagedPrivateReady<'_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let state = self.read_family_publication(
            source_c,
            destination_c,
            family_c,
            inputs,
            publication,
            binding,
            floor,
            private,
            cancel,
        )?;
        if state.member != ImportPublicationMember::Ready {
            return Err(invalid("source publication requires exact family Ready"));
        }
        let (protected, checkout) = private.physical_parts();
        let remaining = state
            .protection
            .installation
            .remaining
            .checked_sub(checkout.expected().header().file_bytes)
            .ok_or_else(|| invalid("source publication checkout envelope insufficient"))?;
        let metadata = add_metadata(floor, checkout.expected().maximum_encoded_bytes())?;
        protected.root().require_envelope(remaining, metadata)?;
        private.require_physical(self, cancel)
    }

    /// The nested RO transactions span `action`, including source COMMIT.
    /// Their positive closes precede source writer close acknowledgement.
    fn with_source_publication_context<T>(
        &self,
        source_c: &Connection,
        family: &ManagedFamilyReady<'_, '_>,
        cancel: &Cancellation,
        action: impl FnOnce(&Connection, &Connection, SourceState) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.with_source_publication_inputs(
            source_c,
            family,
            cancel,
            |inputs, publication, binding, floor, source| {
                managed_destination::valid(database_io::read_cancellable(
                    self.destination.destination_database(),
                    cancel,
                    |destination_c| {
                        managed_destination::valid(database_io::read_cancellable(
                            &self.held.family.catalog,
                            cancel,
                            |family_c| {
                                self.require_source_publication_family(
                                    source_c,
                                    destination_c,
                                    family_c,
                                    inputs,
                                    publication,
                                    binding,
                                    floor,
                                    family.private_ready(),
                                    cancel,
                                )?;
                                action(destination_c, family_c, source)
                            },
                        )?)
                    },
                )?)
            },
        )
    }

    fn read_source_publication(
        &self,
        family: &ManagedFamilyReady<'_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<SourceState> {
        managed_destination::valid(database_io::read_cancellable(
            &self.held.artifacts.database,
            cancel,
            |source_c| {
                self.with_source_publication_context(source_c, family, cancel, |_, _, source| {
                    Ok(source)
                })
            },
        )?)
    }

    pub(in crate::projection::provider::bootstrap::preparation::activation) fn publish_source<
        'p,
        'r,
        'b,
    >(
        &mut self,
        family: &'p ManagedFamilyReady<'r, 'b>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedPublished<'p, 'r, 'b>> {
        self.require_intent()?;
        // Deliberately no self.rehash(): source Ready makes the old local
        // content reader unavailable. Immutable CAS/checkout proofs retain the
        // SAME already-verified image; this operation consumes no source bytes.
        family.private_ready().require_physical(self, cancel)?;
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.boundary(cancel)?;
        let mut before = self.read_source_publication(family, cancel)?;
        while before.phase != TransitionState::Published {
            cancel.check()?;
            let next = match before.phase {
                TransitionState::Staging => &before.ready,
                TransitionState::Ready => &before.published,
                _ => return Err(invalid("source publication predecessor phase differs")),
            };
            let previous = if before.phase == TransitionState::Staging {
                self.staging
            } else {
                &before.ready
            };
            let file = &self.held.artifacts.database;
            database_io::validate_native_filename(file.path())?;
            let admission = database_io::clean_admission(file, false, |source_c| {
                let current = self.with_source_publication_context(
                    source_c,
                    family,
                    cancel,
                    |_, _, source| Ok(source),
                )?;
                if current != before {
                    return Err(invalid("source publication admission predecessor changed"));
                }
                Ok(())
            })?;
            family.private_ready().require_physical(self, cancel)?;
            self.boundary(cancel)?;
            let bare = admission.open(file, close)?;
            let result = (|| {
                admission.require_geometry(&bare)?;
                let source_c = bare.connection();
                source_c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                self.with_source_publication_context(
                    source_c,
                    family,
                    cancel,
                    |destination_c, family_c, current| {
                        if current != before {
                            return Err(invalid("source publication writer predecessor changed"));
                        }
                        let prepared =
                            self.held.artifacts.prepared.as_ref().ok_or_else(|| {
                                invalid("source publication prepared receipt absent")
                            })?;
                        transition_sql::apply_source_publication(
                            source_c,
                            &self.held.artifacts.allocation,
                            prepared,
                            &self.held.binding,
                            self.root.registration().root_id(),
                            previous,
                            next,
                            &before.receipt,
                            Some(cancel),
                        )?;
                        let prospective = self.check_source_publication(
                            source_c,
                            destination_c,
                            family_c,
                            family,
                            cancel,
                        )?;
                        if prospective.phase != next.state
                            || prospective.ready != before.ready
                            || prospective.published != before.published
                            || prospective.receipt != before.receipt
                        {
                            return Err(invalid("source publication prospective state differs"));
                        }
                        family.private_ready().require_physical(self, cancel)?;
                        self.boundary(cancel)?;
                        source_c.execute_batch("COMMIT")?;
                        Ok(())
                    },
                )
            })();
            // Family/destination read closes have already completed. Any
            // close/ACK failure returns immediately, including durable Ready.
            bare.finish_writer(result, close)?;
            let confirmed = self.read_source_publication(family, cancel)?;
            if confirmed.phase != next.state
                || confirmed.ready != before.ready
                || confirmed.published != before.published
                || confirmed.receipt != before.receipt
            {
                return Err(invalid("source publication durable state differs"));
            }
            before = confirmed;
        }
        family.private_ready().require_physical(self, cancel)?;
        self.boundary(cancel)?;
        Ok(ManagedPublished {
            family,
            source: before.published,
        })
    }
}

fn add_metadata(sum: u64, bytes: u64) -> CauseResult<u64> {
    sum.checked_add(bytes)
        .filter(|n| *n <= MAX_METADATA)
        .ok_or_else(|| invalid("source publication metadata floor overflow or exceeds 1GiB"))
}

/// Normalized SQL values only: INTEGER=8, NULL=0, UTF-8 TEXT/BLOB=length.
/// Charge the source receipt's five columns and BOTH complete terminal intent
/// projections conservatively, in addition to the existing OwnedReady floor.
fn source_metadata_floor(
    base: u64,
    ready: &ProviderTransitionIntentRecord,
    published: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) -> CauseResult<u64> {
    ready.require_receipt(receipt)?;
    published.require_transition(ready)?;
    published.require_receipt(receipt)?;
    if ready.role != TransitionRole::Source
        || ready.state != TransitionState::Ready
        || published.role != TransitionRole::Source
        || published.state != TransitionState::Published
    {
        return Err(invalid("source publication metadata phase differs"));
    }
    let mut floor = add_metadata(base, 0)?;
    for bytes in [
        receipt.digest()?.len() as u64,
        receipt.owner_identity_digest.len() as u64,
        receipt.transaction_id.len() as u64,
        receipt
            .companion_id
            .as_ref()
            .map_or(0, |id| id.len() as u64),
        receipt.encode()?.len() as u64,
    ] {
        floor = add_metadata(floor, bytes)?;
    }
    for (intent, state) in [(ready, "ready"), (published, "published")] {
        for bytes in [
            intent.transaction_id.len() as u64,
            "source".len() as u64,
            state.len() as u64,
            8,
            intent
                .reservation
                .as_ref()
                .map_or(0, |value| value.len() as u64),
            intent
                .receipt_id
                .as_ref()
                .map_or(0, |value| value.len() as u64),
            intent.encode()?.len() as u64,
            intent.digest()?.len() as u64,
        ] {
            floor = add_metadata(floor, bytes)?;
        }
    }
    Ok(floor)
}
