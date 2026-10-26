//! Reserve-only family DATA for a separately authenticated managed import.
//! No SQLite opener, transaction completion, lock or content effect lives here.
use super::*;
use crate::projection::provider::records::ProviderBootstrapSource;
use layrs_core::scm::{canonical_json, FileId, FileManifest, FileManifestId};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

const MAX_MANIFESTS: usize = 100_000;
const MAX_MANIFEST_BYTES: usize = 262144;
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_CHUNK_BYTES: u64 = 1024 * 1024;

/// These are independently interpreted caller inputs, not an authority token.
/// The caller retains both private SQL proofs and the native checkpoint, and
/// rederives the whole plan and COMPLETE union from that source plus its image.
/// Comparing this union's commitment alone does not prove image completeness,
/// byte availability, fresh remote rights, or actual destination ownership.
pub(in crate::projection) struct ImportContentInputs<'a> {
    pub allocation: &'a AllocationRecord,
    pub prepared: &'a PreparedRecord,
    pub activation: &'a ProviderActivationRecord,
    pub source: &'a ProviderBootstrapSource,
    pub capture: &'a PrivateCaptureProof,
    pub source_staging: &'a ProviderTransitionIntentRecord,
    pub destination_staging: &'a ProviderTransitionIntentRecord,
    pub required: &'a [FileManifest],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ImportContentReservation {
    Absent,
    Present { remaining: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) struct ImportContentUsage {
    pub content_bytes: u64,
    pub reserved_bytes: u64,
    pub limit_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) struct ImportContentState {
    pub reservation: ImportContentReservation,
    pub usage: ImportContentUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ImportReservationEffect {
    Inserted,
    AlreadyPresent,
}

struct ReservationSpec<'a> {
    id: &'a str,
    owner: &'a str,
    operation: &'a str,
    bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequiredRow<'a> {
    file_id: &'a FileId,
    manifest_id: &'a FileManifestId,
    bytes: u64,
}

fn require_staging_pair(
    inputs: &ImportContentInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    cancellation::check(cancel)?;
    let source = inputs.source_staging;
    let destination = inputs.destination_staging;
    source.validate_fields()?;
    destination.validate_fields()?;
    if source.role != TransitionRole::Source
        || destination.role != TransitionRole::Destination
        || source.state != TransitionState::Staging
        || destination.state != TransitionState::Staging
        || source.transaction_id != destination.transaction_id
        || source.plan != destination.plan
        || source.destination != destination.destination
        || source.reservation != destination.reservation
        || source.receipt_id.is_some()
        || destination.receipt_id.is_some()
        || source.plan.kind != ProviderImportKind::ManagedOrdinary
        || inputs.source.commitment() != &inputs.allocation.source
        || inputs.source.commitment() != &inputs.activation.source
    {
        return Err(invalid(
            "managed content requires identical source/destination Staging",
        ));
    }
    let ImportSource::Provider { version, .. } = &source.plan.source_prefix.source else {
        return Err(invalid("managed content requires provider source prefix"));
    };
    if source.local_generation != version.generation || destination.local_generation != 0 {
        return Err(invalid("managed content Staging generations differ"));
    }
    let operation = source.reservation_operation()?;
    if source.reservation.as_deref() != Some(operation.as_str()) {
        return Err(invalid("managed content reservation identifier differs"));
    }
    require_union(inputs, cancel)
}

