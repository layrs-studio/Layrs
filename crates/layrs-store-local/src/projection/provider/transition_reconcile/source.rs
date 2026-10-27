use super::super::records::import::{
    ImportSource, ImportVersion, ImportedSegment, ProviderTransitionReceiptRecord,
};
use super::*;

/// Historical recovery artifacts are NOT members of the current live image.
/// Their own source version/preimages remain validated by the retained import
/// proof; this kernel checks exact rowsets and passes their bytes through.
pub(in crate::projection::provider) struct InheritedCompanionData<'a> {
    pub header: &'a ProviderCompanionHeaderRecord,
    pub files: &'a [ProviderCompanionFileRecord],
    pub deletions: &'a [ProviderCompanionDeletionRecord],
}

pub(in crate::projection::provider) struct ReconcileSourceData<'a> {
    pub(super) input: SourceObservationData<'a>,
    pub(super) prefix_id: String,
    pub(super) mappings: &'a BTreeMap<FileId, (FileManifestId, u64)>,
    pub(super) inherited: &'a [InheritedCompanionData<'a>],
    pub(super) current_changes: BTreeMap<RepoPath, &'a CapturedChange>,
    latest: Option<&'a CapturedPatch>,
    // Retain the WHOLE proof rather than extracting a digest as permission.
    // No SQL operation or connection revalidation is performed by this module.
    _closure: Option<&'a VerifiedImportClosure>,
}
impl<'a> ReconcileSourceData<'a> {
    pub(in crate::projection::provider) fn normalize(
        input: SourceObservationData<'a>,
        closure: &'a VerifiedImportClosure,
        captures: &'a [CapturedPatch],
        mappings: &'a BTreeMap<FileId, (FileManifestId, u64)>,
        incoming_receipts: &[ProviderTransitionReceiptRecord],
        inherited: &'a [InheritedCompanionData<'a>],
        observation: &TransitionObservationData,
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        let prefix_id = input.prefix.digest()?;
        if closure.prefix_id() != prefix_id || closure.closure() != &input.prefix.closure {
            return Err(invalid("reconcile source import proof differs"));
        }
        // This is the exact SQL rowset encoding, shared with the SQL digester,
        // not a second home-made commitment over a preferred representation.
        if super::super::import_validation::content_mapping_rowset_data(mappings, cancel)?
            != input.prefix.closure.content_mappings
        {
            return Err(invalid("reconcile source mapping scope differs"));
        }
        let source_version = version(&input);
        preflight_captures(captures, cancel)?;
        if captures.len() as u64 != source_version.capture_count {
            return Err(invalid("reconcile capture prefix count differs"));
        }
        let mut previous = None;
        for (index, patch) in captures.iter().enumerate() {
            cancellation::check(cancel)?;
            let (origin, first, last) =
                closure.capture_origin(patch.sequence, source_version.generation)?;
            if patch.sequence != index as u64 + 1
                || patch.schema_version != "layrs.projection-capture.v1"
                || patch.previous_patch_digest.as_ref() != previous
                || &patch.identity != origin
                || patch.projection_generation < first
                || patch.projection_generation > last
                || patch.computed_digest()? != patch.digest
            {
                return Err(invalid("reconcile capture prefix provenance differs"));
            }
            super::super::records::codec::raw_digest(&patch.projection_digest)?;
            previous = Some(&patch.digest);
        }
        if previous.map(String::as_str) != source_version.capture_head.as_deref() {
            return Err(invalid("reconcile capture prefix head differs"));
        }
        validate_inherited(inherited, incoming_receipts, input.prefix, cancel)?;
        let latest = captures
            .last()
            .filter(|patch| patch.projection_generation == source_version.generation);
        let current_changes = latest
            .map(|patch| {
                patch
                    .changes
                    .iter()
                    .map(|change| (change.path.clone(), change))
                    .collect()
            })
            .unwrap_or_default();
        let result = Self {
            input,
            prefix_id,
            mappings,
            inherited,
            current_changes,
            latest,
            _closure: Some(closure),
        };
        result.require_observation(observation)?;
        // At the current source generation a capture is a FULL diff against
        // that exact generation, not a cumulative patch on the previous one.
        if let Some(patch) = latest {
            if patch.projection_digest != version(&result.input).snapshot_digest {
                return Err(invalid("reconcile current capture snapshot differs"));
            }
            for change in &patch.changes {
                let base = result.original(&change.path);
                if base.map(target) != change.expected {
                    return Err(invalid("reconcile current capture preimage differs"));
                }
                if let Some(file) = &change.replacement {
                    if result.mapping(&file.file_id)?.bytes != file.bytes {
                        return Err(invalid("reconcile capture replacement mapping differs"));
                    }
                }
            }
        }
        Ok(result)
    }

    pub(super) fn require_observation(&self, observed: &TransitionObservationData) -> Result<()> {
        let binding = observed.binding();
        let version = version(&self.input);
        if binding.source_prefix_id != self.prefix_id
            || binding.source_identity != *self.input.identity
            || binding.source_generation != version.generation
            || binding.source_snapshot_digest != version.snapshot_digest
            || binding.source_selection_digest != version.selection_digest
            || binding.source_capture_count != version.capture_count
            || binding.source_capture_head != version.capture_head
            || binding.destination_identity != self.input.prefix.destination_identity
        {
            return Err(invalid("reconcile transition observation binding differs"));
        }
        // Do not trust another same-prefix caller's substituted snapshot or
        // selection. Recompute these bounded current DATA commitments.
        let bytes = super::super::records::codec::encode(
            self.input.snapshot,
            super::super::records::codec::SNAPSHOT_MAX,
        )?;
        let selection = crate::projection::selection::encode(self.input.selection)?;
        if crate::projection::database::digest(
            version.generation,
            self.input.snapshot_parent_digest,
            &bytes,
        ) != version.snapshot_digest
            || crate::projection::database::digest(
                self.input.selection_generation,
                Some(self.input.selection_snapshot_digest),
                &selection,
            ) != version.selection_digest
        {
            return Err(invalid("reconcile source snapshot/selection differs"));
        }
        Ok(())
    }
    fn original(&self, path: &RepoPath) -> Option<FileStateData> {
        self.input
            .snapshot
            .entries
            .binary_search_by(|e| e.path.cmp(path))
            .ok()
            .and_then(|index| state(&self.input.snapshot.entries[index]))
    }
    pub(super) fn effective(&self, path: &RepoPath) -> Option<FileStateData> {
        match self.current_changes.get(path) {
            Some(change) => change.replacement.as_ref().map(|file| FileStateData {
                file_id: file.file_id.clone(),
                executable: file.executable,
            }),
            None => self.original(path),
        }
    }
    pub(super) fn mapping(&self, file: &FileId) -> Result<RepresentationData> {
        self.mappings
            .get(file)
            .map(|(manifest, bytes)| RepresentationData {
                file_id: file.clone(),
                manifest_id: manifest.clone(),
                bytes: *bytes,
            })
            .ok_or_else(|| invalid("reconcile exact preimage mapping is missing"))
    }
    pub(super) fn accepted_deletion(&self, path: &RepoPath) -> Result<AcceptedDeletionData> {
        let patch = self
            .latest
            .ok_or_else(|| invalid("reconcile deletion has no accepted provenance"))?;
        let change = self
            .current_changes
            .get(path)
            .filter(|change| {
                change.path == *path && change.replacement.is_none() && change.expected.is_some()
            })
            .ok_or_else(|| invalid("reconcile deletion capture path/preimage differs"))?;
        let expected = change.expected.as_ref().unwrap();
        let version = version(&self.input);
        if patch.sequence == 0
            || patch.sequence != version.capture_count
            || Some(patch.digest.as_str()) != version.capture_head.as_deref()
            || patch.projection_generation != version.generation
            || patch.projection_digest != version.snapshot_digest
            || self.original(path).map(target) != change.expected
        {
            return Err(invalid("reconcile deletion capture escaped source prefix"));
        }
        Ok(AcceptedDeletionData {
            source_prefix_id: self.prefix_id.clone(),
            source_generation: version.generation,
            source_snapshot_digest: version.snapshot_digest.clone(),
            source_capture_count: version.capture_count,
            source_capture_head: version.capture_head.clone().unwrap(),
            sequence: patch.sequence,
            capture_digest: patch.digest.clone(),
            change: (*change).clone(),
            preimage: self.mapping(&expected.file_id)?,
        })
    }
}

