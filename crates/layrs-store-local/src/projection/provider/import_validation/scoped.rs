//! Explicit immutable-prefix SQL projections. Every provider selector uses
//! canonical IDs derived from the verified source/ancestor records, never a
//! whole mutable provider table.
use super::super::records::{
    activation::{ActivationRowData, ProviderActivationRecord},
    import::{
        CompanionDeletionRowData, CompanionFileRowData, CompanionHeaderRowData, CompanionReason,
        ImportSource, ProviderCompanionDeletionRecord, ProviderCompanionFileRecord,
        ProviderCompanionHeaderRecord,
    },
    Record,
};
use super::rows::{Kind, Spec};
use super::*;
use layrs_core::scm::canonical_json;
use layrs_core::scm::{FileId, FileManifestId, RepoPath};
use rusqlite::ToSql;
use std::collections::{BTreeMap, BTreeSet};
mod base;
pub(super) mod catalogs;
mod companions_v2;
pub(super) mod denial_history;

const EMPTY_COUNT: &str = "SELECT 0,0";
const EMPTY_ROWS: &str = "SELECT 0 WHERE 0";
const ROW_LIMIT: u64 = 1_000_000;
const BYTE_LIMIT: u64 = 1024 * 1024 * 1024;

fn ids_json(ids: &BTreeSet<String>) -> CauseResult<String> {
    if ids.len() > ROW_LIMIT as usize {
        return Err(invalid("import scoped ID set exceeds row bound"));
    }
    let json = serde_json::to_string(ids).map_err(|e| invalid(e.to_string()))?;
    if json.len() > 128 * 1024 * 1024 {
        return Err(invalid("import scoped ID parameter exceeds bound"));
    }
    Ok(json)
}

/// The caller has already checked the schema, FORMAT, source history and
/// canonical IDs in `SourceMetadata`. A mismatch leaves the source unchanged.
pub(in crate::projection) fn read_scoped_closure(
    c: &Connection,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    metadata: &SourceMetadata,
    cancel: Option<&Cancellation>,
) -> CauseResult<ImportClosure> {
    let closure = compute_scoped_closure(c, kind, prefix, metadata, cancel)?;
    if closure != prefix.closure {
        return Err(invalid("import immutable SQL closure differs from prefix"));
    }
    Ok(closure)
}

/// A planner may compute commitments as DATA before constructing its prefix.
/// This never grants source/readiness authority; `read_scoped_closure` must
/// independently compare the completed prefix under the same locks/snapshot.
pub(in crate::projection::provider) fn compute_scoped_closure(
    c: &Connection,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    metadata: &SourceMetadata,
    cancel: Option<&Cancellation>,
) -> CauseResult<ImportClosure> {
    let scope = ScopeView::from_metadata(metadata);
    compute_from_scope(c, kind, prefix, &scope, cancel)
}

/// Borrowed, already derived immutable IDs for one historical prefix.
pub(super) struct ScopeView<'a> {
    pub kind: ImportDatabaseKind,
    pub prefix_id: &'a str,
    pub catalog_ids: &'a BTreeSet<String>,
    pub activation_ids: &'a BTreeSet<String>,
    pub receipt_ids: &'a BTreeSet<String>,
    pub companion_ids: &'a BTreeSet<String>,
    pub content_file_ids: &'a BTreeSet<String>,
    pub capture_lengths: &'a BTreeMap<String, u64>,
    pub receipts: &'a [ProviderTransitionReceiptRecord],
    pub lineage: &'a ProvenanceIndex,
}
impl<'a> ScopeView<'a> {
    pub(super) fn from_metadata(metadata: &'a SourceMetadata) -> Self {
        Self {
            kind: metadata.kind,
            prefix_id: &metadata.prefix_id,
            catalog_ids: &metadata.catalog_ids,
            activation_ids: &metadata.activation_ids,
            receipt_ids: &metadata.receipt_ids,
            companion_ids: &metadata.companion_ids,
            content_file_ids: &metadata.content_file_ids,
            capture_lengths: &metadata.capture_lengths,
            receipts: metadata.lineage.receipts(),
            lineage: &metadata.lineage,
        }
    }
}

pub(super) fn compute_from_scope(
    c: &Connection,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    scope: &ScopeView<'_>,
    cancel: Option<&Cancellation>,
) -> CauseResult<ImportClosure> {
    with_scope(c, kind, prefix, scope, cancel, |_, closure| {
        Ok(closure.clone())
    })
}