fn require_union(inputs: &ImportContentInputs<'_>, cancel: Option<&Cancellation>) -> Result<()> {
    cancellation::check(cancel)?;
    let expected = &inputs.source_staging.plan.protected_manifests;
    if inputs.required.len() > MAX_MANIFESTS
        || inputs.required.len() as u64 != expected.count
        || inputs.source.descriptors().len() > MAX_MANIFESTS
        || inputs.capture.content.len() > MAX_MANIFESTS
        || inputs.capture.patches.len() > 4096
    {
        return Err(invalid("managed content complete union count differs"));
    }
    let mut by_id = BTreeMap::new();
    let mut ordered = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    let mut minimum_charge = 65536u64;
    let mut protection_bytes = 65536usize;
    for manifest in inputs.required {
        cancellation::check(cancel)?;
        if manifest.byte_length() > MAX_FILE_BYTES
            // Existing family/provider readers use LIMIT 1025 and refuse >1024
            // chunks. This is a reader compatibility limit, not a schema DDL rule.
            || manifest.chunks().len() > 1024
            || manifest.chunks().iter().any(|chunk| chunk.byte_length() == 0 || chunk.byte_length() > MAX_CHUNK_BYTES)
        {
            return Err(invalid(
                "managed content manifest exceeds family reader bounds",
            ));
        }
        let payload = canonical_json(manifest).map_err(|error| invalid(error.to_string()))?;
        if payload.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("managed content manifest payload exceeds bound"));
        }
        // Include array separators plus a bounded frame for the eventual
        // inline protection. This is compatibility preflight, not protection.
        protection_bytes = protection_bytes
            .checked_add(payload.len())
            .and_then(|bytes| bytes.checked_add(1))
            .filter(|bytes| *bytes <= 16 * 1024 * 1024)
            .ok_or_else(|| invalid("managed content protection metadata exceeds bound"))?;
        let decoded: FileManifest =
            serde_json::from_slice(&payload).map_err(|error| invalid(error.to_string()))?;
        if decoded != *manifest {
            return Err(invalid(
                "managed content manifest canonical roundtrip differs",
            ));
        }
        let file = manifest.file_id().as_str();
        let id = manifest.id().as_str();
        if by_id.insert(id, manifest).is_some()
            || ordered.insert((file, id), manifest).is_some()
            || lengths
                .insert(file, manifest.byte_length())
                .is_some_and(|old| old != manifest.byte_length())
        {
            return Err(invalid(
                "managed content union has duplicate or conflicting representation",
            ));
        }
        let overhead = (manifest.chunks().len() as u64)
            .checked_mul(256)
            .and_then(|bytes| bytes.checked_add(2048))
            .ok_or_else(|| invalid("managed content minimum charge overflow"))?
            .max(payload.len() as u64);
        minimum_charge = minimum_charge
            .checked_add(manifest.byte_length())
            .and_then(|bytes| bytes.checked_add(overhead))
            .ok_or_else(|| invalid("managed content minimum charge overflow"))?;
    }
    if minimum_charge > inputs.source_staging.plan.quota_bytes {
        return Err(invalid(
            "managed content planned reservation undercharges manifest union",
        ));
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
    hash.update(&(ordered.len() as u64).to_be_bytes());
    for manifest in ordered.into_values() {
        cancellation::check(cancel)?;
        let bytes = canonical_json(&RequiredRow {
            file_id: manifest.file_id(),
            manifest_id: manifest.id(),
            bytes: manifest.byte_length(),
        })
        .map_err(|error| invalid(error.to_string()))?;
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(&bytes);
    }
    if expected.digest != format!("blake3:{}", hash.finalize()) {
        return Err(invalid(
            "managed content union commitment differs from plan",
        ));
    }
    for descriptor in inputs.source.descriptors() {
        cancellation::check(cancel)?;
        if by_id.get(descriptor.manifest.id().as_str()).copied() != Some(&descriptor.manifest) {
            return Err(invalid(
                "managed content union omits exact source descriptor",
            ));
        }
    }
    let mut referenced = BTreeSet::new();
    for patch in &inputs.capture.patches {
        cancellation::check(cancel)?;
        for change in &patch.changes {
            cancellation::check(cancel)?;
            if let Some(file) = &change.replacement {
                referenced.insert((file.file_id.as_str(), file.bytes));
                if referenced.len() > MAX_MANIFESTS {
                    return Err(invalid("managed content historical file set exceeds bound"));
                }
            }
        }
    }
    let mut represented = BTreeSet::new();
    for row in &inputs.capture.content {
        cancellation::check(cancel)?;
        if !referenced.contains(&(row.file_id.as_str(), row.bytes)) {
            continue;
        }
        let Some(manifest) = by_id.get(row.manifest_id.as_str()) else {
            return Err(invalid(
                "managed content union omits historical capture mapping",
            ));
        };
        if manifest.file_id().as_str() != row.file_id || manifest.byte_length() != row.bytes {
            return Err(invalid(
                "managed content historical mapping identity differs",
            ));
        }
        represented.insert((row.file_id.as_str(), row.bytes));
    }
    if represented != referenced {
        return Err(invalid(
            "managed content capture history lacks exact manifest mapping",
        ));
    }
    cancellation::check(cancel)?;
    Ok(())
}

fn require_transaction(c: &Connection, write: bool, cancel: Option<&Cancellation>) -> Result<()> {
    cancellation::check(cancel)?;
    if c.is_autocommit()
        || write && c.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, u64>(0))? != 1
    {
        return Err(invalid(
            "managed content requires caller transaction and FK-enabled writer",
        ));
    }
    if require_provider_catalog_schema(c, cancel)? != CatalogSchema::V2 {
        return Err(invalid("managed content requires exact family schema2"));
    }
    Ok(())
}

