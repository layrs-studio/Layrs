//! Full bounded source history metadata, with imported origin resolved before
//! the existing capture/publication interpreters read each row.
use super::super::records::{
    Record,
    import::{ImportSource, ImportSourceReferences},
};
use super::*;
use crate::projection::{capture, publication};
use layrs_core::projection::ProjectionSnapshot;
use rusqlite::OptionalExtension;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::projection) struct SourceMetadata {
    pub(super) kind: ImportDatabaseKind,
    pub(super) prefix_id: String,
    pub(super) lineage: ProvenanceIndex,
    pub(super) content_file_ids: BTreeSet<String>,
    pub(super) capture_lengths: BTreeMap<String, u64>,
    pub(super) catalog_ids: BTreeSet<String>,
    pub(super) activation_ids: BTreeSet<String>,
    pub(super) receipt_ids: BTreeSet<String>,
    pub(super) companion_ids: BTreeSet<String>,
}

/// Explicit historical anchor. A destination commits the inherited generation
/// zero in its own genuine FORMAT and commits the incoming prefix via receipt.
/// This never manufactures the old source FORMAT or its physical authority.
pub(super) struct HistoryAnchor<'a> {
    marker: &'a format::Marker,
    binding: HistoryBinding<'a>,
}
enum HistoryBinding<'a> {
    Live,
    Destination {
        prefix: &'a ProviderImportPrefixRecord,
        receipt: &'a ProviderTransitionReceiptRecord,
    },
    AdmittedSource {
        outgoing_digest: String,
        incoming_prefix: &'a ProviderImportPrefixRecord,
        incoming_receipt: &'a ProviderTransitionReceiptRecord,
    },
}
impl<'a> HistoryAnchor<'a> {
    pub(super) fn destination(
        marker: &'a format::Marker,
        prefix: &'a ProviderImportPrefixRecord,
        receipt: &'a ProviderTransitionReceiptRecord,
    ) -> CauseResult<Self> {
        receipt.require_prefix(prefix)?;
        if marker.identity != prefix.destination_identity
            || marker.import_digest.as_deref() != Some(prefix.digest()?.as_str())
        {
            return Err(invalid("historical destination anchor differs"));
        }
        Ok(Self {
            marker,
            binding: HistoryBinding::Destination { prefix, receipt },
        })
    }

    /// The current FORMAT names its incoming prefix, while this export commits
    /// a distinct outgoing prefix. Keep both links explicit and independently
    /// checked rather than weakening the destination anchor's exact equality.
    fn admitted_source(
        marker: &'a format::Marker,
        outgoing: &ProviderImportPrefixRecord,
        incoming_prefix: &'a ProviderImportPrefixRecord,
        incoming_receipt: &'a ProviderTransitionReceiptRecord,
    ) -> CauseResult<Self> {
        let anchor = Self {
            marker,
            binding: HistoryBinding::AdmittedSource {
                outgoing_digest: outgoing.digest()?,
                incoming_prefix,
                incoming_receipt,
            },
        };
        anchor.require_prefix(outgoing)?;
        Ok(anchor)
    }

    fn require_prefix(&self, prefix: &ProviderImportPrefixRecord) -> CauseResult<()> {
        match &self.binding {
            HistoryBinding::Live => {}
            HistoryBinding::Destination {
                prefix: incoming,
                receipt,
            } => {
                if *incoming != prefix {
                    return Err(invalid("historical destination prefix changed"));
                }
                receipt.require_prefix(prefix)?;
            }
            HistoryBinding::AdmittedSource {
                outgoing_digest,
                incoming_prefix,
                incoming_receipt,
            } => {
                let binding = prefix
                    .source_admission()
                    .ok_or_else(|| invalid("admitted source binding absent"))?;
                incoming_receipt.require_prefix(incoming_prefix)?;
                prefix.require_admission_parent(Some(incoming_receipt))?;
                if prefix.digest()? != *outgoing_digest
                    || incoming_prefix.digest()? != binding.incoming_prefix_id
                    || incoming_receipt.digest()? != binding.incoming_receipt_id
                    || self.marker.identity != incoming_prefix.destination_identity
                    || self.marker.import_digest.as_deref()
                        != Some(binding.incoming_prefix_id.as_str())
                {
                    return Err(invalid("historical admitted source anchor differs"));
                }
            }
        }
        Ok(())
    }
}

