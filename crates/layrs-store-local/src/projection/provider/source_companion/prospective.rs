//! Prospective scalar cost DATA before C exists. Every future protocol shape
//! below is a counting frame, deliberately containing invalid `?` placeholders.
//! No Marker, destination proof, terminal intent, receipt or ExactBinding is
//! constructed, decoded or accepted as authority by this module.
use super::*;
use crate::projection::{
    ProjectionIdentity,
    provider::records::{
        Receipt,
        denied_coverage::DeniedCoverageRef,
        import::{ImportVersion, ProviderImportKind},
        transition::{TransitionPlan, TransitionRowset},
    },
};
use layrs_core::scm::{FileId, FileManifest, FileManifestId, WorktreeId};
use serde::Serialize;
use std::{collections::BTreeMap, io::Write};

const MAX_FORMAT: u64 = 64 * 1024;
const MAX_RECEIPT: u64 = 64 * 1024;
const MAX_INTENT: u64 = 256 * 1024;
const MAX_ROOT: u64 = 16 * 1024 * 1024;
const MAX_PRIVATE: u64 = 1024 * 1024 * 1024;
const FILE_ID_BYTES: usize = 32; // Three canonical decimal u32s and two colons.
const DIGEST_BYTES: usize = 71;
const SOURCE_OPERATION_BYTES: u64 = 83; // provider-companion: + 64 hex.
const ROOT_SCHEMA: &str = "layrs.source-companion-protection.v1";

/// Closed upper bound on the existing source-companion reservation equation.
/// This fixes neither an allocation nor a reservation; the actual writer must
/// still rederive and reserve its exact amount under complete family authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct ProspectiveCompanionCost {
    plan_digest: String,
    transaction_id: String,
    local_generation: u64,
    source_context: String,
    source_rows: u64,
    receipt_bytes: u64,
    ready_bytes: u64,
    published_bytes: u64,
    private_bytes: u64,
    root_bytes: u64,
    reservation_bytes: u64,
}

