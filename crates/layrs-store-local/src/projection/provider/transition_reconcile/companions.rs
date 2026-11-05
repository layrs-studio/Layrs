//! Closed companion derivation. No constructor accepts caller-selected recovery
//! rows: the entire inventory is reconciled again against the retained source.
//! This is DATA, not a source writer, reservation, authority or Ready proof.
use super::super::{
    managed_budget,
    records::{
        codec,
        companion_v2::*,
        import::{ordered_companion_rowset, ImportSource},
    },
};
use super::*;
use layrs_core::scm::canonical_json;

pub(in crate::projection::provider) mod sql;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::projection::provider) struct AuthorizedFileData {
    pub path: RepoPath,
    pub file_id: FileId,
    pub manifest_id: FileManifestId,
    pub bytes: u64,
    pub executable: bool,
}

/// Protect BOTH owners before either new receipt. Each database persists the
/// identical new header/scalars before ITS OWN receipt. B rows remain absent
/// until genuine C private+family Ready. Inherited artifacts remain verbatim.
#[derive(PartialEq, Eq)]
pub(in crate::projection) struct OwnedCompanionData {
    source_prefix_id: String,
    source_selection_payload: Vec<u8>,
    observation_generation: u64,
    observation_digest: String,
    observation_selection_digest: String,
    descriptors_digest: String,
    authority_digest: String,
    denied_paths_digest: String,
    header: Option<CompanionHeaderV2>,
    files: Vec<ProviderCompanionFileRecord>,
    deletions: Vec<ProviderCompanionDeletionRecord>,
    inherited: Vec<OwnedInheritedData>,
    overlay: Vec<CapturedChange>,
    authorized_files: Vec<AuthorizedFileData>,
    representations: Vec<RepresentationData>,
    manifests: Vec<FileManifest>,
    source_mappings: BTreeMap<FileId, (FileManifestId, u64)>,
    metadata_bytes: u64,
    quota_bytes: u64,
}

#[derive(PartialEq, Eq)]
struct OwnedInheritedData {
    header: CompanionHeaderRecord,
    files: Vec<ProviderCompanionFileRecord>,
    deletions: Vec<ProviderCompanionDeletionRecord>,
}

impl OwnedCompanionData {
    /// `metadata_without_companions` is the independently measured transition
    /// metadata envelope (history, intents, receipt, mappings, claims, etc.).
    /// This function adds new companion rows TWICE (source + destination) and
    /// inherited rows ONCE (destination), before any write/reservation effect.
    pub(in crate::projection::provider) fn derive(
        source: &ReconcileSourceData<'_>,
        image: &CompleteSourceImageData<'_, '_>,
        observation: &TransitionObservationData,
        supplied: &[FileManifest],
        metadata_without_companions: u64,
        provider_proof: Option<&super::super::validation::PrivateActiveCaptureValidation>,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        let catalog_id = source.companion_catalog(provider_proof)?;
        let reconciled = reconcile(source, image, observation, cancel)?;
        Self::from_reconciled(
            source,
            image,
            observation,
            reconciled,
            supplied,
            metadata_without_companions,
            catalog_id,
            cancel,
        )
    }

    pub(in crate::projection::provider) fn derive_publication(
        source: &ReconcileSourceData<'_>,
        image: &CompleteSourceImageData<'_, '_>,
        observation: &TransitionObservationData,
        supplied: &[FileManifest],
        metadata_without_companions: u64,
        provider_proof: Option<&super::super::validation::PrivateActivePublicationValidation>,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        let catalog_id = source.publication_companion_catalog(provider_proof)?;
        let reconciled = reconcile(source, image, observation, cancel)?;
        Self::from_reconciled(
            source,
            image,
            observation,
            reconciled,
            supplied,
            metadata_without_companions,
            catalog_id,
            cancel,
        )
    }

