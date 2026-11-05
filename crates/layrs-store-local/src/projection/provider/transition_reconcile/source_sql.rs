//! Rebuild reconciliation inputs from the frozen SQL closure. No nominated
//! row vector is accepted. All rows are DATA inside the retained source read.
use super::*;
use crate::projection::provider::{
    import_validation::{self, ImportDatabaseKind},
    records::companion_v2::CompanionHeaderRecord,
    validation::{PrivateActiveCaptureValidation, PrivateActivePublicationValidation},
};
use rusqlite::Connection;

pub(in crate::projection::provider) fn with_source<T>(
    c: &Connection,
    marker: &crate::projection::format::Marker,
    prefix: &super::super::records::import::ProviderImportPrefixRecord,
    closure: &VerifiedImportClosure,
    proof: &PrivateActivePublicationValidation,
    observed: &TransitionObservationData,
    cancel: Option<&Cancellation>,
    action: impl FnOnce(&ReconcileSourceData<'_>) -> Result<T>,
) -> Result<T> {
    with_source_proof(
        c,
        marker,
        prefix,
        closure,
        SourceProof::Publication(proof),
        observed,
        cancel,
        action,
    )
}

/// Pre-publication source interpretation. A Capture proof is kept distinct
/// from the publication proof used after source Ready/Published.
pub(in crate::projection::provider) fn with_capture_source<T>(
    c: &Connection,
    marker: &crate::projection::format::Marker,
    prefix: &super::super::records::import::ProviderImportPrefixRecord,
    closure: &VerifiedImportClosure,
    proof: &PrivateActiveCaptureValidation,
    observed: &TransitionObservationData,
    cancel: Option<&Cancellation>,
    action: impl FnOnce(&ReconcileSourceData<'_>) -> Result<T>,
) -> Result<T> {
    with_source_proof(
        c,
        marker,
        prefix,
        closure,
        SourceProof::Capture(proof),
        observed,
        cancel,
        action,
    )
}

#[derive(Clone, Copy)]
enum SourceProof<'a> {
    Capture(&'a PrivateActiveCaptureValidation),
    Publication(&'a PrivateActivePublicationValidation),
}

fn with_source_proof<T>(
    c: &Connection,
    marker: &crate::projection::format::Marker,
    prefix: &super::super::records::import::ProviderImportPrefixRecord,
    closure: &VerifiedImportClosure,
    proof: SourceProof<'_>,
    observed: &TransitionObservationData,
    cancel: Option<&Cancellation>,
    action: impl FnOnce(&ReconcileSourceData<'_>) -> Result<T>,
) -> Result<T> {
    closure.require_source_connection(c)?;
    let (active, capture) = match proof {
        SourceProof::Capture(proof) => {
            closure.require_provider_inputs(proof)?;
            (&proof.active, &proof.capture)
        }
        SourceProof::Publication(proof) => {
            closure.require_publication_inputs(proof)?;
            (&proof.active, &proof.capture)
        }
    };
    let mappings = closure.copied_content_mappings(c, prefix, cancel)?;
    let lineage =
        import_validation::read_ancestry(c, marker, ImportDatabaseKind::Provider, prefix, cancel)?;
    let mut groups = Vec::new();
    for receipt in lineage.receipts() {
        cancellation::check(cancel)?;
        let Some(id) = &receipt.companion_id else {
            continue;
        };
        let (header, files, deletions) = read_group(c, id, cancel)?;
        header.as_ref().require_receipt(receipt)?;
        groups.push((header, files, deletions));
    }
    let inherited = groups
        .iter()
        .map(|(header, files, deletions)| InheritedCompanionData {
            header: header.as_ref(),
            files,
            deletions,
        })
        .collect::<Vec<_>>();
    let source = &active.source;
    let input = SourceObservationData {
        prefix,
        identity: &marker.identity,
        snapshot: &active.activated.snapshot,
        snapshot_parent_digest: Some(&active.activation.source_digest),
        selection: source.selection(),
        selection_generation: source.commitment().source_generation,
        selection_snapshot_digest: &source.commitment().snapshot_digest,
    };
    let normalized = ReconcileSourceData::normalize(
        input,
        closure,
        &capture.patches,
        &mappings,
        lineage.receipts(),
        &inherited,
        observed,
        cancel,
    )?;
    let normalized = match proof {
        SourceProof::Capture(proof) => normalized.with_provider_descriptors(proof, cancel)?,
        SourceProof::Publication(proof) => {
            normalized.with_publication_descriptors(proof, cancel)?
        }
    };
    action(&normalized)
}

/// Caller has already performed complete schema and scalar preflight.
pub(in crate::projection::provider) fn read_group(
    c: &Connection,
    id: &str,
    cancel: Option<&Cancellation>,
) -> Result<(
    CompanionHeaderRecord,
    Vec<ProviderCompanionFileRecord>,
    Vec<ProviderCompanionDeletionRecord>,
)> {
    let payload: Vec<u8> = c.query_row(
        "SELECT payload FROM projection_provider_companions WHERE id=?1 AND length(payload)<=65536",
        [id],
        |r| r.get(0),
    )?;
    let header = CompanionHeaderRecord::decode(&payload)?;
    let mut files = Vec::new();
    let mut statement = c.prepare("SELECT path,file_id,bytes,executable,manifest_id,base_file_id,base_executable,reason FROM projection_provider_companion_files WHERE companion_id=?1 ORDER BY path COLLATE BINARY LIMIT 100001")?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let reason: String = row.get(7)?;
        files.push(ProviderCompanionFileRecord {
            schema_version: "layrs.projection-provider-companion-file.v1".into(),
            path: RepoPath::parse(row.get::<_, String>(0)?).map_err(|e| invalid(e.to_string()))?,
            file_id: FileId::parse(row.get::<_, String>(1)?).map_err(|e| invalid(e.to_string()))?,
            bytes: row.get(2)?,
            executable: row.get(3)?,
            manifest_id: FileManifestId::parse(row.get::<_, String>(4)?)
                .map_err(|e| invalid(e.to_string()))?,
            base_file_id: row
                .get::<_, Option<String>>(5)?
                .map(FileId::parse)
                .transpose()
                .map_err(|e| invalid(e.to_string()))?,
            base_executable: row.get(6)?,
            reason: serde_json::from_value(serde_json::Value::String(reason))
                .map_err(|e| invalid(e.to_string()))?,
        });
    }
    let mut deletions = Vec::new();
    let mut statement = c.prepare("SELECT path,base_file_id,base_executable,base_manifest_id,reason FROM projection_provider_companion_deletions WHERE companion_id=?1 ORDER BY path COLLATE BINARY LIMIT 100001")?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        cancellation::check(cancel)?;
        let reason: String = row.get(4)?;
        deletions.push(ProviderCompanionDeletionRecord {
            schema_version: "layrs.projection-provider-companion-deletion.v1".into(),
            path: RepoPath::parse(row.get::<_, String>(0)?).map_err(|e| invalid(e.to_string()))?,
            base_file_id: FileId::parse(row.get::<_, String>(1)?)
                .map_err(|e| invalid(e.to_string()))?,
            base_executable: row.get(2)?,
            base_manifest_id: row
                .get::<_, Option<String>>(3)?
                .map(FileManifestId::parse)
                .transpose()
                .map_err(|e| invalid(e.to_string()))?,
            reason: serde_json::from_value(serde_json::Value::String(reason))
                .map_err(|e| invalid(e.to_string()))?,
        });
    }
    header.as_ref().validate_rows(&files, &deletions)?;
    Ok((header, files, deletions))
}