impl ProspectiveCompanionCost {
    /// Always preflight receipt/terminal encodings, including when this returns
    /// None for no companion. Callers must not skip this function based on the
    /// companion ID: a Planned fitting 256KiB does not prove its receipt fits
    /// 64KiB. Only ManagedOrdinary's future C layout is bounded here.
    pub(in crate::projection::provider) fn derive(
        allocation: &AllocationRecord,
        planned: &ProviderTransitionIntentRecord,
        rows: &OwnedCompanionData,
        cancel: Option<&Cancellation>,
    ) -> Result<Option<Self>> {
        super::super::cancellation::check(cancel)?;
        require_planned(planned)?;
        allocation.validate_fields()?;
        let prefix = &planned.plan.source_prefix;
        let ImportSource::Provider {
            identity,
            private_schema,
            allocation: source_allocation,
            version,
            ..
        } = &prefix.source
        else {
            return Err(invalid("prospective companion requires Provider source"));
        };
        if *private_schema != 8
            || identity != &allocation.format.identity
            || source_allocation != &allocation.data_reference()?
        {
            return Err(invalid("prospective companion source allocation differs"));
        }
        let plan_digest = planned.plan.digest()?;
        let operation = planned.reservation_operation()?;
        if operation.len() != 84 || !operation.is_ascii() {
            return Err(invalid("prospective companion C operation differs"));
        }
        let receipt = receipt_frame(planned, identity, version, &plan_digest)?;
        let receipt_bytes = measure(
            &receipt,
            MAX_RECEIPT,
            "prospective source receipt exceeds 64KiB",
        )?;
        let destination = DestinationFrame {
            backend: "ordinary",
            format: FormatBudget,
            format_file_identity: UnknownAscii::<FILE_ID_BYTES>,
            private_database_identity: UnknownAscii::<FILE_ID_BYTES>,
            family_marker_digest: &allocation.family.marker_digest,
            native_control_identity_hex: None,
            native_root_identity: None,
        };
        let intent = |state| IntentFrame {
            schema_version: &planned.schema_version,
            transaction_id: &planned.transaction_id,
            role: "source",
            state,
            local_generation: planned.local_generation,
            plan: &planned.plan,
            destination: &destination,
            reservation: &operation,
            receipt_id: UnknownAscii::<DIGEST_BYTES>,
        };
        let ready_bytes = measure(
            &intent("ready"),
            MAX_INTENT,
            "prospective source Ready exceeds 256KiB",
        )?;
        let published_bytes = measure(
            &intent("published"),
            MAX_INTENT,
            "prospective source Published exceeds 256KiB",
        )?;
        let Some(companion_id) = planned.plan.companion_id.as_deref() else {
            if rows.header().is_some() {
                return Err(invalid("prospective companion header absent from plan"));
            }
            return Ok(None);
        };
        let header = rows
            .header()
            .ok_or_else(|| invalid("prospective companion closed header absent"))?;
        header.require_prefix(prefix)?;
        if header.digest()? != companion_id
            || header.file_count != planned.plan.companion_files.count
            || header.files_digest != planned.plan.companion_files.digest
            || header.deletion_count != planned.plan.companion_deletions.count
            || header.deletions_digest != planned.plan.companion_deletions.digest
        {
            return Err(invalid(
                "prospective companion closed rows differ from plan",
            ));
        }
        let manifests =
            ordered_manifests(rows.manifests(), &planned.plan.protected_manifests, cancel)?;
        let source_rows = rows.source_row_bytes()?;
        let private_bytes = publication_bound(
            source_rows,
            planned,
            receipt_bytes,
            ready_bytes,
            published_bytes,
        )?;
        let binding = SourceBindingFrame {
            source_owner: &allocation.format.identity.worktree_id,
            source_database: &allocation.private.database_file_identity,
            source_schema: 8,
            companion_id,
            destination: ExactBindingFrame {
                family_incarnation: &allocation.family.incarnation,
                family_root_identity: &allocation.family.root_identity,
                family_marker_digest: &allocation.family.marker_digest,
                family_catalog_identity: &allocation.family.catalog_file_identity,
                plan_digest: &plan_digest,
                prefix_id: &planned.plan.source_prefix_id,
                destination_proof_digest: UnknownAscii::<DIGEST_BYTES>,
                transaction_id: &planned.transaction_id,
                owner: &prefix.destination_identity.worktree_id,
                database_identity: UnknownAscii::<FILE_ID_BYTES>,
                reservation: &operation,
                operation: &operation,
                quota_bytes: planned.plan.quota_bytes,
            },
        };
        // Stored's reservationBytes is a checked i64-sized amount. Count the
        // maximum 19 decimal digits directly in this distinct sizing frame.
        let root_bytes = measure(
            &RootFrame {
                schema: ROOT_SCHEMA,
                binding: &binding,
                private_bytes,
                reservation_bytes: i64::MAX as u64,
                manifests: &manifests,
            },
            MAX_ROOT,
            "prospective source root exceeds 16MiB",
        )?;
        let reservation_bytes = reservation_bound(
            private_bytes,
            root_bytes,
            manifests.len() as u64,
            allocation.format.identity.worktree_id.as_str().len() as u64,
            allocation.private.database_file_identity.len() as u64,
        )?;
        super::super::cancellation::check(cancel)?;
        Ok(Some(Self {
            plan_digest,
            transaction_id: planned.transaction_id.clone(),
            local_generation: planned.local_generation,
            source_context: source_context(allocation)?,
            source_rows,
            receipt_bytes,
            ready_bytes,
            published_bytes,
            private_bytes,
            root_bytes,
            reservation_bytes,
        }))
    }