    fn from_reconciled(
        source: &ReconcileSourceData<'_>,
        image: &CompleteSourceImageData<'_, '_>,
        observation: &TransitionObservationData,
        reconciled: TransitionReconcileData<'_>,
        supplied: &[FileManifest],
        metadata_without_companions: u64,
        catalog_id: Option<String>,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        let (identity, schema, version) = match &source.input.prefix.source {
            ImportSource::Ordinary {
                identity,
                private_schema,
                version,
                ..
            } => (identity, *private_schema, version),
            ImportSource::Provider {
                identity,
                private_schema,
                version,
                ..
            } => (identity, *private_schema, version),
        };
        let files: Vec<_> = reconciled
            .recovery_files
            .into_iter()
            .map(|file| file.row)
            .collect();
        let mut deletions = Vec::new();
        let mut intents = Vec::new();
        for deletion in reconciled.recovery_deletions {
            cancellation::check(cancel)?;
            match deletion {
                RecoveryDeletionIntentData::FromEffectiveFile(row) => deletions.push(row),
                RecoveryDeletionIntentData::FromAcceptedCapture { provenance, reason } => {
                    let accepted = accepted_record(provenance);
                    let expected = accepted.change.expected.as_ref().ok_or_else(|| {
                        invalid("owned companion accepted deletion has no preimage")
                    })?;
                    let path = accepted.change.path.clone();
                    deletions.push(ProviderCompanionDeletionRecord {
                        schema_version: "layrs.projection-provider-companion-deletion.v1".into(),
                        path: path.clone(),
                        base_file_id: expected.file_id.clone(),
                        base_executable: expected.executable,
                        base_manifest_id: Some(accepted.preimage_manifest_id.clone()),
                        reason,
                    });
                    intents.push(CompanionIntentV2::RecoveryAcceptedDeletion {
                        path,
                        reason,
                        accepted,
                    });
                }
            }
        }
        for satisfied in reconciled.satisfied_deletions {
            intents.push(CompanionIntentV2::SatisfiedDeletion {
                path: satisfied.path,
                source_effective_base: satisfied.source_effective_base.map(|base| {
                    CompanionBaseV2 {
                        file_id: base.file_id,
                        executable: base.executable,
                    }
                }),
                accepted: satisfied.accepted_deletion.map(accepted_record),
                destination_generation: satisfied.destination_generation,
                destination_snapshot_digest: satisfied.destination_snapshot_digest,
            });
        }
        deletions.sort_by(|a, b| a.path.cmp(&b.path));
        intents.sort_by(|a, b| a.path().cmp(b.path()));
        let has_new = !files.is_empty() || !deletions.is_empty() || !intents.is_empty();
        let header = if has_new {
            let file_refs = files
                .iter()
                .map(|row| Ok((row.path.as_str(), row.digest()?)))
                .collect::<Result<Vec<_>>>()?;
            let deletion_refs = deletions
                .iter()
                .map(|row| Ok((row.path.as_str(), row.digest()?)))
                .collect::<Result<Vec<_>>>()?;
            let mut preserved_ids = BTreeSet::new();
            for row in &files {
                preserved_ids.insert(row.file_id.clone());
                preserved_ids.extend(row.base_file_id.iter().cloned());
            }
            for row in &deletions {
                preserved_ids.insert(row.base_file_id.clone());
            }
            for intent in &intents {
                if let Some(accepted) = intent.accepted() {
                    if let Some(expected) = &accepted.change.expected {
                        preserved_ids.insert(expected.file_id.clone());
                    }
                }
                if let CompanionIntentV2::SatisfiedDeletion {
                    source_effective_base: Some(base),
                    ..
                } = intent
                {
                    preserved_ids.insert(base.file_id.clone());
                }
            }
            let preserved_representations = reconciled
                .representations
                .iter()
                .filter(|representation| preserved_ids.contains(&representation.file_id))
                .map(|representation| CompanionRepresentationV2 {
                    file_id: representation.file_id.clone(),
                    manifest_id: representation.manifest_id.clone(),
                    bytes: representation.bytes,
                })
                .collect();
            let header = CompanionHeaderV2 {
                schema_version: SCHEMA.into(),
                source_identity: identity.clone(),
                source_private_schema: schema,
                source_generation: version.generation,
                source_snapshot_digest: version.snapshot_digest.clone(),
                source_capture_count: version.capture_count,
                source_capture_head: version.capture_head.clone(),
                import_prefix_id: source.prefix_id.clone(),
                catalog_id,
                content_mappings: source.input.prefix.closure.content_mappings.clone(),
                file_count: files.len() as u64,
                deletion_count: deletions.len() as u64,
                files_digest: ordered_companion_rowset(
                    "layrs-projection-provider-companion-files-v1",
                    &file_refs,
                )?,
                deletions_digest: ordered_companion_rowset(
                    "layrs-projection-provider-companion-deletions-v1",
                    &deletion_refs,
                )?,
                preserved_representations,
                intents,
            };
            header.require_prefix(source.input.prefix)?;
            header.validate_rows(&files, &deletions)?;
            Some(header)
        } else {
            None
        };

        // The kernel unions original mappings, complete live image and every
        // fresh descriptor. Never pick a different manifest by FileId alone.
        let representations = reconciled.representations;
        let manifests = require_union(&representations, supplied, cancel)?;
        let authorized_files = authorized_files(image, observation, &reconciled.overlay, cancel)?;
        // Retained Ready reconstructs B's image from checkout paths in B's
        // selected original namespace. A formerly Reserved path can become
        // readable without ever having belonged to B's complete image. There
        // is no durable bit distinguishing that remote-only file, so refuse
        // this shape before planning rather than publish an unreopenable C.
        let original_selected: BTreeSet<_> = source
            .input
            .snapshot
            .entries
            .iter()
            .filter(|entry| source.input.selection.includes(&entry.path))
            .map(|entry| &entry.path)
            .collect();
        for file in &authorized_files {
            cancellation::check(cancel)?;
            if original_selected.contains(&file.path)
                && !image
                    .entries
                    .get(&file.path)
                    .is_some_and(|entry| entry.file.is_some())
            {
                return Err(invalid(
                    "owned transition checkout invents an original image file",
                ));
            }
        }
        let mut inherited = Vec::new();
        let mut inherited_bytes = 0;
        for artifact in reconciled.inherited {
            cancellation::check(cancel)?;
            // normalize() already authenticated the incoming receipt ancestry;
            // roundtrip retains canonical payloads rather than rewriting origins.
            let header = CompanionHeaderRecord::decode(&artifact.header.encode()?)?;
            let files = artifact
                .files
                .iter()
                .map(|r| ProviderCompanionFileRecord::decode(&r.encode()?))
                .collect::<crate::projection::Result<Vec<_>>>()?;
            let deletions = artifact
                .deletions
                .iter()
                .map(|r| ProviderCompanionDeletionRecord::decode(&r.encode()?))
                .collect::<crate::projection::Result<Vec<_>>>()?;
            inherited_bytes = managed_budget::add_metadata(
                inherited_bytes,
                sql_bytes(&header.as_ref().encode()?, &files, &deletions)?,
            )?;
            inherited.push(OwnedInheritedData {
                header,
                files,
                deletions,
            });
        }
        let new_bytes = header
            .as_ref()
            .map(|h| sql_bytes(&h.encode()?, &files, &deletions))
            .transpose()?
            .unwrap_or(0);
        let mut metadata_bytes = managed_budget::add_metadata(
            managed_budget::add_metadata(metadata_without_companions, inherited_bytes)?,
            new_bytes
                .checked_mul(2)
                .ok_or_else(|| invalid("owned companion source/destination metadata overflow"))?,
        )?;

        // Sticky source mappings remain exact. New FileIds used in companions
        // need source-owned mappings too, BEFORE the source-side receipt FK.
        let mut source_mappings = BTreeMap::new();
        let mut needed = BTreeSet::new();
        for file in &files {
            needed.insert(file.file_id.clone());
            needed.extend(file.base_file_id.iter().cloned());
        }
        for row in &deletions {
            needed.insert(row.base_file_id.clone());
        }
        if let Some(header) = &header {
            for intent in &header.intents {
                if let Some(a) = intent.accepted() {
                    needed.insert(
                        a.change
                            .expected
                            .as_ref()
                            .ok_or_else(|| invalid("owned preimage absent"))?
                            .file_id
                            .clone(),
                    );
                }
                if let CompanionIntentV2::SatisfiedDeletion {
                    source_effective_base: Some(base),
                    ..
                } = intent
                {
                    needed.insert(base.file_id.clone());
                }
            }
        }
        let mut preimages = BTreeSet::new();
        for row in &files {
            preimages.extend(row.base_file_id.iter().cloned());
        }
        for row in &deletions {
            preimages.insert(row.base_file_id.clone());
        }
        if let Some(h) = &header {
            for intent in &h.intents {
                if let Some(a) = intent.accepted() {
                    preimages.insert(
                        a.change
                            .expected
                            .as_ref()
                            .ok_or_else(|| invalid("owned accepted preimage missing"))?
                            .file_id
                            .clone(),
                    );
                }
                if let CompanionIntentV2::SatisfiedDeletion {
                    source_effective_base: Some(base),
                    ..
                } = intent
                {
                    preimages.insert(base.file_id.clone());
                }
            }
        }
        for file in needed {
            if preimages.contains(&file)
                && !source.mappings.contains_key(&file)
                && !source.original_descriptors.contains_key(&file)
            {
                return Err(invalid(
                    "owned original descriptor input absent before effects",
                ));
            }
            let mapping = source
                .mappings
                .get(&file)
                .cloned()
                .or_else(|| source.original_descriptors.get(&file).cloned())
                .or_else(|| {
                    // A freshly created recovery file may supply its own exact
                    // scalar. A source base/preimage must resolve above.
                    files
                        .iter()
                        .find(|r| r.file_id == file)
                        .map(|r| (r.manifest_id.clone(), r.bytes))
                })
                .ok_or_else(|| invalid("owned companion source mapping missing"))?;
            if !representations
                .iter()
                .any(|r| r.file_id == file && r.manifest_id == mapping.0 && r.bytes == mapping.1)
            {
                return Err(invalid(
                    "owned companion mapping outside exact protected union",
                ));
            }
            if !source.mappings.contains_key(&file) {
                metadata_bytes = managed_budget::add_metadata(
                    metadata_bytes,
                    file.as_str().len() as u64 + mapping.0.as_str().len() as u64 + 8,
                )?;
            }
            source_mappings.insert(file, mapping);
        }
        // Autonomous Ready can recover exact representations only from the
        // imported descriptors/mappings, checkout, and companion headers.
        // An observation-only alternate manifest has no durable provenance in
        // those inventories. Refuse it while this is still pure planning DATA,
        // before any source reservation or destination mutation.
        let mut reopen_union = RepresentationUnion::default();
        for (file, (manifest, bytes)) in source
            .mappings
            .iter()
            .chain(source.original_descriptors.iter())
        {
            cancellation::check(cancel)?;
            add_representation(
                &mut reopen_union,
                RepresentationData {
                    file_id: file.clone(),
                    manifest_id: manifest.clone(),
                    bytes: *bytes,
                },
            )?;
        }
        for file in &authorized_files {
            cancellation::check(cancel)?;
            add_representation(
                &mut reopen_union,
                RepresentationData {
                    file_id: file.file_id.clone(),
                    manifest_id: file.manifest_id.clone(),
                    bytes: file.bytes,
                },
            )?;
        }
        for retained in inherited
            .iter()
            .filter_map(|row| match &row.header {
                CompanionHeaderRecord::V2(header) => Some(header),
                _ => None,
            })
            .chain(header.iter())
        {
            for row in &retained.preserved_representations {
                cancellation::check(cancel)?;
                add_representation(
                    &mut reopen_union,
                    RepresentationData {
                        file_id: row.file_id.clone(),
                        manifest_id: row.manifest_id.clone(),
                        bytes: row.bytes,
                    },
                )?;
            }
        }
        if reopen_union.rows.len() != representations.len()
            || representations.iter().any(|row| {
                reopen_union
                    .rows
                    .get(&(row.file_id.clone(), row.manifest_id.clone()))
                    != Some(row)
            })
        {
            return Err(invalid(
                "owned transition union cannot be reconstructed by autonomous Ready",
            ));
        }
        let quota_bytes = managed_budget::quota(
            &manifests,
            authorized_files.iter().map(|f| f.bytes),
            metadata_bytes,
        )?;
        Ok(Self {
            source_prefix_id: source.prefix_id.clone(),
            source_selection_payload: crate::projection::selection::encode(source.input.selection)?,
            observation_generation: observation.binding().generation,
            observation_digest: observation.binding().snapshot_digest.clone(),
            observation_selection_digest: observation.binding().selection_digest.clone(),
            descriptors_digest: observation.binding().descriptors_digest.clone(),
            authority_digest: observation.binding().authority_data_digest.clone(),
            denied_paths_digest: observation.binding().denied_paths_digest.clone(),
            header,
            files,
            deletions,
            inherited,
            overlay: reconciled.overlay,
            authorized_files,
            representations,
            manifests,
            source_mappings,
            metadata_bytes,
            quota_bytes,
        })
    }