fn validate_reservation_only(
    c: &Connection,
    spec: &ReservationSpec<'_>,
    cancel: Option<&Cancellation>,
) -> Result<ImportContentState> {
    cancellation::check(cancel)?;
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
        "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
    ] {
        if c.query_row(sql, [spec.owner], |row| row.get::<_, bool>(0))? {
            return Err(invalid("managed reserve-only owner has root or legacy installation"));
        }
        cancellation::check(cancel)?;
    }
    // No typed exact-installation journal is admitted by this first slice,
    // including another import's rows. Future installation needs a new reader.
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM exact_installations)",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(invalid(
            "managed reserve-only family has unsupported exact installations",
        ));
    }
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1 AND id!=?2)",
        params![spec.owner, spec.id],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(invalid(
            "managed reserve-only owner has foreign reservation",
        ));
    }
    let oversized: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1 AND (length(CAST(owner AS BLOB))>512 OR length(CAST(operation AS BLOB))>128))", [spec.id], |row| row.get(0),
    )?;
    if oversized {
        return Err(invalid("managed reservation text exceeds bound"));
    }
    let reservation: Option<(String, String, u64)> = c
        .query_row(
            "SELECT owner,operation,remaining FROM reservations WHERE id=?1",
            [spec.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let reservation = match reservation {
        None => ImportContentReservation::Absent,
        Some((owner, operation, remaining))
            if owner == spec.owner && operation == spec.operation && remaining == spec.bytes =>
        {
            ImportContentReservation::Present { remaining }
        }
        Some(_) => {
            return Err(invalid(
                "managed reservation identity or exact charge differs",
            ))
        }
    };
    cancellation::check(cancel)?;
    let usage = ImportContentUsage {
        content_bytes: c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)", [], |row| row.get(0))?,
        reserved_bytes: c.query_row("SELECT coalesce(sum(remaining),0) FROM reservations", [], |row| row.get(0))?,
        limit_bytes: c.query_row("SELECT bytes FROM quota WHERE singleton=1", [], |row| row.get(0))?,
    };
    let used = usage
        .content_bytes
        .checked_add(usage.reserved_bytes)
        .ok_or_else(|| invalid("managed family charge overflow"))?;
    if used > usage.limit_bytes {
        return Err(invalid(
            "partial_store_quota: managed family is overcommitted",
        ));
    }
    cancellation::check(cancel)?;
    Ok(ImportContentState { reservation, usage })
}

fn insert_or_replay_reservation(
    c: &Connection,
    spec: &ReservationSpec<'_>,
    cancel: Option<&Cancellation>,
) -> Result<ImportReservationEffect> {
    let before = validate_reservation_only(c, spec, cancel)?;
    if matches!(before.reservation, ImportContentReservation::Present { .. }) {
        return Ok(ImportReservationEffect::AlreadyPresent);
    }
    if before
        .usage
        .content_bytes
        .checked_add(before.usage.reserved_bytes)
        .and_then(|bytes| bytes.checked_add(spec.bytes))
        .is_none_or(|bytes| bytes > before.usage.limit_bytes)
    {
        return Err(invalid(
            "partial_store_quota: insufficient unreserved managed capacity",
        ));
    }
    cancellation::check(cancel)?;
    if c.execute(
        "INSERT INTO reservations(id,owner,operation,remaining) VALUES(?1,?2,?3,?4)",
        params![spec.id, spec.owner, spec.operation, spec.bytes],
    )? != 1
    {
        return Err(invalid("managed reservation insertion count differs"));
    }
    let after = validate_reservation_only(c, spec, cancel)?;
    if after.reservation
        != (ImportContentReservation::Present {
            remaining: spec.bytes,
        })
    {
        return Err(invalid("managed reservation post-insert readback differs"));
    }
    Ok(ImportReservationEffect::Inserted)
}

impl FamilyAccess {
    pub(in crate::projection) fn validate_import_content(
        &self,
        c: &Connection,
        inputs: &ImportContentInputs<'_>,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportContentState> {
        require_transaction(c, false, cancel)?;
        require_staging_pair(inputs, cancel)?;
        if validate_import_base(
            self,
            c,
            inputs.allocation,
            inputs.prepared,
            inputs.activation,
            inputs.capture,
            inputs.source_staging,
            UnreservedPhase::Staging,
            cancel,
        )? != ImportRegistration::Initializing
        {
            return Err(invalid("managed content destination is not Initializing"));
        }
        let operation = inputs.source_staging.reservation_operation()?;
        let owner = inputs
            .destination_staging
            .plan
            .source_prefix
            .destination_identity
            .worktree_id
            .as_str();
        let state = validate_reservation_only(
            c,
            &ReservationSpec {
                id: &operation,
                owner,
                operation: &operation,
                bytes: inputs.source_staging.plan.quota_bytes,
            },
            cancel,
        )?;
        self.verify()?;
        cancellation::check(cancel)?;
        Ok(state)
    }

    /// SQL quota is checked here. The caller separately checks available disk
    /// space under its retained physical family before invoking this writer.
    /// Any error after INSERT requires caller rollback; COMMIT and close remain
    /// with the caller. An exact existing reservation is a no-write replay.
    pub(in crate::projection) fn reserve_import_content(
        &self,
        c: &Connection,
        inputs: &ImportContentInputs<'_>,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportReservationEffect> {
        require_transaction(c, true, cancel)?;
        self.validate_import_content(c, inputs, cancel)?;
        let operation = inputs.source_staging.reservation_operation()?;
        let owner = inputs
            .destination_staging
            .plan
            .source_prefix
            .destination_identity
            .worktree_id
            .as_str();
        let effect = insert_or_replay_reservation(
            c,
            &ReservationSpec {
                id: &operation,
                owner,
                operation: &operation,
                bytes: inputs.source_staging.plan.quota_bytes,
            },
            cancel,
        )?;
        self.validate_import_content(c, inputs, cancel)?;
        Ok(effect)
    }
}

#[cfg(test)]
#[path = "content/tests.rs"]
mod tests;
