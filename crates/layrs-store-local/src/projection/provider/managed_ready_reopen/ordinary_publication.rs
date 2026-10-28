//! Fresh owner-independent B Ready -> Published. B exclusive is acquired
//! before C shared; both are new capabilities, never upgraded or reconstructed
//! from DATA. C/family RO scopes retain checkout/CAS proofs through B close.
//! Clean existing files only: no recovery, repair, pathname adoption or
//! reconstruction of the former source image/allocation is available.
use super::*;
use crate::projection::provider::{
    database_io::WriterCloseObserver,
    destination_ready::read_ordinary_managed_ready,
    ordinary_transition::{self, PublicationCapacity, PublicationState},
    published_handoff::read_ordinary_published_for_ready,
    records::{
        transition::{TransitionRole, TransitionState},
        Record,
    },
    transition_sql::SourceIntentEffect,
};
use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum OrdinaryReadyPublicationEffect {
    Updated,
    AlreadyPublished,
}

/// Bounded actual Published DATA, returned only after observed closes and
/// independent readback. The effect never acknowledges an earlier failed ACK.
pub(in crate::projection::provider) struct OrdinaryReadyPublicationProgress {
    effect: OrdinaryReadyPublicationEffect,
    published: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    published_digest: String,
}
impl OrdinaryReadyPublicationProgress {
    pub(in crate::projection::provider) fn effect(&self) -> OrdinaryReadyPublicationEffect {
        self.effect
    }
    pub(in crate::projection::provider) fn published_digest(&self) -> &str {
        &self.published_digest
    }
    pub(in crate::projection::provider) fn published(&self) -> &ProviderTransitionIntentRecord {
        &self.published
    }
    pub(in crate::projection::provider) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }
}

pub(in crate::projection::provider) struct OrdinaryReadyPublicationFailure {
    pub step: AdmissionStep,
    pub cause: Cause,
    pub held: OrdinaryReadyPublication,
}

/// Owns the exclusive physical source solely through its private exclusive
/// constructor. No shared-source holder or owner conversion is accepted.
pub(in crate::projection::provider) struct OrdinaryReadyPublication {
    destination: OrdinaryReadyInspection,
    source: OrdinaryAdmissionSource,
}

impl OrdinaryReadyPublication {
    pub(in crate::projection::provider) fn open(
        source_root: &Path,
        destination_root: &Path,
        cancel: &Cancellation,
    ) -> Result<Self> {
        let source = OrdinaryAdmissionSource::open_exclusive(source_root, cancel)?;
        let pins = ManagedReadyPins::open(destination_root, cancel)?;
        source.require_destination(&pins, cancel)?;
        let destination = OrdinaryReadyInspection::open_shared(pins, cancel)?;
        source.require_ready_source(&destination.data.ready)?;
        let held = Self {
            destination,
            source,
        };
        held.require_physical(cancel)?;
        // Staging is explicitly refused by validate; no earlier source image,
        // allocation, planner or mutable ordinary opener is reconstructed.
        held.read_current(cancel)?;
        held.require_physical(cancel)?;
        Ok(held)
    }