    /// Bind the immutable plan to the very observation used to classify all
    /// work. DATA equality is not an online authority check; the driver must
    /// still retain/revalidate its actual authority witness through publication.
    pub(in crate::projection::provider) fn require_plan_observation(
        &self,
        prefix: &super::super::records::import::ProviderImportPrefixRecord,
        observation: &TransitionObservationData,
    ) -> Result<()> {
        self.require_denial_plan_observation(prefix, observation)?;
        if self.source_selection_payload
            != crate::projection::selection::encode(observation.selection())?
        {
            return Err(invalid(
                "owned companion plan source/observation or fixed selection differs",
            ));
        }
        Ok(())
    }

    /// C14 may expand selection. Keep the source anchor and actual normalized
    /// destination selection distinct, without weakening the legacy fixed guard.
    pub(in crate::projection::provider) fn require_denial_plan_observation(
        &self,
        prefix: &super::super::records::import::ProviderImportPrefixRecord,
        observation: &TransitionObservationData,
    ) -> Result<()> {
        let binding = observation.binding();
        if self.source_prefix_id != prefix.digest()?
            || binding.source_prefix_id != self.source_prefix_id
            || binding.generation != self.observation_generation
            || binding.snapshot_digest != self.observation_digest
            || binding.selection_digest != self.observation_selection_digest
            || binding.descriptors_digest != self.descriptors_digest
            || binding.authority_data_digest != self.authority_digest
            || binding.denied_paths_digest != self.denied_paths_digest
        {
            return Err(invalid(
                "owned companion plan source/observation binding differs",
            ));
        }
        Ok(())
    }

