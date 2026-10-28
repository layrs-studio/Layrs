//! Caller-owned transaction primitives. Errors require caller rollback; these
//! functions never commit, delete history or infer external effects from DATA.
use super::*;
use crate::projection::provider::{
    import_validation::{self, ImportDatabaseKind},
    managed_admission::read_admitted_import,
    records::{codec, import::ProviderImportPrefixRecord},
    transition_sql::SourceIntentEffect,
};
use rusqlite::params;

#[cfg(test)]
mod tests;

pub(in crate::projection::provider) fn apply_planned(
    c: &Connection,
    marker: &format::Marker,
    expected_prefix: &ProviderImportPrefixRecord,
    next: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<SourceIntentEffect> {
    cancellation::check(cancel)?;
    let before = read_admitted_import(c, marker, cancel)?;
    require_shape(next)?;
    next.require_prefix(expected_prefix)?;
    if next.state != TransitionState::Planned
        || next.transaction_id == before.record().transaction_id
    {
        return Err(invalid(
            "ordinary planned begin state or transaction differs",
        ));
    }
    let current = before
        .outgoing()
        .iter()
        .find(|row| row.transaction_id == next.transaction_id);
    if let Some(previous) = current {
        next.require_transition(previous)?;
        if previous != next {
            return Err(invalid("ordinary planned retry differs from stored intent"));
        }
    } else if before.outgoing().iter().any(|row| is_live(row.state)) {
        return Err(invalid(
            "ordinary source already has a live managed export intent",
        ));
    }
    let payload = next.encode()?;
    let digest = next.digest()?;
    if current.is_none() {
        require_new_lifecycle(c)?;
        let (count, bytes) = preflight_rows(c, cancel)?;
        require_insert_capacity(count, bytes, next, &payload, &digest)?;
    }
    // Validate all source SQL under this same retained transaction. The image
    // is declared DATA here; the ROOT owner independently rehashes its pins.
    verify_prefix(c, marker, before.record(), expected_prefix, cancel)?;
    if current.is_some() {
        require_post(c, marker, before.record(), next, cancel)?;
        return Ok(SourceIntentEffect::AlreadyPresent);
    }
    cancellation::check(cancel)?;
    let changed = c.execute(
        "INSERT INTO projection_provider_transitions(id,role,state,local_generation,reservation,receipt_id,payload,digest) SELECT ?1,'source','planned',?2,NULL,NULL,?3,?4 WHERE NOT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE role='source' AND state IN ('planned','copying','staging','ready'))",
        params![next.transaction_id, next.local_generation, payload, digest],
    )?;
    if changed != 1 {
        return Err(invalid("ordinary planned exact SQL insertion failed"));
    }
    require_post(c, marker, before.record(), next, cancel)?;
    Ok(SourceIntentEffect::Inserted)
}

fn require_insert_capacity(
    count: u64,
    bytes: u64,
    planned: &ProviderTransitionIntentRecord,
    payload: &[u8],
    digest: &str,
) -> CauseResult<()> {
    // A live intent must always retain space for its no-resource cancellation.
    // Encoding the actual terminal candidate also fences its per-record limit;
    // merely reserving four bytes would miss a MAX_BYTES boundary refusal.
    let cancelled = cancelled(planned)?;
    let terminal_payload = cancelled.encode()?;
    let terminal_digest = cancelled.digest()?;
    let reserve = normalized_bytes(planned, "planned", payload, digest).max(normalized_bytes(
        &cancelled,
        "cancelled",
        &terminal_payload,
        &terminal_digest,
    ));
    if count >= MAX_TRANSITIONS
        || bytes
            .checked_add(reserve)
            .is_none_or(|bytes| bytes > MAX_TRANSITION_BYTES)
    {
        return Err(invalid(
            "ordinary planned insertion lacks bounded cancellation capacity",
        ));
    }
    Ok(())
}

pub(super) fn normalized_bytes(
    record: &ProviderTransitionIntentRecord,
    state: &str,
    payload: &[u8],
    digest: &str,
) -> u64 {
    8 + record.transaction_id.len() as u64
        + match record.role {
            TransitionRole::Source => "source".len() as u64,
            TransitionRole::Destination => "destination".len() as u64,
        }
        + state.len() as u64
        + record
            .reservation
            .as_ref()
            .map_or(0, |value| value.len() as u64)
        + record
            .receipt_id
            .as_ref()
            .map_or(0, |value| value.len() as u64)
        + payload.len() as u64
        + digest.len() as u64
}

fn cancelled(
    previous: &ProviderTransitionIntentRecord,
) -> CauseResult<ProviderTransitionIntentRecord> {
    require_shape(previous)?;
    if previous.state != TransitionState::Planned {
        return Err(invalid(
            "ordinary cancellation requires a planned predecessor",
        ));
    }
    let mut next = ProviderTransitionIntentRecord::decode(&previous.encode()?)?;
    next.state = TransitionState::Cancelled;
    next.require_transition(previous)?;
    require_shape(&next)?;
    Ok(next)
}

fn require_new_lifecycle(c: &Connection) -> CauseResult<()> {
    let competing: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_budgets) OR EXISTS(SELECT 1 FROM projection_prejournal) OR EXISTS(SELECT 1 FROM projection_preparations WHERE status='acquiring')",
        [],
        |row| row.get(0),
    )?;
    if competing {
        return Err(invalid(
            "ordinary planned begin conflicts with retained ordinary lifecycle state",
        ));
    }
    Ok(())
}

