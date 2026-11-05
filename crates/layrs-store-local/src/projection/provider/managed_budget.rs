//! Pure logical DATA accounting shared by live planning and terminal reading.
//! No SQL, physical allocation, reservation mutation or earlier-phase authority.
use super::{
    cancellation,
    destination_ready::ManagedReadyClaim,
    records::{
        import::{ImportSource, ProviderTransitionReceiptRecord},
        transition::{ProviderTransitionIntentRecord, TransitionRole, TransitionState},
        Record,
    },
    recovery::error::{invalid, Result},
};
use crate::projection::{capture::CapturedPatch, selection};
use layrs_core::{
    projection::{ProjectionSelection, ProjectionSnapshot},
    scm::{canonical_json, FileManifest},
};
use layrs_vfs::Cancellation;
use serde::Serialize;
use std::collections::BTreeMap;

mod admission;
mod denial;
pub(super) use admission::{
    admission_budget, admission_cost, ordinary_admission_budget, ordinary_admission_cost,
    AdmissionBudget, AdmissionCost,
};
pub(in crate::projection::provider) use denial::DeniedCoverageCost;

const MAX_METADATA: u64 = 1073741824;

pub(super) fn add_metadata(sum: u64, bytes: u64) -> Result<u64> {
    sum.checked_add(bytes)
        .filter(|n| *n <= MAX_METADATA)
        .ok_or_else(|| invalid("Ready metadata floor overflow or exceeds 1GiB"))
}

pub(super) fn quota(
    manifests: &[FileManifest],
    image_lengths: impl IntoIterator<Item = u64>,
    metadata: u64,
) -> Result<u64> {
    if metadata > MAX_METADATA {
        return Err(invalid("managed metadata budget exceeds bound"));
    }
    let mut total = metadata
        .checked_add(65536)
        .ok_or_else(|| invalid("managed quota overflow"))?;
    for manifest in manifests {
        let payload = canonical_json(manifest)
            .map_err(|e| invalid(e.to_string()))?
            .len() as u64;
        let estimate = (manifest.chunks().len() as u64)
            .checked_mul(256)
            .and_then(|n| n.checked_add(2048))
            .ok_or_else(|| invalid("managed quota overflow"))?;
        let overhead = payload
            .checked_mul(2)
            .and_then(|n| n.checked_add(8192))
            .and_then(|n| n.checked_add(estimate))
            .ok_or_else(|| invalid("managed quota overflow"))?;
        total = total
            .checked_add(manifest.byte_length())
            .and_then(|n| n.checked_add(overhead))
            .ok_or_else(|| invalid("managed quota overflow"))?;
    }
    for bytes in image_lengths {
        total = total
            .checked_add(bytes)
            .ok_or_else(|| invalid("managed quota overflow"))?;
    }
    if total > i64::MAX as u64 {
        return Err(invalid("managed quota exceeds SQLite range"));
    }
    Ok(total)
}

pub(super) fn selection_rows_metadata(selected: &ProjectionSelection) -> Result<u64> {
    let payload = selection::encode(selected)?.len() as u64;
    payload
        .checked_add(8 + 64)
        .and_then(|row| row.checked_mul(2))
        .filter(|n| *n <= MAX_METADATA)
        .ok_or_else(|| invalid("managed selection metadata floor overflow or exceeds 1GiB"))
}

#[derive(Clone, Copy)]
pub(super) struct IntentSize {
    pub payload: u64,
    pub scalars: u64,
}
pub(super) fn intent_size(intent: &ProviderTransitionIntentRecord) -> Result<IntentSize> {
    let role = match intent.role {
        TransitionRole::Source => "source",
        TransitionRole::Destination => "destination",
    };
    let state = match intent.state {
        TransitionState::Planned => "planned",
        TransitionState::Copying => "copying",
        TransitionState::Staging => "staging",
        TransitionState::Ready => "ready",
        TransitionState::Published => "published",
        TransitionState::Cancelled => "cancelled",
    };
    Ok(IntentSize {
        payload: intent.encode()?.len() as u64,
        scalars: [
            intent.transaction_id.len() as u64,
            role.len() as u64,
            state.len() as u64,
            8,
            intent.reservation.as_ref().map_or(0, |s| s.len() as u64),
            intent.receipt_id.as_ref().map_or(0, |s| s.len() as u64),
            71,
        ]
        .into_iter()
        .try_fold(0, add_metadata)?,
    })
}

