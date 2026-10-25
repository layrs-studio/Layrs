//! Complete bootstrap SQL interpretation. Pure row comparisons alone are not authority.
use super::{
    database_io,
    records::{self, *},
    recovery::error::{Result, invalid},
};
use crate::projection::{database, retention, selection};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{FileManifest, RepoPath},
};
use rusqlite::Connection;

pub(super) enum PrivateValidation {
    Uninitialized,
    Initialized(MountRecord),
}

pub(super) const EMPTY_TABLES: [&str; 31] = [
    "projection_install",
    "projection_refresh_pending",
    "projection_refresh_history",
    "projection_capture_files",
    "projection_capture_chunks",
    "projection_captures",
    "projection_publication_pending",
    "projection_publications",
    "projection_namespace_pending",
    "projection_namespace_operations",
    "projection_restore_pending",
    "projection_restore_history",
    "projection_rebase_pending",
    "projection_rebase_history",
    "projection_rebase_previews",
    "projection_content",
    "projection_claims",
    "projection_budgets",
    "projection_prejournal",
    "projection_preparations",
    "projection_observations",
    "projection_imports",
    "projection_conversion",
    "projection_provider_activations",
    "projection_provider_capture_transactions",
    "projection_provider_capture_receipts",
    "projection_provider_transitions",
    "projection_provider_transition_receipts",
    "projection_provider_companions",
    "projection_provider_companion_files",
    "projection_provider_companion_deletions",
];
pub(super) const SINGLE_TABLES: [&str; 7] = [
    "projection_meta",
    "projection_snapshots",
    "projection_head",
    "projection_selections",
    "projection_backend",
    "projection_provider_catalogs",
    "projection_provider_mounts",
];

