use super::*;
use layrs_vfs::{CaptureEvidence, ExportCheckpoint};

mod binding;

/// Hash/mode DATA obtained by the external retained-byte adapter. A supplied
/// manifest does not prove bytes; the caller must perform exact hashing and
/// mode observation under the retained checkpoint before and after this use.
pub(in crate::projection::provider) struct ImageFileData<'a> {
    pub path: &'a RepoPath,
    pub manifest: &'a FileManifest,
    /// Unknown mode refuses normalization, including for recovery files.
    pub executable: Option<bool>,
}
pub(super) struct PresentFileData<'a> {
    pub manifest: &'a FileManifest,
    pub executable: bool,
}
pub(super) struct ImageEntryData<'a> {
    pub file: Option<PresentFileData<'a>>,
    pub relocated: bool,
}

/// Closed metadata inventory tied to the ENTIRE borrowed native export.
/// There is no production constructor taking a caller-selected Vec of paths.
/// Native namespace stability, byte hashing and quiescence remain external.
pub(in crate::projection::provider) struct CompleteSourceImageData<'a, 'c> {
    pub(super) source_prefix_id: String,
    pub(super) entries: BTreeMap<RepoPath, ImageEntryData<'a>>,
    _checkpoint: Option<&'a ExportCheckpoint<'c>>,
}
impl<'a, 'c> CompleteSourceImageData<'a, 'c> {
    pub(in crate::projection::provider) fn from_checkpoint(
        source: &ReconcileSourceData<'_>,
        checkpoint: &'a ExportCheckpoint<'c>,
        files: &[ImageFileData<'a>],
        cancel: Option<&Cancellation>,
    ) -> Result<Self> {
        cancellation::check(cancel)?;
        let root = checkpoint
            .root_guard()
            .ok_or_else(|| invalid("reconcile checkpoint lacks provider owner"))?;
        let control_hex: String = root
            .control_directory()
            .identity()
            .encode()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        binding::require_owner(
            &source.input.prefix.source,
            &control_hex,
            &root.filesystem_identity(),
            root.control_directory().format_bytes(),
        )?;
        path_budget(checkpoint.entries().iter().map(|e| e.path.as_str()))?;
        preflight_files(files, cancel)?;
        let manifests = files
            .iter()
            .map(|f| (f.path, f))
            .collect::<BTreeMap<_, _>>();
        if manifests.len() != files.len() {
            return Err(invalid("reconcile image path repeats"));
        }
        let mut entries = BTreeMap::new();
        let mut present = 0usize;
        let mut previous = None;
        let mut deleted_bases = BTreeMap::new();
        for entry in checkpoint.entries() {
            cancellation::check(cancel)?;
            if previous.is_some_and(|p: &RepoPath| p >= &entry.path) {
                return Err(invalid("reconcile complete inventory order differs"));
            }
            previous = Some(&entry.path);
            let file = manifests.get(&entry.path);
            let original = checkpoint.original_path(&entry.path)?;
            let relocated = original.as_ref().is_some_and(|path| path != &entry.path);
            let actual = match &entry.evidence {
                // Exports and reconciliation never admit a file from its listing.
                CaptureEvidence::Vouched { .. } => {
                    return Err(invalid("export evidence must be inspected, not listed"));
                }
                CaptureEvidence::Immutable {
                    file_id,
                    size,
                    original_path,
                } => {
                    let file = file.ok_or_else(|| invalid("reconcile image manifest omitted"))?;
                    if file.manifest.file_id() != file_id
                        || file.manifest.byte_length() != *size
                        || original.as_ref() != Some(original_path)
                    {
                        return Err(invalid("reconcile immutable inventory evidence differs"));
                    }
                    Some(present_file(file)?)
                }
                CaptureEvidence::NeedsHash { size, .. } => {
                    let file = file.ok_or_else(|| invalid("reconcile image manifest omitted"))?;
                    if file.manifest.byte_length() != *size {
                        return Err(invalid("reconcile hashed inventory size differs"));
                    }
                    Some(present_file(file)?)
                }
                CaptureEvidence::Deleted { base } => {
                    if file.is_some() || original.is_some() {
                        return Err(invalid("reconcile deleted inventory has file DATA"));
                    }
                    let old = source
                        .input
                        .snapshot
                        .entries
                        .binary_search_by(|e| e.path.cmp(&entry.path))
                        .ok()
                        .and_then(|index| state(&source.input.snapshot.entries[index]));
                    if old.as_ref().map(|file| &file.file_id) != Some(base) {
                        return Err(invalid("reconcile deleted inventory catalog base differs"));
                    }
                    deleted_bases.insert(entry.path.clone(), base);
                    None
                }
            };
            present += usize::from(actual.is_some());
            entries.insert(
                entry.path.clone(),
                ImageEntryData {
                    file: actual,
                    relocated,
                },
            );
        }
        if present != manifests.len() {
            return Err(invalid(
                "reconcile image has extra manifest outside complete inventory",
            ));
        }
        // Commit the ENTIRE observed native inventory before adding synthetic
        // absences for captured creations missing from the fixed catalogue.
        // A matching prefix label alone does not bind a later checkpoint.
        binding::require_image(
            &source::version(&source.input).effective_image_digest,
            &entries,
            &deleted_bases,
            cancel,
        )?;
        validate_inventory(source, &mut entries, cancel)?;
        Ok(Self {
            source_prefix_id: source.prefix_id.clone(),
            entries,
            _checkpoint: Some(checkpoint),
        })
    }
}

fn present_file<'a>(file: &ImageFileData<'a>) -> Result<PresentFileData<'a>> {
    Ok(PresentFileData {
        manifest: file.manifest,
        executable: file
            .executable
            .ok_or_else(|| invalid("reconcile native file mode is unknown"))?,
    })
}
fn preflight_files(files: &[ImageFileData<'_>], cancel: Option<&Cancellation>) -> Result<()> {
    path_budget(files.iter().map(|f| f.path.as_str()))?;
    let mut total = 0u64;
    let mut file_bytes = 0u64;
    for file in files {
        cancellation::check(cancel)?;
        if file.manifest.byte_length() > crate::projection::capture::MAX_CAPTURE_FILE_BYTES {
            return Err(invalid("reconcile image file size exceeds bound"));
        }
        file_bytes = file_bytes
            .checked_add(file.manifest.byte_length())
            .filter(|n| *n <= crate::projection::capture::MAX_CAPTURE_BYTES)
            .ok_or_else(|| invalid("reconcile image total file bytes exceed bound"))?;
        // Streaming counting preflight before any canonical allocations.
        let bytes = metadata_size(file.manifest, 256 * 1024)?;
        total = total
            .checked_add(bytes)
            .filter(|n| *n <= MAX_METADATA)
            .ok_or_else(|| invalid("reconcile image manifest aggregate exceeds bound"))?;
        present_file(file)?;
    }
    Ok(())
}

fn validate_inventory<'a>(
    source: &ReconcileSourceData<'_>,
    entries: &mut BTreeMap<RepoPath, ImageEntryData<'a>>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    let snapshot = source.input.snapshot;
    require_path_views(
        snapshot
            .entries
            .iter()
            .map(|e| e.path.as_str())
            .chain(entries.keys().map(|p| p.as_str())),
        snapshot.path_semantics,
    )?;
    // Every registered selected source file is explicitly present/deleted.
    // A missing arbitrary catalog row is never treated as a local deletion.
    for entry in &snapshot.entries {
        cancellation::check(cancel)?;
        if source.input.selection.includes(&entry.path)
            && entry.state != EntryState::Reserved
            && matches!(entry.target, Some(TreeEntryTarget::File { .. }))
            && !entries.contains_key(&entry.path)
        {
            return Err(invalid(
                "reconcile complete inventory omitted fixed catalog path",
            ));
        }
    }
    // A local captured creation was not in that immutable catalog. Its later
    // absence is nevertheless observable in the WHOLE retained inventory.
    for (path, change) in &source.current_changes {
        if !entries.contains_key(path) {
            let generation = snapshot
                .entries
                .binary_search_by(|e| e.path.cmp(path))
                .ok()
                .map(|index| &snapshot.entries[index]);
            if generation
                .is_some_and(|entry| matches!(entry.target, Some(TreeEntryTarget::File { .. })))
            {
                return Err(invalid(
                    "reconcile captured catalog path disappeared from inventory",
                ));
            }
            if change.expected.is_some() || change.replacement.is_some() {
                entries.insert(
                    path.clone(),
                    ImageEntryData {
                        file: None,
                        relocated: false,
                    },
                );
            }
        }
    }
    path_budget(entries.keys().map(|p| p.as_str()))?;
    let present: BTreeSet<_> = entries
        .iter()
        .filter(|(_, e)| e.file.is_some())
        .map(|(path, _)| path.as_str())
        .collect();
    for path in &present {
        if path
            .match_indices('/')
            .any(|(at, _)| present.contains(&path[..at]))
        {
            return Err(invalid("reconcile image file owns descendants"));
        }
    }
    Ok(())
}