pub(super) fn version<'a>(input: &'a SourceObservationData<'_>) -> &'a ImportVersion {
    match &input.prefix.source {
        ImportSource::Ordinary { version, .. } | ImportSource::Provider { version, .. } => version,
    }
}

/// Source-defined DATA fixture only. It deliberately supplies no production
/// constructor bypass for a validated SQL closure or a retained checkpoint.
#[cfg(test)]
pub(super) fn from_test_data<'a>(
    input: SourceObservationData<'a>,
    captures: &'a [CapturedPatch],
    mappings: &'a BTreeMap<FileId, (FileManifestId, u64)>,
    inherited: &'a [InheritedCompanionData<'a>],
    observed: &TransitionObservationData,
) -> Result<ReconcileSourceData<'a>> {
    preflight_captures(captures, None)?;
    let latest = captures
        .last()
        .filter(|patch| patch.projection_generation == version(&input).generation);
    let current_changes = latest
        .map(|patch| {
            patch
                .changes
                .iter()
                .map(|change| (change.path.clone(), change))
                .collect()
        })
        .unwrap_or_default();
    let result = ReconcileSourceData {
        prefix_id: input.prefix.digest()?,
        input,
        mappings,
        inherited,
        current_changes,
        latest,
        _closure: None,
    };
    result.require_observation(observed)?;
    Ok(result)
}

