//! Destination Copying -> Staging under the actual durable SourceStaging.
//! A legitimate gap exists after the source commit and before the destination
//! commit. Validate that gap directly; never invent an earlier source proof.
//! Caller retains the source transaction and C pins and owns commit/rollback.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum StageEffect {
    Updated,
    AlreadyPresent,
}

/// Authenticate the exact C phase against a fresh actual SourceStaging row.
/// Both variants contain only the fully copied prefix, without any local tail.
pub(in crate::projection::provider) fn read_stageable(
    c: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<TransitionState> {
    // require_phase is the shared ordinary source interpreter; the phase is
    // an expectation, and must match the actual durable source row exactly.
    let mut expected = source.require_phase(c, marker, TransitionState::Staging, cancel)?;
    // Do not inspect an arbitrary table or view before authenticating exact C8
    // schema and the bounded singleton transition inventory.
    import_validation::require_schema(c, marker, ImportDatabaseKind::Provider, cancel)?;
    require_inventory(c, cancel)?;
    let state: String = c.query_row(
        "SELECT state FROM projection_provider_transitions WHERE id=?1 AND role='destination' AND length(CAST(state AS BLOB))<=16",
        [&expected.transaction_id], |row| row.get(0),
    )?;
    let phase = match state.as_str() {
        "copying" => {
            expected.state = TransitionState::Copying;
            expected.reservation = None;
            TransitionState::Copying
        }
        "staging" => TransitionState::Staging,
        _ => return Err(invalid("ordinary destination is not Copying or Staging")),
    };
    expected.validate_fields()?;
    // All schema, identity, rowsets, operational tables and exact normalized
    // intent fields are inspected, including populated restoration history.
    read_copied_parts(c, marker, source, &expected, cancel)?;
    cancellation::check(cancel)?;
    Ok(phase)
}

pub(in crate::projection::provider) fn apply_staging(
    c: &Connection,
    marker: &format::Marker,
    source: &OrdinaryCopyInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<StageEffect> {
    cancellation::check(cancel)?;
    require_foreign_keys(c)?;
    let next = source.require_phase(c, marker, TransitionState::Staging, cancel)?;
    match read_stageable(c, marker, source, cancel)? {
        TransitionState::Staging => return Ok(StageEffect::AlreadyPresent),
        TransitionState::Copying => {}
        _ => unreachable!("typed stageable reader"),
    }
    let mut previous = ProviderTransitionIntentRecord::decode(&next.encode()?)?;
    previous.state = TransitionState::Copying;
    previous.reservation = None;
    next.require_transition(&previous)?;
    cancellation::check(cancel)?;
    let changed = c.execute(
        "UPDATE projection_provider_transitions SET state='staging',reservation=?1,payload=?2,digest=?3 WHERE id=?4 AND role='destination' AND state='copying' AND local_generation=0 AND reservation IS NULL AND receipt_id IS NULL AND payload=?5 AND digest=?6",
        params![next.reservation, next.encode()?, next.digest()?, next.transaction_id,
            previous.encode()?, previous.digest()?],
    )?;
    if changed != 1 {
        return Err(invalid("ordinary destination Staging exact SQL CAS failed"));
    }
    if read_stageable(c, marker, source, cancel)? != TransitionState::Staging {
        return Err(invalid("ordinary destination Staging poststate differs"));
    }
    cancellation::check(cancel)?;
    Ok(StageEffect::Updated)
}
