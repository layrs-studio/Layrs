//! Ordinary source SQL boundaries around real destination allocation.
//! These primitives prove metadata only; the caller retains and rechecks the
//! genuine source owner and destination pins around the caller-owned transaction.
use super::*;
use crate::projection::provider::{
    managed_admission::read_admitted_import,
    records::{
        import::ProviderImportPrefixRecord,
        transition::{TransitionBackendKind, TransitionDestinationProof},
    },
    transition_sql::SourceIntentEffect,
};
use rusqlite::params;

#[cfg(test)]
mod tests;

/// Refuse before any filesystem effect unless every valid ordinary destination
/// proof fits this record and the complete transition inventory. No authority
/// is inferred from the synthetic values used solely to measure JSON overhead.
pub(in crate::projection::provider) fn require_allocation_capacity(
    c: &Connection,
    marker: &format::Marker,
    planned: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    cancellation::check(cancel)?;
    let admitted = read_admitted_import(c, marker, cancel)?;
    require_shape(planned)?;
    if planned.state != TransitionState::Planned
        || admitted
            .outgoing()
            .iter()
            .find(|row| row.transaction_id == planned.transaction_id)
            != Some(planned)
    {
        return Err(invalid(
            "allocation capacity requires the exact durable Planned row",
        ));
    }
    let payload = planned.encode()?;
    let overhead = ordinary_destination_max_bytes(marker)?;
    let (_, bytes) = preflight_rows(c, cancel)?;
    require_capacity(payload.len(), overhead, bytes)?;
    write::verify_prefix(
        c,
        marker,
        admitted.record(),
        &planned.plan.source_prefix,
        cancel,
    )?;
    cancellation::check(cancel)
}

fn require_capacity(payload_bytes: usize, overhead: usize, bytes: u64) -> CauseResult<()> {
    // Planned and Copying are equally long. Keeping the old `null` length in
    // this upper bound deliberately reserves four extra bytes.
    let upper = payload_bytes
        .checked_add(overhead)
        .ok_or_else(|| invalid("ordinary Copying size overflow"))?;
    if upper > ProviderTransitionIntentRecord::MAX_BYTES {
        return Err(invalid(
            "ordinary allocation lacks per-record Copying capacity",
        ));
    }
    if bytes
        .checked_add(overhead as u64)
        .is_none_or(|size| size > MAX_TRANSITION_BYTES)
    {
        return Err(invalid(
            "ordinary allocation lacks aggregate Copying capacity",
        ));
    }
    Ok(())
}

fn ordinary_destination_max_bytes(marker: &format::Marker) -> CauseResult<usize> {
    // file_identity accepts exactly three canonical u32 components (32 bytes
    // at most); digest tokens are exactly 71 ASCII bytes. Ordinary destinations
    // have no native identities. Measure the actual serialized struct's fixed
    // fields, replacing only FORMAT length with its codec-enforced maximum.
    let max_file = format!("{}:{}:{}", u32::MAX, u32::MAX, u32::MAX);
    let example = TransitionDestinationProof {
        backend: TransitionBackendKind::Ordinary,
        format: marker.clone(),
        format_file_identity: max_file.clone(),
        private_database_identity: max_file,
        family_marker_digest: format!("blake3:{}", "f".repeat(64)),
        native_control_identity_hex: None,
        native_root_identity: None,
    };
    let serialized =
        layrs_core::scm::canonical_json(&example).map_err(|error| invalid(error.to_string()))?;
    serialized
        .len()
        .checked_sub(marker.bytes()?.len())
        .and_then(|overhead| overhead.checked_add(TransitionDestinationProof::MAX_FORMAT_BYTES))
        .ok_or_else(|| invalid("ordinary destination envelope overflow"))
}

pub(in crate::projection::provider) fn apply_copying(
    c: &Connection,
    marker: &format::Marker,
    expected_prefix: &ProviderImportPrefixRecord,
    next: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<SourceIntentEffect> {
    cancellation::check(cancel)?;
    let admitted = read_admitted_import(c, marker, cancel)?;
    require_shape(next)?;
    next.require_prefix(expected_prefix)?;
    if next.state != TransitionState::Copying {
        return Err(invalid("ordinary allocation writer requires Copying"));
    }
    let previous = admitted
        .outgoing()
        .iter()
        .find(|row| row.transaction_id == next.transaction_id)
        .ok_or_else(|| invalid("ordinary Copying has no durable predecessor"))?;
    next.require_transition(previous)?;
    if previous != next && previous.state != TransitionState::Planned {
        return Err(invalid("ordinary Copying predecessor differs"));
    }
    require_destination(c, marker, admitted.record(), next)?;
    write::verify_prefix(c, marker, admitted.record(), expected_prefix, cancel)?;
    if previous == next {
        write::require_post(c, marker, admitted.record(), next, cancel)?;
        return Ok(SourceIntentEffect::AlreadyPresent);
    }
    let payload = next.encode()?;
    let digest = next.digest()?;
    let old_payload = previous.encode()?;
    let old_digest = previous.digest()?;
    let (_, bytes) = preflight_rows(c, cancel)?;
    let old_size = write::normalized_bytes(previous, "planned", &old_payload, &old_digest);
    let new_size = write::normalized_bytes(next, "copying", &payload, &digest);
    if bytes
        .checked_sub(old_size)
        .and_then(|size| size.checked_add(new_size))
        .is_none_or(|size| size > MAX_TRANSITION_BYTES)
    {
        return Err(invalid(
            "ordinary Copying exceeds transition metadata capacity",
        ));
    }
    cancellation::check(cancel)?;
    let changed = c.execute(
        "UPDATE projection_provider_transitions SET state='copying',payload=?1,digest=?2 WHERE id=?3 AND role='source' AND state='planned' AND local_generation=?4 AND reservation IS NULL AND receipt_id IS NULL AND payload=?5 AND digest=?6",
        params![payload, digest, next.transaction_id, next.local_generation, old_payload, old_digest],
    )?;
    if changed != 1 {
        return Err(invalid(
            "ordinary Copying exact SQL compare-and-swap failed",
        ));
    }
    write::require_post(c, marker, admitted.record(), next, cancel)?;
    Ok(SourceIntentEffect::Updated)
}

pub(super) fn require_destination(
    c: &Connection,
    marker: &format::Marker,
    admission: &ManagedAdmissionRecord,
    next: &ProviderTransitionIntentRecord,
) -> CauseResult<()> {
    let destination = next
        .destination
        .as_ref()
        .ok_or_else(|| invalid("ordinary Copying destination absent"))?;
    let source_family = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("ordinary source family binding absent"))?;
    let target_family = destination
        .format
        .shared
        .as_ref()
        .ok_or_else(|| invalid("ordinary target family binding absent"))?;
    if destination.format.identity == marker.identity
        || destination.format.initial_snapshot_digest != marker.initial_snapshot_digest
        || target_family.root != source_family.root
        || target_family.incarnation != source_family.incarnation
        || target_family.root_identity != source_family.root_identity
        || destination.family_marker_digest != admission.binding.family_marker_digest
        || target_family.metadata_path
            != source_family
                .root
                .join("worktrees")
                .join(
                    blake3::hash(destination.format.identity.worktree_id.as_str().as_bytes())
                        .to_hex()
                        .to_string(),
                )
                .join("metadata")
        || destination.private_database_identity == source_family.database_identity
    {
        return Err(invalid(
            "ordinary Copying destination family or initial base differs",
        ));
    }
    destination
        .format
        .validate_initial_selection(&crate::projection::selection::at(c, 0)?)?;
    Ok(())
}