    pub(in crate::projection) fn require_plan(
        &self,
        planned: &ProviderTransitionIntentRecord,
    ) -> Result<()> {
        require_planned(planned)?;
        if planned.plan.digest()? != self.plan_digest
            || planned.transaction_id != self.transaction_id
            || planned.local_generation != self.local_generation
        {
            return Err(invalid("prospective companion belongs to another Planned"));
        }
        Ok(())
    }

    /// Compare only actual closed derivations after the caller has proved C's
    /// allocation and the real source/body. This check grants no such authority.
    pub(in crate::projection::provider) fn require_actual(
        &self,
        actual: &SourceCompanionPublication<'_>,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        super::super::cancellation::check(cancel)?;
        let coverage = actual
            .coverage
            .ok_or_else(|| invalid("actual companion lacks closed denial coverage"))?;
        let b = &actual.protection.binding;
        let c = &b.destination;
        let context = context_digest(&SourceContextFrame {
            source_owner: &b.source_owner,
            source_database: &b.source_database,
            family_incarnation: &c.family_incarnation,
            family_root_identity: &c.family_root_identity,
            family_catalog_identity: &c.family_catalog_identity,
            family_marker_digest: &c.family_marker_digest,
        })?;
        if c.plan_digest != self.plan_digest
            || c.transaction_id != self.transaction_id
            || actual.ready.local_generation != self.local_generation
            || actual.ready.plan.digest()? != self.plan_digest
            || context != self.source_context
            || actual.rows.source_row_bytes()? != self.source_rows
        {
            return Err(invalid(
                "actual companion differs from prospective source plan",
            ));
        }
        coverage.record().require_intent(&actual.ready, cancel)?;
        actual.ready.require_receipt(&actual.receipt)?;
        actual.published.require_receipt(&actual.receipt)?;
        let receipt_bytes = actual.receipt.encode()?.len() as u64;
        let ready_bytes = actual.ready.encode()?.len() as u64;
        let published_bytes = actual.published.encode()?.len() as u64;
        let proof = actual
            .ready
            .destination
            .as_ref()
            .ok_or_else(|| invalid("actual companion C proof absent"))?;
        if proof.format_file_identity.len() > FILE_ID_BYTES
            || proof.private_database_identity.len() > FILE_ID_BYTES
            || !proof.format_file_identity.is_ascii()
            || !proof.private_database_identity.is_ascii()
            || proof.format.bytes()?.len() as u64 > MAX_FORMAT
            || receipt_bytes != self.receipt_bytes
            || ready_bytes > self.ready_bytes
            || published_bytes > self.published_bytes
            || actual.metadata > self.private_bytes
            || actual.protection.reservation_bytes() > self.reservation_bytes
        {
            return Err(invalid(
                "actual companion exceeds prospective encoding envelope",
            ));
        }
        let manifests = ordered_manifests(
            actual.rows.manifests(),
            &actual.ready.plan.protected_manifests,
            cancel,
        )?;
        let root_bytes = measure(
            &RootFrame {
                schema: ROOT_SCHEMA,
                binding: b,
                private_bytes: actual.metadata,
                reservation_bytes: actual.protection.reservation_bytes(),
                manifests: &manifests,
            },
            MAX_ROOT,
            "actual source root exceeds 16MiB",
        )?;
        if root_bytes > self.root_bytes {
            return Err(invalid(
                "actual companion root exceeds prospective envelope",
            ));
        }
        actual.protection.require_envelope(publication_bytes(
            self.source_rows,
            &actual.ready,
            &actual.published,
            &actual.receipt,
        )?)?;
        super::super::cancellation::check(cancel)
    }

    pub(in crate::projection::provider) fn plan_digest(&self) -> &str {
        &self.plan_digest
    }
    pub(in crate::projection::provider) fn receipt_bytes(&self) -> u64 {
        self.receipt_bytes
    }
    pub(in crate::projection::provider) fn ready_bytes(&self) -> u64 {
        self.ready_bytes
    }
    pub(in crate::projection::provider) fn published_bytes(&self) -> u64 {
        self.published_bytes
    }
    pub(in crate::projection::provider) fn private_bytes(&self) -> u64 {
        self.private_bytes
    }
    pub(in crate::projection::provider) fn root_bytes(&self) -> u64 {
        self.root_bytes
    }
    pub(in crate::projection) fn reservation_bytes(&self) -> u64 {
        self.reservation_bytes
    }
}