fn preflight_captures(captures: &[CapturedPatch], cancel: Option<&Cancellation>) -> Result<()> {
    if captures.len() > 4096 {
        return Err(invalid("reconcile capture count exceeds bound"));
    }
    let mut metadata = 0u64;
    for patch in captures {
        cancellation::check(cancel)?;
        if patch.changes.len() > 1024 {
            return Err(invalid("reconcile capture changes exceed bound"));
        }
        path_budget(patch.changes.iter().map(|c| c.path.as_str()))?;
        let mut previous = None;
        for change in &patch.changes {
            cancellation::check(cancel)?;
            if previous.is_some_and(|p: &RepoPath| p >= &change.path)
                || change.expected.is_none() && change.replacement.is_none()
                || change
                    .replacement
                    .as_ref()
                    .is_some_and(|f| f.bytes > crate::projection::capture::MAX_CAPTURE_FILE_BYTES)
            {
                return Err(invalid("reconcile capture change order or values differ"));
            }
            previous = Some(&change.path);
        }
        // Streaming counting serializer precedes computed_digest's clone.
        let bytes = metadata_size(patch, 8 * 1024 * 1024)?;
        metadata = metadata
            .checked_add(bytes)
            .filter(|n| *n <= 128 * 1024 * 1024)
            .ok_or_else(|| invalid("reconcile capture aggregate exceeds bound"))?;
    }
    Ok(())
}

fn validate_inherited(
    inherited: &[InheritedCompanionData<'_>],
    receipts: &[ProviderTransitionReceiptRecord],
    prefix: &super::super::records::import::ProviderImportPrefixRecord,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    if inherited.len() > 4096 || inherited.len() as u64 != prefix.closure.companions.count {
        return Err(invalid("reconcile inherited companion count differs"));
    }
    if receipts.len() > 4096 {
        return Err(invalid("reconcile incoming receipt count exceeds bound"));
    }
    let mut segments = Vec::with_capacity(receipts.len());
    let mut expected = BTreeMap::new();
    let mut previous = None;
    for receipt in receipts {
        cancellation::check(cancel)?;
        // Bounded canonical codec precedes ancestry allocations. The engaged
        // root/segment commitment closes the set, not a supplied Vec length.
        receipt.encode()?;
        segments.push(ImportedSegment::from_receipt(receipt, previous)?);
        if let Some(id) = &receipt.companion_id {
            if expected.insert(id.as_str(), receipt).is_some() {
                return Err(invalid(
                    "reconcile inherited companion has multiple incoming receipts",
                ));
            }
        }
        previous = Some(receipt);
    }
    prefix.validate_segments(&segments)?;
    if expected.len() != inherited.len() {
        return Err(invalid("reconcile inherited companion membership differs"));
    }
    let mut ids = BTreeSet::new();
    let mut files = 0u64;
    let mut deletions = 0u64;
    let mut metadata = 0u64;
    for companion in inherited {
        cancellation::check(cancel)?;
        path_budget(
            companion
                .files
                .iter()
                .map(|f| f.path.as_str())
                .chain(companion.deletions.iter().map(|d| d.path.as_str())),
        )?;
        for row in companion.files {
            let bytes = metadata_size(row, 8192)?;
            metadata = metadata
                .checked_add(bytes)
                .filter(|n| *n <= MAX_METADATA)
                .ok_or_else(|| invalid("reconcile inherited metadata exceeds bound"))?;
        }
        for row in companion.deletions {
            let bytes = metadata_size(row, 8192)?;
            metadata = metadata
                .checked_add(bytes)
                .filter(|n| *n <= MAX_METADATA)
                .ok_or_else(|| invalid("reconcile inherited metadata exceeds bound"))?;
        }
        companion
            .header
            .validate_rows(companion.files, companion.deletions)?;
        let id = companion.header.digest()?;
        let receipt = expected
            .get(id.as_str())
            .ok_or_else(|| invalid("reconcile inherited companion is not engaged"))?;
        receipt.require_companion(companion.header)?;
        if !ids.insert(id) {
            return Err(invalid("reconcile inherited companion repeats"));
        }
        files = files
            .checked_add(companion.files.len() as u64)
            .ok_or_else(|| invalid("reconcile inherited count overflow"))?;
        deletions = deletions
            .checked_add(companion.deletions.len() as u64)
            .ok_or_else(|| invalid("reconcile inherited count overflow"))?;
    }
    if files != prefix.closure.companion_files.count
        || deletions != prefix.closure.companion_deletions.count
    {
        return Err(invalid("reconcile inherited companion row counts differ"));
    }
    // Historical bases and global normalized SQL rowset digests remain under
    // the borrowed full import proof. Header/row substitution is independently
    // closed here by exact rows -> header -> incoming receipt -> prefix root.
    Ok(())
}
