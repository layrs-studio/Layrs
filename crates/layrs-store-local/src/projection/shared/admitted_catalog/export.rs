//! Export-only reservation/content DATA for the complete retained-family
//! group validator. No private B/C reader, opener, physical pin or COMMIT lives
//! here. The driver authenticates actual B Staging and actual C through its
//! closed phase reader, plus source image, complete union and metadata floor,
//! before and after effects. A private-Ready driver may derive this target from
//! canonical destination-Staging predecessor DATA, but must separately validate
//! actual C Staging or Ready. This target alone grants no phase authority.
//!
//! Parent integration must first run its UNCHANGED complete retained-root walk
//! (Published journals, exact debits and shared charged sets), then call
//! validate_and_classify, then retain its final all-reservation classification.
//! These helpers alone are deliberately not a complete family admission gate.
use super::*;
use crate::projection::{
    provider::records::{
        import::{ImportSource, ImportSourceReferences, ProviderImportKind},
        transition::{
            ProviderTransitionIntentRecord, TransitionBackendKind, TransitionRole, TransitionState,
        },
        Record,
    },
    shared::{
        exact_content::{canonical_manifest_bytes, PREPARE_CHARGE},
        provider_access::exact_path,
    },
};
use layrs_core::scm::{FileId, FileManifestId};
use rusqlite::OptionalExtension;
use serde::Serialize;

const MAX_METADATA: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy)]
enum ActiveRoot<'a> {
    Absent,
    Protection(&'a ExactProtection),
}