pub(super) fn with_scope<T>(
    c: &Connection,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    scope: &ScopeView<'_>,
    cancel: Option<&Cancellation>,
    after: impl FnOnce(&[Spec<'_>], &ImportClosure) -> CauseResult<T>,
) -> CauseResult<T> {
    super::restore_history::require_supported_scope(c, prefix, cancel)?;
    if kind != scope.kind || prefix.digest()? != scope.prefix_id {
        return Err(invalid("import source metadata/prefix binding changed"));
    }
    let (generation, capture_count) = match &prefix.source {
        super::super::records::import::ImportSource::Ordinary { version, .. }
        | super::super::records::import::ImportSource::Provider { version, .. } => {
            (version.generation, version.capture_count)
        }
    };
    let denial_ids = denial_history::derive(c, prefix, scope, cancel)?;
    let denial_ids_json = ids_json(&denial_ids)?;
    let denials: [&dyn ToSql; 1] = [&denial_ids_json];
    let mut all_catalog_ids = scope.catalog_ids.clone();
    let activation_records = if kind.has_provider_history() {
        derive_catalog_scope(c, scope.activation_ids, &mut all_catalog_ids, cancel)?
    } else {
        Vec::new()
    };
    let mut all_content_ids = scope.content_file_ids.clone();
    if kind != ImportDatabaseKind::Ordinary {
        for (file_id, bytes) in scope.capture_lengths {
            super::super::cancellation::check(cancel)?;
            require_content_mapping(c, file_id, None, Some(*bytes))?;
        }
    }
    if kind.has_provider_history() {
        derive_companion_scope(c, scope, &mut all_catalog_ids, &mut all_content_ids, cancel)?;
    }
    let catalog_ids = ids_json(&all_catalog_ids)?;
    let catalog_validation = if kind.has_provider_history() {
        catalogs::validate_catalogs(c, &all_catalog_ids, cancel)?
    } else {
        catalogs::CatalogValidation {
            descriptor_count: 0,
            by_id: BTreeMap::new(),
        }
    };
    if !activation_records.is_empty() {
        let sql_activated_digest: String = c.query_row(
            "SELECT digest FROM projection_snapshots WHERE generation=1",
            [],
            |r| r.get(0),
        )?;
        for record in &activation_records {
            super::super::cancellation::check(cancel)?;
            let catalog = catalog_validation
                .by_id
                .get(&record.catalog_id)
                .ok_or_else(|| invalid("import activation lacks validated catalog"))?;
            if canonical_json(&record.source).map_err(|e| invalid(e.to_string()))?
                != catalog.source_commitment
                || record.source_digest != catalog.source_digest
                || record.selection_digest != catalog.selection_digest
                || record.activated_digest != catalog.activated_digest
                || record.activated_digest != sql_activated_digest
            {
                return Err(invalid("import activation source or placement differs"));
            }
            let record_id = record.digest()?;
            let mut found = false;
            for source_prefix in std::iter::once(prefix)
                .chain(scope.receipts.iter().map(|receipt| &receipt.source_prefix))
            {
                if let ImportSource::Provider {
                    activation_map_id,
                    binding,
                    identity,
                    ..
                } = &source_prefix.source
                {
                    if *activation_map_id == record_id {
                        if record.binding != *binding
                            || record.binding.recovery.identity != *identity
                        {
                            return Err(invalid("import historical activation binding differs"));
                        }
                        found = true;
                    }
                }
            }
            if !found {
                return Err(invalid("import activation has no bound source prefix"));
            }
        }
    }
    let activation_ids = ids_json(scope.activation_ids)?;
    let receipt_ids = ids_json(scope.receipt_ids)?;
    let companion_ids = ids_json(scope.companion_ids)?;
    let content_ids = ids_json(&all_content_ids)?;
    let snapshots: [&dyn ToSql; 1] = [&generation];
    let captures: [&dyn ToSql; 1] = [&capture_count];
    let catalogs: [&dyn ToSql; 1] = [&catalog_ids];
    let activations: [&dyn ToSql; 1] = [&activation_ids];
    let receipts: [&dyn ToSql; 1] = [&receipt_ids];
    let companions: [&dyn ToSql; 1] = [&companion_ids];
    let content: [&dyn ToSql; 1] = [&content_ids];
    let empty: [&dyn ToSql; 0] = [];
    let mut specs = Vec::with_capacity(14);
    macro_rules! add {
        ($kind:expr, $preflight:expr, $rows:expr, $params:expr) => {
            specs.push(Spec {
                kind: $kind,
                preflight_sql: $preflight,
                rows_sql: $rows,
                params: $params,
                max_rows: ROW_LIMIT,
                max_bytes: BYTE_LIMIT,
            })
        };
    }
    add!(
        Kind::Snapshots,
        "SELECT count(*),coalesce(sum(8+length(CAST(digest AS BLOB))+coalesce(length(CAST(parent_digest AS BLOB)),0)+length(payload)),0) FROM projection_snapshots WHERE generation<=?1",
        "SELECT generation,digest,parent_digest,payload FROM projection_snapshots WHERE generation<=?1 ORDER BY generation",
        &snapshots
    );
    add!(
        Kind::Selections,
        "SELECT count(*),coalesce(sum(8+length(CAST(digest AS BLOB))+length(payload)),0) FROM projection_selections WHERE generation<=?1",
        "SELECT generation,digest,payload FROM projection_selections WHERE generation<=?1 ORDER BY generation",
        &snapshots
    );
    add!(
        Kind::Captures,
        "SELECT count(*),coalesce(sum(16+length(CAST(digest AS BLOB))+length(payload)),0) FROM projection_captures WHERE sequence<=?1",
        "SELECT sequence,digest,projection_generation,payload FROM projection_captures WHERE sequence<=?1 ORDER BY sequence",
        &captures
    );
    add!(
        Kind::Publications,
        "SELECT count(*),coalesce(sum(16+length(CAST(request_digest AS BLOB))+length(CAST(receipt_digest AS BLOB))+length(receipt_bytes)+coalesce(length(next_snapshot),0)+length(CAST(projection_digest AS BLOB))+length(after_state)),0) FROM projection_publications WHERE sequence<=?1",
        "SELECT sequence,request_digest,receipt_digest,receipt_bytes,next_snapshot,projection_generation,projection_digest,after_state FROM projection_publications WHERE sequence<=?1 ORDER BY sequence",
        &captures
    );
    if !kind.has_provider_history() {
        for row_kind in [
            Kind::Catalogs,
            Kind::Descriptors,
            Kind::ActivationMaps,
            Kind::TransitionReceipts,
            Kind::Companions,
            Kind::CompanionFiles,
            Kind::CompanionDeletions,
        ] {
            add!(row_kind, EMPTY_COUNT, EMPTY_ROWS, &empty);
        }
    } else {
        add!(
            Kind::Catalogs,
            "SELECT count(*),coalesce(sum(length(CAST(id AS BLOB))+8+length(CAST(source_digest AS BLOB))+length(CAST(selection_digest AS BLOB))+length(payload)),0) FROM projection_provider_catalogs WHERE id IN (SELECT value FROM json_each(?1))",
            "SELECT id,source_generation,source_digest,selection_digest,payload FROM projection_provider_catalogs WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id COLLATE BINARY",
            &catalogs
        );
        add!(
            Kind::Descriptors,
            "SELECT count(*),coalesce(sum(length(CAST(catalog_id AS BLOB))+length(CAST(path AS BLOB))+length(CAST(file_id AS BLOB))+16+length(CAST(manifest_id AS BLOB))+length(manifest)),0) FROM projection_provider_descriptors WHERE catalog_id IN (SELECT value FROM json_each(?1))",
            "SELECT catalog_id,path,file_id,bytes,executable,manifest_id,manifest FROM projection_provider_descriptors WHERE catalog_id IN (SELECT value FROM json_each(?1)) ORDER BY catalog_id COLLATE BINARY,path COLLATE BINARY",
            &catalogs
        );
        add!(
            Kind::ActivationMaps,
            "SELECT count(*),coalesce(sum(length(CAST(id AS BLOB))+length(CAST(owner_identity_digest AS BLOB))+length(CAST(mount_id AS BLOB))+length(CAST(catalog_id AS BLOB))+16+length(CAST(source_digest AS BLOB))+length(CAST(activated_digest AS BLOB))+length(payload)),0) FROM projection_provider_activations WHERE id IN (SELECT value FROM json_each(?1))",
            "SELECT id,owner_identity_digest,mount_id,catalog_id,source_generation,source_digest,activated_generation,activated_digest,payload FROM projection_provider_activations WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id COLLATE BINARY",
            &activations
        );
        add!(
            Kind::TransitionReceipts,
            "SELECT count(*),coalesce(sum(length(CAST(id AS BLOB))+length(CAST(owner_identity_digest AS BLOB))+length(CAST(transaction_id AS BLOB))+coalesce(length(CAST(companion_id AS BLOB)),0)+length(payload)),0) FROM projection_provider_transition_receipts WHERE id IN (SELECT value FROM json_each(?1))",
            "SELECT id,owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id COLLATE BINARY",
            &receipts
        );
        add!(
            Kind::Companions,
            "SELECT count(*),coalesce(sum(length(CAST(id AS BLOB))+16+length(CAST(files_digest AS BLOB))+length(CAST(deletions_digest AS BLOB))+length(payload)),0) FROM projection_provider_companions WHERE id IN (SELECT value FROM json_each(?1))",
            "SELECT id,file_count,deletion_count,files_digest,deletions_digest,payload FROM projection_provider_companions WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id COLLATE BINARY",
            &companions
        );
        add!(
            Kind::CompanionFiles,
            "SELECT count(*),coalesce(sum(length(CAST(companion_id AS BLOB))+length(CAST(path AS BLOB))+length(CAST(file_id AS BLOB))+16+length(CAST(manifest_id AS BLOB))+coalesce(length(CAST(base_file_id AS BLOB)),0)+CASE WHEN base_executable IS NULL THEN 0 ELSE 8 END+length(CAST(reason AS BLOB))+length(CAST(digest AS BLOB))),0) FROM projection_provider_companion_files WHERE companion_id IN (SELECT value FROM json_each(?1))",
            "SELECT companion_id,path,file_id,bytes,executable,manifest_id,base_file_id,base_executable,reason,digest FROM projection_provider_companion_files WHERE companion_id IN (SELECT value FROM json_each(?1)) ORDER BY companion_id COLLATE BINARY,path COLLATE BINARY",
            &companions
        );
        add!(
            Kind::CompanionDeletions,
            "SELECT count(*),coalesce(sum(length(CAST(companion_id AS BLOB))+length(CAST(path AS BLOB))+length(CAST(base_file_id AS BLOB))+8+coalesce(length(CAST(base_manifest_id AS BLOB)),0)+length(CAST(reason AS BLOB))+length(CAST(digest AS BLOB))),0) FROM projection_provider_companion_deletions WHERE companion_id IN (SELECT value FROM json_each(?1))",
            "SELECT companion_id,path,base_file_id,base_executable,base_manifest_id,reason,digest FROM projection_provider_companion_deletions WHERE companion_id IN (SELECT value FROM json_each(?1)) ORDER BY companion_id COLLATE BINARY,path COLLATE BINARY",
            &companions
        );
    }
    if kind == ImportDatabaseKind::Ordinary {
        add!(Kind::ContentMappings, EMPTY_COUNT, EMPTY_ROWS, &empty);
    } else {
        add!(
            Kind::ContentMappings,
            "SELECT count(*),coalesce(sum(length(CAST(file_id AS BLOB))+length(CAST(manifest_id AS BLOB))+8),0) FROM projection_content WHERE file_id IN (SELECT value FROM json_each(?1))",
            "SELECT file_id,manifest_id,bytes FROM projection_content WHERE file_id IN (SELECT value FROM json_each(?1)) ORDER BY file_id COLLATE BINARY",
            &content
        );
    }
    if prefix.supports_restore_history() {
        specs.push(Spec {
            kind: Kind::RestoreHistory,
            preflight_sql: "SELECT count(*),coalesce(sum(8+length(CAST(operation_id AS BLOB))+length(payload)),0) FROM projection_restore_history WHERE capture_sequence<=?1",
            rows_sql: "SELECT operation_id,capture_sequence,payload FROM projection_restore_history WHERE capture_sequence<=?1 ORDER BY operation_id COLLATE BINARY",
            params: &captures,
            max_rows: ROW_LIMIT,
            max_bytes: 512 * 1024 * 1024,
        });
    }
    if prefix.closure.denial_observations.is_some() {
        if kind == ImportDatabaseKind::Ordinary {
            add!(Kind::DenialObservations, EMPTY_COUNT, EMPTY_ROWS, &empty);
        } else {
            add!(
                Kind::DenialObservations,
                denial_history::PREFLIGHT_SQL,
                denial_history::ROWS_SQL,
                &denials
            );
        }
    }
    let preflight = rows::preflight(c, &specs, cancel)?;
    let closure = rows::stream(c, &specs, &preflight, cancel)?;
    if closure.denial_observations.as_ref().map(|rows| rows.count)
        != prefix
            .closure
            .denial_observations
            .as_ref()
            .map(|_| denial_ids.len() as u64)
        || closure.catalogs.count != all_catalog_ids.len() as u64
        || closure.descriptors.count != catalog_validation.descriptor_count
        || closure.activation_maps.count != scope.activation_ids.len() as u64
        || closure.transition_receipts.count != scope.receipt_ids.len() as u64
        || closure.companions.count != scope.companion_ids.len() as u64
        || (kind != ImportDatabaseKind::Ordinary
            && closure.content_mappings.count != all_content_ids.len() as u64)
    {
        return Err(invalid("import scoped row is missing from SQL closure"));
    }
    after(&specs, &closure)
}

fn derive_catalog_scope(
    c: &Connection,
    activation_ids: &BTreeSet<String>,
    catalog_ids: &mut BTreeSet<String>,
    cancel: Option<&Cancellation>,
) -> CauseResult<Vec<ProviderActivationRecord>> {
    let ids = ids_json(activation_ids)?;
    let (count, bytes, max_catalog): (u64, u64, u64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(payload)+length(CAST(id AS BLOB))+length(CAST(owner_identity_digest AS BLOB))+length(CAST(mount_id AS BLOB))+length(CAST(catalog_id AS BLOB))+length(CAST(source_digest AS BLOB))+length(CAST(activated_digest AS BLOB))),0),coalesce(max(length(CAST(catalog_id AS BLOB))),0) FROM projection_provider_activations WHERE id IN (SELECT value FROM json_each(?1))",
        [&ids], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if count != activation_ids.len() as u64 || bytes > 128 * 1024 * 1024 || max_catalog > 71 {
        return Err(invalid("import activation map scope missing or oversized"));
    }
    let mut statement = c.prepare(
        "SELECT id,owner_identity_digest,mount_id,catalog_id,source_generation,source_digest,activated_generation,activated_digest,payload FROM projection_provider_activations WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id COLLATE BINARY",
    )?;
    let mut rows = statement.query([&ids])?;
    let mut found = Vec::with_capacity(count as usize);
    while let Some(row) = rows.next()? {
        super::super::cancellation::check(cancel)?;
        let (
            id,
            owner,
            mount,
            catalog,
            source_generation,
            source_digest,
            activated_generation,
            activated_digest,
            payload,
        ): (
            String,
            String,
            String,
            String,
            u64,
            String,
            u64,
            String,
            Vec<u8>,
        ) = (
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
            row.get(8)?,
        );
        let record = ProviderActivationRecord::decode(&payload)?;
        record.require_row(ActivationRowData {
            id: &id,
            owner_identity_digest: &owner,
            mount_id: &mount,
            catalog_id: &catalog,
            source_generation,
            source_digest: &source_digest,
            activated_generation,
            activated_digest: &activated_digest,
            payload: &payload,
        })?;
        catalog_ids.insert(catalog);
        found.push(record);
    }
    Ok(found)
}

fn companion_reason(token: &str) -> CauseResult<CompanionReason> {
    match token {
        "unknownPath" => Ok(CompanionReason::UnknownPath),
        "excludedPath" => Ok(CompanionReason::ExcludedPath),
        "protectedChange" => Ok(CompanionReason::ProtectedChange),
        "revokedRights" => Ok(CompanionReason::RevokedRights),
        "relocatedPlaceholder" => Ok(CompanionReason::RelocatedPlaceholder),
        _ => Err(invalid("import companion reason differs")),
    }
}

fn derive_companion_scope(
    c: &Connection,
    scope: &ScopeView<'_>,
    catalog_ids: &mut BTreeSet<String>,
    content_ids: &mut BTreeSet<String>,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    let ids = ids_json(scope.companion_ids)?;
    let (count, header_bytes, largest_header): (u64, u64, u64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(payload)),0),coalesce(max(length(payload)),0) FROM projection_provider_companions WHERE id IN (SELECT value FROM json_each(?1))",
        [&ids], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if count != scope.companion_ids.len() as u64
        || header_bytes > 64 * 1024 * 1024
        || largest_header > 64 * 1024
    {
        return Err(invalid(
            "import companion header scope missing or oversized",
        ));
    }
    let (file_count, file_bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(CAST(path AS BLOB))+length(CAST(file_id AS BLOB))+length(CAST(manifest_id AS BLOB))),0) FROM projection_provider_companion_files WHERE companion_id IN (SELECT value FROM json_each(?1))",
        [&ids], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (deletion_count, deletion_bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(CAST(path AS BLOB))+length(CAST(base_file_id AS BLOB))),0) FROM projection_provider_companion_deletions WHERE companion_id IN (SELECT value FROM json_each(?1))",
        [&ids], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if file_count
        .checked_add(deletion_count)
        .is_none_or(|n| n > ROW_LIMIT)
        || file_bytes
            .checked_add(deletion_bytes)
            .is_none_or(|n| n > BYTE_LIMIT)
    {
        return Err(invalid("import companion rows exceed aggregate bound"));
    }
    for id in scope.companion_ids {
        super::super::cancellation::check(cancel)?;
        let (file_count, deletion_count, files_digest, deletions_digest, payload):
            (u64,u64,String,String,Vec<u8>) = c.query_row(
            "SELECT file_count,deletion_count,files_digest,deletions_digest,payload FROM projection_provider_companions WHERE id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        )?;
        let header =
            match super::super::records::companion_v2::CompanionHeaderRecord::decode(&payload)? {
                super::super::records::companion_v2::CompanionHeaderRecord::V1(header) => header,
                super::super::records::companion_v2::CompanionHeaderRecord::V2(header) => {
                    if header.digest()? != *id
                        || header.file_count != file_count
                        || header.deletion_count != deletion_count
                        || header.files_digest != files_digest
                        || header.deletions_digest != deletions_digest
                        || header.encode()? != payload
                    {
                        return Err(invalid("import companion v2 SQL header differs"));
                    }
                    let receipt = scope
                        .receipts
                        .iter()
                        .find(|receipt| receipt.companion_id.as_deref() == Some(id))
                        .ok_or_else(|| invalid("import companion v2 has no canonical receipt"))?;
                    companions_v2::validate(
                        c,
                        id,
                        &header,
                        receipt,
                        scope.lineage,
                        content_ids,
                        catalog_ids,
                        cancel,
                    )?;
                    continue;
                }
            };
        header.require_row(CompanionHeaderRowData {
            id,
            file_count,
            deletion_count,
            files_digest: &files_digest,
            deletions_digest: &deletions_digest,
            payload: &payload,
        })?;
        let receipt = scope
            .receipts
            .iter()
            .find(|receipt| receipt.companion_id.as_deref() == Some(id))
            .ok_or_else(|| invalid("import companion has no canonical receipt"))?;
        let (source_identity, source_schema, version) = match &receipt.source_prefix.source {
            ImportSource::Ordinary {
                identity,
                private_schema,
                version,
                ..
            }
            | ImportSource::Provider {
                identity,
                private_schema,
                version,
                ..
            } => (identity, *private_schema, version),
        };
        if &header.source_identity != source_identity
            || header.source_private_schema != source_schema
            || header.source_generation != version.generation
            || header.source_capture_count != version.capture_count
            || header.source_capture_head != version.capture_head
            || header.import_prefix_id != receipt.source_prefix_id
            || header.content_mappings != receipt.content_mappings
        {
            return Err(invalid("import companion/receipt source binding differs"));
        }
        receipt.require_companion(&header)?;
        let effective_base = base::EffectiveBase::at(c, version, scope.lineage)?;
        let expected_catalog = match &receipt.source_prefix.source {
            ImportSource::Ordinary { .. } => None,
            ImportSource::Provider {
                activation_map_id, ..
            } => {
                let length: u64 = c.query_row(
                    "SELECT length(CAST(catalog_id AS BLOB)) FROM projection_provider_activations WHERE id=?1",
                    [activation_map_id], |r| r.get(0),
                )?;
                if length != 71 {
                    return Err(invalid("import companion source catalog ID malformed"));
                }
                Some(c.query_row(
                    "SELECT catalog_id FROM projection_provider_activations WHERE id=?1",
                    [activation_map_id],
                    |r| r.get::<_, String>(0),
                )?)
            }
        };
        if header.catalog_id != expected_catalog {
            return Err(invalid(
                "import companion catalog differs from source prefix",
            ));
        }
        if let Some(catalog) = &header.catalog_id {
            catalog_ids.insert(catalog.clone());
        }
        let mut files = Vec::with_capacity(file_count as usize);
        let mut file_stmt = c.prepare("SELECT path,file_id,bytes,executable,manifest_id,base_file_id,base_executable,reason,digest FROM projection_provider_companion_files WHERE companion_id=?1 ORDER BY path COLLATE BINARY")?;
        let mut file_rows = file_stmt.query([id])?;
        while let Some(row) = file_rows.next()? {
            super::super::cancellation::check(cancel)?;
            let (
                path,
                file_id,
                bytes,
                executable,
                manifest_id,
                base_file_id,
                base_executable,
                reason,
                digest,
            ): (
                String,
                String,
                u64,
                bool,
                String,
                Option<String>,
                Option<bool>,
                String,
                String,
            ) = (
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            );
            let record = ProviderCompanionFileRecord {
                schema_version: "layrs.projection-provider-companion-file.v1".into(),
                path: RepoPath::parse(&path).map_err(|e| invalid(e.to_string()))?,
                file_id: FileId::parse(&file_id).map_err(|e| invalid(e.to_string()))?,
                bytes,
                executable,
                manifest_id: FileManifestId::parse(&manifest_id)
                    .map_err(|e| invalid(e.to_string()))?,
                base_file_id: base_file_id
                    .as_deref()
                    .map(FileId::parse)
                    .transpose()
                    .map_err(|e| invalid(e.to_string()))?,
                base_executable,
                reason: companion_reason(&reason)?,
            };
            record.require_row(
                id,
                CompanionFileRowData {
                    companion_id: id,
                    path: &path,
                    file_id: &file_id,
                    bytes,
                    executable,
                    manifest_id: &manifest_id,
                    base_file_id: base_file_id.as_deref(),
                    base_executable,
                    reason: &reason,
                    digest: &digest,
                },
            )?;
            effective_base.require(
                &record.path,
                record.base_file_id.as_ref(),
                record.base_executable,
            )?;
            require_content_mapping(c, &file_id, Some(&manifest_id), Some(bytes))?;
            content_ids.insert(file_id);
            if let Some(base) = base_file_id {
                require_content_mapping(c, &base, None, None)?;
                content_ids.insert(base);
            }
            files.push(record);
        }
        let mut deletions = Vec::with_capacity(deletion_count as usize);
        let mut deletion_stmt = c.prepare("SELECT path,base_file_id,base_executable,base_manifest_id,reason,digest FROM projection_provider_companion_deletions WHERE companion_id=?1 ORDER BY path COLLATE BINARY")?;
        let mut deletion_rows = deletion_stmt.query([id])?;
        while let Some(row) = deletion_rows.next()? {
            super::super::cancellation::check(cancel)?;
            let (path, base_file_id, base_executable, base_manifest_id, reason, digest): (
                String,
                String,
                bool,
                Option<String>,
                String,
                String,
            ) = (
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            );
            let record = ProviderCompanionDeletionRecord {
                schema_version: "layrs.projection-provider-companion-deletion.v1".into(),
                path: RepoPath::parse(&path).map_err(|e| invalid(e.to_string()))?,
                base_file_id: FileId::parse(&base_file_id).map_err(|e| invalid(e.to_string()))?,
                base_executable,
                base_manifest_id: base_manifest_id
                    .as_deref()
                    .map(FileManifestId::parse)
                    .transpose()
                    .map_err(|e| invalid(e.to_string()))?,
                reason: companion_reason(&reason)?,
            };
            record.require_row(
                id,
                CompanionDeletionRowData {
                    companion_id: id,
                    path: &path,
                    base_file_id: &base_file_id,
                    base_executable,
                    base_manifest_id: base_manifest_id.as_deref(),
                    reason: &reason,
                    digest: &digest,
                },
            )?;
            effective_base.require(
                &record.path,
                Some(&record.base_file_id),
                Some(record.base_executable),
            )?;
            require_content_mapping(c, &base_file_id, base_manifest_id.as_deref(), None)?;
            content_ids.insert(base_file_id);
            deletions.push(record);
        }
        header.validate_rows(&files, &deletions)?;
    }
    Ok(())
}

