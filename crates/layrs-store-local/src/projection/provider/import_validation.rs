//! Read-only, bounded SQL evidence for provider history imports. No result of
//! this module owns a source, native root, content byte, or destination claim.
use super::{
    database_io,
    records::import::{
        ImportClosure, ImportRowset, ImportSource, ImportSourceReferences, ImportedSegment,
        ProviderImportPrefixRecord, ProviderTransitionReceiptRecord, TransitionReceiptRowData,
    },
    recovery::error::{invalid, Result as CauseResult},
    validation::PrivateActiveCaptureValidation,
};
use crate::projection::{database, format, retention, selection};
use layrs_core::scm::canonical_json;
use layrs_vfs::Cancellation;
use rusqlite::Connection;
use std::collections::BTreeMap;

mod ancestry;
mod copy;
mod copied;
mod rows;
mod scoped;
mod source;
pub(in crate::projection::provider) use scoped::compute_scoped_closure;
pub(in crate::projection) use scoped::read_scoped_closure;
pub(in crate::projection) use source::{read_source_metadata, SourceMetadata};

/// A closed SQL DATA interpretation. Source image/bytes, family and native
/// registration retain their independent proofs.
pub(in crate::projection) struct VerifiedImportClosure {
    closure: ImportClosure,
    source_connection: *const Connection,
    marker: format::Marker,
    metadata: SourceMetadata,
    provider_inputs: Option<ProviderInputBinding>,
}
struct ProviderInputBinding {
    activation_id: String,
    source_digest: String,
    capture_count: u64,
    capture_head: Option<String>,
    content_digest: String,
}
impl VerifiedImportClosure {
    pub(in crate::projection) fn closure(&self) -> &ImportClosure {
        &self.closure
    }
    pub(in crate::projection) fn prefix_id(&self) -> &str {
        &self.metadata.prefix_id
    }
    /// Compare the exact private proof used to verify this SQL closure. A
    /// different, individually valid provider source cannot be substituted.
    pub(in crate::projection::provider) fn require_provider_inputs(
        &self,
        proof: &PrivateActiveCaptureValidation,
    ) -> CauseResult<()> {
        let Some(expected) = &self.provider_inputs else {
            return Err(invalid("import closure has no provider source binding"));
        };
        let (_, activation, source, _, captures) = proof.import_parts();
        if activation.digest()? != expected.activation_id
            || source_commitment_digest(source)? != expected.source_digest
            || captures.patches.len() as u64 != expected.capture_count
            || captures.patches.last().map(|patch| patch.digest.as_str())
                != expected.capture_head.as_deref()
            || scoped_content_digest(&self.metadata, captures)? != expected.content_digest
        {
            return Err(invalid("import closure provider inputs differ"));
        }
        Ok(())
    }
    pub(in crate::projection) fn capture_origin(
        &self,
        sequence: u64,
        local_max_generation: u64,
    ) -> CauseResult<(&crate::projection::ProjectionIdentity, u64, u64)> {
        self.metadata
            .lineage
            .capture_source(sequence, local_max_generation)
    }
}