pub(super) fn private(
    c: &Connection,
    allocation: &AllocationRecord,
    prepared: Option<&PreparedRecord>,
) -> Result<PrivateValidation> {
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let observed = database_io::schema_inventory(c)?;
    if version == 0 && observed.is_empty() {
        return Ok(PrivateValidation::Uninitialized);
    }
    if version != 8 {
        return Err(invalid("provider bootstrap requires exact private schema8"));
    }
    let ddl = [
        database::SQL,
        selection::SQL,
        retention::SQL,
        super::schema::SQL,
    ]
    .concat();
    database_io::with_reference(&ddl, |reference| {
        if observed != database_io::schema_inventory(reference)? {
            return Err(invalid("provider private schema fingerprint differs"));
        }
        Ok(())
    })?;
    // Account for every table before any potentially unbounded FK traversal.
    for table in EMPTY_TABLES {
        let nonempty: bool = c.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} LIMIT 1)"),
            [],
            |r| r.get(0),
        )?;
        if nonempty {
            return Err(invalid(format!(
                "provider bootstrap requires empty {table}"
            )));
        }
    }
    for table in SINGLE_TABLES {
        let count: u64 = c.query_row(
            &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT 2)"),
            [],
            |r| r.get(0),
        )?;
        if count != 1 {
            return Err(invalid(format!(
                "provider bootstrap requires exactly one {table} row"
            )));
        }
    }
    let count: u64 = c.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_descriptors LIMIT 100001)",
        [],
        |r| r.get(0),
    )?;
    if count > super::PROVIDER_CATALOG_MAX_DESCRIPTORS {
        return Err(invalid("provider descriptor count exceeds bound"));
    }
    // Exact DDL alone is not evidence that historical writes honored CHECKs.
    // Bound every materialized TEXT projection before fetching Rust Strings or
    // sorting descriptor rows; corrupt/ignore_check_constraints data is refused.
    for (table, predicate) in [
        (
            "projection_snapshots",
            "length(CAST(digest AS BLOB))!=64 OR parent_digest IS NOT NULL",
        ),
        ("projection_selections", "length(CAST(digest AS BLOB))!=64"),
        (
            "projection_backend",
            "backend NOT IN ('ordinary','provider') OR length(CAST(mount_id AS BLOB))>128",
        ),
        (
            "projection_provider_catalogs",
            "length(CAST(id AS BLOB))!=71 OR length(CAST(source_digest AS BLOB))!=64 OR length(CAST(selection_digest AS BLOB))!=64",
        ),
        (
            "projection_provider_mounts",
            "length(CAST(id AS BLOB))>128 OR length(CAST(catalog_id AS BLOB))!=71 OR length(CAST(digest AS BLOB))!=71 OR phase NOT IN ('allocating','control_prepared','control_published')",
        ),
        (
            "projection_provider_descriptors",
            "length(CAST(catalog_id AS BLOB))!=71 OR length(CAST(path AS BLOB))=0 OR length(CAST(path AS BLOB))>4096 OR length(CAST(file_id AS BLOB))!=71 OR length(CAST(manifest_id AS BLOB))!=71 OR executable NOT IN (0,1) OR bytes<0",
        ),
    ] {
        let bad: bool = c.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {predicate})"),
            [],
            |r| r.get(0),
        )?;
        if bad {
            return Err(invalid(format!(
                "provider {table} text/projection bound differs"
            )));
        }
    }
    let (logical, largest): (u64, u64) = c.query_row(
        "SELECT coalesce(sum(length(CAST(path AS BLOB))+length(manifest)+222),0),coalesce(max(length(manifest)),0) FROM projection_provider_descriptors",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // 71+71+8+1+71 = 222, the frozen descriptor logical accounting.
    if logical > super::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES
        || largest > super::PROVIDER_DESCRIPTOR_MAX_BYTES
    {
        return Err(invalid("provider descriptor aggregate exceeds bound"));
    }
    for (table, column, bound) in [
        ("projection_meta", "identity", 65536u64),
        ("projection_snapshots", "payload", 64 * 1024 * 1024),
        ("projection_selections", "payload", 16 * 1024 * 1024),
        (
            "projection_provider_catalogs",
            "payload",
            super::PROVIDER_CATALOG_MAX_BYTES,
        ),
        ("projection_provider_mounts", "payload", 1024 * 1024),
    ] {
        let bytes: u64 =
            c.query_row(&format!("SELECT length({column}) FROM {table}"), [], |r| {
                r.get(0)
            })?;
        if bytes > bound {
            return Err(invalid(format!("provider {table} payload exceeds bound")));
        }
    }
    let (singleton, identity): (u64, Vec<u8>) =
        c.query_row("SELECT singleton,identity FROM projection_meta", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    if singleton != 1 || identity != allocation.format.bytes()? {
        return Err(invalid(
            "provider SQL identity differs from authenticated allocation FORMAT",
        ));
    }
    let (generation, digest, parent, payload): (u64, String, Option<String>, Vec<u8>) = c
        .query_row(
            "SELECT generation,digest,parent_digest,payload FROM projection_snapshots",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    if generation != 0
        || parent.is_some()
        || digest != allocation.source.snapshot_digest
        || database::digest(0, None, &payload) != digest
    {
        return Err(invalid("provider bootstrap snapshot anchor differs"));
    }
    let initial: ProjectionSnapshot = serde_json::from_slice(&payload)
        .map_err(|e| invalid(format!("provider snapshot encoding: {e}")))?;
    if initial
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?
        != payload
    {
        return Err(invalid("provider snapshot is not canonical"));
    }
    let head: (u64, u64) = c.query_row(
        "SELECT singleton,generation FROM projection_head",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let selection_generation: u64 =
        c.query_row("SELECT generation FROM projection_selections", [], |r| {
            r.get(0)
        })?;
    if head != (1, 0) || selection_generation != 0 {
        return Err(invalid(
            "provider bootstrap head/selection generation differs",
        ));
    }
    let selection = selection::at(c, 0)?;
    allocation.format.validate_initial_selection(&selection)?;
    let (backend_singleton, backend_name, backend_mount): (u64, String, Option<String>) = c
        .query_row(
            "SELECT singleton,backend,mount_id FROM projection_backend",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
    let backend = BackendRow {
        singleton: backend_singleton,
        backend: match backend_name.as_str() {
            "provider" => BackendKind::Provider,
            "ordinary" => BackendKind::Ordinary,
            _ => return Err(invalid("unknown provider backend")),
        },
        mount_id: backend_mount,
    };
    let (catalog_id, source_generation, source_digest, selection_digest, catalog_payload): (String,u64,String,String,Vec<u8>) = c.query_row(
        "SELECT id,source_generation,source_digest,selection_digest,payload FROM projection_provider_catalogs", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    )?;
    let catalog = CatalogRecord::decode(&catalog_payload)?;
    if logical
        .checked_add(catalog_payload.len() as u64)
        .is_none_or(|bytes| bytes > super::PROVIDER_CATALOG_AGGREGATE_MAX_BYTES)
    {
        return Err(invalid(
            "provider descriptor/catalog aggregate exceeds bound",
        ));
    }
    struct OwnedRow {
        catalog: String,
        path: String,
        file: String,
        bytes: u64,
        executable: bool,
        manifest_id: String,
        manifest: Vec<u8>,
    }
    let mut statement = c.prepare("SELECT catalog_id,path,file_id,bytes,executable,manifest_id,manifest FROM projection_provider_descriptors ORDER BY catalog_id COLLATE BINARY,path COLLATE BINARY")?;
    let mut rows = statement.query([])?;
    let mut owned = Vec::with_capacity(count as usize);
    let mut inputs = Vec::with_capacity(count as usize);
    while let Some(r) = rows.next()? {
        let row = OwnedRow {
            catalog: r.get(0)?,
            path: r.get(1)?,
            file: r.get(2)?,
            bytes: r.get(3)?,
            executable: r.get(4)?,
            manifest_id: r.get(5)?,
            manifest: r.get(6)?,
        };
        let manifest: FileManifest =
            records::codec::decode(&row.manifest, super::PROVIDER_DESCRIPTOR_MAX_BYTES as usize)?;
        let path = RepoPath::parse(&row.path).map_err(|e| invalid(e.to_string()))?;
        inputs.push(ProviderSourceDescriptor { path, manifest });
        owned.push(row);
    }
    drop(rows);
    drop(statement);
    let source = ProviderBootstrapSource::validate(initial, selection, inputs)?;
    allocation.require_source_and_member(
        &source,
        std::path::Path::new(&allocation.private.metadata_path),
    )?;
    let descriptors: Vec<_> = owned
        .iter()
        .map(|r| DescriptorRow {
            catalog_id: &r.catalog,
            path: &r.path,
            file_id: &r.file,
            bytes: r.bytes,
            executable: r.executable,
            manifest_id: &r.manifest_id,
            manifest: &r.manifest,
        })
        .collect();
    let (mount_id, mount_catalog, phase, mount_payload, mount_digest): (
        String,
        String,
        String,
        Vec<u8>,
        String,
    ) = c.query_row(
        "SELECT id,catalog_id,phase,payload,digest FROM projection_provider_mounts",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let phase = match phase.as_str() {
        "allocating" => MountPhase::Allocating,
        "control_prepared" => MountPhase::ControlPrepared,
        "control_published" => MountPhase::ControlPublished,
        _ => return Err(invalid("unsupported provider bootstrap mount phase")),
    };
    let mount = MountRecord::decode(&mount_payload)?;
    require_bootstrap_row_data(
        &[backend],
        &[CatalogRow {
            id: &catalog_id,
            source_generation,
            source_digest: &source_digest,
            selection_digest: &selection_digest,
            record: &catalog,
        }],
        &descriptors,
        &[MountRow {
            id: &mount_id,
            catalog_id: &mount_catalog,
            phase,
            digest: &mount_digest,
            record: &mount,
        }],
        &source,
        allocation,
        prepared,
    )?;
    let failures: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |r| r.get(0),
    )?;
    if failures {
        return Err(invalid("provider private foreign-key corruption"));
    }
    Ok(PrivateValidation::Initialized(mount))
}

#[cfg(test)]
pub(super) mod tests;