fn require_content_mapping(
    c: &Connection,
    file_id: &str,
    manifest_id: Option<&str>,
    bytes: Option<u64>,
) -> CauseResult<()> {
    let (count, max_manifest_bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(CAST(manifest_id AS BLOB))),0) FROM projection_content WHERE file_id=?1",
        [file_id], |r| Ok((r.get(0)?,r.get(1)?)),
    )?;
    if count != 1 || max_manifest_bytes != 71 {
        return Err(invalid("import exact content mapping missing or malformed"));
    }
    let (actual_manifest, actual_bytes): (String, u64) = c.query_row(
        "SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1",
        [file_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    FileManifestId::parse(&actual_manifest).map_err(|e| invalid(e.to_string()))?;
    if actual_bytes > crate::projection::capture::MAX_CAPTURE_FILE_BYTES
        || manifest_id.is_some_and(|expected| expected != actual_manifest.as_str())
        || bytes.is_some_and(|expected| expected != actual_bytes)
    {
        return Err(invalid("import exact content mapping projection differs"));
    }
    Ok(())
}

/// Called only after the canonical historical scope has been authenticated.
/// V1 has no alternate-reference payload; V2 retains its complete immutable
/// exact set even after no current image path refers to those representations.
pub(super) fn companion_representations(
    c: &Connection,
    ids: &BTreeSet<String>,
    cancel: Option<&Cancellation>,
) -> CauseResult<Vec<super::super::managed_admission::RequiredRepresentation>> {
    use super::super::{managed_admission::RequiredRepresentation, records::companion_v2::*};
    let mut exact = BTreeMap::new();
    let mut lengths = BTreeMap::new();
    let mut owners = BTreeMap::new();
    for id in ids {
        super::super::cancellation::check(cancel)?;
        // The preceding scope validation checked row cardinality, scalar
        // projections, canonical bytes, receipt/provenance and the aggregate.
        // Repeat the per-header bound in SQL before allocating its BLOB.
        let payload: Vec<u8> = c.query_row(
            "SELECT payload FROM projection_provider_companions WHERE id=?1 AND length(payload)<=65536",
            [id], |r| r.get(0),
        )?;
        let header = CompanionHeaderRecord::decode(&payload)?;
        if header.as_ref().digest()? != *id {
            return Err(invalid(
                "inherited companion exact reference header differs",
            ));
        }
        if let CompanionHeaderRecord::V2(header) = header {
            for row in header.preserved_representations {
                super::super::cancellation::check(cancel)?;
                let key = (row.file_id.clone(), row.manifest_id.clone());
                if exact.len() >= 100_000 && !exact.contains_key(&key) {
                    return Err(invalid("inherited companion exact union exceeds bound"));
                }
                if lengths
                    .insert(row.file_id.clone(), row.bytes)
                    .is_some_and(|old| old != row.bytes)
                    || owners
                        .insert(row.manifest_id.clone(), row.file_id.clone())
                        .is_some_and(|old| old != row.file_id)
                    || exact
                        .insert(key, row.bytes)
                        .is_some_and(|old| old != row.bytes)
                {
                    return Err(invalid("inherited companion exact representation conflict"));
                }
            }
        }
        // V1's scalar representations/preimages are already in the exact
        // authenticated copied FileId mappings. Do not synthesize v2 payloads.
    }
    Ok(exact
        .into_iter()
        .map(|((file_id, manifest_id), bytes)| RequiredRepresentation {
            file_id,
            manifest_id,
            bytes,
        })
        .collect())
}

/// Outgoing B proof uses the original INCOMING lineage only. It never calls
/// verify_source_publication and never extends any frozen prefix selector.
pub(in crate::projection::provider) fn validate_outgoing_companion(
    c: &Connection,
    marker: &format::Marker,
    id: &str,
    header: &super::super::records::companion_v2::CompanionHeaderV2,
    receipt: &ProviderTransitionReceiptRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<BTreeSet<String>> {
    let lineage = super::ancestry::read_ancestry(
        c,
        marker,
        ImportDatabaseKind::Provider,
        &receipt.source_prefix,
        cancel,
    )?;
    let mut content = BTreeSet::new();
    let mut catalogs = BTreeSet::new();
    companions_v2::validate_mode(
        c,
        id,
        header,
        receipt,
        &lineage,
        &mut content,
        &mut catalogs,
        cancel,
        true,
    )?;
    // source-only dependencies must be included even where an earlier sticky
    // check needed no selector mutation.
    for intent in &header.intents {
        if let Some(a) = intent.accepted() {
            content.insert(
                a.change
                    .expected
                    .as_ref()
                    .ok_or_else(|| invalid("outgoing preimage absent"))?
                    .file_id
                    .to_string(),
            );
        }
    }
    Ok(content)
}

/// Only a schema-specific ancestry reader or fully interpreted destination
/// prefix provides this lineage. No alternate schema bypass is exposed.
pub(super) fn validate_local_companion_in_lineage(
    c: &Connection,
    id: &str,
    header: &super::super::records::companion_v2::CompanionHeaderV2,
    receipt: &ProviderTransitionReceiptRecord,
    lineage: &ProvenanceIndex,
    cancel: Option<&Cancellation>,
) -> CauseResult<BTreeSet<String>> {
    let mut content = BTreeSet::new();
    let mut catalogs = BTreeSet::new();
    companions_v2::validate_mode(
        c,
        id,
        header,
        receipt,
        lineage,
        &mut content,
        &mut catalogs,
        cancel,
        false,
    )?;
    // source-only dependencies must be included even where an earlier sticky
    // check needed no selector mutation.
    for intent in &header.intents {
        if let Some(a) = intent.accepted() {
            content.insert(
                a.change
                    .expected
                    .as_ref()
                    .ok_or_else(|| invalid("outgoing preimage absent"))?
                    .file_id
                    .to_string(),
            );
        }
    }
    Ok(content)
}
