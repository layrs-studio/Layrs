//! Immutable import/companion DATA for the existing private schema 8.
//! These codecs authenticate no SQL rowset, source store, native root or claim.
use super::{
    activation::{owner_identity_digest, ProviderActivationRecord},
    codec, invalid, AllocationRecord, PreparedRecord, ProviderBootstrapBinding, Receipt,
    ReceiptBindingCheck, ReceiptRef, Record,
};
use crate::projection::{format, ProjectionIdentity, Result};
use layrs_core::scm::{canonical_json, FileId, FileManifestId, RepoPath};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const PREFIX_SCHEMA: &str = "layrs.projection-provider-import-prefix.v1";
const PREFIX_DOMAIN: &str = "layrs-projection-provider-import-prefix-v1";
const PREFIX_SCHEMA_V2: &str = "layrs.projection-provider-import-prefix.v2";
const PREFIX_DOMAIN_V2: &str = "layrs-projection-provider-import-prefix-v2";
const COMPANION_SCHEMA: &str = "layrs.projection-provider-companion.v1";
const COMPANION_DOMAIN: &str = "layrs-projection-provider-companion-v1";
const FILE_SCHEMA: &str = "layrs.projection-provider-companion-file.v1";
const FILE_DOMAIN: &str = "layrs-projection-provider-companion-file-v1";
const DELETION_SCHEMA: &str = "layrs.projection-provider-companion-deletion.v1";
const DELETION_DOMAIN: &str = "layrs-projection-provider-companion-deletion-v1";
const FILES_DOMAIN: &str = "layrs-projection-provider-companion-files-v1";
const DELETIONS_DOMAIN: &str = "layrs-projection-provider-companion-deletions-v1";
const RECEIPT_SCHEMA: &str = "layrs.projection-provider-transition-receipt.v1";
const RECEIPT_DOMAIN: &str = "layrs-projection-provider-transition-receipt-v1";
const SEGMENTS_DOMAIN: &str = "layrs-projection-provider-import-segments-v1";
const MAX_ROWS: u64 = 1_000_000;
const MAX_SEGMENTS: usize = 4096;
const MAX_COMPANION_ROWS: u64 = 100_000;
const MAX_COMPANION_METADATA: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::projection) enum ProviderImportKind {
    ManagedOrdinary,
    ExpandedProvider,
}

/// One commitment to an externally stored, ordered SQL rowset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ImportRowset {
    pub count: u64,
    pub digest: String,
}
impl ImportRowset {
    fn validate(&self) -> Result<()> {
        if self.count > MAX_ROWS {
            return Err(invalid("import rowset count exceeds bound"));
        }
        codec::digest_token(&self.digest)
    }
}

