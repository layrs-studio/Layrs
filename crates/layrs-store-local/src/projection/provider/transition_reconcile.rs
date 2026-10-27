//! Transition reconciliation DATA only. No SQL, filesystem reads, admission,
//! authority check, content acquisition or durable companion publication.
//!
//! The caller must retain its validated source transaction, complete native
//! checkpoint, byte verification and whole-scope authority observations around
//! use of this result. In particular a cached preimage never grants access.
//! The new recovery intents require a future durable codec/receipt protocol;
//! they cannot be passed implicitly to the old bootstrap or deletion codecs.
use super::{
    cancellation,
    import_validation::VerifiedImportClosure,
    records::{
        import::{
            CompanionReason, ProviderCompanionDeletionRecord, ProviderCompanionFileRecord,
            ProviderCompanionHeaderRecord,
        },
        Record,
    },
    recovery::error::{invalid, Result},
    transition_observation::{SourceObservationData, TransitionObservationData},
};
use crate::projection::capture::{CapturedChange, CapturedPatch, CapturedTarget};
use layrs_core::{
    projection::{EntryState, ProjectionEntry, ReservedPaths},
    scm::{FileId, FileManifest, FileManifestId, RepoPath, TreeEntryTarget},
};
use layrs_vfs::Cancellation;
use std::collections::{BTreeMap, BTreeSet};

mod image;
mod source;
pub(super) use image::{CompleteSourceImageData, ImageFileData};
pub(super) use source::{InheritedCompanionData, ReconcileSourceData};

const MAX_PATHS: usize = 100_000;
const MAX_PATH_BYTES: usize = 8 * 1024 * 1024;
const MAX_METADATA: u64 = 256 * 1024 * 1024;

/// Exact representation metadata, NOT evidence that its bytes are available.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct RepresentationData {
    pub file_id: FileId,
    pub manifest_id: FileManifestId,
    pub bytes: u64,
}

/// Exact base at the source Capture, not the original remote generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileStateData {
    pub file_id: FileId,
    pub executable: bool,
}

pub(super) struct RecoveryFileIntentData {
    pub row: ProviderCompanionFileRecord,
    /// Explicitly protects the effective preimage representation too.
    pub base_representation: Option<RepresentationData>,
}

/// Deletion provenance when the effective source has already become Absent.
/// No conversion to ProviderCompanionDeletionRecord is provided.
pub(super) struct AcceptedDeletionData {
    pub source_prefix_id: String,
    pub source_generation: u64,
    pub source_snapshot_digest: String,
    pub source_capture_count: u64,
    pub source_capture_head: String,
    pub sequence: u64,
    pub capture_digest: String,
    pub change: CapturedChange,
    pub preimage: RepresentationData,
}

pub(super) enum RecoveryDeletionIntentData {
    FromEffectiveFile(ProviderCompanionDeletionRecord),
    FromAcceptedCapture {
        provenance: AcceptedDeletionData,
        reason: CompanionReason,
    },
}

/// A deletion remains intentional even if a fresh authorized observation is
/// already Absent. Persist this in the future receipt instead of inventing an
/// invalid Capture change expected=None/replacement=None or losing the intent.
pub(super) struct SatisfiedDeletionIntentData {
    pub path: RepoPath,
    pub source_effective_base: Option<FileStateData>,
    pub accepted_deletion: Option<AcceptedDeletionData>,
    pub destination_generation: u64,
    pub destination_snapshot_digest: String,
}

pub(super) struct TransitionReconcileData<'a> {
    pub source_prefix_id: String,
    pub destination_snapshot_digest: String,
    pub destination_selection_digest: String,
    /// Base-relative to the FRESH observation; sorted and never None -> None.
    pub overlay: Vec<CapturedChange>,
    pub recovery_files: Vec<RecoveryFileIntentData>,
    pub recovery_deletions: Vec<RecoveryDeletionIntentData>,
    pub satisfied_deletions: Vec<SatisfiedDeletionIntentData>,
    /// Original headers and row payloads borrowed verbatim. Not current paths.
    pub inherited: &'a [InheritedCompanionData<'a>],
    /// Complete exact representations, deduplicated by (FileId, ManifestId),
    /// including old mappings even when a live file has another representation.
    pub representations: Vec<RepresentationData>,
}

