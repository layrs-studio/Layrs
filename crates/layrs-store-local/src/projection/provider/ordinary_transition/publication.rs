//! Ordinary source publication SQL DATA only. The caller authenticates actual
//! C Ready, its physical checkout/CAS and complete family Ready independently,
//! retaining those witnesses through COMMIT and positive enclosing closes.
//! Caller transaction/FK ON required, including replay. Errors require rollback.
//! No connection, physical C evidence, durable ACK or admission is created here.
use super::*;
use crate::projection::provider::{
    import_validation::{ImportDatabaseKind, require_live_source_quiescence},
    managed_admission::read_admitted_import,
    transition_sql::SourceIntentEffect,
};
use rusqlite::params;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection::provider) struct PublicationState {
    pub intent: ProviderTransitionIntentRecord,
    pub receipt: Option<ProviderTransitionReceiptRecord>,
}

/// Bounded sizing DATA, not a family capability. The outer complete family
/// interpreter must authenticate `remaining` and the independently rederived
/// destination floor, then require the protected root's full envelope using
/// metadata_floor(). Binding prevents reusing their sizing for another C.
pub(in crate::projection::provider) struct PublicationCapacity {
    destination_ready_digest: String,
    receipt_id: String,
    metadata_floor: u64,
    remaining: u64,
}
impl PublicationCapacity {
    pub(in crate::projection::provider) fn new(
        destination_ready: &ProviderTransitionIntentRecord,
        receipt: &ProviderTransitionReceiptRecord,
        destination_metadata_floor: u64,
        remaining: u64,
    ) -> CauseResult<Self> {
        if !(1..=1024 * 1024 * 1024).contains(&destination_metadata_floor) {
            return Err(invalid(
                "ordinary publication destination metadata floor missing or exceeds bound",
            ));
        }
        let ready = source_ready(destination_ready, receipt)?;
        let published = published(&ready)?;
        let receipt_bytes = receipt.encode()?.len() as u64
            + receipt.digest()?.len() as u64
            + receipt.owner_identity_digest.len() as u64
            + receipt.transaction_id.len() as u64
            + receipt
                .companion_id
                .as_ref()
                .map_or(0, |id| id.len() as u64);
        // Preserve the immutable funded envelope through both separate B
        // commits, conservatively charging both complete B source projections.
        let mut floor = destination_metadata_floor;
        for bytes in [normalized(&ready)?, normalized(&published)?, receipt_bytes] {
            floor = floor
                .checked_add(bytes)
                .filter(|n| *n <= 1024 * 1024 * 1024)
                .ok_or_else(|| invalid("ordinary publication metadata floor exceeds bound"))?;
        }
        if remaining < floor || remaining > ready.plan.quota_bytes {
            return Err(invalid(
                "ordinary publication funded remaining envelope insufficient",
            ));
        }
        Ok(Self {
            destination_ready_digest: destination_ready.digest()?,
            receipt_id: receipt.digest()?,
            metadata_floor: floor,
            remaining,
        })
    }
    pub(in crate::projection::provider) fn metadata_floor(&self) -> u64 {
        self.metadata_floor
    }
    fn require(
        &self,
        destination_ready: &ProviderTransitionIntentRecord,
        receipt: &ProviderTransitionReceiptRecord,
    ) -> CauseResult<()> {
        if self.destination_ready_digest != destination_ready.digest()?
            || self.receipt_id != receipt.digest()?
            || self.remaining < self.metadata_floor
            || self.remaining > destination_ready.plan.quota_bytes
        {
            return Err(invalid("ordinary publication capacity binding differs"));
        }
        Ok(())
    }
}

/// Read actual phase under complete B admission; never route Ready/Published
/// through the Staging planner. Published may retain a historical SQL frontier.
pub(in crate::projection::provider) fn read_publication_state(
    c: &Connection,
    marker: &format::Marker,
    transaction_id: &str,
    cancel: Option<&Cancellation>,
) -> CauseResult<PublicationState> {
    super::super::records::codec::transaction(transaction_id)?;
    let admitted = read_admitted_import(c, marker, cancel)?;
    let intent = admitted
        .outgoing()
        .iter()
        .find(|intent| intent.transaction_id == transaction_id)
        .ok_or_else(|| invalid("ordinary publication source row absent"))?;
    if !matches!(
        intent.state,
        TransitionState::Staging | TransitionState::Ready | TransitionState::Published
    ) {
        return Err(invalid("ordinary publication actual phase differs"));
    }
    if intent.state == TransitionState::Ready {
        require_live_source_quiescence(
            c,
            marker,
            ImportDatabaseKind::AdmittedOrdinary,
            &intent.plan.source_prefix,
            cancel,
        )?;
    }
    let receipt = match intent.receipt_id.as_deref() {
        Some(id) => Some(ProviderTransitionReceiptRecord::decode(
            &admitted.outgoing_receipt(id)?.encode()?,
        )?),
        None => None,
    };
    Ok(PublicationState {
        intent: ProviderTransitionIntentRecord::decode(&intent.encode()?)?,
        receipt,
    })
}

