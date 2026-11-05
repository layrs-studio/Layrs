//! Family publication SQL DATA only. Driver authenticates actual B9 Staging,
//! actual private C8 Ready/receipt, image, complete journal/floor and pins. It
//! retains those snapshots/pins around family exclusive Immediate, COMMIT and
//! positive close, then reauthenticates before ACK. No private SQL is read here.
use super::*;
use crate::projection::{
    provider::records::{
        Record,
        import::{
            ImportSource, ImportSourceReferences, ProviderImportKind,
            ProviderTransitionReceiptRecord,
        },
        transition::{
            ProviderTransitionIntentRecord, TransitionBackendKind, TransitionRole, TransitionState,
        },
    },
    shared::provider_access::exact_path,
};
use rusqlite::OptionalExtension;

/// All borrowed values are descriptive DATA. Ready is the actual decoded C
/// row authenticated externally, never a Staging row used as actual C state.
/// This module grants no private, physical, owner or durable-close authority.
pub(in crate::projection) struct ExportPublicationInputs<'a> {
    pub target: &'a ExportReserveOnlyTarget,
    pub destination_ready: &'a ProviderTransitionIntentRecord,
    pub receipt: &'a ProviderTransitionReceiptRecord,
    pub required: &'a [FileManifest],
    pub protection: &'a ExactProtection,
    /// Complete actual Ready floor, excluding checkout journal.
    pub ready_metadata_floor: u64,
    pub checkout_journal_maximum_bytes: u64,
    pub checkout_file_bytes: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ExportPublicationMember {
    Initializing,
    Ready,
}
impl ExportPublicationMember {
    pub(super) fn sql(self) -> &'static str {
        match self {
            Self::Initializing => "initializing",
            Self::Ready => "ready",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) struct ExportPublicationState {
    pub member: ExportPublicationMember,
    pub protection: ExportProtectionState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ExportPublicationEffect {
    Updated,
    AlreadyReady,
}

fn require_ready(
    retained: &AdmittedFamilyContext,
    inputs: &ExportPublicationInputs<'_>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    cancellation::check(cancel)?;
    let ready = inputs.destination_ready;
    // Bounded canonical record encodings, not merely typed field shape.
    ready.encode()?;
    inputs.receipt.encode()?;
    let binding = inputs.target.binding();
    let proof = ready
        .destination
        .as_ref()
        .ok_or_else(|| invalid("export publication destination proof absent"))?;
    let prefix = &ready.plan.source_prefix;
    let ImportSource::Ordinary {
        private_schema: 9 | 10,
        admission: Some(admission),
        ..
    } = &prefix.source
    else {
        return Err(invalid("export publication requires admitted ordinary B9"));
    };
    prefix.require_references(ImportSourceReferences::AdmittedOrdinary {
        marker: retained.marker(),
        admission,
    })?;
    if admission.admission_digest != retained.admission_digest()
        || admission.incoming_prefix_id != retained.binding().prefix_id
        || admission.database_identity != retained.binding().database_identity
        || ready.role != TransitionRole::Destination
        || ready.state != TransitionState::Ready
        || ready.plan.kind != ProviderImportKind::ManagedOrdinary
        || ready.plan.companion_id.is_some()
        || ready.plan.companion_files.count != 0
        || ready.plan.companion_deletions.count != 0
        || ready.transaction_id != binding.transaction_id
        || ready.plan.digest()? != binding.plan_digest
        || ready.plan.source_prefix_id != binding.prefix_id
        || ready.plan.quota_bytes != binding.quota_bytes
        || ready.reservation.as_deref() != Some(binding.reservation.as_str())
        || ready.reservation_operation()? != binding.operation
        || proof.backend != TransitionBackendKind::Ordinary
        || proof.format != *inputs.target.marker()
        || proof.private_database_identity != binding.database_identity
        || proof.family_marker_digest != binding.family_marker_digest
        || proof.digest(&ready.plan)? != binding.destination_proof_digest
    {
        return Err(invalid(
            "export publication Ready differs from exact ordinary target",
        ));
    }
    ready.require_receipt(inputs.receipt)?;
    inputs
        .protection
        .require_binding(binding, inputs.required, cancel)?;
    cancellation::check(cancel)
}

fn member_phase(
    c: &Connection,
    target: &ExportReserveOnlyTarget,
    cancel: Option<&Cancellation>,
) -> Result<ExportPublicationMember> {
    cancellation::check(cancel)?;
    if c.is_autocommit()
        || super::super::provider_access::require_provider_catalog_schema(c, cancel)?
            != super::super::CatalogSchema::V2
    {
        return Err(invalid(
            "export publication requires schema2 caller transaction",
        ));
    }
    let owner = target.binding().owner.as_str();
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND length(CAST(state AS BLOB))>12)",
        [owner],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(invalid("export publication member phase exceeds bound"));
    }
    let state: Option<String> = c
        .query_row("SELECT state FROM worktrees WHERE id=?1", [owner], |r| {
            r.get(0)
        })
        .optional()?;
    match state.as_deref() {
        Some("initializing") => Ok(ExportPublicationMember::Initializing),
        Some("ready") => Ok(ExportPublicationMember::Ready),
        _ => Err(invalid(
            "export publication exact member absent or phase differs",
        )),
    }
}
#[allow(clippy::too_many_arguments)]
fn publication_parts(
    retained: &AdmittedFamilyContext,
    c: &Connection,
    family_marker: &[u8],
    target: &ExportReserveOnlyTarget,
    required: &[FileManifest],
    protection: &ExactProtection,
    member: ExportPublicationMember,
    cancel: Option<&Cancellation>,
) -> Result<CatalogParts> {
    retained.validate_catalog_parts(
        c,
        family_marker,
        Some(ExportCatalogTarget::Publication {
            target,
            required,
            protection,
            member,
        }),
        cancel,
    )
}
fn compare_and_publish(c: &Connection, target: &ExportReserveOnlyTarget) -> Result<()> {
    if c.is_autocommit() || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1 {
        return Err(invalid(
            "export publication writer requires FK ON caller transaction",
        ));
    }
    let marker = target.marker();
    let shared = marker
        .shared
        .as_ref()
        .ok_or_else(|| invalid("export publication shared FORMAT absent"))?;
    if c.execute(
        "UPDATE worktrees SET state='ready' WHERE id=?1 AND metadata_path=?2 AND database_identity=?3 AND marker=?4 AND state='initializing'",
        params![target.binding().owner.as_str(), exact_path(&shared.metadata_path)?,
            target.binding().database_identity, marker.bytes()?],
    )? != 1 { return Err(invalid("export publication exact member CAS differs")); }
    Ok(())
}
impl AdmittedFamilyContext {
    /// Same complete retained Published+C Protected walk at one bounded exact
    /// C registry phase. No ordinary Lease, private admission or release.
    pub(in crate::projection) fn read_export_publication(
        &self,
        c: &Connection,
        family_marker: &[u8],
        inputs: &ExportPublicationInputs<'_>,
        cancel: Option<&Cancellation>,
    ) -> Result<(AdmittedCatalogProof, ExportPublicationState)> {
        require_ready(self, inputs, cancel)?;
        let member = member_phase(c, inputs.target, cancel)?;
        let parts = publication_parts(
            self,
            c,
            family_marker,
            inputs.target,
            inputs.required,
            inputs.protection,
            member,
            cancel,
        )?;
        let protection = parts
            .protection
            .ok_or_else(|| invalid("export publication protection state absent"))?;
        inputs.target.require_publication_envelope(
            protection,
            inputs.protection,
            inputs.ready_metadata_floor,
            inputs.checkout_journal_maximum_bytes,
            inputs.checkout_file_bytes,
        )?;
        cancellation::check(cancel)?;
        Ok((parts.proof, ExportPublicationState { member, protection }))
    }
    /// C state CAS only. Full Ready replay returns before FK/writer checks.
    /// Caller rolls back every post-mutation error, COMMITs and closes positively
    /// before public ACK. Actual B stays Staging; no private admission is added.
    pub(in crate::projection) fn publish_export_ready(
        &self,
        c: &Connection,
        family_marker: &[u8],
        inputs: &ExportPublicationInputs<'_>,
        cancel: Option<&Cancellation>,
    ) -> Result<ExportPublicationEffect> {
        let (before_proof, before) =
            self.read_export_publication(c, family_marker, inputs, cancel)?;
        if before.member == ExportPublicationMember::Ready {
            return Ok(ExportPublicationEffect::AlreadyReady);
        }
        cancellation::check(cancel)?;
        compare_and_publish(c, inputs.target)?;
        let (after_proof, after) =
            self.read_export_publication(c, family_marker, inputs, cancel)?;
        if after.member != ExportPublicationMember::Ready
            || after.protection != before.protection
            || after_proof.manifests() != before_proof.manifests()
        {
            return Err(invalid(
                "export publication full post-read/accounting differs",
            ));
        }
        cancellation::check(cancel)?;
        Ok(ExportPublicationEffect::Updated)
    }
}
#[cfg(test)]
#[path = "export_publication_tests.rs"]
mod tests;