/// Run every source fence in one caller-held read transaction. The provider
/// proof is the complete private SQL interpreter's output; native READY and
/// source byte availability remain separate caller obligations.
pub(in crate::projection) fn verify_source(
    c: &Connection,
    marker: &format::Marker,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    references: ImportSourceReferences<'_>,
    provider_proof: Option<&PrivateActiveCaptureValidation>,
    image_digest: &str,
    cancel: Option<&Cancellation>,
) -> CauseResult<VerifiedImportClosure> {
    if c.is_autocommit() {
        return Err(invalid("import reader requires a retained SQL snapshot"));
    }
    let metadata = read_source_metadata(c, marker, kind, prefix, references, image_digest, cancel)?;
    ancestry::verify_historical_closures(c, prefix, &metadata, cancel)?;
    match (&prefix.source, provider_proof) {
        (
            ImportSource::Provider {
                activation_map_id,
                binding,
                version,
                ..
            },
            Some(proof),
        ) => {
            let (mount, activation, source, activated, captures) = proof.import_parts();
            if mount.activation_digest != *activation_map_id
                || activation.digest()? != *activation_map_id
                || activation.binding != *binding
                || mount.prepared.binding != *binding
                || source.commitment().catalog_id != activation.catalog_id
                || activated.generation != version.generation
                || activated.digest != version.snapshot_digest
                || captures.patches.len() as u64 != version.capture_count
                || captures.patches.last().map(|patch| patch.digest.as_str())
                    != version.capture_head.as_deref()
            {
                return Err(invalid(
                    "import source differs from complete Active SQL proof",
                ));
            }
        }
        (ImportSource::Ordinary { .. }, None) => {}
        _ => return Err(invalid("import source proof kind differs")),
    }
    let closure = read_scoped_closure(c, kind, prefix, &metadata, cancel)?;
    super::cancellation::check(cancel)?;
    let violations: u64 =
        c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    if violations != 0 {
        return Err(invalid("import source foreign key check differs"));
    }
    super::cancellation::check(cancel)?;
    let provider_inputs = provider_proof
        .map(|proof| bind_provider_inputs(c, &metadata, proof, cancel))
        .transpose()?;
    Ok(VerifiedImportClosure {
        closure,
        source_connection: c,
        marker: marker.clone(),
        metadata,
        provider_inputs,
    })
}

fn source_commitment_digest(
    source: &super::records::ProviderBootstrapSource,
) -> CauseResult<String> {
    let bytes = canonical_json(source.commitment()).map_err(|e| invalid(e.to_string()))?;
    Ok(format!("blake3:{}", blake3::hash(&bytes)))
}

