//! Canonical, non-recursive source provenance. A receipt row is decoded only
//! after a matching schema fingerprint and its bounded SQL length/count probe.
use super::super::records::{Record, import::ImportSource};
use super::scoped::{ScopeView, compute_from_scope};
use super::*;
use crate::projection::capture;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SEGMENTS: usize = 4096;
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
const MAX_ANCESTRY_BYTES: u64 = 16 * 1024 * 1024;

pub(in crate::projection) fn read_ancestry(
    c: &Connection,
    marker: &format::Marker,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<ProvenanceIndex> {
    require_schema(c, marker, kind, cancel)?;
    read_ancestry_rows(c, kind, prefix, cancel)
}

/// The destination-local reader fences its genuine destination schema first.
/// Receipt origins remain the prefix's logical historical source identity.
pub(super) fn read_ancestry_rows(
    c: &Connection,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<ProvenanceIndex> {
    prefix.validate_fields()?;
    if kind == ImportDatabaseKind::AdmittedOrdinary
        && !matches!(
            &prefix.source,
            ImportSource::Ordinary {
                private_schema: 9 | 10,
                admission: Some(_),
                ..
            }
        )
    {
        return Err(invalid("admitted source requires ordinary schema9 binding"));
    }
    if prefix.ancestry_root_receipt.is_some() && !kind.has_provider_history() {
        return Err(invalid(
            "ordinary source cannot contain provider receipt ancestry",
        ));
    }
    let mut reversed = Vec::new();
    let mut seen = BTreeSet::new();
    let mut aggregate = 0u64;
    let mut id = prefix.ancestry_root_receipt.clone();
    while let Some(current) = id {
        super::super::cancellation::check(cancel)?;
        if reversed.len() >= MAX_SEGMENTS || !seen.insert(current.clone()) {
            return Err(invalid(
                "import receipt ancestry cycle or length exceeds bound",
            ));
        }
        let (count, bytes, owner_bytes, transaction_bytes, companion_bytes):
            (u64, u64, u64, u64, u64) = c.query_row(
            "SELECT count(*),coalesce(max(length(payload)),0),coalesce(max(length(CAST(owner_identity_digest AS BLOB))),0),coalesce(max(length(CAST(transaction_id AS BLOB))),0),coalesce(max(length(CAST(companion_id AS BLOB))),0) FROM projection_provider_transition_receipts WHERE id=?1",
            [&current], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        aggregate = aggregate
            .checked_add(bytes)
            .filter(|total| *total <= MAX_ANCESTRY_BYTES)
            .ok_or_else(|| invalid("import receipt ancestry exceeds aggregate byte bound"))?;
        if count != 1
            || bytes > MAX_RECEIPT_BYTES
            || owner_bytes != 71
            || transaction_bytes > 128
            || companion_bytes > 71
        {
            return Err(invalid("import receipt ancestry row missing or oversized"));
        }
        let (owner, transaction, companion, payload): (String, String, Option<String>, Vec<u8>) =
            c.query_row(
                "SELECT owner_identity_digest,transaction_id,companion_id,payload FROM projection_provider_transition_receipts WHERE id=?1",
                [&current], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?;
        let receipt = ProviderTransitionReceiptRecord::decode(&payload)?;
        if receipt.denial_coverage.is_some() && prefix.closure.denial_observations.is_none() {
            return Err(invalid(
                "inherited denial receipt requires prefix v3 transport",
            ));
        }
        receipt.require_row(TransitionReceiptRowData {
            id: &current,
            owner_identity_digest: &owner,
            transaction_id: &transaction,
            companion_id: companion.as_deref(),
            payload: &payload,
        })?;
        id = receipt.parent_receipt_id.clone();
        reversed.push(receipt);
    }
    reversed.reverse();
    let mut segments = Vec::with_capacity(reversed.len());
    for (index, receipt) in reversed.iter().enumerate() {
        super::super::cancellation::check(cancel)?;
        let parent = index.checked_sub(1).map(|previous| &reversed[previous]);
        receipt.source_prefix.require_admission_parent(parent)?;
        segments.push(ImportedSegment::from_receipt(receipt, parent)?);
    }
    prefix.require_admission_parent(reversed.last())?;
    prefix.validate_segments(&segments)?;
    let (identity, capture_count) = match &prefix.source {
        ImportSource::Ordinary {
            identity, version, ..
        }
        | ImportSource::Provider {
            identity, version, ..
        } => (identity, version.capture_count),
    };
    if let Some(last) = reversed.last() {
        if &last.destination_identity != identity {
            return Err(invalid(
                "last imported receipt does not name current source",
            ));
        }
    }
    let local_min_generation = reversed
        .last()
        .map_or(0, |receipt| receipt.destination_observation_generation);
    Ok(ProvenanceIndex {
        segments,
        receipts: reversed,
        local_identity: identity.clone(),
        capture_count,
        local_min_generation,
    })
}

/// Reinterpret copied prefixes oldest-first. The capture frontier only moves
/// forward, so obtaining content IDs does not recursively revisit ancestry.
pub(super) fn verify_historical_closures(
    c: &Connection,
    current: &ProviderImportPrefixRecord,
    metadata: &SourceMetadata,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    let lineage = &metadata.lineage;
    let mut activation_ids = BTreeSet::new();
    let mut receipt_ids = BTreeSet::new();
    let mut companion_ids = BTreeSet::new();
    let catalog_ids = BTreeSet::new();
    let mut content_file_ids = BTreeSet::new();
    let mut capture_lengths = BTreeMap::new();
    let current_max = match &current.source {
        ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => {
            version.generation
        }
    };
    let mut next_capture = 1u64;
    for (index, receipt) in lineage.receipts().iter().enumerate() {
        super::super::cancellation::check(cancel)?;
        let previous_prefix = &receipt.source_prefix;
        previous_prefix.validate_segments(&lineage.segments()[..index])?;
        let (schema, version) = match &previous_prefix.source {
            ImportSource::Ordinary {
                private_schema,
                version,
                ..
            }
            | ImportSource::Provider {
                private_schema,
                version,
                ..
            } => (*private_schema, version),
        };
        let capture_count = version.capture_count;
        let kind = match schema {
            6 => ImportDatabaseKind::Ordinary,
            7 => ImportDatabaseKind::SharedOrdinary,
            8 => ImportDatabaseKind::Provider,
            9 | 10
                if matches!(
                    &previous_prefix.source,
                    ImportSource::Ordinary {
                        admission: Some(_),
                        ..
                    }
                ) =>
            {
                ImportDatabaseKind::AdmittedOrdinary
            }
            _ => return Err(invalid("import ancestor schema unsupported")),
        };
        if (!kind.has_provider_history() && !receipt_ids.is_empty())
            || matches!(&previous_prefix.source, ImportSource::Provider { .. }) && schema != 8
        {
            return Err(invalid("import ancestor kind/schema boundary differs"));
        }
        // Historical schema9 is commitment DATA on the current database, never
        // an instruction to treat that database as the old live admission.
        previous_prefix.require_admission_parent(
            index
                .checked_sub(1)
                .map(|parent| &lineage.receipts()[parent]),
        )?;
        let snapshot_digest: String = c.query_row(
            "SELECT digest FROM projection_snapshots WHERE generation=?1",
            [version.generation],
            |r| r.get(0),
        )?;
        if snapshot_digest != version.snapshot_digest {
            return Err(invalid("import ancestor terminal snapshot differs"));
        }
        let selection_digest: String = c.query_row(
            "SELECT digest FROM projection_selections WHERE generation<=?1 ORDER BY generation DESC LIMIT 1",
            [version.generation], |r| r.get(0),
        )?;
        if selection_digest != version.selection_digest {
            return Err(invalid("import ancestor terminal selection differs"));
        }
        if capture_count > 0 {
            let capture_digest: String = c.query_row(
                "SELECT digest FROM projection_captures WHERE sequence=?1",
                [capture_count],
                |r| r.get(0),
            )?;
            if version.capture_head.as_deref() != Some(capture_digest.as_str()) {
                return Err(invalid("import ancestor terminal capture differs"));
            }
        }
        while next_capture <= capture_count {
            super::super::cancellation::check(cancel)?;
            let (origin, first_generation, max_generation) =
                lineage.capture_source(next_capture, current_max)?;
            let patch = capture::storage::load_metadata_for_origin(
                c,
                next_capture,
                origin,
                max_generation,
            )?;
            if patch.projection_generation < first_generation {
                return Err(invalid(
                    "import ancestor capture generation predates origin",
                ));
            }
            for change in patch.changes {
                if let Some(file) = change.replacement {
                    let id = file.file_id.as_str().to_owned();
                    if capture_lengths
                        .insert(id.clone(), file.bytes)
                        .is_some_and(|prior| prior != file.bytes)
                    {
                        return Err(invalid("import ancestor FileId has conflicting lengths"));
                    }
                    content_file_ids.insert(id);
                }
            }
            next_capture += 1;
        }
        if let ImportSource::Provider {
            activation_map_id, ..
        } = &previous_prefix.source
        {
            activation_ids.insert(activation_map_id.clone());
        }
        let prefix_id = previous_prefix.digest()?;
        // Keep the monotonic capture scope separate: later restore preimages
        // must not become authority for an earlier ancestor's mapping closure.
        let mut historical_content_ids = content_file_ids.clone();
        let mut historical_lengths = capture_lengths.clone();
        super::restore_history::extend_content(
            c,
            previous_prefix,
            lineage,
            &mut historical_content_ids,
            &mut historical_lengths,
            cancel,
        )?;
        let scope = ScopeView {
            kind,
            prefix_id: &prefix_id,
            catalog_ids: &catalog_ids,
            activation_ids: &activation_ids,
            receipt_ids: &receipt_ids,
            companion_ids: &companion_ids,
            content_file_ids: &historical_content_ids,
            capture_lengths: &historical_lengths,
            receipts: &lineage.receipts()[..index],
            lineage,
        };
        if compute_from_scope(c, kind, previous_prefix, &scope, cancel)? != previous_prefix.closure
        {
            return Err(invalid("import ancestor immutable SQL closure differs"));
        }
        receipt_ids.insert(receipt.digest()?);
        if let Some(companion) = &receipt.companion_id {
            companion_ids.insert(companion.clone());
        }
    }
    Ok(())
}