/// Caller supplies a previously authenticated FORMAT and source references.
/// The image digest must come from a separate retained image witness; comparing
/// it here does not turn SQL metadata into proof of native or content bytes.
pub(in crate::projection) fn read_source_metadata(
    c: &Connection,
    marker: &format::Marker,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    references: ImportSourceReferences<'_>,
    image_digest: &str,
    cancel: Option<&Cancellation>,
) -> CauseResult<SourceMetadata> {
    let lineage = ancestry::read_ancestry(c, marker, kind, prefix, cancel)?;
    let current_catalog_id = match &references {
        ImportSourceReferences::Ordinary(_) | ImportSourceReferences::AdmittedOrdinary { .. } => {
            None
        }
        ImportSourceReferences::Provider { allocation, .. } => {
            Some(allocation.source.catalog_id.clone())
        }
    };
    prefix.require_references(references)?;
    let (schema, version, source_identity) = match &prefix.source {
        ImportSource::Ordinary {
            private_schema,
            version,
            identity,
            ..
        }
        | ImportSource::Provider {
            private_schema,
            version,
            identity,
            ..
        } => (*private_schema, version, identity),
    };
    let schema_matches = match kind {
        ImportDatabaseKind::Ordinary => schema == 6,
        ImportDatabaseKind::SharedOrdinary => schema == 7,
        ImportDatabaseKind::Provider => schema == 8,
        // Compared with the genuine admitted proof below as well.
        ImportDatabaseKind::AdmittedOrdinary => matches!(schema, 9 | 10),
    };
    if !schema_matches || image_digest != version.effective_image_digest {
        return Err(invalid(
            "import source schema or independently witnessed image differs",
        ));
    }
    // This is the actual incoming admission from this same retained snapshot.
    // Detached reference DATA alone cannot establish a schema9 source.
    let admitted = if kind == ImportDatabaseKind::AdmittedOrdinary {
        let admitted = super::super::managed_admission::read_admitted_import(c, marker, cancel)?;
        let binding = prefix
            .source_admission()
            .ok_or_else(|| invalid("admitted source binding absent"))?;
        if admitted.private_schema() != schema
            || admitted.record().digest()? != binding.admission_digest
            || admitted.prefix().digest()? != binding.incoming_prefix_id
            || admitted.receipt().digest()? != binding.incoming_receipt_id
            || marker
                .shared
                .as_ref()
                .map(|shared| shared.database_identity.as_str())
                != Some(binding.database_identity.as_str())
            || prefix.ancestry_root_receipt.as_deref() != Some(binding.incoming_receipt_id.as_str())
        {
            return Err(invalid(
                "import source genuine admission differs from prefix",
            ));
        }
        Some(admitted)
    } else {
        None
    };
    if kind.has_provider_history() {
        let (count, max_backend_bytes, max_mount_bytes): (u64, u64, u64) = c.query_row(
            "SELECT count(*),coalesce(max(length(backend)),0),coalesce(max(length(mount_id)),0) FROM projection_backend",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if count != 1 || max_backend_bytes > 16 || max_mount_bytes > 128 {
            return Err(invalid("import source backend row differs"));
        }
        let (backend, mount): (String, Option<String>) = c.query_row(
            "SELECT backend,mount_id FROM projection_backend WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        match (&prefix.source, backend.as_str(), mount.as_deref()) {
            (ImportSource::Ordinary { .. }, "ordinary", None) => {
                for table in [
                    "projection_provider_mounts",
                    "projection_provider_capture_transactions",
                    "projection_provider_capture_receipts",
                ] {
                    super::super::cancellation::check(cancel)?;
                    if c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table})"), [], |r| {
                        r.get::<_, bool>(0)
                    })? {
                        return Err(invalid(
                            "import ordinary source retains live provider state",
                        ));
                    }
                }
                // Schema9 retains its incoming DestinationReady entry, whose
                // exact row and links were validated by read_admitted_import.
                if admitted.is_none()
                    && c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM projection_provider_transitions)",
                        [],
                        |r| r.get::<_, bool>(0),
                    )?
                {
                    return Err(invalid(
                        "import ordinary source retains live provider state",
                    ));
                }
            }
            (ImportSource::Provider { binding, .. }, "provider", Some(id))
                if id == binding.transaction_id => {}
            _ => return Err(invalid("import source backend and prefix kind differ")),
        }
    } else if matches!(&prefix.source, ImportSource::Provider { .. }) {
        return Err(invalid("provider source requires schema8 backend"));
    }
    require_quiescent_history(c, marker, kind, prefix, &lineage, cancel)?;
    let (head_count, head_generation): (u64, Option<u64>) = c.query_row(
        "SELECT count(*),max(generation) FROM projection_head",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if head_count != 1 || head_generation != Some(version.generation) {
        return Err(invalid("import source head differs from prefix"));
    }
    let anchor = match &admitted {
        Some(admitted) => {
            HistoryAnchor::admitted_source(marker, prefix, admitted.prefix(), admitted.receipt())?
        }
        None => HistoryAnchor {
            marker,
            binding: HistoryBinding::Live,
        },
    };
    let metadata = read_historical_metadata(
        c,
        &anchor,
        kind,
        prefix,
        lineage,
        current_catalog_id,
        cancel,
    )?;
    if source_identity != &marker.identity {
        return Err(invalid("import source identity differs from FORMAT"));
    }
    Ok(metadata)
}