fn scoped_content_digest(
    metadata: &SourceMetadata,
    captures: &super::validation::PrivateCaptureProof,
) -> CauseResult<String> {
    if metadata.content_file_ids.len() > 100_000 || captures.content.len() > 100_000 {
        return Err(invalid(
            "import provider content mapping count exceeds bound",
        ));
    }
    let mut proof = BTreeMap::new();
    for row in &captures.content {
        if row.file_id.len() > 71
            || row.manifest_id.len() > 71
            || proof
                .insert(&row.file_id, (&row.manifest_id, row.bytes))
                .is_some()
        {
            return Err(invalid("import provider content proof malformed"));
        }
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-import-verified-content-v1\0");
    hash.update(&(metadata.content_file_ids.len() as u64).to_be_bytes());
    for file_id in &metadata.content_file_ids {
        let (manifest_id, bytes) = proof
            .get(file_id)
            .ok_or_else(|| invalid("import provider capture content mapping absent"))?;
        let encoded =
            canonical_json(&(file_id, manifest_id, bytes)).map_err(|e| invalid(e.to_string()))?;
        hash.update(&(encoded.len() as u64).to_be_bytes());
        hash.update(&encoded);
    }
    Ok(format!("blake3:{}", hash.finalize()))
}

fn bind_provider_inputs(
    c: &Connection,
    metadata: &SourceMetadata,
    proof: &PrivateActiveCaptureValidation,
    cancel: Option<&Cancellation>,
) -> CauseResult<ProviderInputBinding> {
    let (_, activation, source, _, captures) = proof.import_parts();
    let content_digest = scoped_content_digest(metadata, captures)?;
    let proof_content: BTreeMap<_, _> = captures
        .content
        .iter()
        .map(|row| (&row.file_id, (&row.manifest_id, row.bytes)))
        .collect();
    for file_id in &metadata.content_file_ids {
        super::cancellation::check(cancel)?;
        let (manifest_id, bytes): (String, u64) = c.query_row(
            "SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1",
            [file_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if proof_content.get(file_id) != Some(&(&manifest_id, bytes)) {
            return Err(invalid(
                "import provider content proof differs from scoped SQL",
            ));
        }
    }
    Ok(ProviderInputBinding {
        activation_id: activation.digest()?,
        source_digest: source_commitment_digest(source)?,
        capture_count: captures.patches.len() as u64,
        capture_head: captures.patches.last().map(|patch| patch.digest.clone()),
        content_digest,
    })
}

pub(in crate::projection) use copy::copy_verified_prefix;
pub(in crate::projection::provider) use copied::require_exact_copied_rows;

/// Evidence-bearing family of the connection supplied to `require_schema`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::projection) enum ImportDatabaseKind {
    Ordinary,
    SharedOrdinary,
    Provider,
}

/// Fence the exact schema before querying any application table, including
/// the ancestry row. FORMAT is an external pinned input, not inferred from DB.
pub(in crate::projection) fn require_schema(
    c: &Connection,
    marker: &format::Marker,
    kind: ImportDatabaseKind,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    super::cancellation::check(cancel)?;
    marker.validate()?;
    let expected_version = match kind {
        ImportDatabaseKind::Ordinary => 6,
        ImportDatabaseKind::SharedOrdinary => 7,
        ImportDatabaseKind::Provider => 8,
    };
    if marker.shared.is_some() != (kind != ImportDatabaseKind::Ordinary) {
        return Err(invalid("import FORMAT/shared schema family differs"));
    }
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let observed = database_io::schema_inventory(c)?;
    if version != expected_version {
        return Err(invalid("import private schema version differs"));
    }
    let mut ddl = [database::SQL, selection::SQL].concat();
    if kind != ImportDatabaseKind::Ordinary {
        ddl.push_str(retention::SQL);
    }
    if kind == ImportDatabaseKind::Provider {
        ddl.push_str(super::schema::SQL);
    }
    database_io::with_reference(&ddl, |reference| {
        if observed != database_io::schema_inventory(reference)? {
            return Err(invalid("import private schema fingerprint differs"));
        }
        Ok(())
    })?;
    super::cancellation::check(cancel)?;
    let (count, max_bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(identity)),0) FROM projection_meta",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || max_bytes > 64 * 1024 {
        return Err(invalid("import identity row count or size differs"));
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT identity FROM projection_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if bytes != marker.bytes()? {
        return Err(invalid("import private identity differs from FORMAT"));
    }
    Ok(())
}

/// Provenance is a sorted, acyclic chain of canonical accepted receipt DATA.
/// Constructed only by `read_ancestry` after `require_schema` on the source.
pub(in crate::projection) struct ProvenanceIndex {
    segments: Vec<ImportedSegment>,
    receipts: Vec<ProviderTransitionReceiptRecord>,
    local_identity: crate::projection::ProjectionIdentity,
    capture_count: u64,
    local_min_generation: u64,
}
impl ProvenanceIndex {
    pub(in crate::projection) fn segments(&self) -> &[ImportedSegment] {
        &self.segments
    }
    pub(in crate::projection) fn receipts(&self) -> &[ProviderTransitionReceiptRecord] {
        &self.receipts
    }

    /// Resolve a capture sequence using a binary search over non-overlapping
    /// imported intervals, then the bounded local tail.
    pub(in crate::projection) fn capture_source(
        &self,
        sequence: u64,
        local_max_generation: u64,
    ) -> CauseResult<(&crate::projection::ProjectionIdentity, u64, u64)> {
        if sequence == 0 || sequence > self.capture_count {
            return Err(invalid("capture sequence escaped imported source boundary"));
        }
        let at = self
            .segments
            .partition_point(|part| part.capture_count_through < sequence);
        Ok(
            match self
                .segments
                .get(at)
                .filter(|part| sequence > part.capture_count_before)
            {
                Some(part) => (
                    &part.source_identity,
                    part.first_generation,
                    part.last_generation,
                ),
                None => (
                    &self.local_identity,
                    self.local_min_generation,
                    local_max_generation,
                ),
            },
        )
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod two_imports;
