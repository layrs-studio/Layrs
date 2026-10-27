//! Bounded source transition SQL DATA. Destination and imported closure
//! authority are deliberately separate from this private row interpretation.
use super::*;
use crate::projection::provider::records::{
    capture::ProviderCaptureState,
    import::{ImportSource, ImportSourceReferences},
    transition::{
        ProviderTransitionIntentRecord, TransitionRole, TransitionRowData, TransitionState,
    },
    Record,
};

const MAX_ROWS: u64 = 4096;
const MAX_PAYLOAD_TOTAL: u64 = 16 * 1024 * 1024;

mod publication;
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
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 4097)",
        [],
        |row| row.get(0),
    )?;
    if count > MAX_ROWS {
        return Err(invalid("source transition row count exceeds bound"));
    }
    super::super::cancellation::check(cancel)?;
    // Check TEXT and phase projections before sorting or materializing rows.
    // Terminal rows need receipt and destination proof interpretation, which is
    // not part of this source-only mode.
    let phase = match view {
        View::Intent => "state NOT IN ('planned','copying','staging','cancelled') OR receipt_id IS NOT NULL",
        View::Publication => "state NOT IN ('planned','copying','staging','ready','published','cancelled') OR \
            (state IN ('ready','published') AND (receipt_id IS NULL OR typeof(receipt_id)!='text' OR length(CAST(receipt_id AS BLOB))!=71)) OR \
            (state NOT IN ('ready','published') AND receipt_id IS NOT NULL)",
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
    if total > MAX_PAYLOAD_TOTAL || largest > ProviderTransitionIntentRecord::MAX_BYTES as u64 {
        return Err(invalid("source transition payload exceeds bound"));
    }
    super::super::cancellation::check(cancel)?;
    Ok(count)
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
        capture,
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
    capture: &PrivateCaptureProof,
    view: View,
    cancel: Option<&Cancellation>,
) -> Result<Vec<ProviderTransitionIntentRecord>> {
    let count = preflight_view(c, view, cancel)?;
    let pending_capture = capture.transactions.iter().any(|tx| {
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
            || version.capture_count > capture.patches.len() as u64
            || version.capture_head.as_deref()
                != if version.capture_count == 0 {
                    None
                } else {
                    capture
                        .patches
                        .get(version.capture_count as usize - 1)
                        .map(|p| p.digest.as_str())
                }
        {
            return Err(invalid(
                "source transition version differs from active capture history",
            ));
        }
        if !matches!(
            intent.state,
            TransitionState::Cancelled | TransitionState::Published
        ) {
            if pending_capture || pending || version.capture_count != capture.patches.len() as u64 {
                return Err(invalid(
                    "source transition pending head or capture state differs",
                ));
            }
            pending = true;
        }
        transitions.push(intent);
    }
    super::super::cancellation::check(cancel)?;
    Ok(transitions)
}

#[cfg(test)]
#[path = "transition/tests.rs"]
mod tests;
