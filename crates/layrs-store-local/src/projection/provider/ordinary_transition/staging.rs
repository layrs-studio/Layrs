//! Exact ordinary SourceCopying -> SourceStaging SQL only. The caller retains
//! the actual source image, C allocation and copied-prefix/Initializing proofs.
//! No reservation, destination mutation, COMMIT or cleanup is granted here.
use super::*;
use crate::projection::provider::{
    managed_admission::read_admitted_import, records::import::ProviderImportPrefixRecord,
    transition_sql::SourceIntentEffect,
};
use rusqlite::params;

/// A proposed row is only DATA. The writer authenticates its exact durable
/// predecessor before changing SQL; destination readers require actual Stage.
pub(in crate::projection::provider) fn proposed(
    copying: &ProviderTransitionIntentRecord,
) -> CauseResult<ProviderTransitionIntentRecord> {
    require_shape(copying)?;
    if copying.state != TransitionState::Copying {
        return Err(invalid("ordinary Staging proposal requires Copying"));
    }
    let mut next = ProviderTransitionIntentRecord::decode(&copying.encode()?)?;
    next.state = TransitionState::Staging;
    next.reservation = Some(next.reservation_operation()?);
    next.require_transition(copying)?;
    next.encode()?;
    Ok(next)
}

pub(in crate::projection::provider) fn apply_staging(
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
    if next.state != TransitionState::Staging
        || next.reservation.as_deref() != Some(next.reservation_operation()?.as_str())
    {
        return Err(invalid("ordinary source Staging phase/reservation differs"));
    }
    let previous = before
        .outgoing()
        .iter()
        .find(|row| row.transaction_id == next.transaction_id)
        .ok_or_else(|| invalid("ordinary source Staging has no stored predecessor"))?;
    next.require_transition(previous)?;
    if previous != next && previous.state != TransitionState::Copying {
        return Err(invalid(
            "ordinary source Staging predecessor is not Copying",
        ));
    }
    allocation::require_destination(c, marker, before.record(), next)?;
    write::verify_prefix(c, marker, before.record(), expected_prefix, cancel)?;
    if previous == next {
        write::require_post(c, marker, before.record(), next, cancel)?;
        return Ok(SourceIntentEffect::AlreadyPresent);
    }
    let payload = next.encode()?;
    let digest = next.digest()?;
    let old_payload = previous.encode()?;
    let old_digest = previous.digest()?;
    let (_, bytes) = preflight_rows(c, cancel)?;
    let old_bytes = write::normalized_bytes(previous, "copying", &old_payload, &old_digest);
    let new_bytes = write::normalized_bytes(next, "staging", &payload, &digest);
    if bytes
        .checked_sub(old_bytes)
        .and_then(|n| n.checked_add(new_bytes))
        .is_none_or(|n| n > MAX_TRANSITION_BYTES)
    {
        return Err(invalid(
            "ordinary source Staging exceeds transition capacity",
        ));
    }
    cancellation::check(cancel)?;
    if c.execute(
        "UPDATE projection_provider_transitions SET state='staging',reservation=?1,payload=?2,digest=?3 \
         WHERE id=?4 AND role='source' AND state='copying' AND local_generation=?5 \
         AND reservation IS NULL AND receipt_id IS NULL AND payload=?6 AND digest=?7",
        params![next.reservation, payload, digest, next.transaction_id, next.local_generation,
            old_payload, old_digest],
    )? != 1 {
        return Err(invalid("ordinary source Staging exact predecessor CAS failed"));
    }
    write::require_post(c, marker, before.record(), next, cancel)?;
    cancellation::check(cancel)?;
    Ok(SourceIntentEffect::Updated)
}