    pub(in crate::projection::provider) fn source_row_bytes(&self) -> Result<u64> {
        let mut total = match &self.header {
            Some(h) => sql_bytes(&h.encode()?, &self.files, &self.deletions)?,
            None => 0,
        };
        for (file, (manifest, _)) in &self.source_mappings {
            total = managed_budget::add_metadata(
                total,
                file.as_str().len() as u64 + manifest.as_str().len() as u64 + 8,
            )?;
        }
        Ok(total)
    }
    pub(in crate::projection::provider) fn header(&self) -> Option<&CompanionHeaderV2> {
        self.header.as_ref()
    }
    pub(in crate::projection::provider) fn overlay(&self) -> &[CapturedChange] {
        &self.overlay
    }
    pub(in crate::projection::provider) fn authorized_files(&self) -> &[AuthorizedFileData] {
        &self.authorized_files
    }
    pub(in crate::projection::provider) fn manifests(&self) -> &[FileManifest] {
        &self.manifests
    }
    pub(in crate::projection::provider) fn representations(&self) -> &[RepresentationData] {
        &self.representations
    }
    pub(in crate::projection::provider) fn source_mappings(
        &self,
    ) -> &BTreeMap<FileId, (FileManifestId, u64)> {
        &self.source_mappings
    }
    pub(in crate::projection::provider) fn metadata_bytes(&self) -> u64 {
        self.metadata_bytes
    }
    pub(in crate::projection::provider) fn quota_bytes(&self) -> u64 {
        self.quota_bytes
    }
}

