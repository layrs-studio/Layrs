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

impl ManagedImportLease<'_, '_, '_, '_, '_> {
    pub(super) fn require_retained_family_ready(
        &self,
        family: &ManagedFamilyReady<'_, '_>,
        source: &super::source_protection::ManagedSourceCompanionProtection<'_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.read_source_publication(family, Some(source), cancel)?;
        Ok(())
    }
    pub(super) fn require_retained_denial_family_ready(
        &self,
        family: &ManagedFamilyReady<'_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        if self.denial.is_none() || self.companion.is_some() {
            return Err(invalid("retained plain denial Ready mode absent"));
        }
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.read_source_publication(family, None, cancel)?;
        Ok(())
    }

    /// All SQL handles supplied here belong to the caller's continuously held
    /// source snapshot. Complete publication proof remains alive through the
    /// action; no CaptureValidation conversion is performed.
    fn with_source_publication_inputs<T>(
        &self,
        source_c: &Connection,
        family: &ManagedFamilyReady<'_, '_>,
        source_protection: Option<&super::protection::ManagedSourceCompanionProtection<'_, '_>>,
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
        if self.staging.plan.companion_id.is_some() != source_protection.is_some() {
            return Err(invalid(
                "source publication requires exact retained source companion scope",
            ));
        }
        if let Some(source) = source_protection {
            source.require(self, cancel)?;
        }
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
        let derived = self.rederive(source_c, &closure, &proof, cancel)?;
        let (protected, checkout) = private.physical_parts();
        let owned = self.ready_data(derived, protected.root(), cancel)?;
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
        let floor = if source_protection.is_some() || self.denial.is_some() {
            // Companion B is independently funded. Plain denial Ready instead
            // already included B's future publication rows in its C floor.
            // Neither mode may add the same obligation a second time here.
            owned.metadata_floor()
        } else {
            source_metadata_floor(owned.metadata_floor(), &ready, &published, owned.receipt())?
        };
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
            companion: self.ready_companion(&proof),
            denial: self.ready_denial(&proof),
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
                source_context: Some(
                    crate::projection::shared::source_companion::SourceCompanionFamilyContext {
                        publication: &proof,
                        pending: source_protection.map(|p| p.data().protection()),
                    },
                ),
            },
            destination_ready: owned.ready(),
            receipt: owned.receipt(),
        };
        if let Some(source) = source_protection {
            source.data().require(
                &self.held.artifacts.allocation,
                self.staging,
                owned.receipt(),
                Some(cancel),
            )?;
        }
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
        if let Some(source) = source_protection {
            source.require(self, cancel)?;
        }
        self.boundary(cancel)?;
        Ok(result)
    }

    #[cfg(test)]
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn assert_companion_ready_refusals(
        &self,
        family: &ManagedFamilyReady<'_, '_>,
        source: &super::protection::ManagedSourceCompanionProtection<'_, '_>,
        cancel: &Cancellation,
    ) -> CauseResult<()> {
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        managed_destination::valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |source_c| {
                self.with_source_publication_inputs(source_c,family,Some(source),cancel,|inputs,publication,binding,floor,_| {
            managed_destination::valid(database_io::read_cancellable_strict(self.destination.destination_database(),cancel,|destination_c| {
              managed_destination::valid(database_io::read_cancellable_strict(&self.held.family.catalog,cancel,|family_c| {
                self.require_source_publication_family(source_c,destination_c,family_c,inputs,publication,binding,floor,family.private_ready(),cancel)?;
            let checkout=family.private_ready().physical_parts().1;
            let mut missing=inputs.companion.ok_or_else(||invalid("native negative requires actual companion route"))?;
            missing.source_context.pending=None;
            let bad=ManagedReadyInputs { marker:inputs.marker,source:inputs.source,allocation:inputs.allocation,prepared:inputs.prepared,
                activation:inputs.activation,prefix:inputs.prefix,staging:inputs.staging,closure:inputs.closure,
                family_marker_bytes:inputs.family_marker_bytes,data:inputs.data,companion:Some(missing),denial:None };
            let before=destination_c.total_changes();
            let error=crate::projection::provider::destination_ready::read_checkout_family_ready(source_c,destination_c,family_c,&bad,checkout.expected(),Some(cancel)).err().ok_or_else(||invalid("missing B root context accepted C Ready"))?;
            assert!(error.to_string().contains("pending B root absent"),"{error}");
            let shortened=&inputs.data.required[..inputs.data.required.len()-1];
            let wrong_union=crate::projection::provider::destination_ready::ManagedReadyData {observation:inputs.data.observation,placement:inputs.data.placement,capture:inputs.data.capture,
                overlay:inputs.data.overlay,required:shortened,receipt:inputs.data.receipt,ready:inputs.data.ready,claims:inputs.data.claims};
            let bad=ManagedReadyInputs { data:&wrong_union, companion:inputs.companion, ..bad };
            assert!(crate::projection::provider::destination_ready::read_checkout_family_ready(source_c,destination_c,family_c,&bad,checkout.expected(),Some(cancel)).is_err());
            let mut changed=ProviderTransitionIntentRecord::decode(&inputs.data.ready.encode()?)?;
            changed.plan.companion_files.digest=format!("blake3:{}","f".repeat(64));
            let wrong_rows=crate::projection::provider::destination_ready::ManagedReadyData {ready:&changed,required:inputs.data.required,..wrong_union};
            let bad=ManagedReadyInputs {data:&wrong_rows,..bad};
            assert!(crate::projection::provider::destination_ready::read_checkout_family_ready(source_c,destination_c,family_c,&bad,checkout.expected(),Some(cancel)).is_err());
            assert_eq!(before,destination_c.total_changes());
                Ok(())
              })?)
            })?)
          })
            },
        )?)
    }

    fn check_source_publication(
        &self,
        source_c: &Connection,
        destination_c: &Connection,
        family_c: &Connection,
        family: &ManagedFamilyReady<'_, '_>,
        source_protection: Option<&super::protection::ManagedSourceCompanionProtection<'_, '_>>,
        cancel: &Cancellation,
    ) -> CauseResult<SourceState> {
        self.with_source_publication_inputs(
            source_c,
            family,
            source_protection,
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
        if let Some(context) = publication.content.source_context.as_ref() {
            self.held.family.validate_source_companion_catalog(
                family_c,
                &self.held.artifacts.allocation,
                context,
                Some(cancel),
            )?;
            if let Some(root) = context.pending {
                if root.inspect(family_c,Some(cancel))? != crate::projection::shared::source_companion::SourceCompanionPresence::Protected {
                    return Err(invalid("source publication B protection absent"));
                }
                let current = context
                    .publication
                    .source_transitions()
                    .iter()
                    .find(|i| i.transaction_id == root.binding.destination.transaction_id)
                    .ok_or_else(|| invalid("source recovery actual intent absent"))?;
                // Both genuine C Ready scopes were just proved above. This
                // classification cannot substitute for either of those scopes.
                root.recovery(family_c, current.state, true, Some(cancel))?;
            }
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
        source_protection: Option<&super::protection::ManagedSourceCompanionProtection<'_, '_>>,
        cancel: &Cancellation,
        action: impl FnOnce(&Connection, &Connection, SourceState) -> CauseResult<T>,
    ) -> CauseResult<T> {
        self.with_source_publication_inputs(
            source_c,
            family,
            source_protection,
            cancel,
            |inputs, publication, binding, floor, source| {
                managed_destination::valid(database_io::read_cancellable_strict(
                    self.destination.destination_database(),
                    cancel,
                    |destination_c| {
                        managed_destination::valid(database_io::read_cancellable_strict(
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
        source_protection: Option<&super::protection::ManagedSourceCompanionProtection<'_, '_>>,
        cancel: &Cancellation,
    ) -> CauseResult<SourceState> {
        managed_destination::valid(database_io::read_cancellable_strict(
            &self.held.artifacts.database,
            cancel,
            |source_c| {
                self.with_source_publication_context(
                    source_c,
                    family,
                    source_protection,
                    cancel,
                    |_, _, source| Ok(source),
                )
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
        if self.staging.plan.companion_id.is_some() || self.staging.plan.denial_coverage.is_some() {
            return Err(invalid("legacy publish_source requires no new companion"));
        }
        self.publish_source_scoped(family, None, close, cancel)
    }
    pub(super) fn publish_source_denial<'p, 'r, 'b>(
        &mut self,
        family: &'p ManagedFamilyReady<'r, 'b>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedPublished<'p, 'r, 'b>> {
        if self.denial.is_none()
            || self.companion.is_some()
            || self.staging.plan.companion_id.is_some()
        {
            return Err(invalid("plain denial source publication mode absent"));
        }
        self.publish_source_scoped(family, None, close, cancel)
    }
    pub(in crate::projection::provider::bootstrap::preparation::activation) fn publish_source_companion<
        'p,
        'r,
        'b,
    >(
        &mut self,
        family: &'p ManagedFamilyReady<'r, 'b>,
        source: &super::protection::ManagedSourceCompanionProtection<'_, '_>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedPublished<'p, 'r, 'b>> {
        self.publish_source_scoped(family, Some(source), close, cancel)
    }
    fn publish_source_scoped<'p, 'r, 'b>(
        &mut self,
        family: &'p ManagedFamilyReady<'r, 'b>,
        source_protection: Option<&super::protection::ManagedSourceCompanionProtection<'_, '_>>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> CauseResult<ManagedPublished<'p, 'r, 'b>> {
        self.require_intent()?;
        if let Some(source) = source_protection {
            source.require(self, cancel)?;
        }
        // Deliberately no self.rehash(): source Ready makes the old local
        // content reader unavailable. Immutable CAS/checkout proofs retain the
        // SAME already-verified image; this operation consumes no source bytes.
        family.private_ready().require_physical(self, cancel)?;
        let _locks = self.held.family.locks(
            &self.held.artifacts.allocation.format.identity.worktree_id,
            true,
        )?;
        self.boundary(cancel)?;
        let mut before = self.read_source_publication(family, source_protection, cancel)?;
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
                    source_protection,
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
                    source_protection,
                    cancel,
                    |destination_c, family_c, current| {
                        let operation = (|| {
                            if current != before {
                                return Err(invalid(
                                    "source publication writer predecessor changed",
                                ));
                            }
                            let prepared =
                                self.held.artifacts.prepared.as_ref().ok_or_else(|| {
                                    invalid("source publication prepared receipt absent")
                                })?;
                            if let Some(source) = source_protection {
                                transition_sql::apply_source_publication_with_companion(
                                    source_c,
                                    &self.held.artifacts.allocation,
                                    prepared,
                                    &self.held.binding,
                                    self.root.registration().root_id(),
                                    previous,
                                    next,
                                    &before.receipt,
                                    source.data(),
                                    Some(cancel),
                                )?;
                            } else {
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
                            }
                            let prospective = self.check_source_publication(
                                source_c,
                                destination_c,
                                family_c,
                                family,
                                source_protection,
                                cancel,
                            )?;
                            if prospective.phase != next.state
                                || prospective.ready != before.ready
                                || prospective.published != before.published
                                || prospective.receipt != before.receipt
                            {
                                return Err(invalid(
                                    "source publication prospective state differs",
                                ));
                            }
                            family.private_ready().require_physical(self, cancel)?;
                            self.boundary(cancel)?;
                            source_c.execute_batch("COMMIT")?;
                            Ok(())
                        })();
                        // Roll back BEFORE nested C/family scopes close. If a
                        // close itself fails, no subsequent SQL/FS probe runs.
                        match operation {
                            Err(primary) if !source_c.is_autocommit() => {
                                match source_c.execute_batch("ROLLBACK") {
                                    Ok(()) => Err(primary),
                                    Err(e) => Err(Cause::Cleanup {
                                        primary: Box::new(primary),
                                        close: Box::new(e.into()),
                                    }),
                                }
                            }
                            other => other,
                        }
                    },
                )
            })();
            // Family/destination read closes have already completed. Any
            // close/ACK failure returns immediately, including durable Ready.
            bare.finish_writer(result, close)?;
            let confirmed = self.read_source_publication(family, source_protection, cancel)?;
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
pub(super) fn source_metadata_floor(
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
