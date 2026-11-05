//! Exact outgoing denial DATA inventory. Incoming prefix-v3 transport and
//! capacity admission remain separate integration work; these bytes are not
//! silently included in the existing twelve-rowset import metadata proof.
use super::*;
use crate::projection::provider::{
    cancellation, denied_coverage_storage,
    records::{codec, denied_coverage::TransitionDeniedCoverageRecord},
};

pub(super) fn require_count(count: u64) -> Result<()> {
    if count > MAX_ROWS {
        return Err(invalid("source denied coverage row count exceeds bound"));
    }
    Ok(())
}

/// Fence every key/type/length before any global FK traversal, without fetching
/// BLOB payloads. There is no separate small aggregate denial byte allowance:
/// 4096 records, each bounded by its existing 64MiB wire bound, are streamed.
/// A future capacity caller must account for the complete document relation
/// alongside all existing metadata before its effects/commit.
pub(super) fn preflight(c: &Connection, cancel: Option<&Cancellation>) -> Result<u64> {
    cancellation::check(cancel)?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_observations LIMIT 4097)",
        [],
        |row| row.get(0),
    )?;
    require_count(count)?;
    let bad: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_observations WHERE \
         typeof(digest)!='text' OR length(CAST(digest AS BLOB))!=71 \
         OR typeof(payload)!='blob' OR length(payload)<=0 OR length(payload)>?1)",
        [TransitionDeniedCoverageRecord::MAX_BYTES as u64],
        |row| row.get(0),
    )?;
    if bad {
        return Err(invalid(
            "source denied coverage SQL key/type/length differs",
        ));
    }
    let mut statement = c.prepare("SELECT digest,length(payload) FROM projection_observations")?;
    let mut rows = statement.query([])?;
    let mut observed = 0u64;
    let mut total = 0u64;
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let id: String = row.get(0)?;
        codec::raw_digest(
            id.strip_prefix("blake3:")
                .ok_or_else(|| invalid("source denied coverage digest domain differs"))?,
        )?;
        let bytes: u64 = row.get(1)?;
        total = total
            .checked_add(bytes)
            .and_then(|n| n.checked_add(id.len() as u64))
            .ok_or_else(|| invalid("source denied coverage aggregate bytes overflow"))?;
        observed = observed
            .checked_add(1)
            .filter(|n| *n <= MAX_ROWS)
            .ok_or_else(|| invalid("source denied coverage row count exceeds bound"))?;
    }
    if observed != count {
        return Err(invalid("source denied coverage inventory count changed"));
    }
    cancellation::check(cancel)?;
    Ok(count)
}

/// This source currently has no incoming denial lineage. A prefix-v3 empty
/// rowset is also unsupported until the scoped import transport can prove it.
pub(super) fn validate_intent(
    c: &Connection,
    intent: &ProviderTransitionIntentRecord,
    activated: &VersionedProjection,
    cancel: Option<&Cancellation>,
) -> Result<Option<String>> {
    cancellation::check(cancel)?;
    if intent
        .plan
        .source_prefix
        .closure
        .denial_observations
        .is_some()
    {
        return Err(invalid(
            "source inherited denied coverage prefix scope is unsupported",
        ));
    }
    let Some(reference) = &intent.plan.denial_coverage else {
        return Ok(None);
    };
    let body = denied_coverage_storage::read_required(c, reference, cancel)?;
    validate_body(&body, intent, activated, cancel)?;
    Ok(Some(reference.id.clone()))
}

/// Shared DATA checks for an actual stored body and a prospective insertion.
/// Only the actual reader may turn the former into funding proof.
pub(super) fn validate_body(
    body: &TransitionDeniedCoverageRecord,
    intent: &ProviderTransitionIntentRecord,
    activated: &VersionedProjection,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    body.require_intent(intent, cancel)?;
    let snapshot = &activated.snapshot;
    if body.path_semantics != snapshot.path_semantics
        || body.step_id != snapshot.base_step_id
        || body.root_tree_id != snapshot.base_root_tree_id
    {
        return Err(invalid(
            "source denied coverage activated namespace differs",
        ));
    }
    Ok(())
}

/// Only distinct documents owned by fully validated intents may remain. This
/// includes cancelled and published history, and requires zero for v1-only SQL.
pub(super) fn require_inventory(
    c: &Connection,
    expected: &BTreeSet<String>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    if preflight(c, cancel)? != expected.len() as u64 {
        return Err(invalid("source denied coverage exact inventory differs"));
    }
    let mut statement = c.prepare("SELECT digest FROM projection_observations")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let id: String = row.get(0)?;
        if !expected.contains(&id) {
            return Err(invalid("source denied coverage has an unowned body"));
        }
    }
    cancellation::check(cancel)
}

/// Reconstruct fixed B funding from the actual complete bodies only after the
/// enclosing interpreter validated every intent and exact document inventory.
/// These DATA still need the family reader's exact reservation check.
pub(in crate::projection::provider::validation) fn funding(
    c: &Connection,
    allocation: &AllocationRecord,
    intents: &[ProviderTransitionIntentRecord],
    cancel: Option<&Cancellation>,
) -> Result<Vec<crate::projection::shared::source_denial::SourceDeniedCoverageReservation>> {
    use crate::projection::shared::source_denial::SourceDeniedCoverageReservation;
    let mut result = Vec::new();
    let mut operations = BTreeSet::new();
    for intent in intents {
        cancellation::check(cancel)?;
        let Some(reference) = &intent.plan.denial_coverage else {
            continue;
        };
        let body = denied_coverage_storage::read_required(c, reference, cancel)?;
        let reservation = SourceDeniedCoverageReservation::new(allocation, intent, &body, cancel)?;
        if !operations.insert(reservation.operation().to_owned()) {
            return Err(invalid("source denial funding repeats an operation"));
        }
        result.push(reservation);
    }
    cancellation::check(cancel)?;
    Ok(result)
}

#[cfg(test)]
mod tests;