/// Sizing-only serialization projection. This is deliberately NOT a Record or
/// an intent: it cannot be passed as evidence of a historical source/Staging.
pub(super) fn original_staging_sizes(
    ready: &ProviderTransitionIntentRecord,
) -> Result<(IntentSize, IntentSize)> {
    ready.validate_fields()?;
    if ready.role != TransitionRole::Destination || ready.state != TransitionState::Ready {
        return Err(invalid("Ready metadata sizing requires destination Ready"));
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Sizing<'a> {
        schema_version: &'a str,
        transaction_id: &'a str,
        role: TransitionRole,
        state: TransitionState,
        local_generation: u64,
        plan: &'a super::records::transition::TransitionPlan,
        destination: &'a Option<super::records::transition::TransitionDestinationProof>,
        reservation: &'a Option<String>,
        receipt_id: Option<&'a str>,
    }
    let generation = match &ready.plan.source_prefix.source {
        ImportSource::Provider { version, .. } | ImportSource::Ordinary { version, .. } => {
            version.generation
        }
    };
    let size = |role, local_generation, role_len: u64| -> Result<IntentSize> {
        let bytes = canonical_json(&Sizing {
            schema_version: &ready.schema_version,
            transaction_id: &ready.transaction_id,
            role,
            state: TransitionState::Staging,
            local_generation,
            plan: &ready.plan,
            destination: &ready.destination,
            reservation: &ready.reservation,
            receipt_id: None,
        })
        .map_err(|e| invalid(e.to_string()))?;
        if bytes.len() > ProviderTransitionIntentRecord::MAX_BYTES {
            return Err(invalid("Ready staging sizing exceeds bound"));
        }
        Ok(IntentSize {
            payload: bytes.len() as u64,
            scalars: [
                ready.transaction_id.len() as u64,
                role_len,
                7,
                8,
                ready.reservation.as_ref().map_or(0, |s| s.len() as u64),
                71,
            ]
            .into_iter()
            .try_fold(0, add_metadata)?,
        })
    };
    Ok((
        size(TransitionRole::Source, generation, 6)?,
        size(TransitionRole::Destination, 0, 11)?,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn base_metadata_floor(
    copied_bytes: u64,
    observation: &ProjectionSnapshot,
    placement: &ProjectionSnapshot,
    capture: &CapturedPatch,
    plan: &super::records::transition::TransitionPlan,
    selected: &ProjectionSelection,
    source_intent: IntentSize,
    destination_intent: IntentSize,
) -> Result<u64> {
    base_metadata_floor_with_coverage(
        copied_bytes,
        observation,
        placement,
        capture,
        plan,
        selected,
        source_intent,
        destination_intent,
        None,
    )
}

/// The copied prefix already charges inherited documents. Only the current
/// transition's complete document is added here for C. B has a separate fixed
/// reservation before Planned; this floor never supplies that funding.
#[allow(clippy::too_many_arguments)]
pub(super) fn base_metadata_floor_with_coverage(
    copied_bytes: u64,
    observation: &ProjectionSnapshot,
    placement: &ProjectionSnapshot,
    capture: &CapturedPatch,
    plan: &super::records::transition::TransitionPlan,
    selected: &ProjectionSelection,
    source_intent: IntentSize,
    destination_intent: IntentSize,
    coverage: Option<&DeniedCoverageCost>,
) -> Result<u64> {
    let version = match &plan.source_prefix.source {
        ImportSource::Provider { version, .. } | ImportSource::Ordinary { version, .. } => version,
    };
    let mut minimum = denial::add_current_document(copied_bytes, plan, coverage)?;
    for bytes in [
        observation
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?
            .len() as u64,
        placement
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?
            .len() as u64,
        canonical_json(capture)
            .map_err(|e| invalid(e.to_string()))?
            .len() as u64,
        plan.source_prefix.encode()?.len() as u64,
        source_intent.payload,
        destination_intent.payload,
        selection_rows_metadata(selected)?,
        8 + plan.observation.snapshot_digest.len() as u64 + version.snapshot_digest.len() as u64,
        8 + plan.placement_digest.len() as u64 + plan.observation.snapshot_digest.len() as u64,
        8 + 8 + capture.digest.len() as u64,
        8 + 8,
        8 + plan.source_prefix_id.len() as u64,
        source_intent.scalars,
        destination_intent.scalars,
    ] {
        minimum = add_metadata(minimum, bytes)?;
    }
    Ok(minimum)
}

pub(super) fn complete_metadata_floor(
    base: u64,
    ready: &ProviderTransitionIntentRecord,
    receipt: &ProviderTransitionReceiptRecord,
    claim: &ManagedReadyClaim,
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<u64> {
    cancellation::check(cancel)?;
    if ready.role != TransitionRole::Destination || ready.state != TransitionState::Ready {
        return Err(invalid(
            "Ready metadata requires destination Ready projection",
        ));
    }
    let mut floor = add_metadata(base, 0)?;
    for bytes in [
        receipt.encode()?.len() as u64,
        receipt.digest()?.len() as u64,
        receipt.owner_identity_digest.len() as u64,
        receipt.transaction_id.len() as u64,
        receipt.companion_id.as_ref().map_or(0, |v| v.len() as u64),
        claim.root_id.len() as u64,
        claim.digest.len() as u64,
        claim.kind.len() as u64,
        claim.owner.len() as u64,
        ready.encode()?.len() as u64,
        ready.transaction_id.len() as u64,
        11,
        5,
        8,
        ready.reservation.as_ref().map_or(0, |v| v.len() as u64),
        ready.receipt_id.as_ref().map_or(0, |v| v.len() as u64),
        ready.digest()?.len() as u64,
        content_mapping_metadata(required, cancel)?,
    ] {
        floor = add_metadata(floor, bytes)?;
    }
    Ok(floor)
}

pub(super) fn content_mapping_metadata(
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<u64> {
    if required.len() > 100_000 {
        return Err(invalid("Ready content metadata count exceeds bound"));
    }
    let mut files = BTreeMap::new();
    for manifest in required {
        cancellation::check(cancel)?;
        let length = manifest.id().as_str().len() as u64;
        files
            .entry(manifest.file_id())
            .and_modify(|bytes: &mut u64| *bytes = (*bytes).max(length))
            .or_insert(length);
    }
    let mut bytes = 0;
    for (file, length) in files {
        cancellation::check(cancel)?;
        bytes = add_metadata(bytes, file.as_str().len() as u64)?;
        bytes = add_metadata(bytes, length)?;
        bytes = add_metadata(bytes, 8)?;
    }
    Ok(bytes)
}

/// Pure conservative C Ready and admission peak before physical C allocation.
/// Payload maxima are the existing closed codecs; scalar/row costs use the
/// exact available plan, observation, selection, capture and manifest union.
/// No destination FORMAT/proof, Ready record or funding capability is invented.
/// Inherited documents are already in copied_bytes; current is charged once.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn prospective_denial_metadata_floor(
    copied_bytes: u64,
    observation: &ProjectionSnapshot,
    placement: &ProjectionSnapshot,
    capture: &CapturedPatch,
    planned: &ProviderTransitionIntentRecord,
    selected: &ProjectionSelection,
    required: &[FileManifest],
    coverage: &DeniedCoverageCost,
    cancel: Option<&Cancellation>,
) -> Result<u64> {
    cancellation::check(cancel)?;
    planned.validate_fields()?;
    if planned.role != TransitionRole::Source
        || planned.state != TransitionState::Planned
        || planned.destination.is_some()
        || planned.reservation.is_some()
        || planned.receipt_id.is_some()
        || planned.plan.denial_coverage.is_none()
        || planned.plan.kind != super::records::import::ProviderImportKind::ManagedOrdinary
    {
        return Err(invalid(
            "prospective denial sizing requires unallocated v2 source Planned",
        ));
    }
    planned.encode()?;
    let plan = &planned.plan;
    super::bootstrap::require_manifest_union(plan, required)?;
    let version = plan.source_prefix.source.version();
    let observation_bytes = observation
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?;
    let placement_bytes = placement
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?;
    selected
        .validate_snapshot(observation)
        .map_err(|e| invalid(e.to_string()))?;
    selected
        .validate_snapshot(placement)
        .map_err(|e| invalid(e.to_string()))?;
    if observation.base_step_id != plan.observation.step_id
        || observation.base_root_tree_id != plan.observation.root_tree_id
        || observation.observed_policy_set_id != plan.observation.policy_set_id
        || observation.observed_authorization_epoch != plan.observation.authorization_epoch
        || placement.base_step_id != observation.base_step_id
        || placement.base_root_tree_id != observation.base_root_tree_id
        || crate::projection::database::digest(
            plan.observation.generation,
            Some(&version.snapshot_digest),
            &observation_bytes,
        ) != plan.observation.snapshot_digest
        || crate::projection::database::digest(
            plan.placement_generation,
            Some(&plan.observation.snapshot_digest),
            &placement_bytes,
        ) != plan.placement_digest
        || crate::projection::database::digest(
            plan.observation.generation,
            Some(&plan.observation.snapshot_digest),
            &selection::encode(selected)?,
        ) != plan.observation.selection_digest
        || capture.sequence != plan.capture_sequence
        || capture.previous_patch_digest != version.capture_head
        || capture.projection_generation != plan.placement_generation
        || capture.projection_digest != plan.placement_digest
        || capture.identity != plan.source_prefix.destination_identity
        || capture.digest != plan.capture_digest
        || capture.computed_digest()? != capture.digest
    {
        return Err(invalid(
            "prospective denial sizing rows differ from immutable plan",
        ));
    }
    // These are sizing projections, not ProviderTransitionIntentRecords. The
    // real Staging/Ready bodies require physical destination identities later.
    let transaction_bytes = planned.transaction_id.len() as u64;
    let reservation_bytes = planned.reservation_operation()?.len() as u64;
    let staging_size = |role_bytes| -> Result<IntentSize> {
        Ok(IntentSize {
            payload: ProviderTransitionIntentRecord::MAX_BYTES as u64,
            scalars: [transaction_bytes, role_bytes, 7, 8, reservation_bytes, 71]
                .into_iter()
                .try_fold(0, add_metadata)?,
        })
    };
    let base = base_metadata_floor_with_coverage(
        copied_bytes,
        observation,
        placement,
        capture,
        plan,
        selected,
        staging_size(6)?,
        staging_size(11)?,
        Some(coverage),
    )?;
    let mut minimum = base;
    // Match complete_metadata_floor's columns without constructing its future
    // receipt/claim/Ready objects. ExactProtection's IDs are raw 64-byte hashes;
    // this managed claim's kind and owner are fixed by the current transaction.
    for bytes in [
        ProviderTransitionReceiptRecord::MAX_BYTES as u64,
        71,
        71,
        transaction_bytes,
        plan.companion_id.as_ref().map_or(0, |id| id.len() as u64),
        64,
        64,
        "provider-import".len() as u64,
        transaction_bytes,
        ProviderTransitionIntentRecord::MAX_BYTES as u64,
        transaction_bytes,
        11,
        5,
        8,
        reservation_bytes,
        71,
        71,
        content_mapping_metadata(required, cancel)?,
    ] {
        minimum = add_metadata(minimum, bytes)?;
    }
    if plan.companion_id.is_none() {
        // No independent source-companion envelope s exists. Keep the source
        // publication peak in the plan's existing metadata allowance, matching
        // source_metadata_floor without inventing future terminal records.
        for bytes in [
            71,
            71,
            transaction_bytes,
            ProviderTransitionReceiptRecord::MAX_BYTES as u64,
        ] {
            minimum = add_metadata(minimum, bytes)?;
        }
        for state_bytes in [5, 9] {
            for bytes in [
                transaction_bytes,
                6,
                state_bytes,
                8,
                reservation_bytes,
                71,
                ProviderTransitionIntentRecord::MAX_BYTES as u64,
                71,
            ] {
                minimum = add_metadata(minimum, bytes)?;
            }
        }
    }
    cancellation::check(cancel)?;
    // Admission copies the checkout journal before deleting its operational
    // rows and adds permanent provenance. Fund this peak before Planned fixes
    // the quota; later readers must never grow an undersized reservation.
    Ok(
        admission::denial_admission_budget(minimum, placement, selected, capture, cancel)?
            .minimum_metadata(),
    )
}

#[cfg(test)]
#[path = "managed_budget/denial_start_tests.rs"]
mod denial_start_tests;