/// Deterministic DATA kernel. Neither a success nor an overlay row supplies a
/// new access capability. A later driver must acquire and protect every exact
/// representation and persist ALL intent variants before publishing Ready.
pub(super) fn reconcile<'a>(
    source: &'a ReconcileSourceData<'a>,
    image: &CompleteSourceImageData<'a>,
    observation: &TransitionObservationData,
    cancel: Option<&Cancellation>,
) -> Result<TransitionReconcileData<'a>> {
    cancellation::check(cancel)?;
    source.require_observation(observation)?;
    if image.source_prefix_id != source.prefix_id {
        return Err(invalid("reconcile image source anchor differs"));
    }
    cancellation::check(cancel)?;
    let old = source.input.snapshot;
    let fresh = observation.snapshot();
    let old_entries: BTreeMap<_, _> = old.entries.iter().map(|e| (&e.path, e)).collect();
    let fresh_entries: BTreeMap<_, _> = fresh.entries.iter().map(|e| (&e.path, e)).collect();
    // Unavailable selected FILES are readable placeholders, not protected
    // directory frontiers. A Reserved entry blocks itself, ancestors and children.
    let old_fences = fences(old, &[], cancel)?;
    let fresh_fences = fences(fresh, observation.denied_paths(), cancel)?;
    image::require_path_views(
        old.entries
            .iter()
            .map(|e| e.path.as_str())
            .chain(fresh.entries.iter().map(|e| e.path.as_str()))
            .chain(image.entries.keys().map(|p| p.as_str())),
        old.path_semantics,
    )?;
    let mut paths = BTreeSet::new();
    for path in image.entries.keys().chain(source.current_changes.keys()) {
        paths.insert(path.clone());
    }
    path_budget(paths.iter().map(|p| p.as_str()))?;
    let mut union = RepresentationUnion::default();
    for (file, (manifest, bytes)) in source.mappings {
        add_representation(
            &mut union,
            RepresentationData {
                file_id: file.clone(),
                manifest_id: manifest.clone(),
                bytes: *bytes,
            },
        )?;
    }
    for file in image.entries.values().filter_map(|e| e.file.as_ref()) {
        add_representation(&mut union, representation(file.manifest))?;
    }
    for descriptor in observation.descriptors() {
        add_representation(&mut union, representation(&descriptor.manifest))?;
    }
    let mut result = TransitionReconcileData {
        source_prefix_id: source.prefix_id.clone(),
        destination_snapshot_digest: observation.binding().snapshot_digest.clone(),
        destination_selection_digest: observation.binding().selection_digest.clone(),
        overlay: Vec::new(),
        recovery_files: Vec::new(),
        recovery_deletions: Vec::new(),
        satisfied_deletions: Vec::new(),
        inherited: source.inherited,
        representations: Vec::new(),
    };
    for path in paths {
        cancellation::check(cancel)?;
        let original_entry = old_entries.get(&path).copied();
        let original = original_entry.and_then(|e| state(e));
        let effective = source.effective(&path);
        let actual_entry = image.entries.get(&path);
        let actual_file = actual_entry.and_then(|e| e.file.as_ref());
        let actual = actual_file.map(|f| FileStateData {
            file_id: f.manifest.file_id().clone(),
            executable: f.executable,
        });
        let relocated = actual_entry.is_some_and(|e| e.relocated);
        // Includes accepted work unchanged since Capture AND deletion of a
        // captured creation whose remote generation was already Absent.
        if actual == original
            && actual == effective
            && !relocated
            && original_entry.is_some_and(|entry| {
                source.input.selection.includes(&path) && entry.state != EntryState::Reserved
            })
        {
            continue;
        }
        if actual.is_none()
            && original.is_none()
            && effective.is_none()
            && !source.current_changes.contains_key(&path)
        {
            continue;
        }
        let source_eligible = original_entry.is_some_and(|entry| {
            source.input.selection.includes(&path)
                && entry.writable
                && entry.state != EntryState::Reserved
                && (entry.state == EntryState::Absent || state(entry).is_some())
        }) && old_fences.check_path(path.as_str()).is_ok()
            && !relocated;
        let fresh_entry = fresh_entries.get(&path).copied();
        let authorized = source_eligible
            && observation.selection().includes(&path)
            && fresh_fences.check_path(path.as_str()).is_ok()
            && fresh_entry.is_some_and(|entry| {
                entry.writable && (entry.state == EntryState::Absent || state(entry).is_some())
            });
        if authorized {
            let destination_base = fresh_entry.and_then(state);
            if actual != destination_base {
                result.overlay.push(CapturedChange {
                    path: path.clone(),
                    expected: destination_base.map(target),
                    replacement: actual_file.map(|file| crate::projection::capture::CapturedFile {
                        file_id: file.manifest.file_id().clone(),
                        bytes: file.manifest.byte_length(),
                        executable: file.executable,
                    }),
                });
            } else if actual.is_none() {
                let accepted_deletion = if effective.is_none() {
                    Some(source.accepted_deletion(&path)?)
                } else {
                    None
                };
                result
                    .satisfied_deletions
                    .push(SatisfiedDeletionIntentData {
                        path: path.clone(),
                        source_effective_base: effective,
                        accepted_deletion,
                        destination_generation: observation.binding().generation,
                        destination_snapshot_digest: observation.binding().snapshot_digest.clone(),
                    });
            }
            continue;
        }
        let reason = if relocated {
            CompanionReason::RelocatedPlaceholder
        } else if original_entry.is_none() {
            CompanionReason::UnknownPath
        } else if !source.input.selection.includes(&path) {
            CompanionReason::ExcludedPath
        } else if !source_eligible {
            CompanionReason::ProtectedChange
        } else {
            CompanionReason::RevokedRights
        };
        if let Some(file) = actual_file {
            // Historical FileId-PK wins for the companion row, but BOTH exact
            // representations stay protected. Never rewrite an inherited PK.
            let current = representation(file.manifest);
            let mapped = source
                .mappings
                .get(file.manifest.file_id())
                .map(|(id, bytes)| RepresentationData {
                    file_id: current.file_id.clone(),
                    manifest_id: id.clone(),
                    bytes: *bytes,
                });
            let chosen = mapped.as_ref().unwrap_or(&current);
            let base_representation = effective
                .as_ref()
                .map(|s| source.mapping(&s.file_id))
                .transpose()?;
            let row = ProviderCompanionFileRecord {
                schema_version: "layrs.projection-provider-companion-file.v1".into(),
                path,
                file_id: chosen.file_id.clone(),
                bytes: chosen.bytes,
                executable: file.executable,
                manifest_id: chosen.manifest_id.clone(),
                base_file_id: effective.as_ref().map(|s| s.file_id.clone()),
                base_executable: effective.as_ref().map(|s| s.executable),
                reason,
            };
            row.encode()?;
            result.recovery_files.push(RecoveryFileIntentData {
                row,
                base_representation,
            });
        } else if let Some(base) = effective {
            let mapping = source.mapping(&base.file_id)?;
            let row = ProviderCompanionDeletionRecord {
                schema_version: "layrs.projection-provider-companion-deletion.v1".into(),
                path,
                base_file_id: base.file_id,
                base_executable: base.executable,
                base_manifest_id: Some(mapping.manifest_id),
                reason,
            };
            row.encode()?;
            result
                .recovery_deletions
                .push(RecoveryDeletionIntentData::FromEffectiveFile(row));
        } else {
            result
                .recovery_deletions
                .push(RecoveryDeletionIntentData::FromAcceptedCapture {
                    provenance: source.accepted_deletion(&path)?,
                    reason,
                });
        }
    }
    result.representations = union.rows.into_values().collect();
    cancellation::check(cancel)?;
    Ok(result)
}