fn require_planned(planned: &ProviderTransitionIntentRecord) -> Result<()> {
    planned.encode()?;
    if planned.role != TransitionRole::Source
        || planned.state != TransitionState::Planned
        || planned.plan.denial_coverage.is_none()
        || planned.destination.is_some()
        || planned.reservation.is_some()
        || planned.receipt_id.is_some()
        || planned.plan.kind != ProviderImportKind::ManagedOrdinary
    {
        return Err(invalid(
            "prospective companion requires unallocated Ordinary v2 source Planned",
        ));
    }
    Ok(())
}

fn add_cost(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| invalid("prospective companion cost overflow"))
}

fn publication_bound(
    base: u64,
    planned: &ProviderTransitionIntentRecord,
    receipt: u64,
    ready: u64,
    published: u64,
) -> Result<u64> {
    // Identical scalar equation to publication_bytes, using counted frames in
    // place of protocol values which cannot yet be constructed.
    let mut total = add_cost(
        base,
        64 + 64 + "provider-companion".len() as u64 + planned.transaction_id.len() as u64,
    )?;
    for n in [
        71,
        71,
        planned.transaction_id.len() as u64,
        planned
            .plan
            .companion_id
            .as_ref()
            .map_or(0, |s| s.len() as u64),
        receipt,
    ] {
        total = add_cost(total, n)?;
    }
    for (payload, state) in [(ready, "ready"), (published, "published")] {
        for n in [
            planned.transaction_id.len() as u64,
            6,
            state.len() as u64,
            8,
            84,
            71,
            payload,
            71,
        ] {
            total = add_cost(total, n)?;
        }
    }
    if total > MAX_PRIVATE {
        return Err(invalid("prospective source private rows exceed 1GiB"));
    }
    Ok(total)
}

fn reservation_bound(
    private: u64,
    root: u64,
    manifests: u64,
    owner: u64,
    database: u64,
) -> Result<u64> {
    let root_scalars = add_cost(
        add_cost(add_cost(64, owner)?, database)?,
        SOURCE_OPERATION_BYTES + 64,
    )?;
    let index = manifests
        .checked_mul(64 + 71)
        .ok_or_else(|| invalid("prospective source index overflow"))?;
    let reservation_scalars = add_cost(SOURCE_OPERATION_BYTES * 2 + 8, owner)?;
    add_cost(
        add_cost(add_cost(add_cost(private, root)?, root_scalars)?, index)?,
        reservation_scalars,
    )
}

