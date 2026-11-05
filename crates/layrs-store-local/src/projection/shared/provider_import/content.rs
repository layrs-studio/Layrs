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
    pub source_context:
        Option<crate::projection::shared::source_companion::SourceCompanionFamilyContext<'a>>,
}

impl<'a> ImportContentInputs<'a> {
    fn source_inputs(&self) -> ImportSourceInputs<'_, '_> {
        ImportSourceInputs::provider_with_companions(
            self.allocation,
            self.prepared,
            self.activation,
            self.capture,
            self.source_context.as_ref(),
        )
    }
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
            .and_then(|bytes| bytes.checked_add(8192))
            .and_then(|bytes| {
                (payload.len() as u64)
                    .checked_mul(2)
                    .and_then(|payload| bytes.checked_add(payload))
            })
            .ok_or_else(|| invalid("managed content minimum charge overflow"))?;
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
    /// Protection admission is another metadata-only phase. The caller must
    /// first verify the complete CAS outside gates and retain its proof/pins.
    /// private_metadata_floor is DATA rederived from the immutable whole plan
    /// by that caller, not a measurement or an authorization supplied by SQL.
    pub(in crate::projection) fn validate_import_protection(
        &self,
        c: &Connection,
        inputs: &ImportContentInputs<'_>,
        binding: &crate::projection::shared::exact_content::ExactBinding,
        protection: &crate::projection::shared::exact_protection::ExactProtection,
        private_metadata_floor: u64,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportProtectionState> {
        self.validate_import_protection_at_member(
            c,
            inputs,
            binding,
            protection,
            private_metadata_floor,
            ImportMemberPhase::Initializing,
            cancel,
        )
    }

    pub(super) fn validate_import_protection_at_member(
        &self,
        c: &Connection,
        inputs: &ImportContentInputs<'_>,
        binding: &crate::projection::shared::exact_content::ExactBinding,
        protection: &crate::projection::shared::exact_protection::ExactProtection,
        private_metadata_floor: u64,
        member_phase: ImportMemberPhase,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportProtectionState> {
        require_transaction(c, false, cancel)?;
        require_staging_pair(inputs, cancel)?;
        require_source_envelope(c, inputs, cancel)?;
        if installation_binding(inputs)? != *binding {
            return Err(invalid(
                "exact protection binding differs from managed import",
            ));
        }
        if validate_import_base_at_member_for_source(
            self,
            c,
            &inputs.source_inputs(),
            inputs.source_staging,
            UnreservedPhase::Staging,
            member_phase,
            cancel,
        )? != Some(member_phase)
        {
            return Err(invalid(
                "exact protection owner differs from expected member phase",
            ));
        }
        let installation =
            crate::projection::shared::exact_content::validate_protected_installations(
                c,
                binding,
                inputs.required,
                protection,
                cancel,
            )?;
        protection.require_envelope(installation.remaining, private_metadata_floor)?;
        let root = protection.inspect(c, cancel)?;
        self.verify()?;
        cancellation::check(cancel)?;
        Ok(ImportProtectionState { installation, root })
    }

    /// Atomic family SQL effect only. No bytes are verified here and this does
    /// not grant Ready. Caller owns FK ON, rollback, COMMIT and positive closes;
    /// it revalidates its retained PublishedCasProof immediately around scope.
    pub(in crate::projection) fn protect_import(
        &self,
        c: &Connection,
        inputs: &ImportContentInputs<'_>,
        binding: &crate::projection::shared::exact_content::ExactBinding,
        protection: &crate::projection::shared::exact_protection::ExactProtection,
        private_metadata_floor: u64,
        cancel: Option<&Cancellation>,
    ) -> Result<crate::projection::shared::exact_protection::ProtectionEffect> {
        require_transaction(c, true, cancel)?;
        let before = self.validate_import_protection(
            c,
            inputs,
            binding,
            protection,
            private_metadata_floor,
            cancel,
        )?;
        let effect = protection.insert_or_replay(c, cancel)?;
        let after = self.validate_import_protection(
            c,
            inputs,
            binding,
            protection,
            private_metadata_floor,
            cancel,
        )?;
        if after.root != crate::projection::shared::exact_protection::ProtectionPresence::Present
            || before.installation != after.installation
        {
            return Err(invalid(
                "exact protection changed installation accounting or readback differs",
            ));
        }
        Ok(effect)
    }

    /// Installation admission is separate from the strict reserve-only reader.
    /// Private proofs, complete-plan rederivation and physical pins stay with
    /// the caller. Typed rows and durable debits are interpreted in this one
    /// catalog transaction; no byte I/O, opener, commit or cleanup is granted.
    pub(in crate::projection) fn validate_import_installation(
        &self,
        c: &Connection,
        inputs: &ImportContentInputs<'_>,
        binding: &crate::projection::shared::exact_content::ExactBinding,
        cancel: Option<&Cancellation>,
    ) -> Result<crate::projection::shared::exact_content::ExactInstallState> {
        require_transaction(c, false, cancel)?;
        require_staging_pair(inputs, cancel)?;
        require_source_envelope(c, inputs, cancel)?;
        if installation_binding(inputs)? != *binding {
            return Err(invalid(
                "exact installation binding differs from managed import",
            ));
        }
        if validate_import_base_for_source(
            self,
            c,
            &inputs.source_inputs(),
            inputs.source_staging,
            UnreservedPhase::Staging,
            cancel,
        )? != ImportRegistration::Initializing
        {
            return Err(invalid("exact installation owner is not Initializing"));
        }
        let state = crate::projection::shared::exact_content::validate_installations(
            c,
            binding,
            inputs.required,
            cancel,
        )?;
        self.verify()?;
        cancellation::check(cancel)?;
        Ok(state)
    }

    pub(in crate::projection) fn validate_import_content(
        &self,
        c: &Connection,
        inputs: &ImportContentInputs<'_>,
        cancel: Option<&Cancellation>,
    ) -> Result<ImportContentState> {
        require_transaction(c, false, cancel)?;
        require_staging_pair(inputs, cancel)?;
        require_source_envelope(c, inputs, cancel)?;
        if validate_import_base_for_source(
            self,
            c,
            &inputs.source_inputs(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) struct ImportProtectionState {
    pub installation: crate::projection::shared::exact_content::ExactInstallState,
    pub root: crate::projection::shared::exact_protection::ProtectionPresence,
}

pub(in crate::projection) fn installation_binding(
    inputs: &ImportContentInputs<'_>,
) -> Result<crate::projection::shared::exact_content::ExactBinding> {
    let source = inputs.source_staging;
    let destination = source
        .destination
        .as_ref()
        .ok_or_else(|| invalid("exact installation destination absent"))?;
    let family = &inputs.allocation.family;
    let operation = source.reservation_operation()?;
    let binding = crate::projection::shared::exact_content::ExactBinding {
        family_incarnation: family.incarnation.clone(),
        family_root_identity: family.root_identity.clone(),
        family_marker_digest: family.marker_digest.clone(),
        family_catalog_identity: family.catalog_file_identity.clone(),
        plan_digest: source.plan.digest()?,
        prefix_id: source.plan.source_prefix_id.clone(),
        destination_proof_digest: destination.digest(&source.plan)?,
        transaction_id: source.transaction_id.clone(),
        owner: destination.format.identity.worktree_id.clone(),
        database_identity: destination.private_database_identity.clone(),
        reservation: operation.clone(),
        operation,
        quota_bytes: source.plan.quota_bytes,
    };
    binding.validate()?;
    Ok(binding)
}

#[cfg(test)]
#[path = "content/tests.rs"]
mod tests;

fn require_source_envelope(
    c: &Connection,
    inputs: &ImportContentInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    if inputs.source_staging.plan.companion_id.is_none() {
        return Ok(());
    }
    let context = inputs
        .source_context
        .as_ref()
        .ok_or_else(|| invalid("companion C effects require complete source family context"))?;
    let root = context
        .pending
        .ok_or_else(|| invalid("companion C effects require separate fixed B reservation"))?;
    root.require_plan(inputs.allocation, inputs.source_staging, cancel)?;
    if root.inspect(c, cancel)?
        == crate::projection::shared::source_companion::SourceCompanionPresence::Absent
    {
        return Err(invalid("companion C effects cannot precede B reservation"));
    }
    root.require_union(inputs.required, cancel)?;
    root.require_capacity(c, cancel)
}