fn accepted_record(value: AcceptedDeletionData) -> AcceptedDeletionV2 {
    AcceptedDeletionV2 {
        source_prefix_id: value.source_prefix_id,
        source_generation: value.source_generation,
        source_snapshot_digest: value.source_snapshot_digest,
        source_capture_count: value.source_capture_count,
        source_capture_head: value.source_capture_head,
        sequence: value.sequence,
        capture_digest: value.capture_digest,
        change: value.change,
        preimage_manifest_id: value.preimage.manifest_id,
        preimage_bytes: value.preimage.bytes,
    }
}

fn require_union(
    required: &[RepresentationData],
    supplied: &[FileManifest],
    cancel: Option<&Cancellation>,
) -> Result<Vec<FileManifest>> {
    if required.len() > 100_000 || supplied.len() != required.len() {
        return Err(invalid(
            "owned companion manifest union cardinality differs",
        ));
    }
    let required: BTreeMap<_, _> = required
        .iter()
        .map(|r| ((&r.file_id, &r.manifest_id), r.bytes))
        .collect();
    let mut actual = BTreeMap::new();
    let mut bytes = 0;
    for manifest in supplied {
        cancellation::check(cancel)?;
        bytes = managed_budget::add_metadata(bytes, metadata_size(manifest, 256 * 1024)?)?;
        let key = (manifest.file_id(), manifest.id());
        if required.get(&key) != Some(&manifest.byte_length())
            || actual.insert(key, manifest).is_some()
        {
            return Err(invalid("owned companion exact manifest union differs"));
        }
    }
    // Bounds were checked before duplicating canonical manifest structures.
    Ok(actual.into_values().cloned().collect())
}