fn ordered_manifests<'a>(
    manifests: &'a [FileManifest],
    commitment: &TransitionRowset,
    cancel: Option<&Cancellation>,
) -> Result<Vec<&'a FileManifest>> {
    if manifests.len() > 100000 {
        return Err(invalid("prospective companion union exceeds bound"));
    }
    let mut ordered = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    for m in manifests {
        super::super::cancellation::check(cancel)?;
        if m.chunks().len() > 1024
            || m.byte_length() > 1024 * 1024 * 1024
            || m.chunks()
                .iter()
                .any(|c| c.byte_length() == 0 || c.byte_length() > 1048576)
            || ordered.insert(m.id(), m).is_some()
            || lengths
                .insert(m.file_id(), m.byte_length())
                .is_some_and(|old| old != m.byte_length())
        {
            return Err(invalid(
                "prospective companion union repeats or exceeds bound",
            ));
        }
        crate::projection::shared::exact_content::canonical_manifest_bytes(m)?;
    }
    let mut by_file = BTreeMap::new();
    for m in ordered.values() {
        by_file.insert((m.file_id(), m.id()), *m);
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
    hash.update(&(by_file.len() as u64).to_be_bytes());
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Row<'a> {
        file_id: &'a FileId,
        manifest_id: &'a FileManifestId,
        bytes: u64,
    }
    for m in by_file.into_values() {
        super::super::cancellation::check(cancel)?;
        let bytes = layrs_core::scm::canonical_json(&Row {
            file_id: m.file_id(),
            manifest_id: m.id(),
            bytes: m.byte_length(),
        })
        .map_err(|e| invalid(e.to_string()))?;
        hash.update(&(bytes.len() as u64).to_be_bytes());
        hash.update(&bytes);
    }
    if commitment.count != ordered.len() as u64
        || commitment.digest != format!("blake3:{}", hash.finalize())
    {
        return Err(invalid("prospective companion union differs from plan"));
    }
    Ok(ordered.into_values().collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceContextFrame<'a> {
    source_owner: &'a WorktreeId,
    source_database: &'a str,
    family_incarnation: &'a str,
    family_root_identity: &'a str,
    family_catalog_identity: &'a str,
    family_marker_digest: &'a str,
}
fn context_digest(frame: &SourceContextFrame<'_>) -> Result<String> {
    let bytes = layrs_core::scm::canonical_json(frame).map_err(|e| invalid(e.to_string()))?;
    Ok(format!("blake3:{}", blake3::hash(&bytes)))
}
fn source_context(a: &AllocationRecord) -> Result<String> {
    context_digest(&SourceContextFrame {
        source_owner: &a.format.identity.worktree_id,
        source_database: &a.private.database_file_identity,
        family_incarnation: &a.family.incarnation,
        family_root_identity: &a.family.root_identity,
        family_catalog_identity: &a.family.catalog_file_identity,
        family_marker_digest: &a.family.marker_digest,
    })
}