/// Convert the caller's actual decoded complete C Ready DATA. Change only its
/// role and source-local generation; every plan/proof/reservation/receipt stays
/// byte-exact. This function does not authenticate a physical destination.
fn source_ready(
    destination: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
) -> CauseResult<ProviderTransitionIntentRecord> {
    destination.require_receipt(receipt)?;
    if destination.role != TransitionRole::Destination
        || destination.state != TransitionState::Ready
    {
        return Err(invalid(
            "ordinary publication requires actual decoded destination Ready DATA",
        ));
    }
    let mut ready = ProviderTransitionIntentRecord::decode(&destination.encode()?)?;
    ready.role = TransitionRole::Source;
    ready.local_generation = ready.plan.source_prefix.source.version().generation;
    require_shape(&ready)?;
    ready.require_receipt(receipt)?;
    Ok(ready)
}

fn published(
    ready: &ProviderTransitionIntentRecord,
) -> CauseResult<ProviderTransitionIntentRecord> {
    let mut next = ProviderTransitionIntentRecord::decode(&ready.encode()?)?;
    next.state = TransitionState::Published;
    next.require_transition(ready)?;
    require_shape(&next)?;
    Ok(next)
}

pub(in crate::projection::provider) fn apply_ready(
    c: &Connection,
    marker: &format::Marker,
    previous_staging: &ProviderTransitionIntentRecord,
    destination_ready: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    capacity: &PublicationCapacity,
    cancel: Option<&Cancellation>,
) -> CauseResult<SourceIntentEffect> {
    let next = source_ready(destination_ready, receipt)?;
    if previous_staging.state != TransitionState::Staging {
        return Err(invalid(
            "ordinary source Ready predecessor must be Staging DATA",
        ));
    }
    apply(
        c,
        marker,
        previous_staging,
        &next,
        destination_ready,
        receipt,
        capacity,
        cancel,
    )
}

pub(in crate::projection::provider) fn apply_published(
    c: &Connection,
    marker: &format::Marker,
    previous_ready: &ProviderTransitionIntentRecord,
    destination_ready: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    capacity: &PublicationCapacity,
    cancel: Option<&Cancellation>,
) -> CauseResult<SourceIntentEffect> {
    let ready = source_ready(destination_ready, receipt)?;
    if previous_ready != &ready {
        return Err(invalid(
            "ordinary source Published predecessor differs from C Ready",
        ));
    }
    apply(
        c,
        marker,
        previous_ready,
        &published(&ready)?,
        destination_ready,
        receipt,
        capacity,
        cancel,
    )
}

