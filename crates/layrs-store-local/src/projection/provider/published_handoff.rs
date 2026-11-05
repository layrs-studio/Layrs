//! Historical source publication observation for the future admission handoff.
//! This proves source SQL only. Destination DATA is not physical authority;
//! an admission writer must separately retain and revalidate genuine Ready.
//! A caller-owned transaction may contain uncommitted writes. Durable admission
//! must invoke this interpreter through its own independently opened read-only
//! source connection, retain that snapshot through destination COMMIT, and
//! observe positive closes. This type alone does not prove source durability.
use super::{
    cancellation,
    records::{
        import::ProviderTransitionReceiptRecord,
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        AllocationRecord, PreparedRecord, ProviderBootstrapBinding, Record,
    },
    recovery::error::{invalid, Result},
    validation::{self, PrivateActivePublicationValidation},
};
use layrs_vfs::Cancellation;
use rusqlite::Connection;

/// Actual admitted ordinary source SQL at a retained historical publication.
/// This never grants durability: the admission driver owns an independent
/// read-only source connection and observes its close after destination COMMIT.
pub(super) struct OrdinaryPublishedForReady<'s> {
    connection: &'s Connection,
    marker: crate::projection::format::Marker,
    published: ProviderTransitionIntentRecord,
    receipt: ProviderTransitionReceiptRecord,
    ready: ProviderTransitionIntentRecord,
}

impl OrdinaryPublishedForReady<'_> {
    pub(super) fn published(&self) -> &ProviderTransitionIntentRecord {
        &self.published
    }

    pub(super) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.receipt
    }

    pub(super) fn require(&self, c: &Connection, cancel: Option<&Cancellation>) -> Result<()> {
        if c.is_autocommit() || !std::ptr::eq(c, self.connection) {
            return Err(invalid(
                "ordinary publication handoff snapshot ended or differs",
            ));
        }
        let fresh =
            read_ordinary_published_for_ready(c, &self.marker, &self.ready, &self.receipt, cancel)?;
        if fresh.published != self.published || fresh.receipt != self.receipt {
            return Err(invalid(
                "ordinary publication handoff historical rows changed",
            ));
        }
        Ok(())
    }
}

pub(super) fn read_ordinary_published_for_ready<'s>(
    c: &'s Connection,
    marker: &crate::projection::format::Marker,
    ready: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    cancel: Option<&Cancellation>,
) -> Result<OrdinaryPublishedForReady<'s>> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "ordinary publication handoff requires retained transaction",
        ));
    }
    let ready_bytes = ready.encode()?;
    receipt.encode()?;
    if ready.role != TransitionRole::Destination
        || ready.state != TransitionState::Ready
        || !matches!(
            ready.plan.source_prefix.source,
            super::records::import::ImportSource::Ordinary {
                private_schema: 9 | 10,
                admission: Some(_),
                ..
            }
        )
    {
        return Err(invalid(
            "ordinary publication handoff target origin or phase differs",
        ));
    }
    ready.require_receipt(receipt)?;
    // Complete current admission/outgoing partition; Published may predate the
    // current head. No live planner or reconstructed Staging is used here.
    let observed = super::ordinary_transition::read_publication_state(
        c,
        marker,
        &ready.transaction_id,
        cancel,
    )?;
    let published = observed.intent;
    let actual_receipt = observed
        .receipt
        .ok_or_else(|| invalid("ordinary publication handoff receipt absent"))?;
    if published.role != TransitionRole::Source
        || published.state != TransitionState::Published
        || published.plan != ready.plan
        || published.destination != ready.destination
        || published.reservation != ready.reservation
        || published.receipt_id != ready.receipt_id
        || actual_receipt != *receipt
    {
        return Err(invalid(
            "ordinary publication handoff actual source differs from Ready",
        ));
    }
    published.require_receipt(&actual_receipt)?;
    cancellation::check(cancel)?;
    Ok(OrdinaryPublishedForReady {
        connection: c,
        marker: marker.clone(),
        published,
        receipt: actual_receipt,
        ready: ProviderTransitionIntentRecord::decode(&ready_bytes)?,
    })
}

#[derive(Clone, Copy)]
pub(super) struct PublicationSource<'a> {
    pub allocation: &'a AllocationRecord,
    pub prepared: &'a PreparedRecord,
    pub binding: &'a ProviderBootstrapBinding,
    pub root_id: [u8; 16],
}

pub(super) struct PublishedForReady<'s, 'a> {
    connection: &'s Connection,
    source: PublicationSource<'a>,
    full: PrivateActivePublicationValidation,
    transition_index: usize,
    receipt_index: usize,
    ready: ProviderTransitionIntentRecord,
}

impl PublishedForReady<'_, '_> {
    /// Accessors expose the actual interpreted source DATA only.
    pub(super) fn published(&self) -> &ProviderTransitionIntentRecord {
        &self.full.transitions[self.transition_index]
    }

    pub(super) fn receipt(&self) -> &ProviderTransitionReceiptRecord {
        &self.full.receipts[self.receipt_index]
    }

    /// A Connection borrow does not prevent ending/restarting its transaction.
    /// Reinterpret the complete current source before using its historical ACK.
    /// Later coherent captures are allowed; the published transaction is fixed.
    pub(super) fn require(&self, c: &Connection, cancel: Option<&Cancellation>) -> Result<()> {
        if !std::ptr::eq(c, self.connection) || c.is_autocommit() {
            return Err(invalid(
                "publication handoff source snapshot ended or differs",
            ));
        }
        let fresh = read_published_for_ready(c, self.source, &self.ready, self.receipt(), cancel)?;
        if fresh.published() != self.published() || fresh.receipt() != self.receipt() {
            return Err(invalid("publication handoff historical source changed"));
        }
        Ok(())
    }
}

pub(super) fn read_published_for_ready<'s, 'a>(
    c: &'s Connection,
    source: PublicationSource<'a>,
    ready: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    cancel: Option<&Cancellation>,
) -> Result<PublishedForReady<'s, 'a>> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "publication handoff requires a retained source transaction",
        ));
    }
    // Encode performs size preflight before semantic comparisons/cloning.
    let ready_bytes = ready.encode()?;
    receipt.encode()?;
    if ready.role != TransitionRole::Destination || ready.state != TransitionState::Ready {
        return Err(invalid(
            "publication handoff target is not destination Ready",
        ));
    }
    ready.require_receipt(receipt)?;
    let full = validation::private_active_publication_with_source(
        c,
        source.allocation,
        source.prepared,
        source.binding,
        Some(source.root_id),
        cancel,
    )?;
    let transition_index = full
        .transitions
        .iter()
        .position(|intent| {
            intent.transaction_id == ready.transaction_id
                && intent.role == TransitionRole::Source
                && intent.state == TransitionState::Published
        })
        .ok_or_else(|| invalid("publication handoff has no actual SourcePublished"))?;
    let published = &full.transitions[transition_index];
    if published.plan != ready.plan
        || published.destination != ready.destination
        || published.reservation != ready.reservation
        || published.receipt_id != ready.receipt_id
    {
        return Err(invalid("publication handoff source and destination differ"));
    }
    published.require_receipt(receipt)?;
    let receipt_index = full
        .receipts
        .iter()
        .position(|observed| observed == receipt)
        .ok_or_else(|| invalid("publication handoff actual receipt differs"))?;
    cancellation::check(cancel)?;
    Ok(PublishedForReady {
        connection: c,
        source,
        full,
        transition_index,
        receipt_index,
        ready: ProviderTransitionIntentRecord::decode(&ready_bytes)?,
    })
}