pub(in crate::projection::provider) fn cancel_planned(
    c: &Connection,
    marker: &format::Marker,
    transaction_id: &str,
    cancel: Option<&Cancellation>,
) -> CauseResult<SourceIntentEffect> {
    cancellation::check(cancel)?;
    codec::transaction(transaction_id)?;
    let before = read_admitted_import(c, marker, cancel)?;
    let previous = before
        .outgoing()
        .iter()
        .find(|row| row.transaction_id == transaction_id)
        .ok_or_else(|| invalid("ordinary cancellation has no outgoing intent"))?;
    require_shape(previous)?;
    if previous.state == TransitionState::Cancelled {
        // A terminal retry remains valid after later ordinary generations or
        // captures. It cannot authorize a new effect against that later suffix.
        previous.require_transition(previous)?;
        require_post(c, marker, before.record(), previous, cancel)?;
        return Ok(SourceIntentEffect::AlreadyPresent);
    }
    verify_prefix(
        c,
        marker,
        before.record(),
        &previous.plan.source_prefix,
        cancel,
    )?;
    let next = cancelled(previous)?;
    cancellation::check(cancel)?;
    let changed = c.execute(
        "UPDATE projection_provider_transitions SET state='cancelled',payload=?1,digest=?2 WHERE id=?3 AND role='source' AND state='planned' AND local_generation=?4 AND reservation IS NULL AND receipt_id IS NULL AND payload=?5 AND digest=?6",
        params![next.encode()?, next.digest()?, previous.transaction_id, previous.local_generation, previous.encode()?, previous.digest()?],
    )?;
    if changed != 1 {
        return Err(invalid(
            "ordinary cancellation exact SQL compare-and-swap failed",
        ));
    }
    require_post(c, marker, before.record(), &next, cancel)?;
    Ok(SourceIntentEffect::Updated)
}

pub(super) fn verify_prefix(
    c: &Connection,
    marker: &format::Marker,
    admission: &ManagedAdmissionRecord,
    prefix: &ProviderImportPrefixRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    let binding = actual_binding(marker, admission)?;
    let image = match &prefix.source {
        ImportSource::Ordinary {
            private_schema: 9,
            version,
            ..
        } => &version.effective_image_digest,
        _ => return Err(invalid("ordinary intent requires admitted source prefix")),
    };
    import_validation::verify_source(
        c,
        marker,
        ImportDatabaseKind::AdmittedOrdinary,
        prefix,
        ImportSourceReferences::AdmittedOrdinary {
            marker,
            admission: &binding,
        },
        None,
        image,
        cancel,
    )?;
    cancellation::check(cancel)
}

pub(super) fn require_post(
    c: &Connection,
    marker: &format::Marker,
    admission: &ManagedAdmissionRecord,
    expected: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    let after = read_admitted_import(c, marker, cancel)?;
    if after.record() != admission
        || after
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == expected.transaction_id)
            != Some(expected)
    {
        return Err(invalid(
            "ordinary outgoing post-write admission or row differs",
        ));
    }
    cancellation::check(cancel)
}