fn state(entry: &ProjectionEntry) -> Option<FileStateData> {
    match &entry.target {
        Some(TreeEntryTarget::File { file_id }) => Some(FileStateData {
            file_id: file_id.clone(),
            executable: entry.executable,
        }),
        _ => None,
    }
}
fn target(file: FileStateData) -> CapturedTarget {
    CapturedTarget {
        file_id: file.file_id,
        executable: file.executable,
    }
}
fn representation(manifest: &FileManifest) -> RepresentationData {
    RepresentationData {
        file_id: manifest.file_id().clone(),
        manifest_id: manifest.id().clone(),
        bytes: manifest.byte_length(),
    }
}
#[derive(Default)]
struct RepresentationUnion {
    rows: BTreeMap<(FileId, FileManifestId), RepresentationData>,
    lengths: BTreeMap<FileId, u64>,
}
fn add_representation(union: &mut RepresentationUnion, value: RepresentationData) -> Result<()> {
    if union.rows.len() >= 1_000_000
        && !union
            .rows
            .contains_key(&(value.file_id.clone(), value.manifest_id.clone()))
    {
        return Err(invalid("reconcile representation union exceeds bound"));
    }
    if union
        .lengths
        .get(&value.file_id)
        .is_some_and(|length| *length != value.bytes)
    {
        return Err(invalid("reconcile FileId lengths differ"));
    }
    union.lengths.insert(value.file_id.clone(), value.bytes);
    union
        .rows
        .insert((value.file_id.clone(), value.manifest_id.clone()), value);
    Ok(())
}
fn path_budget<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut bytes = 0usize;
    for (index, path) in paths.into_iter().enumerate() {
        bytes = bytes
            .checked_add(path.len())
            .ok_or_else(|| invalid("reconcile path budget overflow"))?;
        if index >= MAX_PATHS || path.is_empty() || path.len() > 4096 || bytes > MAX_PATH_BYTES {
            return Err(invalid("reconcile path budget exceeds bound"));
        }
    }
    Ok(())
}
fn fences(
    snapshot: &layrs_core::projection::ProjectionSnapshot,
    denied: &[super::transition_observation::DeniedPathData],
    cancel: Option<&Cancellation>,
) -> Result<ReservedPaths> {
    cancellation::check(cancel)?;
    ReservedPaths::new(
        snapshot
            .entries
            .iter()
            .filter(|e| e.state == EntryState::Reserved)
            .map(|e| e.path.as_str())
            .chain(denied.iter().map(|p| p.path.as_str())),
        snapshot.path_semantics,
    )
    .map_err(|e| invalid(e.to_string()))
}

/// Counting serializer only; bounds work before canonical Value/Vec clones.
fn metadata_size(value: &impl serde::Serialize, limit: usize) -> Result<u64> {
    struct Count {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("reconcile metadata bound exceeded"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count { bytes: 0, limit };
    serde_json::to_writer(&mut count, value).map_err(|e| invalid(e.to_string()))?;
    Ok(count.bytes as u64)
}

#[cfg(test)]
mod tests;