/// Cross-view alias checking, including directory spellings. Bounded before
/// prefix-index allocation. Identical exact paths across views are permitted.
pub(super) fn require_path_views<'a>(
    paths: impl Iterator<Item = &'a str> + Clone,
    semantics: layrs_core::projection::PathSemantics,
) -> Result<()> {
    let mut upper = 0usize;
    for (index, path) in paths.clone().enumerate() {
        if index >= 3 * MAX_PATHS || path.len() > 4096 || path.split('/').count() > 128 {
            return Err(invalid("reconcile cross-view path bound differs"));
        }
        upper = upper
            .checked_add((path.len() * 6 + 128) * path.split('/').count())
            .filter(|n| *n <= MAX_METADATA as usize)
            .ok_or_else(|| invalid("reconcile path index exceeds bound"))?;
    }
    let mut prefixes = BTreeMap::new();
    for path in paths {
        ReservedPaths::new([path], semantics).map_err(|e| invalid(e.to_string()))?;
        for (at, _) in path
            .match_indices('/')
            .chain(std::iter::once((path.len(), "/")))
        {
            let spelling = &path[..at];
            let key = match semantics {
                layrs_core::projection::PathSemantics::CaseSensitive => spelling.to_owned(),
                layrs_core::projection::PathSemantics::PortableWindows => spelling.to_uppercase(),
            };
            if prefixes
                .insert(key, spelling)
                .is_some_and(|old| old != spelling)
            {
                return Err(invalid("reconcile cross-view paths alias"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn from_test_inventory<'a>(
    source: &ReconcileSourceData<'_>,
    files: &[ImageFileData<'a>],
    deleted: &[RepoPath],
) -> Result<CompleteSourceImageData<'a, 'a>> {
    preflight_files(files, None)?;
    path_budget(deleted.iter().map(|p| p.as_str()))?;
    let mut entries = BTreeMap::new();
    for file in files {
        if entries
            .insert(
                file.path.clone(),
                ImageEntryData {
                    file: Some(present_file(file)?),
                    relocated: false,
                },
            )
            .is_some()
        {
            return Err(invalid("reconcile test inventory repeats path"));
        }
    }
    for path in deleted {
        if entries
            .insert(
                path.clone(),
                ImageEntryData {
                    file: None,
                    relocated: false,
                },
            )
            .is_some()
        {
            return Err(invalid("reconcile test inventory repeats path"));
        }
    }
    validate_inventory(source, &mut entries, None)?;
    Ok(CompleteSourceImageData {
        source_prefix_id: source.prefix_id.clone(),
        entries,
        _checkpoint: None,
    })
}
