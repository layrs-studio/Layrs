//! Companion v2 DATA. The existing SQL header BLOB carries every new intent;
//! deletion scalar rows alone must never be used to reconstruct provenance.
//! No schema8 DDL is changed. An old v1 decoder rejects this header verbatim.
use super::{Record, codec, import::*};
use crate::projection::{ProjectionIdentity, Result, capture::CapturedChange, format, invalid};
use layrs_core::scm::{FileId, FileManifestId, RepoPath};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::projection) const SCHEMA: &str = "layrs.projection-provider-companion.v2";
const DOMAIN: &str = "layrs-projection-provider-companion-v2";
const FILES: &str = "layrs-projection-provider-companion-files-v1";
const DELETIONS: &str = "layrs-projection-provider-companion-deletions-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct CompanionBaseV2 {
    pub file_id: FileId,
    pub executable: bool,
}

/// Exact encodings that belong to preserved work, including alternate
/// ManifestIds for a FileId whose historical projection_content PK is sticky.
/// Persisting these references is essential: the current native image may no
/// longer contain a companion when its ordinary descendant is re-exported.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct CompanionRepresentationV2 {
    pub file_id: FileId,
    pub manifest_id: FileManifestId,
    pub bytes: u64,
}

/// A reference to an EXACT accepted Capture, not a replacement Capture or a
/// claim of authority. The importer must resolve and verify the original patch
/// at sequence/digest, its origin interval, snapshot and original expected base.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct AcceptedDeletionV2 {
    pub source_prefix_id: String,
    pub source_generation: u64,
    pub source_snapshot_digest: String,
    pub source_capture_count: u64,
    pub source_capture_head: String,
    pub sequence: u64,
    pub capture_digest: String,
    pub change: CapturedChange,
    pub preimage_manifest_id: FileManifestId,
    pub preimage_bytes: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::projection) enum CompanionIntentV2 {
    /// Has a legacy-shaped deletion scalar row, but effective base is ABSENT.
    /// Its original base is only justified by this accepted Capture reference.
    RecoveryAcceptedDeletion {
        path: RepoPath,
        reason: CompanionReason,
        accepted: AcceptedDeletionV2,
    },
    /// No deletion scalar row and no None -> None Capture change is fabricated.
    /// This durable intent is included in the header ID bound by the receipt.
    SatisfiedDeletion {
        path: RepoPath,
        source_effective_base: Option<CompanionBaseV2>,
        accepted: Option<AcceptedDeletionV2>,
        destination_generation: u64,
        destination_snapshot_digest: String,
    },
}
impl CompanionIntentV2 {
    pub fn path(&self) -> &RepoPath {
        match self {
            Self::RecoveryAcceptedDeletion { path, .. } | Self::SatisfiedDeletion { path, .. } => {
                path
            }
        }
    }
    pub fn accepted(&self) -> Option<&AcceptedDeletionV2> {
        match self {
            Self::RecoveryAcceptedDeletion { accepted, .. } => Some(accepted),
            Self::SatisfiedDeletion { accepted, .. } => accepted.as_ref(),
        }
    }
}