fn authorized_files(
    image: &CompleteSourceImageData<'_, '_>,
    observation: &TransitionObservationData,
    overlay: &[CapturedChange],
    cancel: Option<&Cancellation>,
) -> Result<Vec<AuthorizedFileData>> {
    let mut changes: BTreeMap<_, _> = overlay.iter().map(|c| (&c.path, c)).collect();
    let descriptors: BTreeMap<_, _> = observation
        .descriptors()
        .iter()
        .map(|d| (&d.path, d))
        .collect();
    let mut result = Vec::new();
    let mut total = 0u64;
    for entry in &observation.snapshot().entries {
        cancellation::check(cancel)?;
        if !observation.selection().includes(&entry.path) || entry.state == EntryState::Reserved {
            continue;
        }
        let replacement = changes.remove(&entry.path);
        let actual = match replacement {
            Some(change) if change.replacement.is_none() => continue,
            Some(change) => {
                let target = change
                    .replacement
                    .as_ref()
                    .ok_or_else(|| invalid("owned overlay replacement absent"))?;
                let file = image
                    .entries
                    .get(&entry.path)
                    .and_then(|e| e.file.as_ref())
                    .ok_or_else(|| invalid("owned overlay image omitted"))?;
                if file.manifest.file_id() != &target.file_id
                    || file.manifest.byte_length() != target.bytes
                    || file.executable != target.executable
                {
                    return Err(invalid("owned overlay manifest/mode differs"));
                }
                (file.manifest, target.executable)
            }
            None => match &entry.target {
                Some(TreeEntryTarget::File { file_id }) => {
                    let descriptor = descriptors
                        .get(&entry.path)
                        .ok_or_else(|| invalid("owned authorized base descriptor omitted"))?;
                    if &descriptor.file_id != file_id {
                        return Err(invalid("owned authorized base descriptor differs"));
                    }
                    (&descriptor.manifest, entry.executable)
                }
                None if entry.state == EntryState::Absent => continue,
                _ => {
                    return Err(invalid(
                        "owned authorized selected namespace is not ordinary",
                    ));
                }
            },
        };
        total = total
            .checked_add(actual.0.byte_length())
            .filter(|n| *n <= crate::projection::capture::MAX_CAPTURE_BYTES)
            .ok_or_else(|| invalid("owned authorized complete checkout exceeds image bound"))?;
        result.push(AuthorizedFileData {
            path: entry.path.clone(),
            file_id: actual.0.file_id().clone(),
            manifest_id: actual.0.id().clone(),
            bytes: actual.0.byte_length(),
            executable: actual.1,
        });
    }
    if !changes.is_empty() {
        return Err(invalid("owned overlay escaped authorized namespace"));
    }
    let paths: BTreeSet<_> = result.iter().map(|r| r.path.as_str()).collect();
    if paths.iter().any(|path| {
        path.match_indices('/')
            .any(|(at, _)| paths.contains(&path[..at]))
    }) {
        return Err(invalid("owned authorized file owns descendants"));
    }
    Ok(result)
}

/// Logical SQL byte budget, including scalar projections actually persisted.
/// Physical SQLite page/WAL overhead remains in the existing reservation engine.
pub(in crate::projection::provider) fn sql_bytes(
    header: &[u8],
    files: &[ProviderCompanionFileRecord],
    deletions: &[ProviderCompanionDeletionRecord],
) -> Result<u64> {
    let mut total = managed_budget::add_metadata(header.len() as u64, 71 + 16 + 142)?;
    for row in files {
        row.encode()?;
        let reason = canonical_json(&row.reason).map_err(|e| invalid(e.to_string()))?;
        let bytes = 71
            + row.path.as_str().len() as u64
            + 71
            + 8
            + 8
            + 71
            + row.base_file_id.as_ref().map_or(0, |_| 71)
            + row.base_executable.map_or(0, |_| 8)
            + (reason.len() as u64 - 2)
            + 71;
        total = managed_budget::add_metadata(total, bytes)?;
    }
    for row in deletions {
        row.encode()?;
        let reason = canonical_json(&row.reason).map_err(|e| invalid(e.to_string()))?;
        let bytes = 71
            + row.path.as_str().len() as u64
            + 71
            + 8
            + row.base_manifest_id.as_ref().map_or(0, |_| 71)
            + (reason.len() as u64 - 2)
            + 71;
        total = managed_budget::add_metadata(total, bytes)?;
    }
    Ok(total)
}

#[cfg(test)]
#[path = "companions/denial_tests.rs"]
pub(in crate::projection::provider) mod denial_tests;
