//! Reconstruct every referenced immutable catalog against snapshot/selection0.
use super::super::super::records::{
    activation::activation_snapshot, source::CatalogRecord, ProviderBootstrapSource,
    ProviderSourceDescriptor,
};
use super::*;
use crate::projection::{database, selection};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{canonical_json, FileManifest},
};

const MAX_ALL_DESCRIPTOR_BYTES: u64 = 1024 * 1024 * 1024;

pub(super) struct CatalogSemantics {
    pub source_commitment: Vec<u8>,
    pub source_digest: String,
    pub selection_digest: String,
    pub activated_digest: String,
}
pub(super) struct CatalogValidation {
    pub descriptor_count: u64,
    pub by_id: BTreeMap<String, CatalogSemantics>,
}

pub(super) fn validate_catalogs(
    c: &Connection,
    ids: &BTreeSet<String>,
    cancel: Option<&Cancellation>,
) -> CauseResult<CatalogValidation> {
    Ok(validate_catalogs_inner(c, ids, None, cancel)?.0)
}

/// Reuse the complete catalog/descriptor interpreter while retaining only the
/// requested current source, rather than every historical catalog's inputs.
pub(in crate::projection::provider::import_validation) fn reconstruct_catalog(
    c: &Connection,
    id: &str,
    cancel: Option<&Cancellation>,
) -> CauseResult<ProviderBootstrapSource> {
    let ids = BTreeSet::from([id.to_owned()]);
    validate_catalogs_inner(c, &ids, Some(id), cancel)?
        .1
        .ok_or_else(|| invalid("import reconstructed catalog missing"))
}