/// The 64KiB header bound is deliberately unchanged. Intent metadata is bounded
/// BEFORE canonical allocation, not silently spilled into an unowned sidecar.
/// A conversion exceeding that bound stays unready; a larger format needs a
/// separately versioned schema/migration, not a durable-quota increase.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct CompanionHeaderV2 {
    pub schema_version: String,
    pub source_identity: ProjectionIdentity,
    pub source_private_schema: u32,
    pub source_generation: u64,
    pub source_snapshot_digest: String,
    pub source_capture_count: u64,
    pub source_capture_head: Option<String>,
    pub import_prefix_id: String,
    pub catalog_id: Option<String>,
    pub content_mappings: ImportRowset,
    pub file_count: u64,
    pub deletion_count: u64,
    pub files_digest: String,
    pub deletions_digest: String,
    pub preserved_representations: Vec<CompanionRepresentationV2>,
    pub intents: Vec<CompanionIntentV2>,
}
impl Record for CompanionHeaderV2 {
    const MAX_BYTES: usize = 64 * 1024;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, SCHEMA)?;
        format::validate_identity(&self.source_identity)?;
        if !matches!(self.source_private_schema, 6 | 7 | 8 | 9 | 10)
            || self.source_generation > i64::MAX as u64
            || self.source_capture_count > 4096
            || self.source_capture_head.is_some() != (self.source_capture_count > 0)
            || self.file_count > 100_000
            || self.deletion_count > 100_000
            || self.intents.len() > 100_000
            || self.preserved_representations.len() > 100_000
            || self.content_mappings.count > 1_000_000
        {
            return Err(invalid("companion v2 source/count exceeds bound"));
        }
        codec::raw_digest(&self.source_snapshot_digest)?;
        codec::digest_token(&self.import_prefix_id)?;
        for token in [self.catalog_id.as_ref(), self.source_capture_head.as_ref()]
            .into_iter()
            .flatten()
        {
            codec::digest_token(token)?;
        }
        for token in [
            &self.files_digest,
            &self.deletions_digest,
            &self.content_mappings.digest,
        ] {
            codec::digest_token(token)?;
        }
        let mut previous_representation = None;
        let mut lengths = BTreeMap::new();
        let mut manifest_owners = BTreeMap::new();
        for representation in &self.preserved_representations {
            let key = (&representation.file_id, &representation.manifest_id);
            if previous_representation.is_some_and(|previous| previous >= key)
                || representation.bytes > crate::projection::capture::MAX_CAPTURE_FILE_BYTES
                || lengths
                    .insert(&representation.file_id, representation.bytes)
                    .is_some_and(|old| old != representation.bytes)
                || manifest_owners
                    .insert(&representation.manifest_id, &representation.file_id)
                    .is_some_and(|old| old != &representation.file_id)
            {
                return Err(invalid(
                    "companion v2 exact representation order/identity differs",
                ));
            }
            previous_representation = Some(key);
        }
        let mut previous = None;
        for intent in &self.intents {
            let path = intent.path();
            if path.as_str().is_empty()
                || path.as_str().len() > 4096
                || previous.is_some_and(|p: &RepoPath| p >= path)
            {
                return Err(invalid("companion v2 intent order/path differs"));
            }
            previous = Some(path);
            if let Some(accepted) = intent.accepted() {
                codec::digest_token(&accepted.source_prefix_id)?;
                codec::raw_digest(&accepted.source_snapshot_digest)?;
                codec::digest_token(&accepted.source_capture_head)?;
                codec::digest_token(&accepted.capture_digest)?;
                if accepted.source_prefix_id != self.import_prefix_id
                    || accepted.source_generation != self.source_generation
                    || accepted.source_snapshot_digest != self.source_snapshot_digest
                    || accepted.source_capture_count != self.source_capture_count
                    || Some(&accepted.source_capture_head) != self.source_capture_head.as_ref()
                    || accepted.sequence == 0
                    || accepted.sequence != self.source_capture_count
                    || accepted.capture_digest != accepted.source_capture_head
                    || accepted.change.path != *path
                    || accepted.change.expected.is_none()
                    || accepted.change.replacement.is_some()
                    || accepted.preimage_bytes > crate::projection::capture::MAX_CAPTURE_FILE_BYTES
                {
                    return Err(invalid("companion v2 accepted deletion anchor differs"));
                }
            }
            if let CompanionIntentV2::SatisfiedDeletion {
                source_effective_base,
                accepted,
                destination_generation,
                destination_snapshot_digest,
                ..
            } = intent
            {
                if source_effective_base.is_some() == accepted.is_some()
                    || *destination_generation <= self.source_generation
                    || *destination_generation >= i64::MAX as u64
                {
                    return Err(invalid(
                        "companion v2 satisfied deletion provenance differs",
                    ));
                }
                codec::raw_digest(destination_snapshot_digest)?;
            }
        }
        Ok(())
    }
}
impl CompanionHeaderV2 {
    pub fn digest(&self) -> Result<String> {
        Ok(codec::hash(DOMAIN, &self.encode()?))
    }

    pub fn require_prefix(&self, prefix: &ProviderImportPrefixRecord) -> Result<()> {
        self.encode()?;
        prefix.encode()?;
        let (identity, schema, version, catalog_is_allowed) = match &prefix.source {
            ImportSource::Provider {
                identity,
                private_schema,
                version,
                ..
            } => (identity, private_schema, version, true),
            ImportSource::Ordinary {
                identity,
                private_schema,
                version,
                ..
            } => (identity, private_schema, version, false),
        };
        if &self.source_identity != identity
            || self.source_private_schema != *schema
            || self.source_generation != version.generation
            || self.source_snapshot_digest != version.snapshot_digest
            || self.source_capture_count != version.capture_count
            || self.source_capture_head != version.capture_head
            || self.import_prefix_id != prefix.digest()?
            || self.content_mappings != prefix.closure.content_mappings
            || self.catalog_id.is_some() != catalog_is_allowed
        {
            return Err(invalid("companion v2 exact source prefix differs"));
        }
        Ok(())
    }