/// Fixed-size header, never inline imported history or namespace rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ImportClosure {
    pub snapshots: ImportRowset,
    pub selections: ImportRowset,
    pub captures: ImportRowset,
    pub publications: ImportRowset,
    pub catalogs: ImportRowset,
    pub descriptors: ImportRowset,
    pub activation_maps: ImportRowset,
    pub transition_receipts: ImportRowset,
    pub companions: ImportRowset,
    /// Global normalized SQL rows over every inherited companion.
    pub companion_files: ImportRowset,
    pub companion_deletions: ImportRowset,
    pub content_mappings: ImportRowset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_history: Option<ImportRowset>,
}
impl ImportClosure {
    fn validate(&self) -> Result<()> {
        let rowsets = [
            &self.snapshots,
            &self.selections,
            &self.captures,
            &self.publications,
            &self.catalogs,
            &self.descriptors,
            &self.activation_maps,
            &self.transition_receipts,
            &self.companions,
            &self.companion_files,
            &self.companion_deletions,
            &self.content_mappings,
        ];
        let mut total = 0u64;
        for rowset in rowsets.into_iter().chain(self.restore_history.iter()) {
            rowset.validate()?;
            total = codec::add(total, rowset.count)?;
        }
        if self.snapshots.count == 0 || self.selections.count == 0 || total > MAX_ROWS {
            return Err(invalid("import immutable closure count exceeds bound"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ImportVersion {
    pub generation: u64,
    pub snapshot_digest: String,
    pub selection_digest: String,
    pub capture_count: u64,
    pub capture_head: Option<String>,
    pub effective_image_digest: String,
}
impl ImportVersion {
    fn validate(&self) -> Result<()> {
        if self.generation > i64::MAX as u64 {
            return Err(invalid("import generation exceeds SQLite integer range"));
        }
        codec::raw_digest(&self.snapshot_digest)?;
        codec::raw_digest(&self.selection_digest)?;
        codec::raw_digest(&self.effective_image_digest)?;
        if self.capture_count > MAX_ROWS || self.capture_head.is_some() != (self.capture_count > 0)
        {
            return Err(invalid("import capture head/count differs"));
        }
        if let Some(head) = &self.capture_head {
            codec::digest_token(head)?;
        }
        Ok(())
    }
}

/// Provider variant carries explicit A/P/map DATA references; the future
/// interpreter must still read their canonical source receipts and FORMAT.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) enum ImportSource {
    Ordinary {
        identity: ProjectionIdentity,
        format_digest: String,
        private_schema: u32,
        version: ImportVersion,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkout_identity: Option<String>,
    },
    Provider {
        identity: ProjectionIdentity,
        format_digest: String,
        private_schema: u32,
        allocation: ReceiptRef,
        prepared: ReceiptRef,
        binding: ProviderBootstrapBinding,
        activation_map_id: String,
        version: ImportVersion,
    },
}
impl ImportSource {
    fn identity(&self) -> &ProjectionIdentity {
        match self {
            Self::Ordinary { identity, .. } | Self::Provider { identity, .. } => identity,
        }
    }
    fn version(&self) -> &ImportVersion {
        match self {
            Self::Ordinary { version, .. } | Self::Provider { version, .. } => version,
        }
    }
    fn validate(&self) -> Result<()> {
        format::validate_identity(self.identity())?;
        let (format_digest, schema) = match self {
            Self::Ordinary {
                format_digest,
                private_schema,
                ..
            }
            | Self::Provider {
                format_digest,
                private_schema,
                ..
            } => (format_digest, private_schema),
        };
        codec::digest_token(format_digest)?;
        if !matches!(schema, 6 | 7 | 8) {
            return Err(invalid("import source private schema unsupported"));
        }
        self.version().validate()?;
        if let Self::Ordinary {
            checkout_identity: Some(checkout_identity),
            ..
        } = self
        {
            codec::directory_identity(checkout_identity)?;
        }
        if let Self::Provider {
            allocation,
            prepared,
            binding,
            activation_map_id,
            private_schema,
            identity,
            ..
        } = self
        {
            if *private_schema != 8
                || binding.prepared.as_ref().map(|p| &p.receipt) != Some(prepared)
                || &binding.allocation != allocation
                || &binding.recovery.identity != identity
            {
                return Err(invalid("import provider receipt references differ"));
            }
            codec::digest_token(activation_map_id)?;
            binding.validate_fields()?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderImportPrefixRecord {
    pub schema_version: String,
    pub kind: ProviderImportKind,
    pub source: ImportSource,
    pub destination_identity: ProjectionIdentity,
    pub inherited_segments: ImportRowset,
    pub ancestry_root_receipt: Option<String>,
    pub closure: ImportClosure,
}

pub(in crate::projection) enum ImportSourceReferences<'a> {
    Ordinary(&'a format::Marker),
    Provider {
        allocation: &'a AllocationRecord,
        prepared: &'a PreparedRecord,
        activation: &'a ProviderActivationRecord,
    },
}

impl Record for ProviderImportPrefixRecord {
    const MAX_BYTES: usize = 16 * 1024 * 1024;
    fn validate_fields(&self) -> Result<()> {
        let has_checkout = match &self.source {
            ImportSource::Ordinary {
                checkout_identity, ..
            } => checkout_identity.is_some(),
            ImportSource::Provider { .. } => self.supports_restore_history(),
        };
        match self.schema_version.as_str() {
            PREFIX_SCHEMA if self.closure.restore_history.is_none() && !has_checkout => {}
            PREFIX_SCHEMA_V2 if self.closure.restore_history.is_some() && has_checkout => {}
            _ => {
                return Err(invalid(
                    "import prefix version/restore history fields differ",
                ))
            }
        }
        self.source.validate()?;
        format::validate_identity(&self.destination_identity)?;
        self.closure.validate()?;
        self.inherited_segments.validate()?;
        if self.inherited_segments.count > MAX_SEGMENTS as u64
            || self.ancestry_root_receipt.is_some() != (self.inherited_segments.count > 0)
            || self.closure.captures.count != self.source.version().capture_count
            || self.source.version().generation.checked_add(1) != Some(self.closure.snapshots.count)
            || self.closure.transition_receipts.count < self.inherited_segments.count
            || self.source.identity() == &self.destination_identity
        {
            return Err(invalid("import prefix boundaries differ"));
        }
        if let Some(root) = &self.ancestry_root_receipt {
            codec::digest_token(root)?;
        }
        Ok(())
    }
}
impl ProviderImportPrefixRecord {
    /// Old prefixes cannot authorize restore rows, including provider origins
    /// whose bootstrap binding already contains a checkout identity.
    pub fn supports_restore_history(&self) -> bool {
        self.schema_version == PREFIX_SCHEMA_V2
    }
    pub fn authenticated_checkout_identity(&self) -> Result<Option<&str>> {
        self.validate_fields()?;
        if !self.supports_restore_history() {
            return Ok(None);
        }
        Ok(match &self.source {
            ImportSource::Ordinary {
                checkout_identity, ..
            } => checkout_identity.as_deref(),
            ImportSource::Provider { binding, .. } => {
                Some(binding.recovery.checkout_identity.as_str())
            }
        })
    }
    pub fn digest(&self) -> Result<String> {
        let bytes = self.encode()?;
        let domain = match self.schema_version.as_str() {
            PREFIX_SCHEMA => PREFIX_DOMAIN,
            PREFIX_SCHEMA_V2 => PREFIX_DOMAIN_V2,
            _ => return Err(invalid("import prefix schemaVersion mismatch")),
        };
        Ok(codec::hash(domain, &bytes))
    }
    pub fn require_row(&self, digest: &str, payload: &[u8]) -> Result<()> {
        if digest != self.digest()? || payload != self.encode()? {
            return Err(invalid("import prefix SQL row differs"));
        }
        Ok(())
    }
    pub fn require_references(&self, source: ImportSourceReferences<'_>) -> Result<()> {
        self.validate_fields()?;
        match (&self.source, source) {
            (
                ImportSource::Ordinary {
                    identity,
                    format_digest,
                    checkout_identity,
                    ..
                },
                ImportSourceReferences::Ordinary(marker),
            ) => {
                marker.validate()?;
                if identity != &marker.identity
                    || checkout_identity
                        .as_deref()
                        .is_some_and(|checkout| checkout != marker.checkout_identity())
                    || format_digest
                        != &codec::hash(
                            "layrs-projection-provider-import-format-v1",
                            &marker.bytes()?,
                        )
                {
                    return Err(invalid("import ordinary source FORMAT differs"));
                }
            }
            (
                ImportSource::Provider {
                    identity,
                    format_digest,
                    allocation: a,
                    prepared: p,
                    binding,
                    activation_map_id,
                    version,
                    ..
                },
                ImportSourceReferences::Provider {
                    allocation,
                    prepared,
                    activation,
                },
            ) => {
                allocation.validate_fields()?;
                prepared.require_allocation(allocation)?;
                activation.validate_fields()?;
                if binding.require_references(allocation, Some(prepared), None)?
                    != ReceiptBindingCheck::NoTerminal
                {
                    return Err(invalid("import provider source is terminal"));
                }
                if identity != &allocation.format.identity
                    || format_digest
                        != &codec::hash(
                            "layrs-projection-provider-import-format-v1",
                            &allocation.format.bytes()?,
                        )
                    || a != &allocation.data_reference()?
                    || p != &prepared.data_reference()?
                    || activation_map_id != &activation.digest()?
                    || activation.binding != *binding
                    || activation.owner_identity_digest != owner_identity_digest(allocation)?
                    || activation.mount_id != allocation.transaction_id
                    || activation.source != allocation.source
                    || activation.catalog_id != allocation.source.catalog_id
                    || activation.activated_generation > version.generation
                    || (activation.activated_generation == version.generation
                        && activation.activated_digest != version.snapshot_digest)
                {
                    return Err(invalid("import provider A/P/map source differs"));
                }
            }
            _ => return Err(invalid("import source kind/reference differs")),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::projection) enum CompanionReason {
    UnknownPath,
    ExcludedPath,
    ProtectedChange,
    RevokedRights,
    RelocatedPlaceholder,
}
impl CompanionReason {
    fn token(self) -> &'static str {
        match self {
            Self::UnknownPath => "unknownPath",
            Self::ExcludedPath => "excludedPath",
            Self::ProtectedChange => "protectedChange",
            Self::RevokedRights => "revokedRights",
            Self::RelocatedPlaceholder => "relocatedPlaceholder",
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderCompanionHeaderRecord {
    pub schema_version: String,
    pub source_identity: ProjectionIdentity,
    pub source_private_schema: u32,
    pub source_generation: u64,
    pub source_capture_count: u64,
    pub source_capture_head: Option<String>,
    pub import_prefix_id: String,
    pub catalog_id: Option<String>,
    pub content_mappings: ImportRowset,
    pub file_count: u64,
    pub deletion_count: u64,
    pub files_digest: String,
    pub deletions_digest: String,
}
impl Record for ProviderCompanionHeaderRecord {
    const MAX_BYTES: usize = 64 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, COMPANION_SCHEMA)?;
        format::validate_identity(&self.source_identity)?;
        if !matches!(self.source_private_schema, 6 | 7 | 8)
            || self.source_generation > i64::MAX as u64
            || self.source_capture_count > MAX_ROWS
            || self.source_capture_head.is_some() != (self.source_capture_count > 0)
            || self.file_count > MAX_COMPANION_ROWS
            || self.deletion_count > MAX_COMPANION_ROWS
        {
            return Err(invalid("companion source or row count exceeds bound"));
        }
        if let Some(head) = &self.source_capture_head {
            codec::digest_token(head)?;
        }
        codec::digest_token(&self.import_prefix_id)?;
        if let Some(catalog) = &self.catalog_id {
            codec::digest_token(catalog)?;
        }
        self.content_mappings.validate()?;
        codec::digest_token(&self.files_digest)?;
        codec::digest_token(&self.deletions_digest)
    }
}
impl ProviderCompanionHeaderRecord {
    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(COMPANION_DOMAIN, &self.encode()?))
    }
    pub fn require_row(&self, row: CompanionHeaderRowData<'_>) -> Result<()> {
        if row.id != self.digest()?
            || row.file_count != self.file_count
            || row.deletion_count != self.deletion_count
            || row.files_digest != self.files_digest
            || row.deletions_digest != self.deletions_digest
            || row.payload != self.encode()?
        {
            return Err(invalid("companion SQL header projection differs"));
        }
        Ok(())
    }
    pub fn validate_rows(
        &self,
        files: &[ProviderCompanionFileRecord],
        deletions: &[ProviderCompanionDeletionRecord],
    ) -> Result<()> {
        self.validate_fields()?;
        if files.len() as u64 != self.file_count
            || deletions.len() as u64 != self.deletion_count
            || files.len() > MAX_COMPANION_ROWS as usize
            || deletions.len() > MAX_COMPANION_ROWS as usize
        {
            return Err(invalid("companion row count differs"));
        }
        let mut bytes = self.encode()?.len() as u64;
        let mut paths = BTreeSet::new();
        let mut file_refs = Vec::with_capacity(files.len());
        let mut deletion_refs = Vec::with_capacity(deletions.len());
        for file in files {
            let payload = file.encode()?;
            bytes = codec::add(bytes, payload.len() as u64 + 71)?;
            if bytes > MAX_COMPANION_METADATA || !paths.insert(file.path.as_str()) {
                return Err(invalid(
                    "companion metadata budget or path uniqueness differs",
                ));
            }
            file_refs.push((file.path.as_str(), file.digest()?));
        }
        for deletion in deletions {
            let payload = deletion.encode()?;
            bytes = codec::add(bytes, payload.len() as u64 + 71)?;
            if bytes > MAX_COMPANION_METADATA || !paths.insert(deletion.path.as_str()) {
                return Err(invalid(
                    "companion metadata budget or path uniqueness differs",
                ));
            }
            deletion_refs.push((deletion.path.as_str(), deletion.digest()?));
        }
        if ordered_companion_rowset(FILES_DOMAIN, &file_refs)? != self.files_digest
            || ordered_companion_rowset(DELETIONS_DOMAIN, &deletion_refs)? != self.deletions_digest
        {
            return Err(invalid("companion ordered rowset differs"));
        }
        Ok(())
    }
}

pub(in crate::projection) struct CompanionHeaderRowData<'a> {
    pub id: &'a str,
    pub file_count: u64,
    pub deletion_count: u64,
    pub files_digest: &'a str,
    pub deletions_digest: &'a str,
    pub payload: &'a [u8],
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderCompanionFileRecord {
    pub schema_version: String,
    pub path: RepoPath,
    pub file_id: FileId,
    pub bytes: u64,
    pub executable: bool,
    pub manifest_id: FileManifestId,
    pub base_file_id: Option<FileId>,
    pub base_executable: Option<bool>,
    pub reason: CompanionReason,
}
impl Record for ProviderCompanionFileRecord {
    const MAX_BYTES: usize = 8192;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, FILE_SCHEMA)?;
        path(&self.path)?;
        if self.bytes > crate::projection::capture::MAX_CAPTURE_FILE_BYTES
            || self.base_file_id.is_some() != self.base_executable.is_some()
        {
            return Err(invalid("companion file length or base pair differs"));
        }
        Ok(())
    }
}
impl ProviderCompanionFileRecord {
    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(FILE_DOMAIN, &self.encode()?))
    }
    pub fn require_row(
        &self,
        expected_companion_id: &str,
        row: CompanionFileRowData<'_>,
    ) -> Result<()> {
        codec::digest_token(expected_companion_id)?;
        if row.companion_id != expected_companion_id
            || row.path != self.path.as_str()
            || row.file_id != self.file_id.as_str()
            || row.bytes != self.bytes
            || row.executable != self.executable
            || row.manifest_id != self.manifest_id.as_str()
            || row.base_file_id != self.base_file_id.as_ref().map(FileId::as_str)
            || row.base_executable != self.base_executable
            || row.reason != self.reason.token()
            || row.digest != self.digest()?
        {
            return Err(invalid("companion file SQL projection differs"));
        }
        Ok(())
    }
}
pub(in crate::projection) struct CompanionFileRowData<'a> {
    pub companion_id: &'a str,
    pub path: &'a str,
    pub file_id: &'a str,
    pub bytes: u64,
    pub executable: bool,
    pub manifest_id: &'a str,
    pub base_file_id: Option<&'a str>,
    pub base_executable: Option<bool>,
    pub reason: &'a str,
    pub digest: &'a str,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderCompanionDeletionRecord {
    pub schema_version: String,
    pub path: RepoPath,
    pub base_file_id: FileId,
    pub base_executable: bool,
    pub base_manifest_id: Option<FileManifestId>,
    pub reason: CompanionReason,
}
impl Record for ProviderCompanionDeletionRecord {
    const MAX_BYTES: usize = 8192;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, DELETION_SCHEMA)?;
        path(&self.path)
    }
}
impl ProviderCompanionDeletionRecord {
    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(DELETION_DOMAIN, &self.encode()?))
    }
    pub fn require_row(
        &self,
        expected_companion_id: &str,
        row: CompanionDeletionRowData<'_>,
    ) -> Result<()> {
        codec::digest_token(expected_companion_id)?;
        if row.companion_id != expected_companion_id
            || row.path != self.path.as_str()
            || row.base_file_id != self.base_file_id.as_str()
            || row.base_executable != self.base_executable
            || row.base_manifest_id != self.base_manifest_id.as_ref().map(FileManifestId::as_str)
            || row.reason != self.reason.token()
            || row.digest != self.digest()?
        {
            return Err(invalid("companion deletion SQL projection differs"));
        }
        Ok(())
    }
}
pub(in crate::projection) struct CompanionDeletionRowData<'a> {
    pub companion_id: &'a str,
    pub path: &'a str,
    pub base_file_id: &'a str,
    pub base_executable: bool,
    pub base_manifest_id: Option<&'a str>,
    pub reason: &'a str,
    pub digest: &'a str,
}