fn validate_catalogs_inner(
    c: &Connection,
    ids: &BTreeSet<String>,
    retain: Option<&str>,
    cancel: Option<&Cancellation>,
) -> CauseResult<(CatalogValidation, Option<ProviderBootstrapSource>)> {
    if ids.is_empty() {
        return Ok((
            CatalogValidation {
                descriptor_count: 0,
                by_id: BTreeMap::new(),
            },
            None,
        ));
    }
    let (snapshot_length,): (u64,) = c.query_row(
        "SELECT length(payload) FROM projection_snapshots WHERE generation=0",
        [],
        |r| Ok((r.get(0)?,)),
    )?;
    if snapshot_length > 64 * 1024 * 1024 {
        return Err(invalid("import catalog initial snapshot exceeds bound"));
    }
    let initial_bytes: Vec<u8> = c.query_row(
        "SELECT payload FROM projection_snapshots WHERE generation=0",
        [],
        |r| r.get(0),
    )?;
    let initial: ProjectionSnapshot =
        serde_json::from_slice(&initial_bytes).map_err(|e| invalid(e.to_string()))?;
    if initial
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?
        != initial_bytes
    {
        return Err(invalid("import catalog initial snapshot differs"));
    }
    let selected = selection::at(c, 0)?;
    let mut total_rows = 0u64;
    let mut total_bytes = 0u64;
    let mut by_id = BTreeMap::new();
    let mut retained = None;
    for id in ids {
        super::super::super::cancellation::check(cancel)?;
        let (count, bytes): (u64,u64) = c.query_row(
            "SELECT count(*),coalesce(max(length(payload)),0) FROM projection_provider_catalogs WHERE id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?)),
        )?;
        if count != 1 || bytes > 64 * 1024 * 1024 {
            return Err(invalid("import catalog row missing or oversized"));
        }
        let (source_generation, source_digest, selection_digest, payload):
            (u64,String,String,Vec<u8>) = c.query_row(
            "SELECT source_generation,source_digest,selection_digest,payload FROM projection_provider_catalogs WHERE id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        )?;
        let catalog = CatalogRecord::decode(&payload)?;
        if catalog.id()? != *id
            || catalog.source_generation != source_generation
            || catalog.source_digest != source_digest
            || catalog.selection_digest != selection_digest
            || catalog.encode()? != payload
        {
            return Err(invalid("import catalog SQL projection differs"));
        }
        let (row_count, wire_bytes): (u64,u64) = c.query_row(
            "SELECT count(*),coalesce(sum(length(CAST(path AS BLOB))+length(CAST(file_id AS BLOB))+length(CAST(manifest_id AS BLOB))+length(manifest)+16),0) FROM projection_provider_descriptors WHERE catalog_id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?)),
        )?;
        total_rows = total_rows
            .checked_add(row_count)
            .filter(|n| *n <= ROW_LIMIT)
            .ok_or_else(|| invalid("import descriptor count exceeds bound"))?;
        total_bytes = total_bytes
            .checked_add(wire_bytes)
            .filter(|n| *n <= MAX_ALL_DESCRIPTOR_BYTES)
            .ok_or_else(|| invalid("import descriptor aggregate bytes exceed bound"))?;
        if row_count != catalog.descriptor_count
            || row_count > 100_000
            || wire_bytes > 256 * 1024 * 1024
        {
            return Err(invalid("import descriptor rowset count or bytes differs"));
        }
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_provider_descriptors WHERE catalog_id=?1 AND (length(CAST(path AS BLOB))>4096 OR length(CAST(file_id AS BLOB))!=71 OR length(CAST(manifest_id AS BLOB))!=71 OR length(manifest)>262144))",
            [id], |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("import descriptor SQL scalar exceeds bound"));
        }
        let mut statement = c.prepare("SELECT path,file_id,bytes,executable,manifest_id,manifest FROM projection_provider_descriptors WHERE catalog_id=?1 ORDER BY path COLLATE BINARY")?;
        let mut rows = statement.query([id])?;
        let mut inputs = Vec::with_capacity(row_count as usize);
        let mut sql_rows = Vec::with_capacity(row_count as usize);
        while let Some(row) = rows.next()? {
            super::super::super::cancellation::check(cancel)?;
            let (path, file_id, bytes, executable, manifest_id, wire): (
                String,
                String,
                u64,
                bool,
                String,
                Vec<u8>,
            ) = (
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            );
            if path.len() > 4096
                || file_id.len() != 71
                || manifest_id.len() != 71
                || wire.len() > 256 * 1024
            {
                return Err(invalid("import descriptor SQL scalar exceeds bound"));
            }
            let manifest: FileManifest =
                serde_json::from_slice(&wire).map_err(|e| invalid(e.to_string()))?;
            if layrs_core::scm::canonical_json(&manifest).map_err(|e| invalid(e.to_string()))?
                != wire
            {
                return Err(invalid("import descriptor manifest is not canonical"));
            }
            inputs.push(ProviderSourceDescriptor {
                path: layrs_core::scm::RepoPath::parse(&path)
                    .map_err(|e| invalid(e.to_string()))?,
                manifest,
            });
            sql_rows.push((path, file_id, bytes, executable, manifest_id));
        }
        let validated =
            ProviderBootstrapSource::validate(initial.clone(), selected.clone(), inputs)?;
        if validated.catalog() != &catalog
            || validated.commitment().catalog_id != *id
            || validated.descriptors().len() != sql_rows.len()
        {
            return Err(invalid("import catalog does not match initial source"));
        }
        for (descriptor, (path, file_id, bytes, executable, manifest_id)) in
            validated.descriptors().iter().zip(sql_rows)
        {
            if descriptor.path.as_str() != path
                || descriptor.file_id.as_str() != file_id
                || descriptor.bytes != bytes
                || descriptor.executable != executable
                || descriptor.manifest_id.as_str() != manifest_id
            {
                return Err(invalid("import descriptor SQL row differs from source"));
            }
        }
        let activated = activation_snapshot(&validated)?;
        let activated_bytes = activated
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?;
        let source_digest = validated.commitment().snapshot_digest.clone();
        by_id.insert(
            id.clone(),
            CatalogSemantics {
                source_commitment: canonical_json(validated.commitment())
                    .map_err(|e| invalid(e.to_string()))?,
                source_digest: source_digest.clone(),
                selection_digest: validated.commitment().selection_digest.clone(),
                activated_digest: database::digest(1, Some(&source_digest), &activated_bytes),
            },
        );
        if retain == Some(id.as_str()) {
            retained = Some(validated);
        }
    }
    Ok((
        CatalogValidation {
            descriptor_count: total_rows,
            by_id,
        },
        retained,
    ))
}
