//! Bounded source transition SQL DATA. Destination and imported closure
//! authority are deliberately separate from this private row interpretation.
use super::*;
use crate::projection::provider::records::{
    Record,
    capture::ProviderCaptureState,
    import::{ImportSource, ImportSourceReferences},
    transition::{
        ProviderTransitionIntentRecord, TransitionRole, TransitionRowData, TransitionState,
    },
};
use std::collections::BTreeSet;

const MAX_ROWS: u64 = 4096;
const MAX_PAYLOAD_TOTAL: u64 = 16 * 1024 * 1024;

fn require_bounds(rows: u64, payload: u64, largest: u64) -> Result<()> {
    if rows > MAX_ROWS {
        return Err(invalid("source transition row count exceeds bound"));
    }
    if payload > MAX_PAYLOAD_TOTAL || largest > ProviderTransitionIntentRecord::MAX_BYTES as u64 {
        return Err(invalid("source transition payload exceeds bound"));
    }
    Ok(())
}

/// Prospective size of one exact source CAS inside the caller's retained
/// transaction. The caller must first run the complete source reader; this
/// arithmetic neither authenticates a native owner nor grants a write scope.
pub(in crate::projection::provider) fn preflight_source_transition_update(
    c: &Connection,
    expected: &ProviderTransitionIntentRecord,
    next: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    super::super::cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "source transition preflight requires retained transaction",
        ));
    }
    if expected.role != TransitionRole::Source || next.role != TransitionRole::Source {
        return Err(invalid(
            "source transition preflight requires source records",
        ));
    }
    next.require_transition(expected)?;
    let before = expected.encode()?;
    let after = next.encode()?;
    let exact: u64 = c.query_row(
        "SELECT count(*) FROM projection_provider_transitions WHERE id=?1 AND payload=?2 AND digest=?3",
        rusqlite::params![expected.transaction_id, before, expected.digest()?],
        |r| r.get(0),
    )?;
    if exact != 1 {
        return Err(invalid("source transition preflight actual row differs"));
    }
    let (count, total, largest): (u64, u64, u64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(payload)),0),coalesce(max(length(payload)),0) FROM projection_provider_transitions",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let prospective = total
        .checked_sub(before.len() as u64)
        .and_then(|value| value.checked_add(after.len() as u64))
        .ok_or_else(|| invalid("source transition payload overflow"))?;
    require_bounds(count, prospective, largest.max(after.len() as u64))?;
    super::super::cancellation::check(cancel)
}

fn require_pending_head(
    pending_capture: bool,
    pending_transition: bool,
    capture_count: u64,
    patch_count: u64,
) -> Result<()> {
    if pending_capture || pending_transition || capture_count != patch_count {
        return Err(invalid(
            "source transition pending head or capture state differs",
        ));
    }
    Ok(())
}

mod coverage;
mod publication;
pub(super) use coverage::funding as coverage_funding;
pub(super) use publication::{preflight_publication, validate_publication};

#[derive(Clone, Copy)]
enum View {
    Intent,
    Publication,
}

/// Fence this table before capture validation performs its global FK scan.
/// The full reader repeats the fence before materializing any row.
pub(super) fn preflight(c: &Connection, cancel: Option<&Cancellation>) -> Result<u64> {
    preflight_view(c, View::Intent, cancel)
}

fn preflight_view(c: &Connection, view: View, cancel: Option<&Cancellation>) -> Result<u64> {
    super::super::cancellation::check(cancel)?;
    coverage::preflight(c, cancel)?;
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 4097)",
        [],
        |row| row.get(0),
    )?;
    require_bounds(count, 0, 0)?;
    super::super::cancellation::check(cancel)?;
    // Check TEXT and phase projections before sorting or materializing rows.
    // Terminal rows need receipt and destination proof interpretation, which is
    // not part of this source-only mode.
    let phase = match view {
        View::Intent => {
            "state NOT IN ('planned','copying','staging','cancelled') OR receipt_id IS NOT NULL"
        }
        View::Publication => {
            "state NOT IN ('planned','copying','staging','ready','published','cancelled') OR \
            (state IN ('ready','published') AND (receipt_id IS NULL OR typeof(receipt_id)!='text' OR length(CAST(receipt_id AS BLOB))!=71)) OR \
            (state NOT IN ('ready','published') AND receipt_id IS NOT NULL)"
        }
    };
    let bad: bool = c.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM projection_provider_transitions WHERE \
         typeof(id)!='text' OR length(CAST(id AS BLOB))=0 OR length(CAST(id AS BLOB))>128 \
         OR role!='source' OR ({phase}) \
         OR typeof(local_generation)!='integer' OR local_generation<0 \
         OR (reservation IS NOT NULL AND (typeof(reservation)!='text' OR length(CAST(reservation AS BLOB))>128)) \
         OR typeof(payload)!='blob' \
         OR typeof(digest)!='text' OR length(CAST(digest AS BLOB))!=71)"),
        [], |row| row.get(0),
    )?;
    if bad {
        return Err(invalid("source transition SQL projection differs"));
    }
    let (total, largest): (u64, u64) = c.query_row(
        "SELECT coalesce(sum(length(payload)),0),coalesce(max(length(payload)),0) FROM projection_provider_transitions",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    require_bounds(count, total, largest)?;
    super::super::cancellation::check(cancel)?;
    Ok(count)
}

