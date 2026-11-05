use super::{ProviderBootstrapSource, Record, Result, SourceCommitment, codec, invalid};
use crate::projection::{RecoveryBinding, format};
use layrs_core::scm::FileId;
use layrs_vfs::{ControlDirectoryIdentity, ControlDirectoryStageIdentity};
use serde::{Deserialize, Serialize};

pub(super) const ALLOCATION_DOMAIN: &str = "layrs-projection-provider-allocation-v1";
pub(super) const PREPARED_DOMAIN: &str = "layrs-projection-provider-prepared-v1";
pub(super) const ABANDONED_DOMAIN: &str = "layrs-projection-provider-abandoned-v1";
pub(super) const ELIGIBILITY_DOMAIN: &str = "layrs-projection-provider-bootstrap-eligibility-v1";
pub(super) const MOUNT_DOMAIN: &str = "layrs-projection-provider-mount-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptRef {
    pub file_identity: String,
    pub byte_length: u64,
    pub digest: String,
}
impl ReceiptRef {
    fn validate(&self, max: u64) -> Result<()> {
        codec::file_identity(&self.file_identity)?;
        codec::digest_token(&self.digest)?;
        if self.byte_length == 0 || self.byte_length > max {
            return Err(invalid("receipt reference byte length invalid"));
        }
        Ok(())
    }
}
/// A computed reference is DATA, never proof that a final receipt was published.
pub(in crate::projection) trait Receipt: Record {
    const DOMAIN: &'static str;
    fn file_identity(&self) -> &str;
    fn data_reference(&self) -> Result<ReceiptRef> {
        let bytes = self.encode()?;
        Ok(ReceiptRef {
            file_identity: self.file_identity().into(),
            byte_length: bytes.len() as u64,
            digest: codec::hash(Self::DOMAIN, &bytes),
        })
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct FamilyRecord {
    pub root: String,
    pub canonical_root: String,
    pub incarnation: String,
    pub root_identity: String,
    pub marker_file_identity: String,
    pub marker_digest: String,
    pub metadata_identity: String,
    pub catalog_file_identity: String,
}
impl FamilyRecord {
    pub(in crate::projection::provider) fn validate(&self) -> Result<()> {
        codec::locator(&self.root)?;
        codec::locator(&self.canonical_root)?;
        if self.incarnation.is_empty() || self.incarnation.len() > 128 {
            return Err(invalid("family incarnation length invalid"));
        }
        codec::directory_identity(&self.root_identity)?;
        codec::directory_identity(&self.metadata_identity)?;
        codec::file_identity(&self.marker_file_identity)?;
        codec::file_identity(&self.catalog_file_identity)?;
        codec::digest_token(&self.marker_digest)
    }
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct PrivateRecord {
    pub metadata_path: String,
    pub canonical_metadata_path: String,
    pub metadata_identity: String,
    pub provider_control_identity: String,
    pub database_file_identity: String,
}
impl PrivateRecord {
    fn validate(&self) -> Result<()> {
        codec::locator(&self.metadata_path)?;
        codec::locator(&self.canonical_metadata_path)?;
        codec::directory_identity(&self.metadata_identity)?;
        codec::directory_identity(&self.provider_control_identity)?;
        codec::file_identity(&self.database_file_identity)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct AllocationRecord {
    pub schema_version: String,
    pub transaction_id: String,
    pub receipt_file_identity: String,
    pub family: FamilyRecord,
    pub private: PrivateRecord,
    pub stage_identity_hex: String,
    pub format: format::Marker,
    pub format_bytes: u64,
    pub format_file_id: FileId,
    pub source: SourceCommitment,
}
impl Record for AllocationRecord {
    const MAX_BYTES: usize = 128 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(
            &self.schema_version,
            "layrs.projection-provider-allocation.v1",
        )?;
        codec::transaction(&self.transaction_id)?;
        codec::file_identity(&self.receipt_file_identity)?;
        self.family.validate()?;
        self.private.validate()?;
        self.source.validate_fields()?;
        let stage = self.stage_identity()?;
        codec::preflight(&self.format, codec::FORMAT_MAX)?;
        let bytes = self.format.bytes()?;
        let marker = format::parse_marker_bytes(&bytes)?;
        let shared = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("provider allocation requires format4"))?;
        let fields = serde_json::to_value(&marker).map_err(invalid)?;
        // A can be checked before SQL is readable. Its selection commitment must
        // agree with the unchanged Marker's All/explicit-Ep encoding; actual
        // non-All selection bytes are compared later against validated source.
        let all = crate::projection::selection::encode(
            &layrs_core::projection::ProjectionSelection::All,
        )?;
        let all_ep = blake3::hash(&all).to_hex().to_string();
        match fields.get("initialSelectionDigest") {
            None if self.source.selection_payload_digest == all_ep
                && self.source.selection_bytes == all.len() as u64
                && self.source.selection_digest
                    == crate::projection::database::digest(
                        0,
                        Some(&self.source.snapshot_digest),
                        &all,
                    ) => {}
            Some(serde_json::Value::String(ep))
                if ep == &self.source.selection_payload_digest && ep != &all_ep => {}
            _ => {
                return Err(invalid(
                    "allocation Marker/source selection commitment differs",
                ));
            }
        }
        let member = std::path::Path::new(&self.family.root)
            .join("worktrees")
            .join(
                blake3::hash(marker.identity.worktree_id.as_str().as_bytes())
                    .to_hex()
                    .as_str(),
            )
            .join("metadata");
        // Match existing member_metadata's lexical derivation, without Path's
        // Windows case equivalence or filesystem canonicalization/adoption.
        if member.as_os_str() != std::path::Path::new(&self.private.metadata_path).as_os_str()
            || member.as_os_str() != shared.metadata_path.as_os_str()
        {
            return Err(invalid("allocation deterministic member metadata differs"));
        }
        if fields.get("formatVersion").and_then(|v| v.as_u64()) != Some(4)
            || marker.import_digest.is_some()
            || marker.initial_snapshot_digest != self.source.snapshot_digest
            || self.format_bytes != bytes.len() as u64
            || self.format_bytes == 0
            || self.format_bytes > codec::FORMAT_MAX as u64
            || FileId::from_bytes(&bytes) != self.format_file_id
            || marker.checkout_root
                != stage
                    .root()
                    .to_str()
                    .ok_or_else(|| invalid("non-UTF8 stage root"))?
            || marker.checkout_identity() != stage.root_filesystem_identity()
            || fields.get("storeIdentity").and_then(|v| v.as_str())
                != Some(stage.directory_filesystem_identity().as_str())
            || marker.metadata_identity != self.private.metadata_identity
            || shared.root.to_str() != Some(self.family.root.as_str())
            || shared.root_identity != self.family.root_identity
            || shared.incarnation != self.family.incarnation
            || shared.metadata_path.to_str() != Some(self.private.metadata_path.as_str())
            || shared.database_identity != self.private.database_file_identity
        {
            return Err(invalid(
                "allocation FORMAT/source/native comparison fields differ",
            ));
        }
        Ok(())
    }
}
impl Receipt for AllocationRecord {
    const DOMAIN: &'static str = ALLOCATION_DOMAIN;
    fn file_identity(&self) -> &str {
        &self.receipt_file_identity
    }
}
impl AllocationRecord {
    pub fn stage_identity(&self) -> Result<ControlDirectoryStageIdentity> {
        let bytes = codec::decode_hex(
            &self.stage_identity_hex,
            layrs_vfs::CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES,
        )?;
        let stage = ControlDirectoryStageIdentity::decode(&bytes).map_err(invalid)?;
        if stage.encode() != bytes {
            return Err(invalid("stage identity codec is not canonical"));
        }
        Ok(stage)
    }
    /// Caller supplies the existing member_metadata(worktree) result. This is a
    /// lexical comparison only, not a filesystem locator/canonicalization call.
    pub fn require_source_and_member(
        &self,
        source: &ProviderBootstrapSource,
        member_metadata: &std::path::Path,
    ) -> Result<()> {
        self.validate_fields()?;
        source.require_commitment(&self.source)?;
        self.format.validate_initial_selection(source.selection())?;
        if member_metadata.to_str() != Some(self.private.metadata_path.as_str()) {
            return Err(invalid("allocation member metadata locator differs"));
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct PreparedRecord {
    pub schema_version: String,
    pub transaction_id: String,
    pub receipt_file_identity: String,
    pub allocation: ReceiptRef,
    pub control_identity_hex: String,
}
impl Record for PreparedRecord {
    const MAX_BYTES: usize = 64 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(
            &self.schema_version,
            "layrs.projection-provider-prepared.v1",
        )?;
        codec::transaction(&self.transaction_id)?;
        codec::file_identity(&self.receipt_file_identity)?;
        self.allocation
            .validate(AllocationRecord::MAX_BYTES as u64)?;
        self.control_identity()?;
        Ok(())
    }
}
impl Receipt for PreparedRecord {
    const DOMAIN: &'static str = PREPARED_DOMAIN;
    fn file_identity(&self) -> &str {
        &self.receipt_file_identity
    }
}
impl PreparedRecord {
    pub fn control_identity(&self) -> Result<ControlDirectoryIdentity> {
        let bytes = codec::decode_hex(
            &self.control_identity_hex,
            layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES,
        )?;
        let identity = ControlDirectoryIdentity::decode(&bytes).map_err(invalid)?;
        if identity.encode() != bytes {
            return Err(invalid("control identity codec is not canonical"));
        }
        Ok(identity)
    }
    pub fn require_allocation(&self, allocation: &AllocationRecord) -> Result<()> {
        self.validate_fields()?;
        if self.transaction_id != allocation.transaction_id
            || self.allocation != allocation.data_reference()?
        {
            return Err(invalid("prepared allocation reference differs"));
        }
        let identity = self.control_identity()?;
        let stage = allocation.stage_identity()?;
        if identity.stage_identity() != &stage {
            return Err(invalid("prepared stage binding differs"));
        }
        // Parsing/validation uses VFS's actual codec. R4 additionally specifies
        // this literal DATA comparison for size/content, which have no getters.
        // This does not construct a prepared receipt, expected guard or phase.
        let encoded = identity.encode();
        let tail = &encoded[encoded.len() - 75..]; // successful decoder guarantees this length
        let size = u32::from_le_bytes(tail[..4].try_into().map_err(invalid)?);
        if size as u64 != allocation.format_bytes
            || &tail[4..] != allocation.format_file_id.as_str().as_bytes()
        {
            return Err(invalid("prepared FORMAT size/content differs"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::projection) enum MountPhase {
    Allocating,
    ControlPrepared,
    ControlPublished,
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct MountRecord {
    pub schema_version: String,
    pub transaction_id: String,
    pub catalog_id: String,
    pub phase: MountPhase,
    pub allocation: ReceiptRef,
    #[serde(deserialize_with = "codec::required_option")]
    pub prepared: Option<ReceiptRef>,
}
impl Record for MountRecord {
    const MAX_BYTES: usize = 1024 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, "layrs.projection-provider-mount.v1")?;
        codec::transaction(&self.transaction_id)?;
        codec::digest_token(&self.catalog_id)?;
        self.allocation
            .validate(AllocationRecord::MAX_BYTES as u64)?;
        if (self.phase == MountPhase::Allocating) != self.prepared.is_none() {
            return Err(invalid("mount phase/prepared projection mismatch"));
        }
        if let Some(p) = &self.prepared {
            p.validate(PreparedRecord::MAX_BYTES as u64)?;
        }
        Ok(())
    }
}
impl MountRecord {
    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(MOUNT_DOMAIN, &self.encode()?))
    }
    /// External P may exist while allocating; this never promotes the phase.
    pub fn require_references(
        &self,
        a: &AllocationRecord,
        p: Option<&PreparedRecord>,
    ) -> Result<()> {
        self.validate_fields()?;
        if self.transaction_id != a.transaction_id
            || self.catalog_id != a.source.catalog_id
            || self.allocation != a.data_reference()?
        {
            return Err(invalid("mount allocation/catalog binding differs"));
        }
        if let Some(p) = p {
            p.require_allocation(a)?;
        }
        if self.phase != MountPhase::Allocating {
            let p = p.ok_or_else(|| invalid("prepared mount lacks final prepared data"))?;
            if self.prepared.as_ref() != Some(&p.data_reference()?) {
                return Err(invalid("mount prepared reference differs"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::projection) enum RegistryAtDecision {
    Absent,
    Initializing,
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::projection) enum PrivateState {
    Uninitialized,
    InitialSchema8 {
        catalog_id: String,
        snapshot_digest: String,
        selection_digest: String,
        mount_digest: String,
    },
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct EligibilityRecord {
    pub schema_version: String,
    pub registry_at_decision: RegistryAtDecision,
    pub private_state: PrivateState,
}
impl Record for EligibilityRecord {
    const MAX_BYTES: usize = 16 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(
            &self.schema_version,
            "layrs.projection-provider-bootstrap-eligibility.v1",
        )?;
        if let PrivateState::InitialSchema8 {
            catalog_id,
            snapshot_digest,
            selection_digest,
            mount_digest,
        } = &self.private_state
        {
            codec::digest_token(catalog_id)?;
            codec::raw_digest(snapshot_digest)?;
            codec::raw_digest(selection_digest)?;
            codec::digest_token(mount_digest)?;
        }
        Ok(())
    }
}
impl EligibilityRecord {
    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(ELIGIBILITY_DOMAIN, &self.encode()?))
    }
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct AbandonedRecord {
    pub schema_version: String,
    pub transaction_id: String,
    pub receipt_file_identity: String,
    pub allocation: ReceiptRef,
    #[serde(deserialize_with = "codec::required_option")]
    pub prepared: Option<ReceiptRef>,
    pub eligibility: EligibilityRecord,
    pub eligibility_digest: String,
}
impl Record for AbandonedRecord {
    const MAX_BYTES: usize = 16 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(
            &self.schema_version,
            "layrs.projection-provider-abandoned.v1",
        )?;
        codec::transaction(&self.transaction_id)?;
        codec::file_identity(&self.receipt_file_identity)?;
        self.allocation
            .validate(AllocationRecord::MAX_BYTES as u64)?;
        if let Some(p) = &self.prepared {
            p.validate(PreparedRecord::MAX_BYTES as u64)?;
        }
        if self.eligibility_digest != self.eligibility.digest()? {
            return Err(invalid("eligibility digest mismatch"));
        }
        Ok(())
    }
}
impl Receipt for AbandonedRecord {
    const DOMAIN: &'static str = ABANDONED_DOMAIN;
    fn file_identity(&self) -> &str {
        &self.receipt_file_identity
    }
}
impl AbandonedRecord {
    /// Receipt DATA only, usable while either SQL database is unavailable. A/P
    /// authenticity and absence are the caller's responsibility. This rejects
    /// A-derived contradictions but never validates the claimed private state or
    /// mount digest, decides eligibility, or authorizes deletion.
    pub fn require_receipt_references(
        &self,
        a: &AllocationRecord,
        p: Option<&PreparedRecord>,
    ) -> Result<()> {
        self.validate_fields()?;
        if self.transaction_id != a.transaction_id || self.allocation != a.data_reference()? {
            return Err(invalid("terminal allocation reference differs"));
        }
        if let Some(p) = p {
            p.require_allocation(a)?;
        }
        if self.prepared != p.map(Receipt::data_reference).transpose()? {
            return Err(invalid(
                "terminal must reference existing final prepared data",
            ));
        }
        if let PrivateState::InitialSchema8 {
            catalog_id,
            snapshot_digest,
            selection_digest,
            ..
        } = &self.eligibility.private_state
        {
            if catalog_id != &a.source.catalog_id
                || snapshot_digest != &a.source.snapshot_digest
                || selection_digest != &a.source.selection_digest
            {
                return Err(invalid("terminal allocation source commitment differs"));
            }
        }
        Ok(())
    }

    /// Full comparison against caller-supplied private-state DATA. `None` means
    /// independently established uninitialized SQL, NEVER unknown/hot SQL. An
    /// initialized claim requires the actual validated mount; none is inferred
    /// from this receipt. This pure helper still establishes no SQL authority.
    pub fn require_references(
        &self,
        a: &AllocationRecord,
        p: Option<&PreparedRecord>,
        mount: Option<&MountRecord>,
    ) -> Result<()> {
        self.require_receipt_references(a, p)?;
        match &self.eligibility.private_state {
            PrivateState::Uninitialized if mount.is_none() => (),
            PrivateState::InitialSchema8 { mount_digest, .. } => {
                let m = mount.ok_or_else(|| invalid("initialized eligibility lacks mount data"))?;
                m.require_references(a, p)?;
                if mount_digest != &m.digest()? {
                    return Err(invalid("terminal initialized mount commitment differs"));
                }
            }
            _ => return Err(invalid("uninitialized eligibility has mount data")),
        }
        Ok(())
    }
}

/// Private DATA disposition, never an accepted phase or SQL eligibility proof.
/// Every T-present receipt comparison remains pending until actual private SQL
/// state and (for InitialSchema8) its validated mount have been checked.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum ReceiptBindingCheck {
    NoTerminal,
    TerminalPendingValidation,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedBinding {
    pub receipt: ReceiptRef,
    pub control_identity_hex: String,
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBootstrapBinding {
    pub schema_version: String,
    pub recovery: RecoveryBinding,
    pub transaction_id: String,
    pub allocation: ReceiptRef,
    #[serde(deserialize_with = "codec::required_option")]
    pub prepared: Option<PreparedBinding>,
    #[serde(deserialize_with = "codec::required_option")]
    pub abandoned: Option<ReceiptRef>,
}
impl Record for ProviderBootstrapBinding {
    // Derived from the three bounded refs, existing bounded binding and one
    // bounded control codec; this is not an additional durable receipt kind.
    const MAX_BYTES: usize = 64 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(
            &self.schema_version,
            "layrs.projection-provider-bootstrap-binding.v1",
        )?;
        codec::transaction(&self.transaction_id)?;
        self.allocation
            .validate(AllocationRecord::MAX_BYTES as u64)?;
        format::validate_identity(&self.recovery.identity)?;
        codec::directory_identity(&self.recovery.checkout_identity)?;
        codec::file_identity(&self.recovery.database_identity)?;
        codec::digest_token(&self.recovery.marker_digest)?;
        let family = self
            .recovery
            .family_marker_digest
            .as_deref()
            .ok_or_else(|| invalid("provider binding missing family digest"))?;
        codec::digest_token(family)?;
        if let Some(p) = &self.prepared {
            p.receipt.validate(PreparedRecord::MAX_BYTES as u64)?;
            let bytes = codec::decode_hex(
                &p.control_identity_hex,
                layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES,
            )?;
            let decoded = ControlDirectoryIdentity::decode(&bytes).map_err(invalid)?;
            if decoded.encode() != bytes {
                return Err(invalid("binding control identity is not canonical"));
            }
        }
        if let Some(t) = &self.abandoned {
            t.validate(AbandonedRecord::MAX_BYTES as u64)?;
        }
        Ok(())
    }
}
impl ProviderBootstrapBinding {
    /// Pure comparisons only; the caller must authenticate all final files and
    /// their absence/native pins before using this as an expected recovery key.
    pub(in crate::projection) fn require_references(
        &self,
        a: &AllocationRecord,
        p: Option<&PreparedRecord>,
        t: Option<&AbandonedRecord>,
    ) -> Result<ReceiptBindingCheck> {
        self.validate_fields()?;
        if self.transaction_id != a.transaction_id
            || self.allocation != a.data_reference()?
            || self.recovery.identity != a.format.identity
            || self.recovery.checkout_identity != a.format.checkout_identity()
            || self.recovery.database_identity != a.private.database_file_identity
            || self.recovery.marker_digest
                != codec::hash("layrs-projection-recovery-marker-v1", &a.format.bytes()?)
            || self.recovery.family_marker_digest.as_ref() != Some(&a.family.marker_digest)
        {
            return Err(invalid("bootstrap binding allocation differs"));
        }
        match (&self.prepared, p) {
            (None, None) => (),
            (Some(bound), Some(p)) => {
                p.require_allocation(a)?;
                if bound.receipt != p.data_reference()?
                    || bound.control_identity_hex != p.control_identity_hex
                {
                    return Err(invalid(
                        "bootstrap prepared binding changed; rebind required",
                    ));
                }
            }
            _ => {
                return Err(invalid(
                    "bootstrap prepared presence changed; rebind required",
                ));
            }
        }
        if let Some(t) = t {
            t.require_receipt_references(a, p)?;
        }
        if self.abandoned != t.map(Receipt::data_reference).transpose()? {
            return Err(invalid("bootstrap terminal changed; rebind required"));
        }
        Ok(if t.is_some() {
            ReceiptBindingCheck::TerminalPendingValidation
        } else {
            ReceiptBindingCheck::NoTerminal
        })
    }
}
