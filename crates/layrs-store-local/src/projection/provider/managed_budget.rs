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
pub(super) use admission::{admission_budget, admission_cost, AdmissionBudget, AdmissionCost};

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
    let version = match &plan.source_prefix.source {
        ImportSource::Provider { version, .. } | ImportSource::Ordinary { version, .. } => version,
    };
    let mut minimum = copied_bytes;
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