    pub fn require_receipt(&self, receipt: &ProviderTransitionReceiptRecord) -> Result<()> {
        receipt.require_prefix(&receipt.source_prefix)?;
        self.require_prefix(&receipt.source_prefix)?;
        if receipt.companion_id.as_deref() != Some(self.digest()?.as_str())
            || receipt.companion_files.count != self.file_count
            || receipt.companion_files.digest != self.files_digest
            || receipt.companion_deletions.count != self.deletion_count
            || receipt.companion_deletions.digest != self.deletions_digest
        {
            return Err(invalid("companion v2 receipt rowsets differ"));
        }
        for intent in &self.intents {
            if let CompanionIntentV2::SatisfiedDeletion {
                destination_generation,
                destination_snapshot_digest,
                ..
            } = intent
            {
                if *destination_generation != receipt.destination_observation_generation
                    || *destination_snapshot_digest != receipt.destination_observation_digest
                {
                    return Err(invalid(
                        "companion v2 satisfied observation/receipt differs",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn validate_rows(
        &self,
        files: &[ProviderCompanionFileRecord],
        deletions: &[ProviderCompanionDeletionRecord],
    ) -> Result<()> {
        self.encode()?;
        if files.len() as u64 != self.file_count || deletions.len() as u64 != self.deletion_count {
            return Err(invalid("companion v2 row cardinality differs"));
        }
        let mut paths = BTreeSet::new();
        let mut bytes = self.encode()?.len() as u64;
        let mut file_refs = Vec::with_capacity(files.len());
        let mut deletion_refs = Vec::with_capacity(deletions.len());
        let mut needed = BTreeSet::new();
        for row in files {
            bytes = codec::add(bytes, row.encode()?.len() as u64 + 71)?;
            if !paths.insert(&row.path) {
                return Err(invalid("companion v2 repeated path"));
            }
            file_refs.push((row.path.as_str(), row.digest()?));
            needed.insert(&row.file_id);
            needed.extend(row.base_file_id.iter());
            self.require_representation(&row.file_id, &row.manifest_id, row.bytes)?;
        }
        let mut deletion_by_path = BTreeMap::new();
        for row in deletions {
            bytes = codec::add(bytes, row.encode()?.len() as u64 + 71)?;
            if !paths.insert(&row.path) {
                return Err(invalid("companion v2 repeated path"));
            }
            deletion_refs.push((row.path.as_str(), row.digest()?));
            deletion_by_path.insert(&row.path, row);
            needed.insert(&row.base_file_id);
        }
        if bytes > 1024 * 1024 * 1024
            || ordered_companion_rowset(FILES, &file_refs)? != self.files_digest
            || ordered_companion_rowset(DELETIONS, &deletion_refs)? != self.deletions_digest
        {
            return Err(invalid("companion v2 ordered rows/budget differ"));
        }
        for intent in &self.intents {
            if let Some(accepted) = intent.accepted() {
                let expected = accepted
                    .change
                    .expected
                    .as_ref()
                    .ok_or_else(|| invalid("companion v2 accepted preimage absent"))?;
                needed.insert(&expected.file_id);
                self.require_representation(
                    &expected.file_id,
                    &accepted.preimage_manifest_id,
                    accepted.preimage_bytes,
                )?;
            }
            if let CompanionIntentV2::SatisfiedDeletion {
                source_effective_base: Some(base),
                ..
            } = intent
            {
                needed.insert(&base.file_id);
            }
            match intent {
                CompanionIntentV2::RecoveryAcceptedDeletion {
                    path,
                    reason,
                    accepted,
                } => {
                    let row = deletion_by_path
                        .get(path)
                        .ok_or_else(|| invalid("companion v2 accepted deletion scalar omitted"))?;
                    let expected = accepted
                        .change
                        .expected
                        .as_ref()
                        .ok_or_else(|| invalid("companion v2 preimage absent"))?;
                    if row.base_file_id != expected.file_id
                        || row.base_executable != expected.executable
                        || row.base_manifest_id.as_ref() != Some(&accepted.preimage_manifest_id)
                        || row.reason != *reason
                    {
                        return Err(invalid("companion v2 accepted scalar/provenance differ"));
                    }
                }
                CompanionIntentV2::SatisfiedDeletion { path, .. } if paths.contains(path) => {
                    return Err(invalid(
                        "companion v2 satisfied deletion conflicts with scalar row",
                    ));
                }
                _ => {}
            }
        }
        if self
            .preserved_representations
            .iter()
            .any(|representation| !needed.contains(&representation.file_id))
            || needed.iter().any(|file| {
                !self
                    .preserved_representations
                    .iter()
                    .any(|representation| &representation.file_id == *file)
            })
        {
            return Err(invalid(
                "companion v2 preserved representation membership differs",
            ));
        }
        Ok(())
    }

    pub fn require_representation(
        &self,
        file: &FileId,
        manifest: &FileManifestId,
        bytes: u64,
    ) -> Result<()> {
        if !self
            .preserved_representations
            .iter()
            .any(|r| &r.file_id == file && &r.manifest_id == manifest && r.bytes == bytes)
        {
            return Err(invalid(
                "companion v2 exact preserved representation omitted",
            ));
        }
        Ok(())
    }
}

/// Typed canonical dispatch, with duplicate/unknown-field rejection in both
/// branches. Never decode via Value (which would erase duplicate keys).
#[derive(PartialEq, Eq)]
pub(in crate::projection) enum CompanionHeaderRecord {
    V1(ProviderCompanionHeaderRecord),
    V2(CompanionHeaderV2),
}

/// Borrow the original variant without cloning, normalizing its source schema,
/// or laundering a v2 intention through a v1 effective-base header.
#[derive(Clone, Copy)]
pub(in crate::projection) enum CompanionHeaderRef<'a> {
    V1(&'a ProviderCompanionHeaderRecord),
    V2(&'a CompanionHeaderV2),
}

pub(in crate::projection) struct CompanionHeaderColumns<'a> {
    pub file_count: u64,
    pub deletion_count: u64,
    pub files_digest: &'a str,
    pub deletions_digest: &'a str,
}

impl<'a> CompanionHeaderRef<'a> {
    pub fn encode(self) -> Result<Vec<u8>> {
        match self {
            Self::V1(header) => header.encode(),
            Self::V2(header) => header.encode(),
        }
    }
    pub fn digest(self) -> Result<String> {
        match self {
            Self::V1(header) => header.digest(),
            Self::V2(header) => header.digest(),
        }
    }
    pub fn columns(self) -> CompanionHeaderColumns<'a> {
        match self {
            Self::V1(header) => CompanionHeaderColumns {
                file_count: header.file_count,
                deletion_count: header.deletion_count,
                files_digest: &header.files_digest,
                deletions_digest: &header.deletions_digest,
            },
            Self::V2(header) => CompanionHeaderColumns {
                file_count: header.file_count,
                deletion_count: header.deletion_count,
                files_digest: &header.files_digest,
                deletions_digest: &header.deletions_digest,
            },
        }
    }
    pub fn validate_rows(
        self,
        files: &[ProviderCompanionFileRecord],
        deletions: &[ProviderCompanionDeletionRecord],
    ) -> Result<()> {
        match self {
            Self::V1(header) => header.validate_rows(files, deletions),
            Self::V2(header) => header.validate_rows(files, deletions),
        }
    }
    pub fn require_receipt(self, receipt: &ProviderTransitionReceiptRecord) -> Result<()> {
        match self {
            Self::V1(header) => receipt.require_companion(header),
            Self::V2(header) => header.require_receipt(receipt),
        }
    }
}

impl CompanionHeaderRecord {
    pub fn as_ref(&self) -> CompanionHeaderRef<'_> {
        match self {
            Self::V1(header) => CompanionHeaderRef::V1(header),
            Self::V2(header) => CompanionHeaderRef::V2(header),
        }
    }

    pub fn decode(payload: &[u8]) -> Result<Self> {
        if payload.len() > CompanionHeaderV2::MAX_BYTES || payload.is_empty() {
            return Err(invalid("companion header byte bound differs"));
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Version {
            schema_version: String,
        }
        let version: Version = serde_json::from_slice(payload).map_err(invalid)?;
        match version.schema_version.as_str() {
            "layrs.projection-provider-companion.v1" => {
                Ok(Self::V1(ProviderCompanionHeaderRecord::decode(payload)?))
            }
            SCHEMA => Ok(Self::V2(CompanionHeaderV2::decode(payload)?)),
            _ => Err(invalid("unsupported companion header codec")),
        }
    }
}