#[allow(clippy::too_many_arguments)]
fn require_source_version<'i>(
    intent: &'i ProviderTransitionIntentRecord,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
    source: &ProviderBootstrapSource,
    patches: &[crate::projection::capture::CapturedPatch],
) -> Result<&'i crate::projection::provider::records::import::ImportVersion> {
    intent
        .plan
        .source_prefix
        .require_references(ImportSourceReferences::Provider {
            allocation,
            prepared,
            activation,
        })?;
    let ImportSource::Provider { version, .. } = &intent.plan.source_prefix.source else {
        return Err(invalid("source transition is not from this provider"));
    };
    if version.generation != activated.generation
        || version.snapshot_digest != activated.digest
        || version.selection_digest != source.commitment().selection_digest
        || version.capture_count > patches.len() as u64
        || version.capture_head.as_deref()
            != if version.capture_count == 0 {
                None
            } else {
                patches
                    .get(version.capture_count as usize - 1)
                    .map(|p| p.digest.as_str())
            }
    {
        return Err(invalid(
            "source transition version differs from active capture history",
        ));
    }
    Ok(version)
}

/// Read-only prospective DATA admission under the caller's fully interpreted
/// source snapshot. This neither writes tentative rows nor creates actual
/// funding proof. Replays must use the complete actual reader instead.
pub(in crate::projection::provider) fn preflight_denial_start(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    before: &PrivateActiveTransitionValidation,
    next: &ProviderTransitionIntentRecord,
    body: &crate::projection::provider::records::denied_coverage::DerivedDeniedCoverage,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    super::super::cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "prospective denial start requires a retained SQL snapshot",
        ));
    }
    let payload = next.encode()?;
    if ProviderTransitionIntentRecord::decode(&payload)? != *next {
        return Err(invalid("prospective denial intent codec roundtrip differs"));
    }
    if next.role != TransitionRole::Source
        || next.state != TransitionState::Planned
        || next.destination.is_some()
        || next.reservation.is_some()
        || next.receipt_id.is_some()
        || next.plan.denial_coverage.is_none()
        || next
            .plan
            .source_prefix
            .closure
            .denial_observations
            .is_some()
        || before.transitions.iter().any(|row| {
            row.transaction_id == next.transaction_id || row.state != TransitionState::Cancelled
        })
    {
        return Err(invalid(
            "prospective denial start requires a fresh source Planned",
        ));
    }
    let a = &before.active;
    let version = require_source_version(
        next,
        allocation,
        prepared,
        &a.activation,
        &a.activated,
        &a.source,
        &before.capture.patches,
    )?;
    require_pending_head(
        before.capture.transactions.iter().any(|tx| {
            matches!(
                &tx.state,
                ProviderCaptureState::Hashing | ProviderCaptureState::Staging { .. }
            )
        }),
        false,
        version.capture_count,
        before.capture.patches.len() as u64,
    )?;
    // Exercise the exact persisted codec before funding, then apply the actual
    // reader's full intent and activated-namespace predicates. The decoded
    // copy is DATA only and is dropped before reading the retained inventory.
    {
        let encoded = body.record().encode()?;
        let decoded = crate::projection::provider::records::denied_coverage::TransitionDeniedCoverageRecord::decode(&encoded)?;
        if decoded != *body.record() {
            return Err(invalid("prospective denial body codec roundtrip differs"));
        }
    }
    coverage::validate_body(body.record(), next, &a.activated, cancel)?;
    let rows = preflight(c, cancel)?;
    let total: u64 = c.query_row(
        "SELECT coalesce(sum(length(payload)),0) FROM projection_provider_transitions",
        [],
        |row| row.get(0),
    )?;
    // This one live row grows with its destination proof, reservation and
    // receipt after funding and allocation. Reserve its full codec allowance
    // now, while refusal still precedes B and physical C effects. This is
    // logical journal headroom, not another SQL row or a second family charge.
    let live_payload_bound = ProviderTransitionIntentRecord::MAX_BYTES as u64;
    require_bounds(
        rows.checked_add(1)
            .ok_or_else(|| invalid("source transition row count overflow"))?,
        total
            .checked_add(live_payload_bound)
            .ok_or_else(|| invalid("source transition payload overflow"))?,
        live_payload_bound,
    )?;
    let observations = coverage::preflight(c, cancel)?;
    // The complete before-reader already requires every body to have an exact
    // owner. A new transaction's body cannot be adopted from that inventory.
    let reference = next.plan.denial_coverage.as_ref().unwrap();
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_observations WHERE digest=?1)",
        [&reference.id],
        |row| row.get(0),
    )?;
    if exists {
        return Err(invalid("prospective denial start body is already owned"));
    }
    coverage::require_count(
        observations
            .checked_add(1)
            .ok_or_else(|| invalid("source denied coverage row count overflow"))?,
    )?;
    super::super::cancellation::check(cancel)
}