/// Expected target DATA, not evidence that B/C private SQL, C's physical files,
/// image completeness or a metadata-floor calculation is authentic. Closed
/// fields merely keep all catalog comparisons tied to one immutable plan.
/// The same immutable target also describes Installing and Protected; only the
/// complete parent gate chooses that interpretation and supplies the full union.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct ExportReserveOnlyTarget {
    source_marker: format::Marker,
    source_binding: ExactBinding,
    source_admission_digest: String,
    marker: format::Marker,
    binding: ExactBinding,
    metadata_bytes: u64,
    private_metadata_floor: u64,
    root_payload_bytes: u64,
    required_manifests: u64,
    required_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ExportReservation {
    Absent,
    Present { remaining: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) struct ExportUsage {
    pub content_bytes: u64,
    pub reserved_bytes: u64,
    pub limit_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) struct ExportReserveOnlyState {
    pub reservation: ExportReservation,
    pub usage: ExportUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ExportReservationEffect {
    Inserted,
    AlreadyPresent,
}

impl ExportReserveOnlyTarget {
    /// `baseline_quota` is Q0 from complete owner rederivation with M=0;
    /// `private_metadata_floor` comes from the actual source/destination pair.
    /// These are descriptive inputs. This constructor does not validate their
    /// source SQL or replace the driver's complete rederivation of Q, M and C9
    /// admission funding. No caller-selected subset becomes content authority.
    pub(in crate::projection) fn new(
        retained: &AdmittedFamilyContext,
        source_staging: &ProviderTransitionIntentRecord,
        destination_staging: &ProviderTransitionIntentRecord,
        required: &[FileManifest],
        baseline_quota: u64,
        private_metadata_floor: u64,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        require_pair(source_staging, destination_staging)?;
        require_union(source_staging, required, cancel)?;
        let source_marker = retained.marker();
        let prefix = &source_staging.plan.source_prefix;
        let ImportSource::Ordinary {
            private_schema: 9,
            admission: Some(admission),
            ..
        } = &prefix.source
        else {
            return Err(invalid("export target requires admitted ordinary source"));
        };
        if admission.admission_digest != retained.admission_digest()
            || admission.incoming_prefix_id != retained.binding().prefix_id
            || admission.database_identity != retained.binding().database_identity
        {
            return Err(invalid(
                "export target differs from retained source admission",
            ));
        }
        // Actual incoming receipt authentication belongs to the retained
        // private interpreter; the detached family context has no receipt body.
        prefix.require_references(ImportSourceReferences::AdmittedOrdinary {
            marker: source_marker,
            admission,
        })?;
        let destination = source_staging
            .destination
            .as_ref()
            .ok_or_else(|| invalid("export target destination proof absent"))?;
        let marker = &destination.format;
        let shared = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("export target is not shared"))?;
        if destination.backend != TransitionBackendKind::Ordinary
            || destination.native_control_identity_hex.is_some()
            || destination.native_root_identity.is_some()
            || marker.identity == source_marker.identity
            || shared.database_identity != destination.private_database_identity
            || destination.private_database_identity == retained.binding().database_identity
            || destination.family_marker_digest != retained.binding().family_marker_digest
        {
            return Err(invalid("export target source/destination family differs"));
        }
        let operation = source_staging.reservation_operation()?;
        let binding = ExactBinding {
            family_incarnation: retained.binding().family_incarnation.clone(),
            family_root_identity: retained.binding().family_root_identity.clone(),
            family_marker_digest: retained.binding().family_marker_digest.clone(),
            family_catalog_identity: retained.binding().family_catalog_identity.clone(),
            plan_digest: source_staging.plan.digest()?,
            prefix_id: source_staging.plan.source_prefix_id.clone(),
            destination_proof_digest: destination.digest(&source_staging.plan)?,
            transaction_id: source_staging.transaction_id.clone(),
            owner: marker.identity.worktree_id.clone(),
            database_identity: destination.private_database_identity.clone(),
            reservation: operation.clone(),
            operation,
            quota_bytes: source_staging.plan.quota_bytes,
        };
        binding.validate()?;
        let metadata_bytes = binding
            .quota_bytes
            .checked_sub(baseline_quota)
            .filter(|bytes| *bytes <= MAX_METADATA && *bytes >= private_metadata_floor)
            .ok_or_else(|| {
                invalid("export target metadata is below actual floor or exceeds bound")
            })?;
        if private_metadata_floor == 0 || private_metadata_floor > MAX_METADATA {
            return Err(invalid(
                "export target private metadata floor missing or exceeds bound",
            ));
        }
        let root = ExactProtection::new(
            &binding.family_incarnation,
            &binding.owner,
            &binding.database_identity,
            &binding.transaction_id,
            required,
            cancel,
        )?;
        // M independently retains both private metadata and family root/links.
        // Installed content is never charged against this logical margin here.
        root.require_envelope(metadata_bytes, private_metadata_floor)?;
        let result = Self {
            source_marker: source_marker.clone(),
            source_binding: retained.binding().clone(),
            source_admission_digest: retained.admission_digest().into(),
            marker: marker.clone(),
            binding,
            metadata_bytes,
            private_metadata_floor,
            root_payload_bytes: root.payload().len() as u64,
            required_manifests: required.len() as u64,
            required_digest: source_staging.plan.protected_manifests.digest.clone(),
        };
        cancellation::check(cancel)?;
        Ok(result)
    }

    pub(in crate::projection) fn binding(&self) -> &ExactBinding {
        &self.binding
    }
    pub(in crate::projection) fn marker(&self) -> &format::Marker {
        &self.marker
    }

    /// Call only AFTER all retained roots and their Published journals/debits
    /// have passed. Add C only when its exact full reservation actually exists;
    /// the parent's final inventory still rejects every unknown exact group.
    pub(super) fn validate_and_classify(
        &self,
        c: &Connection,
        retained: &AdmittedFamilyContext,
        family: &FamilyMarker,
        classified: &mut BTreeSet<String>,
        cancel: Option<&Cancellation>,
    ) -> Result<ExportReserveOnlyState> {
        let state = self.inspect_target(c, retained, family, cancel)?;
        if classified.contains(&self.binding.reservation) {
            return Err(invalid(
                "export target collides with a retained import group",
            ));
        }
        // Keep one classified slot available even while C is absent, so the
        // prospective INSERT cannot cross the family-wide group bound.
        if classified.len() >= MAX_GROUPS {
            return Err(invalid(
                "export retained and active group count exceeds bound",
            ));
        }
        if matches!(state.reservation, ExportReservation::Present { .. }) {
            classified.insert(self.binding.reservation.clone());
        }
        Ok(state)
    }

    /// Parent-only Installing interpretation AFTER every retained group has
    /// passed. The final parent inventory must still classify every journal
    /// and import reservation. Actual B/C Staging and bytes remain driver-owned.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn validate_installing_and_classify(
        &self,
        c: &Connection,
        retained: &AdmittedFamilyContext,
        family: &FamilyMarker,
        required: &[FileManifest],
        classified: &mut BTreeSet<String>,
        charged_chunks: &mut BTreeSet<String>,
        charged_manifests: &mut BTreeSet<String>,
        cancel: Option<&Cancellation>,
    ) -> Result<ExactInstallState> {
        Ok(self
            .validate_active_and_classify(
                c,
                retained,
                family,
                required,
                ActiveRoot::Absent,
                classified,
                charged_chunks,
                charged_manifests,
                cancel,
            )?
            .installation)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn validate_protection_and_classify(
        &self,
        c: &Connection,
        retained: &AdmittedFamilyContext,
        family: &FamilyMarker,
        required: &[FileManifest],
        protection: &ExactProtection,
        classified: &mut BTreeSet<String>,
        charged_chunks: &mut BTreeSet<String>,
        charged_manifests: &mut BTreeSet<String>,
        cancel: Option<&Cancellation>,
    ) -> Result<ExportProtectionState> {
        self.validate_active_and_classify(
            c,
            retained,
            family,
            required,
            ActiveRoot::Protection(protection),
            classified,
            charged_chunks,
            charged_manifests,
            cancel,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_active_and_classify(
        &self,
        c: &Connection,
        retained: &AdmittedFamilyContext,
        family: &FamilyMarker,
        required: &[FileManifest],
        root_mode: ActiveRoot<'_>,
        classified: &mut BTreeSet<String>,
        charged_chunks: &mut BTreeSet<String>,
        charged_manifests: &mut BTreeSet<String>,
        cancel: Option<&Cancellation>,
    ) -> Result<ExportProtectionState> {
        cancellation::check(cancel)?;
        if retained.marker() != &self.source_marker
            || retained.binding() != &self.source_binding
            || retained.admission_digest() != self.source_admission_digest
        {
            return Err(invalid("export Installing retained source context changed"));
        }
        require_union_commitment(
            &self.required_digest,
            self.required_manifests,
            required,
            cancel,
        )?;
        self.require_member(c, retained, family)?;
        let owner = self.binding.owner.as_str();
        let root = match root_mode {
            ActiveRoot::Absent => {
                if c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
                    [owner],
                    |r| r.get::<_, bool>(0),
                )? {
                    return Err(invalid("export Installing target has root effects"));
                }
                ProtectionPresence::Absent
            }
            ActiveRoot::Protection(protection) => {
                protection.require_binding(&self.binding, required, cancel)?;
                protection.inspect(c, cancel)?
            }
        };
        cancellation::check(cancel)?;
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
            [owner], |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("export active target has legacy installation effects"));
        }
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1 AND id!=?2)",
            params![owner, self.binding.reservation],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("export Installing target has another reservation"));
        }
        if classified.len() >= MAX_GROUPS || classified.contains(&self.binding.reservation) {
            return Err(invalid("export Installing group collides or exceeds bound"));
        }
        let state = self.binding.validate_export_installations(
            c,
            required,
            charged_chunks,
            charged_manifests,
            cancel,
        )?;
        // Existing C journals already contribute to global records. Add only
        // missing future C rows, retaining the preallocation full-lifecycle cap.
        let usage = global_usage(c, state.records, self.required_manifests, cancel)?;
        if state.content_bytes != usage.content_bytes
            || state.reserved_bytes != usage.reserved_bytes
            || state.limit_bytes != usage.limit_bytes
        {
            return Err(invalid("export Installing global accounting differs"));
        }
        if let ActiveRoot::Protection(protection) = root_mode {
            if state.records != self.required_manifests
                || state.published != self.required_manifests
            {
                return Err(invalid(
                    "export protection requires the complete Published union",
                ));
            }
            protection.require_envelope(state.remaining, self.private_metadata_floor)?;
        }
        classified.insert(self.binding.reservation.clone());
        cancellation::check(cancel)?;
        Ok(ExportProtectionState {
            installation: state,
            root,
        })
    }

    /// Low-level parent-only INSERT primitive. The public export writer
    /// MUST run its full retained walk/final classification before AND after
    /// this call in the same transaction. This cannot be called by a generic
    /// Lease or the Provider import wrappers. Caller owns rollback/COMMIT/close.
    pub(super) fn insert_or_replay_after_retained_validation(
        &self,
        c: &Connection,
        retained: &AdmittedFamilyContext,
        family: &FamilyMarker,
        cancel: Option<&Cancellation>,
    ) -> Result<ExportReservationEffect> {
        if c.is_autocommit() || c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, u64>(0))? != 1
        {
            return Err(invalid(
                "export reservation writer requires FK-enabled transaction",
            ));
        }
        let before = self.inspect_target(c, retained, family, cancel)?;
        if matches!(before.reservation, ExportReservation::Present { .. }) {
            return Ok(ExportReservationEffect::AlreadyPresent);
        }
        require_additional_capacity(before.usage, self.binding.quota_bytes)?;
        cancellation::check(cancel)?;
        if c.execute(
            "INSERT INTO reservations(id,owner,operation,remaining) VALUES(?1,?2,?3,?4)",
            params![
                self.binding.reservation,
                self.binding.owner.as_str(),
                self.binding.operation,
                self.binding.quota_bytes
            ],
        )? != 1
        {
            return Err(invalid("export reservation insertion count differs"));
        }
        let after = self.inspect_target(c, retained, family, cancel)?;
        if after.reservation
            != (ExportReservation::Present {
                remaining: self.binding.quota_bytes,
            })
            || after.usage.content_bytes != before.usage.content_bytes
            || after.usage.limit_bytes != before.usage.limit_bytes
            || before
                .usage
                .reserved_bytes
                .checked_add(self.binding.quota_bytes)
                != Some(after.usage.reserved_bytes)
        {
            return Err(invalid(
                "export reservation exact prospective readback differs",
            ));
        }
        cancellation::check(cancel)?;
        Ok(ExportReservationEffect::Inserted)
    }

    fn inspect_target(
        &self,
        c: &Connection,
        retained: &AdmittedFamilyContext,
        family: &FamilyMarker,
        cancel: Option<&Cancellation>,
    ) -> Result<ExportReserveOnlyState> {
        cancellation::check(cancel)?;
        if c.is_autocommit() {
            return Err(invalid(
                "export reserve-only validation requires retained transaction",
            ));
        }
        if retained.marker() != &self.source_marker
            || retained.binding() != &self.source_binding
            || retained.admission_digest() != self.source_admission_digest
        {
            return Err(invalid("export target retained source context changed"));
        }
        self.require_member(c, retained, family)?;
        let owner = self.binding.owner.as_str();
        for sql in [
            "SELECT EXISTS(SELECT 1 FROM roots WHERE owner=?1)",
            "SELECT EXISTS(SELECT 1 FROM installations i JOIN reservations r ON r.id=i.reservation WHERE r.owner=?1)",
            "SELECT EXISTS(SELECT 1 FROM exact_installations e JOIN reservations r ON r.id=e.reservation WHERE r.owner=?1)",
        ] {
            cancellation::check(cancel)?;
            if c.query_row(sql, [owner], |r|r.get::<_,bool>(0))? {
                return Err(invalid("export ReserveOnly target has root or installation effects"));
            }
        }
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM reservations WHERE owner=?1 AND id!=?2)",
            params![owner, self.binding.reservation],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("export ReserveOnly target has another reservation"));
        }
        let reservation = self.read_reservation(c)?;
        // ReserveOnly has no C journals, so this adds the future C row count
        // once to all currently retained journals. Refuse before reserving a
        // transfer that could never fit the installation engine's global cap.
        let usage = global_usage(c, 0, self.required_manifests, cancel)?;
        cancellation::check(cancel)?;
        Ok(ExportReserveOnlyState { reservation, usage })
    }

    fn require_member(
        &self,
        c: &Connection,
        retained: &AdmittedFamilyContext,
        family: &FamilyMarker,
    ) -> Result<()> {
        self.binding.validate()?;
        self.marker.validate()?;
        let shared = self
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("export target shared FORMAT absent"))?;
        let source = retained
            .marker()
            .shared
            .as_ref()
            .ok_or_else(|| invalid("export source shared FORMAT absent"))?;
        let member = source
            .root
            .join("worktrees")
            .join(
                blake3::hash(self.binding.owner.as_str().as_bytes())
                    .to_hex()
                    .to_string(),
            )
            .join("metadata");
        if shared.root != source.root
            || shared.incarnation != family.incarnation
            || shared.root_identity != family.root_identity
            || shared.metadata_path != member
            || shared.database_identity != self.binding.database_identity
            || self.binding.family_incarnation != family.incarnation
            || self.binding.family_root_identity != family.root_identity
            || self.binding.family_catalog_identity != family.database_identity
            || self.binding.family_marker_digest != retained.binding().family_marker_digest
            || self.marker.identity.worktree_id != self.binding.owner
            || self.marker.import_digest.as_deref() != Some(self.binding.prefix_id.as_str())
            || self.marker.identity.store_id != family.identity.store_id
            || self.marker.identity.space_id != family.identity.space_id
            || self.marker.identity.origin != family.identity.origin
            || self.marker.identity.principal_id != family.identity.principal_id
        {
            return Err(invalid(
                "export target differs from retained family FORMAT/member",
            ));
        }
        let marker_bytes = self.marker.bytes()?;
        let member_path = exact_path(&member)?;
        if marker_bytes.len() > 65536 || member_path.len() > 4096 {
            return Err(invalid("export target registry metadata exceeds bound"));
        }
        let bounded: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND length(CAST(metadata_path AS BLOB))<=4096 AND length(CAST(database_identity AS BLOB))<=512 AND length(marker)<=65536 AND length(CAST(state AS BLOB))<=12)",
            [self.binding.owner.as_str()], |r|r.get(0))?;
        if !bounded {
            return Err(invalid(
                "export target Initializing registration absent or exceeds bound",
            ));
        }
        let (path, database, marker, state): (String, String, Vec<u8>, String) = c.query_row(
            "SELECT metadata_path,database_identity,marker,state FROM worktrees WHERE id=?1",
            [self.binding.owner.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        if path != member_path
            || database != self.binding.database_identity
            || marker != marker_bytes
            || state != "initializing"
        {
            return Err(invalid(
                "export target registry differs from exact Initializing member",
            ));
        }
        Ok(())
    }

    fn read_reservation(&self, c: &Connection) -> Result<ExportReservation> {
        if c.query_row("SELECT EXISTS(SELECT 1 FROM reservations WHERE id=?1 AND (length(CAST(owner AS BLOB))>512 OR length(CAST(operation AS BLOB))>128))",
            [&self.binding.reservation], |r|r.get::<_,bool>(0))? {
            return Err(invalid("export reservation text exceeds bound"));
        }
        let row: Option<(String, String, u64)> = c
            .query_row(
                "SELECT owner,operation,remaining FROM reservations WHERE id=?1",
                [&self.binding.reservation],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        match row {
            None => Ok(ExportReservation::Absent),
            Some((owner, operation, remaining))
                if owner == self.binding.owner.as_str()
                    && operation == self.binding.operation
                    && remaining == self.binding.quota_bytes =>
            {
                Ok(ExportReservation::Present { remaining })
            }
            Some(_) => Err(invalid(
                "export reservation owner, operation or full charge differs",
            )),
        }
    }
}

fn require_pair(
    source: &ProviderTransitionIntentRecord,
    destination: &ProviderTransitionIntentRecord,
) -> Result<()> {
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
        || source.reservation.as_deref() != Some(source.reservation_operation()?.as_str())
        || source.plan.kind != ProviderImportKind::ManagedOrdinary
        || source.plan.companion_id.is_some()
        || source.plan.companion_files.count != 0
        || source.plan.companion_deletions.count != 0
    {
        return Err(invalid(
            "export ReserveOnly requires identical ordinary source/destination Staging",
        ));
    }
    Ok(())
}

fn require_union(
    source: &ProviderTransitionIntentRecord,
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    require_union_commitment(
        &source.plan.protected_manifests.digest,
        source.plan.protected_manifests.count,
        required,
        cancel,
    )
}

fn require_union_commitment(
    digest: &str,
    count: u64,
    required: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<()> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct RequiredRow<'a> {
        file_id: &'a FileId,
        manifest_id: &'a FileManifestId,
        bytes: u64,
    }
    if required.len() > MAX_MANIFESTS as usize || required.len() as u64 != count {
        return Err(invalid("export ReserveOnly complete union count differs"));
    }
    let mut rows = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut lengths = BTreeMap::new();
    for manifest in required {
        cancellation::check(cancel)?;
        canonical_manifest_bytes(manifest)?;
        if rows
            .insert((manifest.file_id(), manifest.id()), manifest)
            .is_some()
            || !ids.insert(manifest.id())
            || lengths
                .insert(manifest.file_id(), manifest.byte_length())
                .is_some_and(|old| old != manifest.byte_length())
        {
            return Err(invalid(
                "export ReserveOnly union duplicates or contradicts representation",
            ));
        }
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-managed-protected-manifests-v1\0");
    hash.update(&(rows.len() as u64).to_be_bytes());
    for manifest in rows.into_values() {
        cancellation::check(cancel)?;
        let payload = canonical_json(&RequiredRow {
            file_id: manifest.file_id(),
            manifest_id: manifest.id(),
            bytes: manifest.byte_length(),
        })
        .map_err(|e| invalid(e.to_string()))?;
        hash.update(&(payload.len() as u64).to_be_bytes());
        hash.update(&payload);
    }
    if digest != format!("blake3:{}", hash.finalize()) {
        return Err(invalid(
            "export ReserveOnly complete union commitment differs",
        ));
    }
    Ok(())
}

fn global_usage(
    c: &Connection,
    current_target_manifests: u64,
    target_manifests: u64,
    cancel: Option<&Cancellation>,
) -> Result<ExportUsage> {
    cancellation::check(cancel)?;
    let records: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM exact_installations LIMIT 100001)",
        [],
        |r| r.get(0),
    )?;
    require_future_record_capacity(records, current_target_manifests, target_manifests)?;
    let manifests: u64 = c.query_row(
        "SELECT coalesce(sum(length(manifest)),0) FROM exact_installations",
        [],
        |r| r.get(0),
    )?;
    let base: u64 = c.query_row("SELECT coalesce((SELECT sum(bytes) FROM chunks),0)+coalesce((SELECT sum(length(payload)) FROM manifests),0)+coalesce((SELECT sum(bytes) FROM sweep),0)", [], |r|r.get(0))?;
    let reserved: u64 = c.query_row(
        "SELECT coalesce(sum(remaining),0) FROM reservations",
        [],
        |r| r.get(0),
    )?;
    let limit: u64 = c.query_row("SELECT bytes FROM quota WHERE singleton=1", [], |r| {
        r.get(0)
    })?;
    usage(base, records, manifests, reserved, limit)
}

