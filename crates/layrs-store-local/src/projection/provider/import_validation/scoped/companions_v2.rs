//! Autonomous v2 companion provenance checks on the caller's authenticated
//! private history/lineage. No Provider, native root, online grant or byte read.
use super::*;
use crate::projection::{capture, offline, provider::records::companion_v2::*};
use layrs_core::projection::{EntryState, ReservedPaths};
use layrs_core::scm::TreeEntryTarget;

#[allow(clippy::too_many_arguments)]
pub(super) fn validate(
    c: &Connection,
    id: &str,
    header: &CompanionHeaderV2,
    receipt: &ProviderTransitionReceiptRecord,
    lineage: &ProvenanceIndex,
    content_ids: &mut BTreeSet<String>,
    catalog_ids: &mut BTreeSet<String>,
    cancel: Option<&Cancellation>,
) -> CauseResult<()> {
    validate_mode(
        c,
        id,
        header,
        receipt,
        lineage,
        content_ids,
        catalog_ids,
        cancel,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_mode(
    c: &Connection,
    id: &str,
    header: &CompanionHeaderV2,
    receipt: &ProviderTransitionReceiptRecord,
    lineage: &ProvenanceIndex,
    content_ids: &mut BTreeSet<String>,
    catalog_ids: &mut BTreeSet<String>,
    cancel: Option<&Cancellation>,
    source_outgoing: bool,
) -> CauseResult<()> {
    header.require_receipt(receipt)?;
    if header.digest()? != id {
        return Err(invalid("import v2 companion ID differs"));
    }
    let counts: (u64,u64) = c.query_row("SELECT (SELECT count(*) FROM projection_provider_companion_files WHERE companion_id=?1),(SELECT count(*) FROM projection_provider_companion_deletions WHERE companion_id=?1)", [id], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if counts != (header.file_count, header.deletion_count) {
        return Err(invalid("import v2 companion exact cardinality differs"));
    }
    let version = receipt.source_prefix.source.version();
    let effective = base::EffectiveBase::at(c, version, lineage)?;
    let source =
        offline::snapshot_at(c, version.generation, Some(&version.snapshot_digest))?.snapshot;
    let source_selection = selection::at(c, version.generation)?;
    source_selection
        .validate_snapshot(&source)
        .map_err(|e| invalid(e.to_string()))?;
    let source_selection_digest: String = c.query_row("SELECT digest FROM projection_selections WHERE generation<=?1 ORDER BY generation DESC LIMIT 1", [version.generation], |r| r.get(0))?;
    if source_selection_digest != version.selection_digest {
        return Err(invalid("import v2 source selection anchor differs"));
    }
    let expected_catalog = match &receipt.source_prefix.source {
        ImportSource::Ordinary { .. } => None,
        ImportSource::Provider {
            activation_map_id, ..
        } => Some(c.query_row(
            "SELECT catalog_id FROM projection_provider_activations WHERE id=?1",
            [activation_map_id],
            |r| r.get::<_, String>(0),
        )?),
    };
    if header.catalog_id != expected_catalog {
        return Err(invalid("import v2 companion source catalog differs"));
    }
    if let Some(catalog) = &header.catalog_id {
        catalog_ids.insert(catalog.clone());
    }

    let mut files = Vec::with_capacity(header.file_count as usize);
    let mut statement = c.prepare("SELECT path,file_id,bytes,executable,manifest_id,base_file_id,base_executable,reason,digest FROM projection_provider_companion_files WHERE companion_id=?1 ORDER BY path COLLATE BINARY")?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        super::super::super::cancellation::check(cancel)?;
        let (path,file,bytes,executable,manifest,base_file,base_executable,reason,digest):
            (String,String,u64,bool,String,Option<String>,Option<bool>,String,String) =
            (row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?);
        let record = ProviderCompanionFileRecord {
            schema_version: "layrs.projection-provider-companion-file.v1".into(),
            path: RepoPath::parse(&path).map_err(|e| invalid(e.to_string()))?,
            file_id: FileId::parse(&file).map_err(|e| invalid(e.to_string()))?,
            bytes,
            executable,
            manifest_id: FileManifestId::parse(&manifest).map_err(|e| invalid(e.to_string()))?,
            base_file_id: base_file
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
                file_id: &file,
                bytes,
                executable,
                manifest_id: &manifest,
                base_file_id: base_file.as_deref(),
                base_executable,
                reason: &reason,
                digest: &digest,
            },
        )?;
        effective.require(
            &record.path,
            record.base_file_id.as_ref(),
            record.base_executable,
        )?;
        header.require_representation(&record.file_id, &record.manifest_id, bytes)?;
        require_sticky_representation(c, header, &record.file_id, Some(bytes))?;
        content_ids.insert(file);
        if let Some(base) = base_file {
            require_content_mapping(c, &base, None, None)?;
            content_ids.insert(base);
        }
        files.push(record);
    }
    let accepted_by_path: BTreeMap<_, _> = header
        .intents
        .iter()
        .filter_map(|intent| match intent {
            CompanionIntentV2::RecoveryAcceptedDeletion { path, accepted, .. } => {
                Some((path, accepted))
            }
            _ => None,
        })
        .collect();
    let mut deletions = Vec::with_capacity(header.deletion_count as usize);
    let mut statement = c.prepare("SELECT path,base_file_id,base_executable,base_manifest_id,reason,digest FROM projection_provider_companion_deletions WHERE companion_id=?1 ORDER BY path COLLATE BINARY")?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        super::super::super::cancellation::check(cancel)?;
        let (path, file, executable, manifest, reason, digest): (
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
            base_file_id: FileId::parse(&file).map_err(|e| invalid(e.to_string()))?,
            base_executable: executable,
            base_manifest_id: manifest
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
                base_file_id: &file,
                base_executable: executable,
                base_manifest_id: manifest.as_deref(),
                reason: &reason,
                digest: &digest,
            },
        )?;
        if accepted_by_path.contains_key(&record.path) {
            effective.require(&record.path, None, None)?;
        } else {
            effective.require(
                &record.path,
                Some(&record.base_file_id),
                Some(record.base_executable),
            )?;
        }
        require_content_mapping(c, &file, manifest.as_deref(), None)?;
        content_ids.insert(file);
        deletions.push(record);
    }
    header.validate_rows(&files, &deletions)?;

    // A FileId may have additional protected representations, but its original
    // historical mapping is never replaced by a preferred alternate manifest.
    let mut preimages = BTreeSet::new();
    // Include the file itself: its historical FileId mapping may differ from
    // this scalar's exact retained alternate representation.
    preimages.extend(files.iter().map(|row| &row.file_id));
    for row in &files {
        preimages.extend(row.base_file_id.iter());
    }
    for row in &deletions {
        preimages.insert(&row.base_file_id);
    }
    for intent in &header.intents {
        if let CompanionIntentV2::SatisfiedDeletion {
            source_effective_base: Some(base),
            ..
        } = intent
        {
            preimages.insert(&base.file_id);
        }
        if let Some(accepted) = intent.accepted() {
            if let Some(expected) = &accepted.change.expected {
                preimages.insert(&expected.file_id);
            }
        }
    }
    for file in preimages {
        super::super::super::cancellation::check(cancel)?;
        let (manifest, bytes): (String, u64) = c.query_row(
            "SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1",
            [file.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let manifest = FileManifestId::parse(&manifest).map_err(|e| invalid(e.to_string()))?;
        header.require_representation(file, &manifest, bytes)?;
    }

    // Every accepted deletion names this exact source Capture head. Load and
    // validate it once, using imported identity/interval BEFORE interpretation.
    let accepted_patch = if header.intents.iter().any(|i| i.accepted().is_some()) {
        let (origin, first, last) =
            lineage.capture_source(header.source_capture_count, header.source_generation)?;
        let patch = capture::storage::load_metadata_for_origin(
            c,
            header.source_capture_count,
            origin,
            last,
        )?;
        if patch.projection_generation < first
            || patch.projection_generation != header.source_generation
            || patch.projection_digest != header.source_snapshot_digest
            || Some(&patch.digest) != header.source_capture_head.as_ref()
        {
            return Err(invalid("import v2 accepted Capture provenance differs"));
        }
        Some(patch)
    } else {
        None
    };
    let fresh = if !source_outgoing
        && header
            .intents
            .iter()
            .any(|i| matches!(i, CompanionIntentV2::SatisfiedDeletion { .. }))
    {
        Some(
            offline::snapshot_at(
                c,
                receipt.destination_observation_generation,
                Some(&receipt.destination_observation_digest),
            )?
            .snapshot,
        )
    } else {
        None
    };
    for intent in &header.intents {
        super::super::super::cancellation::check(cancel)?;
        if let Some(accepted) = intent.accepted() {
            let patch = accepted_patch
                .as_ref()
                .ok_or_else(|| invalid("import v2 accepted patch absent"))?;
            let actual = patch
                .changes
                .binary_search_by(|c| c.path.cmp(intent.path()))
                .ok()
                .and_then(|i| patch.changes.get(i));
            if actual != Some(&accepted.change)
                || patch.sequence != accepted.sequence
                || patch.digest != accepted.capture_digest
            {
                return Err(invalid("import v2 accepted deletion exact payload differs"));
            }
            let original = source
                .entries
                .binary_search_by(|e| e.path.cmp(intent.path()))
                .ok()
                .and_then(|i| source.entries.get(i));
            let expected = accepted
                .change
                .expected
                .as_ref()
                .ok_or_else(|| invalid("import v2 accepted preimage missing"))?;
            if !original.is_some_and(|e| matches!(&e.target,Some(TreeEntryTarget::File { file_id }) if file_id == &expected.file_id) && e.executable == expected.executable) {
                return Err(invalid("import v2 accepted original path preimage differs"));
            }
            effective.require(intent.path(), None, None)?;
            require_content_mapping(
                c,
                expected.file_id.as_str(),
                Some(accepted.preimage_manifest_id.as_str()),
                Some(accepted.preimage_bytes),
            )?;
            content_ids.insert(expected.file_id.to_string());
        }
        if let CompanionIntentV2::SatisfiedDeletion {
            path,
            source_effective_base,
            accepted,
            ..
        } = intent
        {
            effective.require(
                path,
                source_effective_base.as_ref().map(|b| &b.file_id),
                source_effective_base.as_ref().map(|b| b.executable),
            )?;
            if let Some(base) = source_effective_base {
                require_content_mapping(c, base.file_id.as_str(), None, None)?;
                content_ids.insert(base.file_id.to_string());
            } else if accepted.is_none() {
                return Err(invalid("import v2 satisfied deletion lost source intent"));
            }
            if source_outgoing {
                // B has no C observation snapshot. The header codec binds the
                // exact observation to this receipt; private+family C Ready
                // independently prove its authorized absence at publication.
                if !source_selection.includes(path)
                    || !source
                        .entries
                        .iter()
                        .any(|e| &e.path == path && e.writable && e.state != EntryState::Reserved)
                {
                    return Err(invalid("outgoing satisfied deletion source scope differs"));
                }
                ReservedPaths::new(
                    source
                        .entries
                        .iter()
                        .filter(|e| e.state == EntryState::Reserved)
                        .map(|e| e.path.as_str()),
                    source.path_semantics,
                )
                .map_err(|e| invalid(e.to_string()))?
                .check_path(path.as_str())
                .map_err(|e| invalid(e.to_string()))?;
                continue;
            }
            let fresh = fresh
                .as_ref()
                .ok_or_else(|| invalid("import v2 satisfied observation missing"))?;
            if fresh.base_step_id != source.base_step_id
                || fresh.base_root_tree_id != source.base_root_tree_id
                || fresh.path_semantics != source.path_semantics
            {
                return Err(invalid("import v2 satisfied observation Step/root differs"));
            }
            let old = source
                .entries
                .binary_search_by(|e| e.path.cmp(path))
                .ok()
                .and_then(|i| source.entries.get(i));
            let new = fresh
                .entries
                .binary_search_by(|e| e.path.cmp(path))
                .ok()
                .and_then(|i| fresh.entries.get(i));
            if !source_selection.includes(path)
                || !old.is_some_and(|e| e.writable && e.state != EntryState::Reserved)
                || !new.is_some_and(|e| {
                    e.writable && e.state == EntryState::Absent && e.target.is_none()
                })
            {
                return Err(invalid(
                    "import v2 satisfied deletion lacks exact authorized absence",
                ));
            }
            for snapshot in [&source, fresh] {
                ReservedPaths::new(
                    snapshot
                        .entries
                        .iter()
                        .filter(|e| e.state == EntryState::Reserved)
                        .map(|e| e.path.as_str()),
                    snapshot.path_semantics,
                )
                .map_err(|e| invalid(e.to_string()))?
                .check_path(path.as_str())
                .map_err(|e| invalid(e.to_string()))?;
            }
        }
    }
    Ok(())
}

fn require_sticky_representation(
    c: &Connection,
    header: &CompanionHeaderV2,
    file: &FileId,
    expected_bytes: Option<u64>,
) -> CauseResult<()> {
    require_content_mapping(c, file.as_str(), None, expected_bytes)?;
    let (manifest, bytes): (String, u64) = c.query_row(
        "SELECT manifest_id,bytes FROM projection_content WHERE file_id=?1",
        [file.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let manifest = FileManifestId::parse(&manifest).map_err(|e| invalid(e.to_string()))?;
    header.require_representation(file, &manifest, bytes)?;
    Ok(())
}
