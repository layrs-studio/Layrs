use super::{Record, Result, codec, invalid};
use crate::projection::{database, provider, selection};
use layrs_core::{
    projection::{EntryState, PathSemantics, ProjectionSelection, ProjectionSnapshot},
    scm::{FileId, FileManifest, FileManifestId, RepoPath, TreeEntryTarget},
};
use layrs_vfs::{Catalog, CatalogEntry};
use serde::{Deserialize, Serialize};

pub(super) const DESCRIPTORS_DOMAIN: &str = "layrs-projection-provider-descriptors-v1";
pub(super) const CATALOG_DOMAIN: &str = "layrs-projection-provider-catalog-v1";
const DESCRIPTOR_WIRE_MAX: usize =
    provider::PROVIDER_DESCRIPTOR_MAX_BYTES as usize + 6 * codec::PATH_MAX + 1024;

/// Owned input, not authority or a content-verification result. Deliberately
/// not Clone: a caller transfers the potentially large manifest collection.
pub struct ProviderSourceDescriptor {
    pub path: RepoPath,
    pub manifest: FileManifest,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct DescriptorRecord {
    pub schema_version: String,
    pub path: RepoPath,
    pub file_id: FileId,
    pub bytes: u64,
    pub executable: bool,
    pub manifest_id: FileManifestId,
    // A typed object, not Vec<u8>. SQL stores C(manifest), never C(Vec<u8>).
    pub manifest: FileManifest,
}
impl Record for DescriptorRecord {
    const MAX_BYTES: usize = DESCRIPTOR_WIRE_MAX;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(
            &self.schema_version,
            "layrs.projection-provider-descriptor.v1",
        )?;
        if self.path.as_str().is_empty()
            || self.path.as_str().len() > codec::PATH_MAX
            || &self.file_id != self.manifest.file_id()
            || &self.manifest_id != self.manifest.id()
            || self.bytes != self.manifest.byte_length()
            || self.bytes > i64::MAX as u64
        {
            return Err(invalid(
                "descriptor projection differs from manifest or exceeds bounds",
            ));
        }
        // The existing decoder recomputes the manifest ID and coverage. This
        // verifies metadata consistency, not the absent file/chunk bytes.
        self.manifest_bytes()?;
        Ok(())
    }
}
impl DescriptorRecord {
    pub fn manifest_bytes(&self) -> Result<Vec<u8>> {
        codec::encode(
            &self.manifest,
            provider::PROVIDER_DESCRIPTOR_MAX_BYTES as usize,
        )
    }
    pub fn logical_bytes(&self, manifest_bytes: usize) -> Result<u64> {
        codec::add(
            71 + 71 + 8 + 1 + 71,
            codec::add(self.path.as_str().len() as u64, manifest_bytes as u64)?,
        )
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct CatalogRecord {
    pub schema_version: String,
    pub source_generation: u64,
    pub source_digest: String,
    pub selection_digest: String,
    pub vfs_catalog_version: u64,
    pub vfs_catalog_id: FileId,
    pub descriptor_count: u64,
    pub descriptor_wire_bytes: u64,
    pub descriptor_bytes: u64,
    pub descriptors_digest: String,
}
impl Record for CatalogRecord {
    const MAX_BYTES: usize = provider::PROVIDER_CATALOG_MAX_BYTES as usize;
    fn validate_fields(&self) -> Result<()> {
        codec::schema(&self.schema_version, "layrs.projection-provider-catalog.v1")?;
        codec::raw_digest(&self.source_digest)?;
        codec::raw_digest(&self.selection_digest)?;
        codec::digest_token(&self.descriptors_digest)?;
        validate_counts(
            self.source_generation,
            self.vfs_catalog_version,
            self.descriptor_count,
            self.descriptor_bytes,
            self.descriptor_wire_bytes,
        )
    }
}
impl CatalogRecord {
    pub fn id(&self) -> Result<String> {
        Ok(codec::hash(CATALOG_DOMAIN, &self.encode()?))
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::projection) struct SourceCommitment {
    pub source_generation: u64,
    pub snapshot_bytes: u64,
    pub snapshot_digest: String,
    pub selection_bytes: u64,
    pub selection_payload_digest: String,
    pub selection_digest: String,
    pub catalog_bytes: u64,
    pub catalog_id: String,
    pub catalog_aggregate_bytes: u64,
    pub descriptor_count: u64,
    pub descriptor_bytes: u64,
    pub descriptor_wire_bytes: u64,
    pub descriptors_digest: String,
    pub vfs_catalog_version: u64,
    pub vfs_catalog_id: FileId,
}
impl SourceCommitment {
    /// Shape/accounting checks only. Exact counters/digests require comparison
    /// with ProviderBootstrapSource::commitment(), not this method alone.
    pub fn validate_fields(&self) -> Result<()> {
        validate_counts(
            self.source_generation,
            self.vfs_catalog_version,
            self.descriptor_count,
            self.descriptor_bytes,
            self.descriptor_wire_bytes,
        )?;
        for digest in [
            &self.snapshot_digest,
            &self.selection_payload_digest,
            &self.selection_digest,
        ] {
            codec::raw_digest(digest)?;
        }
        codec::digest_token(&self.catalog_id)?;
        codec::digest_token(&self.descriptors_digest)?;
        if self.snapshot_bytes == 0
            || self.snapshot_bytes > codec::SNAPSHOT_MAX as u64
            || self.selection_bytes == 0
            || self.selection_bytes > codec::SELECTION_MAX as u64
            || self.catalog_bytes == 0
            || self.catalog_bytes > provider::PROVIDER_CATALOG_MAX_BYTES
            || self.catalog_aggregate_bytes
                != codec::add(self.catalog_bytes, self.descriptor_bytes)?
            || self.catalog_aggregate_bytes > provider::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES
        {
            return Err(invalid("source commitment budget/accounting mismatch"));
        }
        Ok(())
    }
}

fn validate_counts(
    generation: u64,
    version: u64,
    count: u64,
    logical: u64,
    wire: u64,
) -> Result<()> {
    if generation != 0
        || version != 0
        || count > provider::PROVIDER_CATALOG_MAX_DESCRIPTORS
        || logical > provider::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES
        || (count == 0 && (logical != 0 || wire != 0))
        || (count != 0 && (logical == 0 || wire == 0))
    {
        return Err(invalid(
            "invalid fresh source generation or descriptor accounting",
        ));
    }
    // Bounds the plausibility of untrusted counters before exact recomputation.
    let max_wire = count
        .checked_mul(DESCRIPTOR_WIRE_MAX as u64)
        .ok_or_else(|| invalid("descriptor wire accounting overflow"))?;
    if wire > max_wire {
        return Err(invalid("descriptor wire accounting exceeds row bounds"));
    }
    Ok(())
}

/// Immutable owned pure-validation result. Not a snapshot source, capability,
/// SQL acceptance result, network authorization or verified byte availability.
/// Heavy fields are private and deliberately not Clone.
pub struct ProviderBootstrapSource {
    initial: ProjectionSnapshot,
    selection: ProjectionSelection,
    descriptors: Vec<DescriptorRecord>,
    catalog: CatalogRecord,
    vfs_catalog: Catalog,
    commitment: SourceCommitment,
}
impl ProviderBootstrapSource {
    pub fn validate(
        initial: ProjectionSnapshot,
        selection: ProjectionSelection,
        mut inputs: Vec<ProviderSourceDescriptor>,
    ) -> std::result::Result<Self, crate::projection::ProjectionStoreError> {
        if inputs.len() as u64 > provider::PROVIDER_CATALOG_MAX_DESCRIPTORS
            || initial.entries.len() > 100_000
        {
            return Err(invalid("source descriptor count exceeded"));
        }
        if initial.path_semantics != PathSemantics::PortableWindows {
            return Err(invalid(
                "provider source requires portable Windows path semantics",
            ));
        }
        if initial
            .entries
            .iter()
            .any(|e| e.state == EntryState::Materialized)
        {
            return Err(invalid(
                "fresh provider source cannot claim Materialized entries",
            ));
        }
        selection.validate_snapshot(&initial)?;
        let snapshot_bytes = initial.canonical_bytes()?;
        // Check a typed-value-preserving canonical roundtrip as well as core's
        // existing snapshot structural/encoded-size checks.
        if codec::decode::<ProjectionSnapshot>(&snapshot_bytes, codec::SNAPSHOT_MAX)? != initial {
            return Err(invalid("source snapshot canonical value changed"));
        }
        let selection_bytes = selection::encode(&selection)?;
        if selection_bytes.len() > codec::SELECTION_MAX {
            return Err(invalid("selection budget exceeded"));
        }
        let sd = database::digest(0, None, &snapshot_bytes);
        let ed = database::digest(0, Some(&sd), &selection_bytes);
        let ep = blake3::hash(&selection_bytes).to_hex().to_string();

        inputs.sort_by(|a, b| a.path.cmp(&b.path));
        if inputs.windows(2).any(|pair| pair[0].path == pair[1].path) {
            return Err(invalid("duplicate source descriptor path"));
        }
        let mut expected = initial
            .entries
            .iter()
            .filter(|e| selection.includes(&e.path))
            .filter(|e| e.state != EntryState::Absent);
        let mut descriptors = Vec::with_capacity(inputs.len());
        let mut vfs_entries = Vec::with_capacity(inputs.len());
        let mut logical = 0;
        for input in inputs {
            let entry = expected
                .next()
                .ok_or_else(|| invalid("extra source descriptor"))?;
            let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
                return Err(invalid("unsupported selected provider target"));
            };
            if input.path != entry.path || input.manifest.file_id() != file_id {
                return Err(invalid(
                    "source descriptor does not match selected snapshot target",
                ));
            }
            let row = DescriptorRecord {
                schema_version: "layrs.projection-provider-descriptor.v1".into(),
                path: input.path,
                file_id: file_id.clone(),
                bytes: input.manifest.byte_length(),
                executable: entry.executable,
                manifest_id: input.manifest.id().clone(),
                manifest: input.manifest,
            };
            row.validate_fields()?;
            logical = codec::add(logical, row.logical_bytes(row.manifest_bytes()?.len())?)?;
            if logical > provider::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES {
                return Err(invalid("descriptor aggregate budget exceeded"));
            }
            vfs_entries.push(CatalogEntry {
                path: row.path.clone(),
                file_id: row.file_id.clone(),
                size: row.bytes,
            });
            descriptors.push(row);
        }
        if expected.next().is_some() {
            return Err(invalid("missing or unsupported selected source descriptor"));
        }
        // Real constructor enforces its 32MiB/100k namespace/alias/range bounds;
        // no substitute identity function or copied VFS catalog validation.
        let vfs_catalog = Catalog::new(0, vfs_entries).map_err(invalid)?;
        let mut h = blake3::Hasher::new();
        h.update(&layrs_core::scm::domain_separated_bytes(
            DESCRIPTORS_DOMAIN,
            &[],
        ));
        h.update(&(descriptors.len() as u64).to_le_bytes());
        let mut wire = 0;
        for row in &descriptors {
            let encoded = row.encode()?;
            wire = codec::add(wire, encoded.len() as u64)?;
            h.update(&(encoded.len() as u64).to_le_bytes());
            h.update(&encoded);
            // Each row/manifest encoding is dropped here; no aggregate Vec<R>
            // serialization or duplicate aggregate descriptor byte buffer.
        }
        let catalog = CatalogRecord {
            schema_version: "layrs.projection-provider-catalog.v1".into(),
            source_generation: 0,
            source_digest: sd.clone(),
            selection_digest: ed.clone(),
            vfs_catalog_version: 0,
            vfs_catalog_id: vfs_catalog.identity().clone(),
            descriptor_count: descriptors.len() as u64,
            descriptor_wire_bytes: wire,
            descriptor_bytes: logical,
            descriptors_digest: format!("blake3:{}", h.finalize()),
        };
        let q = catalog.encode()?;
        let commitment = SourceCommitment {
            source_generation: 0,
            snapshot_bytes: snapshot_bytes.len() as u64,
            snapshot_digest: sd,
            selection_bytes: selection_bytes.len() as u64,
            selection_payload_digest: ep,
            selection_digest: ed,
            catalog_bytes: q.len() as u64,
            catalog_id: codec::hash(CATALOG_DOMAIN, &q),
            catalog_aggregate_bytes: codec::add(q.len() as u64, logical)?,
            descriptor_count: catalog.descriptor_count,
            descriptor_bytes: logical,
            descriptor_wire_bytes: wire,
            descriptors_digest: catalog.descriptors_digest.clone(),
            vfs_catalog_version: 0,
            vfs_catalog_id: vfs_catalog.identity().clone(),
        };
        commitment.validate_fields()?;
        Ok(Self {
            initial,
            selection,
            descriptors,
            catalog,
            vfs_catalog,
            commitment,
        })
    }
    pub(in crate::projection) fn initial(&self) -> &ProjectionSnapshot {
        &self.initial
    }
    pub(in crate::projection) fn selection(&self) -> &ProjectionSelection {
        &self.selection
    }
    pub(in crate::projection) fn descriptors(&self) -> &[DescriptorRecord] {
        &self.descriptors
    }
    pub(in crate::projection) fn catalog(&self) -> &CatalogRecord {
        &self.catalog
    }
    pub(in crate::projection) fn vfs_catalog(&self) -> &Catalog {
        &self.vfs_catalog
    }
    pub(in crate::projection) fn commitment(&self) -> &SourceCommitment {
        &self.commitment
    }
    pub(in crate::projection) fn require_commitment(&self, expected: &SourceCommitment) -> Result<()> {
        if expected != &self.commitment {
            return Err(invalid("source commitment differs from recomputed rows"));
        }
        Ok(())
    }
}