fn require_record_capacity(retained: u64, target: u64) -> Result<()> {
    if retained
        .checked_add(target)
        .is_none_or(|count| count > MAX_MANIFESTS)
    {
        return Err(invalid(
            "export retained and future exact journal count exceeds bound",
        ));
    }
    Ok(())
}

fn require_future_record_capacity(
    global: u64,
    current_target: u64,
    complete_target: u64,
) -> Result<()> {
    if global > MAX_MANIFESTS || current_target > complete_target {
        return Err(invalid(
            "export current exact journal count exceeds bound or union",
        ));
    }
    let retained = global
        .checked_sub(current_target)
        .ok_or_else(|| invalid("export current target exceeds global journal count"))?;
    require_record_capacity(retained, complete_target)
}

fn usage(
    base: u64,
    records: u64,
    journal_manifests: u64,
    reserved: u64,
    limit: u64,
) -> Result<ExportUsage> {
    let content_bytes = records
        .checked_mul(PREPARE_CHARGE)
        .and_then(|n| n.checked_add(journal_manifests))
        .and_then(|n| n.checked_add(base))
        .ok_or_else(|| invalid("export family global content charge overflow"))?;
    if content_bytes
        .checked_add(reserved)
        .is_none_or(|n| n > limit)
    {
        return Err(invalid(
            "partial_store_quota: export retained/active family is overcommitted",
        ));
    }
    Ok(ExportUsage {
        content_bytes,
        reserved_bytes: reserved,
        limit_bytes: limit,
    })
}