// '?' is ASCII without JSON escaping and is deliberately not a valid digest,
// file identity or operation. Its encoded width equals any allowed unknown of
// the same width, without constructing an authoritative protocol value.
#[derive(Clone, Copy)]
struct UnknownAscii<const N: usize>;
impl<const N: usize> Serialize for UnknownAscii<N> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let bytes = [b'?'; N];
        s.serialize_str(std::str::from_utf8(&bytes).map_err(serde::ser::Error::custom)?)
    }
}
struct FormatBudget;
impl Serialize for FormatBudget {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        // Include the JSON string's two quotes in FORMAT's entire 64KiB
        // budget. This invalid string is never decoded as a Marker.
        s.serialize_str(&"?".repeat(MAX_FORMAT as usize - 2))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DestinationFrame<'a> {
    backend: &'static str,
    format: FormatBudget,
    format_file_identity: UnknownAscii<FILE_ID_BYTES>,
    private_database_identity: UnknownAscii<FILE_ID_BYTES>,
    family_marker_digest: &'a str,
    native_control_identity_hex: Option<()>,
    native_root_identity: Option<()>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IntentFrame<'a> {
    schema_version: &'a str,
    transaction_id: &'a str,
    role: &'static str,
    state: &'static str,
    local_generation: u64,
    plan: &'a TransitionPlan,
    destination: &'a DestinationFrame<'a>,
    reservation: &'a str,
    receipt_id: UnknownAscii<DIGEST_BYTES>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptFrame<'a> {
    schema_version: &'static str,
    owner_identity_digest: UnknownAscii<DIGEST_BYTES>,
    transaction_id: &'a str,
    transition_plan_digest: &'a str,
    destination_proof_digest: UnknownAscii<DIGEST_BYTES>,
    source_prefix_id: &'a str,
    source_prefix: &'a ProviderImportPrefixRecord,
    parent_receipt_id: &'a Option<String>,
    source_identity: &'a ProjectionIdentity,
    destination_identity: &'a ProjectionIdentity,
    imported_max_generation: u64,
    destination_observation_generation: u64,
    destination_observation_digest: &'a str,
    destination_placement_generation: u64,
    destination_placement_digest: &'a str,
    imported_capture_count: u64,
    destination_capture_sequence: u64,
    destination_capture_digest: &'a str,
    protected_manifests: &'a TransitionRowset,
    content_mappings: &'a ImportRowset,
    companion_id: &'a Option<String>,
    companion_files: &'a TransitionRowset,
    companion_deletions: &'a TransitionRowset,
    denial_coverage: &'a DeniedCoverageRef,
}
fn receipt_frame<'a>(
    planned: &'a ProviderTransitionIntentRecord,
    identity: &'a ProjectionIdentity,
    version: &'a ImportVersion,
    plan_digest: &'a str,
) -> Result<ReceiptFrame<'a>> {
    let p = &planned.plan;
    Ok(ReceiptFrame {
        schema_version: "layrs.projection-provider-transition-receipt.v2",
        owner_identity_digest: UnknownAscii,
        transaction_id: &planned.transaction_id,
        transition_plan_digest: plan_digest,
        destination_proof_digest: UnknownAscii,
        source_prefix_id: &p.source_prefix_id,
        source_prefix: &p.source_prefix,
        parent_receipt_id: &p.source_prefix.ancestry_root_receipt,
        source_identity: identity,
        destination_identity: &p.source_prefix.destination_identity,
        imported_max_generation: version.generation,
        destination_observation_generation: p.observation.generation,
        destination_observation_digest: &p.observation.snapshot_digest,
        destination_placement_generation: p.placement_generation,
        destination_placement_digest: &p.placement_digest,
        imported_capture_count: version.capture_count,
        destination_capture_sequence: p.capture_sequence,
        destination_capture_digest: &p.capture_digest,
        protected_manifests: &p.protected_manifests,
        content_mappings: &p.source_prefix.closure.content_mappings,
        companion_id: &p.companion_id,
        companion_files: &p.companion_files,
        companion_deletions: &p.companion_deletions,
        denial_coverage: p
            .denial_coverage
            .as_ref()
            .ok_or_else(|| invalid("prospective receipt requires denial reference"))?,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExactBindingFrame<'a> {
    family_incarnation: &'a str,
    family_root_identity: &'a str,
    family_marker_digest: &'a str,
    family_catalog_identity: &'a str,
    plan_digest: &'a str,
    prefix_id: &'a str,
    destination_proof_digest: UnknownAscii<DIGEST_BYTES>,
    transaction_id: &'a str,
    owner: &'a WorktreeId,
    database_identity: UnknownAscii<FILE_ID_BYTES>,
    reservation: &'a str,
    operation: &'a str,
    quota_bytes: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceBindingFrame<'a> {
    source_owner: &'a WorktreeId,
    source_database: &'a str,
    source_schema: u32,
    companion_id: &'a str,
    destination: ExactBindingFrame<'a>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RootFrame<'a, B: Serialize> {
    schema: &'static str,
    binding: &'a B,
    private_bytes: u64,
    reservation_bytes: u64,
    manifests: &'a [&'a FileManifest],
}

struct Count {
    bytes: u64,
    limit: u64,
    error: &'static str,
}
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other(self.error))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn measure(value: &impl Serialize, limit: u64, error: &'static str) -> Result<u64> {
    // JCS uses serde_json's string escaping; key ordering changes no width.
    // Known protocol integers already passed Planned's lossless canonical
    // roundtrip and are <= i64::MAX (plain decimal below JCS's 1e21 cutoff).
    // New byte costs are <=1GiB; the sole 19-digit slot is sizing DATA only.
    let mut count = Count {
        bytes: 0,
        limit,
        error,
    };
    serde_json::to_writer(&mut count, value).map_err(|_| invalid(error))?;
    Ok(count.bytes)
}

#[cfg(test)]
mod tests;