fn apply(
    c: &Connection,
    marker: &format::Marker,
    previous: &ProviderTransitionIntentRecord,
    next: &ProviderTransitionIntentRecord,
    destination_ready: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    capacity: &PublicationCapacity,
    cancel: Option<&Cancellation>,
) -> CauseResult<SourceIntentEffect> {
    cancellation::check(cancel)?;
    if c.is_autocommit() || !c.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, bool>(0))? {
        return Err(invalid(
            "ordinary publication requires caller transaction and foreign keys",
        ));
    }
    capacity.require(destination_ready, receipt)?;
    require_shape(previous)?;
    next.require_transition(previous)?;
    next.require_receipt(receipt)?;
    let before = read_admitted_import(c, marker, cancel)?;
    let current = before
        .outgoing()
        .iter()
        .find(|row| row.transaction_id == next.transaction_id)
        .ok_or_else(|| invalid("ordinary publication stored predecessor absent"))?;
    // Ready is a live frontier, including a Ready replay. Reject pending or
    // unrepresented operational history before any effect. Actual Published
    // remains historical and may coexist with coherent later source work.
    if current.state == TransitionState::Ready {
        require_live_source_quiescence(
            c,
            marker,
            ImportDatabaseKind::AdmittedOrdinary,
            &current.plan.source_prefix,
            cancel,
        )?;
    }
    if current == next {
        if before.outgoing_receipt(&receipt.digest()?)? != receipt {
            return Err(invalid("ordinary publication replay receipt differs"));
        }
        // Published replay intentionally avoids the current-prefix planner.
        // The complete admission/outgoing reader already checked its bounded
        // historical frontier and exact disjoint receipt partition.
        cancellation::check(cancel)?;
        return Ok(SourceIntentEffect::AlreadyPresent);
    }
    if current != previous {
        return Err(invalid(
            "ordinary publication exact stored predecessor differs",
        ));
    }
    // Both genuine effects start from a live current frontier. Authenticate the
    // source closure before first receipt insertion, while it is still Staging.
    if next.state == TransitionState::Ready {
        write::verify_prefix(c, marker, before.record(), &next.plan.source_prefix, cancel)?;
    } else if before.outgoing_receipt(&receipt.digest()?)? != receipt {
        return Err(invalid("ordinary Published stored Ready receipt differs"));
    }
    require_capacity(c, previous, next, receipt, cancel)?;
    cancellation::check(cancel)?;
    if next.state == TransitionState::Ready && c.execute(
        "INSERT INTO projection_provider_transition_receipts(id,owner_identity_digest,transaction_id,companion_id,payload) VALUES(?1,?2,?3,?4,?5)",
        params![receipt.digest()?, receipt.owner_identity_digest, receipt.transaction_id,
            receipt.companion_id, receipt.encode()?],
    )? != 1 {
        return Err(invalid("ordinary Ready receipt exact insertion failed"));
    }
    let changed = if next.state == TransitionState::Ready {
        c.execute(
            "UPDATE projection_provider_transitions SET state='ready',receipt_id=?1,payload=?2,digest=?3 WHERE id=?4 AND role='source' AND state='staging' AND local_generation=?5 AND reservation IS ?6 AND receipt_id IS NULL AND payload=?7 AND digest=?8",
            params![next.receipt_id, next.encode()?, next.digest()?, previous.transaction_id,
                previous.local_generation, previous.reservation, previous.encode()?, previous.digest()?],
        )?
    } else {
        c.execute(
            "UPDATE projection_provider_transitions SET state='published',payload=?1,digest=?2 WHERE id=?3 AND role='source' AND state='ready' AND local_generation=?4 AND reservation IS ?5 AND receipt_id IS ?6 AND payload=?7 AND digest=?8",
            params![next.encode()?, next.digest()?, previous.transaction_id, previous.local_generation,
                previous.reservation, previous.receipt_id, previous.encode()?, previous.digest()?],
        )?
    };
    if changed != 1 {
        return Err(invalid("ordinary publication exact source CAS failed"));
    }
    write::require_post(c, marker, before.record(), next, cancel)?;
    let after = read_admitted_import(c, marker, cancel)?;
    if after.source_published() != before.source_published()
        || after.prefix() != before.prefix()
        || after.ready() != before.ready()
        || after.receipt() != before.receipt()
        || after.outgoing_receipt(&receipt.digest()?)? != receipt
    {
        return Err(invalid(
            "ordinary publication changed incoming admission or outgoing receipt",
        ));
    }
    cancellation::check(cancel)?;
    Ok(SourceIntentEffect::Updated)
}

fn normalized(record: &ProviderTransitionIntentRecord) -> CauseResult<u64> {
    let state = match record.state {
        TransitionState::Staging => "staging",
        TransitionState::Ready => "ready",
        TransitionState::Published => "published",
        _ => return Err(invalid("ordinary publication sizing phase differs")),
    };
    Ok(write::normalized_bytes(
        record,
        state,
        &record.encode()?,
        &record.digest()?,
    ))
}

fn require_capacity(
    c: &Connection,
    previous: &ProviderTransitionIntentRecord,
    next: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    let (_, bytes) = preflight_rows(c, cancel)?;
    let terminal = if next.state == TransitionState::Ready {
        published(next)?
    } else {
        ProviderTransitionIntentRecord::decode(&next.encode()?)?
    };
    let required = normalized(next)?.max(normalized(&terminal)?);
    if bytes
        .checked_sub(normalized(previous)?)
        .and_then(|n| n.checked_add(required))
        .is_none_or(|n| n > MAX_TRANSITION_BYTES)
    {
        return Err(invalid(
            "ordinary publication lacks terminal transition capacity",
        ));
    }
    if next.state == TransitionState::Ready {
        // Count and aggregate are authenticated by the complete before-reader;
        // measure only outgoing receipts, leaving ancestry's budget untouched.
        let (count, bytes): (u64, u64) = c.query_row(
            "SELECT count(*),coalesce(sum(length(CAST(r.id AS BLOB))+length(CAST(r.owner_identity_digest AS BLOB))+length(CAST(r.transaction_id AS BLOB))+coalesce(length(CAST(r.companion_id AS BLOB)),0)+length(r.payload)),0) FROM projection_provider_transitions t JOIN projection_provider_transition_receipts r ON t.receipt_id=r.id WHERE t.role='source' AND t.state IN ('ready','published')",
            [], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let new_bytes = receipt.encode()?.len() as u64
            + receipt.digest()?.len() as u64
            + receipt.owner_identity_digest.len() as u64
            + receipt.transaction_id.len() as u64
            + receipt
                .companion_id
                .as_ref()
                .map_or(0, |id| id.len() as u64);
        if count >= MAX_TRANSITIONS - 1
            || bytes
                .checked_add(new_bytes)
                .is_none_or(|n| n > receipts::MAX_OUTGOING_BYTES)
        {
            return Err(invalid(
                "ordinary publication lacks outgoing receipt capacity",
            ));
        }
    }
    Ok(())
}