/// Common immutable high-water interpreter. Live-source backend, current head,
/// references and quiescence remain exclusively in `read_source_metadata`.
pub(super) fn read_historical_metadata(
    c: &Connection,
    anchor: &HistoryAnchor<'_>,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    lineage: ProvenanceIndex,
    current_catalog_id: Option<String>,
    cancel: Option<&Cancellation>,
) -> CauseResult<SourceMetadata> {
    anchor.require_prefix(prefix)?;
    let marker = anchor.marker;
    let version = match &prefix.source {
        ImportSource::Provider { version, .. } | ImportSource::Ordinary { version, .. } => version,
    };
    let (snapshot_count, max_generation, snapshot_bytes, max_digest_bytes, max_parent_bytes):
        (u64, Option<u64>, u64, u64, u64) = c.query_row(
        "SELECT count(*),max(generation),coalesce(sum(length(payload)+length(CAST(digest AS BLOB))+coalesce(length(CAST(parent_digest AS BLOB)),0)),0),coalesce(max(length(CAST(digest AS BLOB))),0),coalesce(max(length(CAST(parent_digest AS BLOB))),0) FROM projection_snapshots WHERE generation<=?1",
        [version.generation], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    let (selection_count, min_selection, max_selection, selection_bytes): (u64, Option<u64>, Option<u64>, u64) = c.query_row(
        "SELECT count(*),min(generation),max(generation),coalesce(sum(length(payload)),0) FROM projection_selections WHERE generation<=?1",
        [version.generation], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let expected = version
        .generation
        .checked_add(1)
        .ok_or_else(|| invalid("import source generation overflow"))?;
    if snapshot_count != expected
        || selection_count != prefix.closure.selections.count
        || min_selection != Some(0)
        || max_selection.is_none_or(|g| g > version.generation)
        || max_generation != Some(version.generation)
        || snapshot_bytes > 512 * 1024 * 1024
        || max_digest_bytes != 64
        || max_parent_bytes > 64
        || selection_bytes > 256 * 1024 * 1024
    {
        return Err(invalid("import source snapshot/selection prefix has a gap"));
    }
    let mut previous: Option<String> = None;
    let mut statement = c.prepare(
        "SELECT generation,digest,parent_digest,payload FROM projection_snapshots WHERE generation<=?1 ORDER BY generation",
    )?;
    let mut rows = statement.query([version.generation])?;
    let mut generation = 0u64;
    while let Some(row) = rows.next()? {
        super::super::cancellation::check(cancel)?;
        let found: u64 = row.get(0)?;
        let digest: String = row.get(1)?;
        let parent: Option<String> = row.get(2)?;
        let bytes: Vec<u8> = row.get(3)?;
        if found != generation
            || parent != previous
            || bytes.len() > 64 * 1024 * 1024
            || database::digest(found, parent.as_deref(), &bytes) != digest
        {
            return Err(invalid("import source snapshot ancestry or digest differs"));
        }
        let snapshot: ProjectionSnapshot =
            serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
        if snapshot
            .canonical_bytes()
            .map_err(|e| invalid(e.to_string()))?
            != bytes
        {
            return Err(invalid("import source snapshot is not canonical"));
        }
        let selected = selection::at(c, found)?;
        selected
            .validate_snapshot(&snapshot)
            .map_err(|e| invalid(e.to_string()))?;
        if found == 0 {
            if digest != marker.initial_snapshot_digest {
                return Err(invalid(
                    "import source initial snapshot differs from FORMAT",
                ));
            }
            marker.validate_initial_selection(&selected)?;
        }
        previous = Some(digest);
        generation += 1;
    }
    if generation != expected || previous.as_ref() != Some(&version.snapshot_digest) {
        return Err(invalid("import source terminal snapshot differs"));
    }
    let selection_digest: String = c.query_row(
        "SELECT digest FROM projection_selections WHERE generation<=?1 ORDER BY generation DESC LIMIT 1",
        [version.generation], |r| r.get(0),
    )?;
    if selection_digest != version.selection_digest {
        return Err(invalid("import source terminal selection differs"));
    }
    let (capture_count, max_sequence, capture_bytes): (u64, Option<u64>, u64) = c.query_row(
        "SELECT count(*),max(sequence),coalesce(sum(length(payload)),0) FROM projection_captures WHERE sequence<=?1",
        [version.capture_count], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let (publication_count, publication_bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(sum(length(receipt_bytes)+length(after_state)+coalesce(length(next_snapshot),0)),0) FROM projection_publications WHERE sequence<=?1",
        [version.capture_count], |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if capture_count != version.capture_count
        || max_sequence != (version.capture_count > 0).then_some(version.capture_count)
        || capture_bytes > 512 * 1024 * 1024
        || publication_count > capture_count
        || publication_bytes > 512 * 1024 * 1024
    {
        return Err(invalid("import capture prefix count or sequence has a gap"));
    }
    let mut content_file_ids = BTreeSet::new();
    let mut capture_lengths = BTreeMap::new();
    for sequence in 1..=version.capture_count {
        super::super::cancellation::check(cancel)?;
        let (origin, first_generation, max_generation) =
            lineage.capture_source(sequence, version.generation)?;
        let patch =
            capture::storage::load_metadata_for_origin(c, sequence, origin, max_generation)?;
        if patch.projection_generation < first_generation {
            return Err(invalid("capture predates its verified origin interval"));
        }
        for change in &patch.changes {
            if let Some(file) = &change.replacement {
                let id = file.file_id.as_str().to_owned();
                if capture_lengths
                    .insert(id.clone(), file.bytes)
                    .is_some_and(|prior| prior != file.bytes)
                {
                    return Err(invalid("import capture FileId has conflicting lengths"));
                }
                content_file_ids.insert(id);
            }
        }
        if publication::storage::completed_record_for_origin(c, sequence, origin, max_generation)?
            .is_some()
        {
            // The existing interpreter authenticates the publication and its
            // paired capture; its metadata remains in the bounded rowset.
        }
        if sequence == version.capture_count
            && patch.digest != version.capture_head.as_deref().unwrap_or("")
        {
            return Err(invalid("import terminal capture digest differs"));
        }
    }
    if version.capture_count == 0 && version.capture_head.is_some() {
        return Err(invalid("import zero-capture head differs"));
    }
    super::restore_history::extend_content(
        c,
        prefix,
        &lineage,
        &mut content_file_ids,
        &mut capture_lengths,
        cancel,
    )?;
    for (index, receipt) in lineage.receipts().iter().enumerate() {
        super::super::cancellation::check(cancel)?;
        let (next_generation, next_capture_count) = match lineage.receipts().get(index + 1) {
            Some(next) => match &next.source_prefix.source {
                ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => {
                    (version.generation, version.capture_count)
                }
            },
            None => (version.generation, version.capture_count),
        };
        if receipt.destination_observation_generation > next_generation
            || receipt.destination_placement_generation > next_generation
            || receipt.destination_capture_sequence > next_capture_count
        {
            return Err(invalid(
                "import receipt destination escaped source high-water",
            ));
        }
        for (generation, expected_digest) in [
            (
                receipt.destination_observation_generation,
                &receipt.destination_observation_digest,
            ),
            (
                receipt.destination_placement_generation,
                &receipt.destination_placement_digest,
            ),
        ] {
            let digest: String = c.query_row(
                "SELECT digest FROM projection_snapshots WHERE generation=?1",
                [generation],
                |r| r.get(0),
            )?;
            if &digest != expected_digest {
                return Err(invalid("import receipt destination snapshot differs"));
            }
        }
        let patch = capture::storage::load_metadata_for_origin(
            c,
            receipt.destination_capture_sequence,
            &receipt.destination_identity,
            receipt.destination_placement_generation,
        )?;
        if patch.digest != receipt.destination_capture_digest
            || patch.projection_generation != receipt.destination_placement_generation
        {
            return Err(invalid(
                "import receipt destination capture identity or placement differs",
            ));
        }
    }
    // Rows after the immutable high-water must not affect this source prefix.
    let _: Option<u64> = c
        .query_row(
            "SELECT sequence FROM projection_captures WHERE sequence>?1 ORDER BY sequence LIMIT 1",
            [version.capture_count],
            |r| r.get(0),
        )
        .optional()?;
    let mut catalog_ids = BTreeSet::new();
    let mut activation_ids = BTreeSet::new();
    let mut receipt_ids = BTreeSet::new();
    let mut companion_ids = BTreeSet::new();
    if let Some(id) = current_catalog_id {
        catalog_ids.insert(id);
    }
    for receipt in lineage.receipts() {
        super::super::cancellation::check(cancel)?;
        receipt_ids.insert(receipt.digest()?);
        if let Some(id) = &receipt.companion_id {
            companion_ids.insert(id.clone());
        }
        if let ImportSource::Provider {
            activation_map_id, ..
        } = &receipt.source_prefix.source
        {
            activation_ids.insert(activation_map_id.clone());
        }
    }
    if let ImportSource::Provider {
        activation_map_id, ..
    } = &prefix.source
    {
        activation_ids.insert(activation_map_id.clone());
    }
    Ok(SourceMetadata {
        kind,
        prefix_id: prefix.digest()?,
        lineage,
        content_file_ids,
        capture_lengths,
        catalog_ids,
        activation_ids,
        receipt_ids,
        companion_ids,
    })
}

/// Operational SQL refusal for an independently authenticated live source.
/// This does not authenticate current bytes, a snapshot, or a source closure.
/// Historical Published replay must not use this live-frontier requirement.
pub(in crate::projection::provider) fn require_live_source_quiescence(
    c: &Connection,
    marker: &format::Marker,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    let lineage = ancestry::read_ancestry(c, marker, kind, prefix, cancel)?;
    require_quiescent_history(c, marker, kind, prefix, &lineage, cancel)
}

fn require_quiescent_history(
    c: &Connection,
    marker: &format::Marker,
    kind: ImportDatabaseKind,
    prefix: &ProviderImportPrefixRecord,
    lineage: &ProvenanceIndex,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    let expected_import = lineage
        .receipts()
        .last()
        .map(|receipt| receipt.source_prefix_id.as_str());
    if marker.import_digest.as_deref() != expected_import {
        return Err(invalid(
            "import FORMAT digest differs from final ancestor prefix",
        ));
    }
    if prefix.supports_restore_history() {
        let version = match &prefix.source {
            ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => {
                version
            }
        };
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_restore_history WHERE capture_sequence>?1)",
            [version.capture_count],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(invalid(
                "live import restore history exceeds source capture frontier",
            ));
        }
    }
    for table in [
        "projection_install",
        "projection_refresh_pending",
        "projection_refresh_history",
        "projection_publication_pending",
        "projection_namespace_pending",
        "projection_namespace_operations",
        "projection_restore_pending",
        "projection_restore_history",
        "projection_rebase_pending",
        "projection_rebase_history",
        "projection_rebase_previews",
    ] {
        super::super::cancellation::check(cancel)?;
        if table == "projection_restore_history" && prefix.supports_restore_history() {
            continue;
        }
        let query = format!("SELECT EXISTS(SELECT 1 FROM {table} LIMIT 1)");
        if c.query_row(&query, [], |r| r.get::<_, bool>(0))? {
            return Err(invalid(format!(
                "import source has unrepresented {table} history"
            )));
        }
    }
    if kind != ImportDatabaseKind::Ordinary {
        let (count, bytes): (u64, u64) = c.query_row(
            "SELECT count(*),coalesce(max(length(payload)),0) FROM projection_imports",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count != u64::from(!lineage.receipts().is_empty()) || bytes > 16 * 1024 * 1024 {
            return Err(invalid(
                "import source inherited prefix count or size differs",
            ));
        }
        if let Some(last) = lineage.receipts().last() {
            let (digest, payload): (String, Vec<u8>) = c.query_row(
                "SELECT digest,payload FROM projection_imports WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            last.source_prefix.require_row(&digest, &payload)?;
            if digest != last.source_prefix_id {
                return Err(invalid("import source inherited prefix/receipt differs"));
            }
        }
        if c.query_row(
            "SELECT EXISTS(SELECT 1 FROM projection_conversion)",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid(
                "import source has unrepresented legacy conversion state",
            ));
        }
    }
    Ok(())
}