fn path(value: &RepoPath) -> Result<()> {
    if value.as_str().is_empty() || value.as_str().len() > codec::PATH_MAX {
        return Err(invalid("companion path exceeds UTF-8 byte bound"));
    }
    RepoPath::parse(value.as_str()).map_err(invalid).map(|_| ())
}

/// Exact schema-8 rowset encoding; callers supply SQL BINARY path order.
pub(in crate::projection) fn ordered_companion_rowset(
    domain: &'static str,
    rows: &[(&str, String)],
) -> Result<String> {
    if rows.len() > MAX_COMPANION_ROWS as usize
        || !matches!(domain, FILES_DOMAIN | DELETIONS_DOMAIN)
    {
        return Err(invalid("companion rowset kind or count invalid"));
    }
    let mut h = blake3::Hasher::new();
    h.update(domain.as_bytes());
    h.update(&[0]);
    h.update(&(rows.len() as u64).to_be_bytes());
    let mut previous = None;
    for (path, digest) in rows {
        if path.is_empty()
            || path.len() > codec::PATH_MAX
            || previous.is_some_and(|p: &str| p.as_bytes() >= path.as_bytes())
        {
            return Err(invalid("companion rowset path order differs"));
        }
        codec::digest_token(digest)?;
        h.update(&(path.len() as u64).to_be_bytes());
        h.update(path.as_bytes());
        h.update(digest.as_bytes());
        previous = Some(*path);
    }
    Ok(format!("blake3:{}", h.finalize()))
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ProviderTransitionReceiptRecord {
    pub schema_version: String,
    pub owner_identity_digest: String,
    pub transaction_id: String,
    /// Digest of the complete canonical transition plan, independent of this receipt.
    pub transition_plan_digest: String,
    /// Digest of the verified destination proof at the import boundary.
    pub destination_proof_digest: String,
    pub source_prefix_id: String,
    pub source_prefix: ProviderImportPrefixRecord,
    pub parent_receipt_id: Option<String>,
    pub source_identity: ProjectionIdentity,
    pub destination_identity: ProjectionIdentity,
    pub imported_max_generation: u64,
    pub destination_observation_generation: u64,
    pub destination_observation_digest: String,
    pub destination_placement_generation: u64,
    pub destination_placement_digest: String,
    pub imported_capture_count: u64,
    pub destination_capture_sequence: u64,
    pub destination_capture_digest: String,
    pub protected_manifests: ImportRowset,
    pub content_mappings: ImportRowset,
    pub companion_id: Option<String>,
    /// This companion's ordered path+row-digest commitments, identical to
    /// `ProviderCompanionHeaderRecord.files_digest/deletions_digest`.
    pub companion_files: ImportRowset,
    pub companion_deletions: ImportRowset,
}
impl Record for ProviderTransitionReceiptRecord {
    const MAX_BYTES: usize = 64 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, RECEIPT_SCHEMA)?;
        codec::digest_token(&self.owner_identity_digest)?;
        codec::digest_token(&self.transition_plan_digest)?;
        codec::digest_token(&self.destination_proof_digest)?;
        format::validate_identity(&self.source_identity)?;
        format::validate_identity(&self.destination_identity)?;
        if self.transaction_id.is_empty()
            || self.transaction_id.len() > 128
            || self.transaction_id.bytes().any(|byte| byte == 0)
            || self.source_identity == self.destination_identity
            || self.destination_observation_generation <= self.imported_max_generation
            || self.destination_placement_generation <= self.destination_observation_generation
            || self.destination_capture_sequence <= self.imported_capture_count
            || self.destination_placement_generation > i64::MAX as u64
            || self.destination_capture_sequence > i64::MAX as u64
        {
            return Err(invalid(
                "transition receipt identity or generation boundary differs",
            ));
        }
        codec::digest_token(&self.source_prefix_id)?;
        self.source_prefix.validate_fields()?;
        if self.source_prefix_id != self.source_prefix.digest()?
            || self.parent_receipt_id != self.source_prefix.ancestry_root_receipt
            || self.source_identity != *self.source_prefix.source.identity()
            || self.destination_identity != self.source_prefix.destination_identity
            || self.imported_max_generation != self.source_prefix.source.version().generation
            || self.imported_capture_count != self.source_prefix.source.version().capture_count
            || self.content_mappings != self.source_prefix.closure.content_mappings
        {
            return Err(invalid("transition receipt embedded source prefix differs"));
        }
        if let Some(parent) = &self.parent_receipt_id {
            codec::digest_token(parent)?;
        }
        codec::raw_digest(&self.destination_observation_digest)?;
        codec::raw_digest(&self.destination_placement_digest)?;
        codec::digest_token(&self.destination_capture_digest)?;
        self.protected_manifests.validate()?;
        self.content_mappings.validate()?;
        self.companion_files.validate()?;
        self.companion_deletions.validate()?;
        if self
            .protected_manifests
            .count
            .checked_add(self.content_mappings.count)
            .and_then(|n| n.checked_add(self.companion_files.count))
            .and_then(|n| n.checked_add(self.companion_deletions.count))
            .is_none_or(|n| n > MAX_ROWS)
            || self.companion_id.is_none()
                && (self.companion_files.count != 0 || self.companion_deletions.count != 0)
        {
            return Err(invalid("transition receipt closure exceeds bound"));
        }
        if let Some(companion) = &self.companion_id {
            codec::digest_token(companion)?;
        }
        Ok(())
    }
}
impl ProviderTransitionReceiptRecord {
    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(RECEIPT_DOMAIN, &self.encode()?))
    }
    pub fn require_row(&self, row: TransitionReceiptRowData<'_>) -> Result<()> {
        if row.id != self.digest()?
            || row.owner_identity_digest != self.owner_identity_digest
            || row.transaction_id != self.transaction_id
            || row.companion_id != self.companion_id.as_deref()
            || row.payload != self.encode()?
        {
            return Err(invalid("transition receipt SQL projection differs"));
        }
        Ok(())
    }
    pub fn require_prefix(&self, prefix: &ProviderImportPrefixRecord) -> Result<()> {
        self.validate_fields()?;
        prefix.validate_fields()?;
        if &self.source_prefix != prefix
            || self.source_prefix_id != prefix.digest()?
            || self.source_identity != *prefix.source.identity()
            || self.destination_identity != prefix.destination_identity
            || self.imported_max_generation != prefix.source.version().generation
            || self.imported_capture_count != prefix.source.version().capture_count
            || self.content_mappings != prefix.closure.content_mappings
        {
            return Err(invalid("transition receipt imported prefix differs"));
        }
        Ok(())
    }

    pub fn require_companion(&self, header: &ProviderCompanionHeaderRecord) -> Result<()> {
        self.validate_fields()?;
        header.validate_fields()?;
        let header_id = header.digest()?;
        if self.companion_id.as_deref() != Some(header_id.as_str())
            || self.companion_files.count != header.file_count
            || self.companion_files.digest != header.files_digest
            || self.companion_deletions.count != header.deletion_count
            || self.companion_deletions.digest != header.deletions_digest
        {
            return Err(invalid("transition receipt companion rowsets differ"));
        }
        Ok(())
    }
}
pub(in crate::projection) struct TransitionReceiptRowData<'a> {
    pub id: &'a str,
    pub owner_identity_digest: &'a str,
    pub transaction_id: &'a str,
    pub companion_id: Option<&'a str>,
    pub payload: &'a [u8],
}