pub(super) fn validate(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
    source: &ProviderBootstrapSource,
    capture: &PrivateCaptureProof,
    cancel: Option<&Cancellation>,
) -> Result<Vec<ProviderTransitionIntentRecord>> {
    validate_view(
        c,
        allocation,
        prepared,
        activation,
        activated,
        source,
        &capture.transactions,
        &capture.patches,
        View::Intent,
        cancel,
    )
}

fn validate_view(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    activated: &VersionedProjection,
    source: &ProviderBootstrapSource,
    transactions: &[crate::projection::provider::records::capture::ProviderCaptureTransactionRecord],
    patches: &[crate::projection::capture::CapturedPatch],
    view: View,
    cancel: Option<&Cancellation>,
) -> Result<Vec<ProviderTransitionIntentRecord>> {
    let count = preflight_view(c, view, cancel)?;
    let pending_capture = transactions.iter().any(|tx| {
        matches!(
            &tx.state,
            ProviderCaptureState::Hashing | ProviderCaptureState::Staging { .. }
        )
    });
    let mut statement = c.prepare(
        "SELECT id,role,state,local_generation,reservation,receipt_id,payload,digest \
         FROM projection_provider_transitions ORDER BY id COLLATE BINARY",
    )?;
    let mut rows = statement.query([])?;
    let mut transitions = Vec::with_capacity(count as usize);
    let mut pending = false;
    let mut coverage_ids = BTreeSet::new();
    while let Some(row) = rows.next()? {
        super::super::cancellation::check(cancel)?;
        let id: String = row.get(0)?;
        let role: String = row.get(1)?;
        let state: String = row.get(2)?;
        let generation: u64 = row.get(3)?;
        let reservation: Option<String> = row.get(4)?;
        let receipt_id: Option<String> = row.get(5)?;
        let payload: Vec<u8> = row.get(6)?;
        let digest: String = row.get(7)?;
        let intent = ProviderTransitionIntentRecord::decode(&payload)?;
        intent.require_row(TransitionRowData {
            id: &id,
            role: &role,
            state: &state,
            local_generation: generation,
            reservation: reservation.as_deref(),
            receipt_id: receipt_id.as_deref(),
            payload: &payload,
            digest: &digest,
        })?;
        if intent.role != TransitionRole::Source
            || !(matches!(
                intent.state,
                TransitionState::Planned
                    | TransitionState::Copying
                    | TransitionState::Staging
                    | TransitionState::Cancelled
            ) || matches!(view, View::Publication)
                && matches!(
                    intent.state,
                    TransitionState::Ready | TransitionState::Published
                ))
        {
            return Err(invalid("source transition phase or role differs"));
        }
        let version = require_source_version(
            &intent, allocation, prepared, activation, activated, source, patches,
        )?;
        if let Some(id) = coverage::validate_intent(c, &intent, activated, cancel)? {
            coverage_ids.insert(id);
        }
        if !matches!(
            intent.state,
            TransitionState::Cancelled | TransitionState::Published
        ) {
            require_pending_head(
                pending_capture,
                pending,
                version.capture_count,
                patches.len() as u64,
            )?;
            pending = true;
        }
        transitions.push(intent);
    }
    coverage::require_inventory(c, &coverage_ids, cancel)?;
    super::super::cancellation::check(cancel)?;
    Ok(transitions)
}

#[cfg(test)]
#[path = "transition/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "transition/preflight_tests.rs"]
mod preflight_tests;