fn require_additional_capacity(usage: ExportUsage, additional: u64) -> Result<()> {
    if usage
        .content_bytes
        .checked_add(usage.reserved_bytes)
        .and_then(|n| n.checked_add(additional))
        .is_none_or(|n| n > usage.limit_bytes)
    {
        return Err(invalid(
            "partial_store_quota: insufficient unreserved export capacity",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserve_capacity_includes_every_retained_journal_and_manifest_charge() {
        let state = usage(100, 2, 37, 200, 100 + 2 * PREPARE_CHARGE + 37 + 200 + 50).unwrap();
        assert_eq!(state.content_bytes, 100 + 2 * PREPARE_CHARGE + 37);
        require_additional_capacity(state, 50).unwrap();
        assert!(require_additional_capacity(state, 51).is_err());
    }
    #[test]
    fn retained_content_overflow_and_overcommit_refuse_before_reservation() {
        assert!(usage(u64::MAX, 1, 0, 0, u64::MAX).is_err());
        assert!(usage(100, 1, 10, 10, 100 + PREPARE_CHARGE + 19).is_err());
        assert!(require_additional_capacity(
            ExportUsage {
                content_bytes: 1,
                reserved_bytes: 0,
                limit_bytes: u64::MAX
            },
            u64::MAX
        )
        .is_err());
    }

    #[test]
    fn reservation_preflights_retained_and_future_journals_at_the_global_limit() {
        require_record_capacity(MAX_MANIFESTS - 1, 1).unwrap();
        require_record_capacity(MAX_MANIFESTS, 0).unwrap();
        assert!(require_record_capacity(MAX_MANIFESTS, 1).is_err());
        assert!(require_record_capacity(MAX_MANIFESTS - 1, 2).is_err());
        assert!(require_record_capacity(1, u64::MAX).is_err());
    }

    #[test]
    fn installing_future_capacity_counts_existing_target_journals_once() {
        require_future_record_capacity(MAX_MANIFESTS, 1, 1).unwrap();
        require_future_record_capacity(MAX_MANIFESTS - 1, 1, 2).unwrap();
        require_future_record_capacity(MAX_MANIFESTS, MAX_MANIFESTS, MAX_MANIFESTS).unwrap();
        assert!(require_future_record_capacity(MAX_MANIFESTS, 1, 2).is_err());
        assert!(require_future_record_capacity(MAX_MANIFESTS + 1, 1, 1).is_err());
        assert!(require_future_record_capacity(1, 2, 2).is_err());
        assert!(require_future_record_capacity(1, 1, 0).is_err());
    }
}