#[cfg(test)]
#[path = "import/tests.rs"]
mod tests;

/// Transient DATA projection of normalized inherited transition receipts.
/// The prefix holds only its count/digest/root receipt, never this vector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct ImportedSegment {
    pub receipt_id: String,
    pub parent_receipt_id: Option<String>,
    pub source_identity: ProjectionIdentity,
    pub first_generation: u64,
    pub last_generation: u64,
    /// Open lower bound: captured sequences n with before < n <= through.
    pub capture_count_before: u64,
    pub capture_count_through: u64,
}
impl ImportedSegment {
    /// Derive only from a canonical immutable receipt and its exact parent.
    pub fn from_receipt(
        receipt: &ProviderTransitionReceiptRecord,
        parent: Option<&ProviderTransitionReceiptRecord>,
    ) -> Result<Self> {
        receipt.validate_fields()?;
        let (first_generation, capture_count_before, parent_receipt_id) = match parent {
            Some(parent) => {
                parent.validate_fields()?;
                let parent_id = parent.digest()?;
                if receipt.parent_receipt_id.as_deref() != Some(parent_id.as_str())
                    || parent.destination_identity != receipt.source_identity
                {
                    return Err(invalid("import parent receipt identity differs"));
                }
                (
                    parent
                        .imported_max_generation
                        .checked_add(1)
                        .ok_or_else(|| invalid("import generation interval overflow"))?,
                    parent.imported_capture_count,
                    Some(parent_id),
                )
            }
            None => {
                if receipt.parent_receipt_id.is_some() {
                    return Err(invalid("import ancestry parent receipt absent"));
                }
                (0, 0, None)
            }
        };
        let segment = Self {
            receipt_id: receipt.digest()?,
            parent_receipt_id,
            source_identity: receipt.source_identity.clone(),
            first_generation,
            last_generation: receipt.imported_max_generation,
            capture_count_before,
            capture_count_through: receipt.imported_capture_count,
        };
        if segment.first_generation > segment.last_generation
            || segment.capture_count_before > segment.capture_count_through
        {
            return Err(invalid("import receipt interval is empty or reversed"));
        }
        Ok(segment)
    }
}
impl ProviderImportPrefixRecord {
    pub fn validate_segments(&self, segments: &[ImportedSegment]) -> Result<()> {
        self.validate_fields()?;
        if segments.len() > MAX_SEGMENTS || segments.len() as u64 != self.inherited_segments.count {
            return Err(invalid("import ancestry count differs"));
        }
        let mut seen = BTreeSet::new();
        let mut previous_generation: Option<u64> = None;
        let mut previous_capture: Option<u64> = None;
        let mut parent = None;
        let mut hasher = blake3::Hasher::new();
        hasher.update(SEGMENTS_DOMAIN.as_bytes());
        hasher.update(&[0]);
        hasher.update(&(segments.len() as u64).to_be_bytes());
        let mut metadata_bytes = 0u64;
        for segment in segments {
            format::validate_identity(&segment.source_identity)?;
            codec::digest_token(&segment.receipt_id)?;
            if segment.parent_receipt_id.as_ref() != parent
                || !seen.insert(segment.receipt_id.as_str())
                || segment.first_generation > segment.last_generation
                || segment.capture_count_before > segment.capture_count_through
                || previous_generation.map_or(segment.first_generation != 0, |end| {
                    end.checked_add(1) != Some(segment.first_generation)
                })
                || previous_capture.map_or(segment.capture_count_before != 0, |through| {
                    through != segment.capture_count_before
                })
            {
                return Err(invalid("import ancestry cycle, order or interval overlap"));
            }
            codec::preflight(segment, 4096)?;
            let bytes = canonical_json(segment).map_err(invalid)?;
            metadata_bytes = codec::add(metadata_bytes, bytes.len() as u64 + 8)?;
            if bytes.len() > 4096 || metadata_bytes > 16 * 1024 * 1024 {
                return Err(invalid("import ancestry segment exceeds bound"));
            }
            hasher.update(&(bytes.len() as u64).to_be_bytes());
            hasher.update(&bytes);
            parent = Some(&segment.receipt_id);
            previous_generation = Some(segment.last_generation);
            previous_capture = Some(segment.capture_count_through);
        }
        if previous_generation.is_some_and(|end| end > self.source.version().generation)
            || previous_capture.is_some_and(|end| end > self.source.version().capture_count)
            || parent != self.ancestry_root_receipt.as_ref()
            || format!("blake3:{}", hasher.finalize()) != self.inherited_segments.digest
        {
            return Err(invalid("import ancestry commitment differs"));
        }
        Ok(())
    }
}