    pub(in crate::projection::provider) fn publish(
        self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<OrdinaryReadyPublicationProgress, OrdinaryReadyPublicationFailure>
    {
        let mut step = AdmissionStep::Validating;
        let result = self.publish_inner(close, cancel, &mut step);
        result.map_err(|cause| OrdinaryReadyPublicationFailure {
            step,
            cause,
            held: self,
        })
    }

    /// Writer-free resolution requires actual B Published. Unlike admission
    /// resolution, B is indispensable here; actual Ready remains a refusal.
    pub(in crate::projection::provider) fn resolve(
        self,
        cancel: &Cancellation,
    ) -> std::result::Result<OrdinaryReadyPublicationProgress, OrdinaryReadyPublicationFailure>
    {
        let mut step = AdmissionStep::Validating;
        let result = (|| {
            self.require_physical(cancel)?;
            let before = self.read_current(cancel)?;
            require_published(&before)?;
            step = AdmissionStep::ReadScopesClosed;
            self.finish_readback(
                before,
                OrdinaryReadyPublicationEffect::AlreadyPublished,
                cancel,
                &mut step,
            )
        })();
        result.map_err(|cause| OrdinaryReadyPublicationFailure {
            step,
            cause,
            held: self,
        })
    }

    fn require_identity(&self, cancel: &Cancellation) -> Result<()> {
        self.source.require_identity(cancel)?;
        self.destination.require_identity(cancel)
    }

    fn require_physical(&self, cancel: &Cancellation) -> Result<()> {
        self.source.require_identity(cancel)?;
        self.destination.require_physical(cancel)
    }

    /// Complete actual B phase/frontier/receipt plus autonomous C8/family.
    /// Under SQL/gate this performs identity checks only, no physical hashes
    /// or namespace enumeration. Every capacity is derived anew from real C.
    fn validate(
        &self,
        source: &Connection,
        destination: &Connection,
        family: &Connection,
        cancel: &Cancellation,
    ) -> Result<(
        PublicationState,
        PublicationCapacity,
        ProviderTransitionIntentRecord,
        ProviderTransitionReceiptRecord,
    )> {
        self.source.require_snapshot(source, cancel)?;
        let actual = read_ordinary_managed_ready(
            destination,
            &self.destination.data.marker,
            family,
            Some(cancel),
        )?;
        let capacity = actual.publication_capacity(Some(cancel))?;
        self.source.require_ready_source(actual.ready())?;
        let state = ordinary_transition::read_publication_state(
            source,
            &self.source.pins.marker,
            &actual.ready().transaction_id,
            Some(cancel),
        )?;
        let ready = actual.ready();
        if !matches!(
            state.intent.state,
            TransitionState::Ready | TransitionState::Published
        ) || state.intent.role != TransitionRole::Source
            || state.intent.plan != ready.plan
            || state.intent.destination != ready.destination
            || state.intent.local_generation != ready.plan.source_prefix.source.version().generation
            || state.intent.reservation != ready.reservation
            || state.intent.receipt_id != ready.receipt_id
            || state.receipt.as_ref() != Some(actual.receipt())
        {
            return Err(invalid(
                "fresh Ordinary publication requires exact B Ready/Published and C receipt",
            ));
        }
        state.intent.require_receipt(actual.receipt())?;
        if state.intent.state == TransitionState::Published {
            // This genuine historical interpreter accepts coherent later B
            // suffixes; no current-prefix planner is used for replay.
            let published = read_ordinary_published_for_ready(
                source,
                &self.source.pins.marker,
                ready,
                actual.receipt(),
                Some(cancel),
            )?;
            if published.published() != &state.intent
                || Some(published.receipt()) != state.receipt.as_ref()
            {
                return Err(invalid(
                    "fresh Ordinary actual historical Published differs",
                ));
            }
        }
        let actual = actual.into_snapshot()?;
        self.destination.data.require_same(&actual)?;
        self.require_identity(cancel)?;
        Ok((state, capacity, actual.ready, actual.receipt))
    }

    fn read_current(&self, cancel: &Cancellation) -> Result<PublicationState> {
        self.require_identity(cancel)?;
        let pins = &self.destination.pins;
        let _catalog = StoreLock::acquire_existing(
            &pins.family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        valid(database_io::read_cancellable_strict(
            &pins.database,
            cancel,
            |destination| {
                valid(database_io::read_cancellable_strict(
                    &pins.family.catalog,
                    cancel,
                    |family| {
                        valid(database_io::read_cancellable_strict(
                            &self.source.pins.database,
                            cancel,
                            |source| Ok(self.validate(source, destination, family, cancel)?.0),
                        )?)
                    },
                )?)
            },
        )?)
    }

    fn publish_inner(
        &self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> Result<OrdinaryReadyPublicationProgress> {
        self.require_physical(cancel)?;
        let before = self.read_current(cancel)?;
        if before.intent.state == TransitionState::Published {
            *step = AdmissionStep::ReadScopesClosed;
            return self.finish_readback(
                before,
                OrdinaryReadyPublicationEffect::AlreadyPublished,
                cancel,
                step,
            );
        }
        self.require_physical(cancel)?;
        let pins = &self.destination.pins;
        let published = {
            let _catalog = StoreLock::acquire_existing(
                &pins.family.root().join("metadata/catalog.lock"),
                SessionMode::Shared,
            )?;
            valid(database_io::read_cancellable_strict(
                &pins.database,
                cancel,
                |destination| {
                    valid(database_io::read_cancellable_strict(
                        &pins.family.catalog,
                        cancel,
                        |family| {
                            // Independent clean RO B preflight positively closes
                            // before the one writer opens; C/family remain retained.
                            let admission = database_io::clean_admission(
                                &self.source.pins.database,
                                false,
                                |source| {
                                    let (current, _, _, _) =
                                        self.validate(source, destination, family, cancel)?;
                                    require_same_state(&before, &current)
                                },
                            )?;
                            self.require_identity(cancel)?;
                            *step = AdmissionStep::WriterOpenAttempted;
                            let bare = admission.open(&self.source.pins.database, close)?;
                            let result = (|| {
                                admission.require_geometry(&bare)?;
                                let source = bare.connection();
                                source.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                                let (current, capacity, ready, receipt) =
                                    self.validate(source, destination, family, cancel)?;
                                require_same_state(&before, &current)?;
                                *step = AdmissionStep::MutationAttempted;
                                if ordinary_transition::apply_published(
                                    source,
                                    &self.source.pins.marker,
                                    &current.intent,
                                    &ready,
                                    &receipt,
                                    &capacity,
                                    Some(cancel),
                                )? != SourceIntentEffect::Updated
                                {
                                    return Err(invalid(
                                        "fresh Ordinary publication did not update actual Ready",
                                    ));
                                }
                                let (after, _, _, _) =
                                    self.validate(source, destination, family, cancel)?;
                                require_published(&after)?;
                                after.intent.require_transition(&current.intent)?;
                                self.require_identity(cancel)?;
                                cancel.check()?;
                                *step = AdmissionStep::CommitAttempted;
                                source.execute_batch("COMMIT")?;
                                *step = AdmissionStep::CommitReturned;
                                // No validation SQL, hash or namespace walk after
                                // COMMIT before the mandatory observed B writer close.
                                self.require_identity(cancel)?;
                                Ok(after)
                            })();
                            // Exactly one explicit rollback on an active failed
                            // transaction, followed only by the mandatory real close.
                            let result = rollback_failed_action(bare.connection(), result);
                            let published = bare.finish_writer(result, close)?;
                            *step = AdmissionStep::WriterClosed;
                            Ok(published)
                        },
                    )?)
                },
            )?)?
        };
        *step = AdmissionStep::ReadScopesClosed;
        // B writer -> family RO -> C RO all closed positively, gate released.
        // Failed callbacks/closes cannot reach physical checks or fresh reads.
        self.finish_readback(
            published,
            OrdinaryReadyPublicationEffect::Updated,
            cancel,
            step,
        )
    }

    fn finish_readback(
        &self,
        expected: PublicationState,
        effect: OrdinaryReadyPublicationEffect,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> Result<OrdinaryReadyPublicationProgress> {
        self.require_physical(cancel)?;
        let current = self.read_current(cancel)?;
        require_published(&current)?;
        require_same_state(&expected, &current)?;
        self.require_physical(cancel)?;
        let published_digest = current.intent.digest()?;
        let receipt = current
            .receipt
            .ok_or_else(|| invalid("fresh Ordinary Published receipt absent"))?;
        *step = AdmissionStep::FreshVerified;
        Ok(OrdinaryReadyPublicationProgress {
            effect,
            published: current.intent,
            receipt,
            published_digest,
        })
    }
}

fn require_published(state: &PublicationState) -> Result<()> {
    if state.intent.state != TransitionState::Published || state.receipt.is_none() {
        return Err(invalid(
            "fresh Ordinary publication resolution requires actual Published",
        ));
    }
    Ok(())
}

fn require_same_state(expected: &PublicationState, current: &PublicationState) -> Result<()> {
    if expected != current {
        return Err(invalid(
            "fresh Ordinary publication actual phase/receipt changed",
        ));
    }
    Ok(())
}

fn rollback_failed_action<T>(source: &Connection, result: Result<T>) -> Result<T> {
    if result.is_ok() || source.is_autocommit() {
        return result;
    }
    match (
        result,
        source.execute_batch("ROLLBACK").map_err(Cause::from),
    ) {
        (Err(primary), Err(close)) => Err(Cause::Cleanup {
            primary: Box::new(primary),
            close: Box::new(close),
        }),
        (result, Ok(())) => result,
        (Ok(_), Err(close)) => Err(close),
    }
}
