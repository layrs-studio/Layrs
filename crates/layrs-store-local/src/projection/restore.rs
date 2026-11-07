//! Explicit local restoration of selected captured leaves. Remote bases and
//! permissions never roll back; every overwritten byte remains recoverable.
mod engine;
pub(super) use engine::resume_with_access;
#[cfg(windows)]
pub(in crate::projection) use engine::{RestoreRecoveryData, recovery_preflight};
mod files;
use super::*;
use crate::secure_fs::PinnedDirectory;
pub(in crate::projection) use engine::LocalRestorePlan;
#[cfg_attr(not(windows), allow(unused_imports))]
pub(in crate::projection) use engine::imported_history_preview;
pub(super) use engine::verify_history;
#[cfg(windows)]
pub(in crate::projection) use engine::{local_history_preview, local_pending_preview};
use layrs_core::{
    projection::EntryState,
    scm::{RepoPath, canonical_json},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{collections::BTreeMap, io::Read};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
// Content ids are inline (71 bytes, no allocation per copy): a version
// naming several of them outweighs the other variants of this short-lived
// value.
#[allow(clippy::large_enum_variant)]
pub enum RestoreSource {
    Version {
        version: layrs_core::projection::ProjectionVersion,
        paths: Vec<RepoPath>,
    },
    Capture {
        sequence: u64,
        paths: Vec<RepoPath>,
    },
    Inverse {
        operation_id: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreDestination {
    pub identity: ProjectionIdentity,
    /// Native checkout-directory identity, not a path or content digest.
    pub checkout_identity: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestorePreview {
    pub operation_id: String,
    /// Absent only when reading historical, unbound receipts. Such previews
    /// cannot authorize an apply or a pending-journal replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<RestoreDestination>,
    pub source: RestoreSource,
    pub projection_generation: u64,
    pub projection_digest: String,
    pub checkout_digest: String,
    pub after_checkout_digest: String,
    pub before: BTreeMap<RepoPath, Option<CapturedFile>>,
    pub after: BTreeMap<RepoPath, Option<CapturedFile>>,
    pub digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreReceipt {
    pub operation_id: String,
    pub capture: CapturedPatch,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePoint {
    Staged,
    JournalDurable,
    PathQuarantined,
    PathInstalled,
    BeforeCommit,
    AfterCommit,
}
pub trait RestoreFaults {
    fn check(&self, point: RestorePoint) -> io::Result<()>;
}
pub struct NoRestoreFaults;
impl RestoreFaults for NoRestoreFaults {
    fn check(&self, _: RestorePoint) -> io::Result<()> {
        Ok(())
    }
}

impl ProjectionStore {
    pub fn preview_restore(
        &self,
        expected_generation: u64,
        source: &RestoreSource,
    ) -> Result<RestorePreview> {
        let access = format::access(self, SessionMode::Shared)?;
        access.run(|access: &mut format::Access| -> Result<_> {
            access.with_content(|c, scope| database::require_mutable_with_scope(c, self, scope))?;
            let state = database::current(&access.connection)?;
            if state.generation != expected_generation {
                return Err(invalid("restore projection changed"));
            }
            let preview =
                access.with_content(|c, scope| preview(self, c, &state, source, Some(scope)))?;
            access.verify()?;
            Ok(preview)
        })
    }
    pub fn apply_restore(&self, preview: &RestorePreview) -> Result<RestoreReceipt> {
        self.apply_restore_with_faults(preview, &NoRestoreFaults)
    }
    pub fn apply_restore_with_faults(
        &self,
        preview: &RestorePreview,
        faults: &dyn RestoreFaults,
    ) -> Result<RestoreReceipt> {
        engine::apply(self, preview, faults)
    }
    pub fn resume_restore(root: impl AsRef<Path>) -> Result<RestoreReceipt> {
        let root = std::path::absolute(root)?;
        let marker = format::read_marker(&root)?;
        engine::resume(&Self { root, marker })
    }
}
fn digest<T: Serialize>(domain: &[u8], value: &T) -> Result<String> {
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(&canonical_json(value).map_err(invalid)?);
    Ok(format!("blake3:{}", h.finalize()))
}
fn preview_digest(preview: &RestorePreview) -> Result<String> {
    if preview.operation_id.len() > 96
        || preview.operation_id.is_empty()
        || !preview
            .operation_id
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-')
    {
        return Err(invalid("invalid restore operation identifier"));
    }
    let mut copy = preview.clone();
    copy.digest.clear();
    digest(b"layrs-projection-restore-preview-v1\0", &copy)
}
fn destination(store: &ProjectionStore) -> RestoreDestination {
    RestoreDestination {
        identity: store.identity().clone(),
        checkout_identity: store.marker.checkout_identity().to_owned(),
    }
}
fn validate_destination(store: &ProjectionStore, preview: &RestorePreview) -> Result<()> {
    if preview.destination.as_ref() != Some(&destination(store)) {
        return Err(invalid("restore preview destination differs or is unbound"));
    }
    Ok(())
}
fn checkout_digest(files: &BTreeMap<String, Option<CapturedFile>>) -> Result<String> {
    digest(b"layrs-projection-restore-checkout-v1\0", files)
}
fn validate_paths(
    snapshot: &ProjectionSnapshot,
    paths: &BTreeMap<RepoPath, Option<CapturedFile>>,
) -> Result<()> {
    if paths.is_empty() || paths.len() > 1024 {
        return Err(invalid("restore requires1..1024 paths"));
    }
    let fences = snapshot.reserved_paths()?;
    let mut total = 0u64;
    for (path, file) in paths {
        fences.check_path(path.as_str())?;
        let entry = snapshot
            .entries
            .binary_search_by(|e| e.path.cmp(path))
            .ok()
            .map(|n| &snapshot.entries[n])
            .ok_or_else(|| invalid("restore path is not currently observed"))?;
        if !entry.writable
            || !matches!(entry.state, EntryState::Materialized | EntryState::Absent)
            || !matches!(
                &entry.target,
                None | Some(layrs_core::scm::TreeEntryTarget::File { .. })
            )
        {
            return Err(invalid(
                "restore requires currently writable regular-file leaves",
            ));
        }
        if let Some(file) = file {
            if file.bytes > MAX_CAPTURE_FILE_BYTES {
                return Err(invalid("restore file exceeds1GiB"));
            }
            total = total
                .checked_add(file.bytes)
                .filter(|n| *n <= MAX_CAPTURE_BYTES)
                .ok_or_else(|| invalid("restore bytes exceed4GiB"))?;
            if cfg!(windows) && file.executable {
                return Err(invalid("Windows cannot restore executable mode"));
            }
        }
    }
    Ok(())
}
fn targets(
    c: &Connection,
    store: &ProjectionStore,
    source: &RestoreSource,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<BTreeMap<RepoPath, Option<CapturedFile>>> {
    // C14: only the local post-Ready suffix may be sourced. The immutable Ready
    // capture, imported captures/history and remote versions stay closed.
    let denial = scope.and_then(|scope| scope.denial_ready());
    if let Some(ready) = &denial {
        require_local_source(c, store, source, ready, scope)?;
    }
    match source {
        RestoreSource::Version { version, paths } => {
            offline::restore_targets_with_scope(c, store, version, paths, scope)
        }
        RestoreSource::Capture { sequence, paths } => {
            if paths.is_empty() || paths.len() > 1024 {
                return Err(invalid("restore requires1..1024 paths"));
            }
            let patch = capture::storage::load_with_scope(c, store, *sequence, scope)?;
            let mut result = BTreeMap::new();
            for path in paths {
                let change = patch
                    .changes
                    .iter()
                    .find(|change| &change.path == path)
                    .ok_or_else(|| invalid("selected path is absent from capture"))?;
                if result
                    .insert(path.clone(), change.replacement.clone())
                    .is_some()
                {
                    return Err(invalid("duplicate restore path"));
                }
            }
            Ok(result)
        }
        RestoreSource::Inverse { operation_id } => {
            let journal = engine::history_with_scope(c, store, operation_id, scope)?
                .ok_or_else(|| invalid("restore operation unknown"))?;
            Ok(journal.0.preview.before)
        }
    }
}
fn require_local_source(
    c: &Connection,
    store: &ProjectionStore,
    source: &RestoreSource,
    ready: &format::DenialReady,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<()> {
    let sequence = match source {
        // The fixed, fully retained Ready image (discarding local work) or a
        // post-Ready local Capture addressed as a Version (product offline route,
        // whose digest the version resolver verifies).
        RestoreSource::Version { version, .. } => {
            use layrs_core::projection::ProjectionVersion;
            let own = &store.identity().worktree_id;
            let local = match version {
                ProjectionVersion::Generation {
                    worktree_id,
                    generation,
                    digest,
                } => {
                    worktree_id == own && *generation == ready.generation && *digest == ready.digest
                }
                ProjectionVersion::Capture {
                    worktree_id,
                    sequence,
                    ..
                } => worktree_id == own && *sequence > ready.capture_sequence,
                ProjectionVersion::Remote { .. } => false,
            };
            if !local {
                return Err(invalid(
                    "C14 local Restore accepts only the fixed Ready version, post-Ready Capture or local inverse sources",
                ));
            }
            return Ok(());
        }
        RestoreSource::Capture { sequence, .. } => *sequence,
        RestoreSource::Inverse { operation_id } => {
            engine::history_with_scope(c, store, operation_id, scope)?
                .ok_or_else(|| invalid("restore operation unknown"))?
                .1
                .sequence
        }
    };
    if sequence <= ready.capture_sequence {
        return Err(invalid(
            "C14 local Restore source precedes fixed Ready capture frontier",
        ));
    }
    Ok(())
}

/// Full-checkout diff a restore's forced capture appends: snapshot order, only
/// scanned leaves, equal targets skipped (the scan.rs change derivation).
fn resulting_changes(
    snapshot: &ProjectionSnapshot,
    resulting: &BTreeMap<String, Option<CapturedFile>>,
) -> Vec<CapturedChange> {
    let mut changes = Vec::new();
    for entry in &snapshot.entries {
        let Some(actual) = resulting.get(entry.path.as_str()) else {
            continue;
        };
        let expected = entry.target.as_ref().and_then(|t| match t {
            layrs_core::scm::TreeEntryTarget::File { file_id } => Some((file_id, entry.executable)),
            _ => None,
        });
        if actual.as_ref().map(|f| (&f.file_id, f.executable)) != expected {
            changes.push(CapturedChange {
                path: entry.path.clone(),
                expected: expected.map(|(id, executable)| capture::CapturedTarget {
                    file_id: id.clone(),
                    executable,
                }),
                replacement: actual.clone(),
            });
        }
    }
    changes
}

fn preview(
    store: &ProjectionStore,
    c: &Connection,
    state: &VersionedProjection,
    source: &RestoreSource,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<RestorePreview> {
    preview_with_changes(store, c, state, source, scope).map(|(preview, _)| preview)
}

fn preview_with_changes(
    store: &ProjectionStore,
    c: &Connection,
    state: &VersionedProjection,
    source: &RestoreSource,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<(RestorePreview, Vec<CapturedChange>)> {
    let after = targets(c, store, source, scope)?;
    validate_paths(&state.snapshot, &after)?;
    let scan = capture::scan::scan_with_scope(store, c, &state.snapshot, false, scope)?;
    let mut resulting = scan.files.clone();
    let mut before = BTreeMap::new();
    for (path, target) in &after {
        let old = scan
            .files
            .get(path.as_str())
            .ok_or_else(|| invalid("restore path is not a regular leaf"))?
            .clone();
        before.insert(path.clone(), old);
        resulting.insert(path.to_string(), target.clone());
        if let Some(file) = target {
            capture::storage::file_manifest_with_scope(c, store, file, scope)?;
        }
    }
    validate_paths(&state.snapshot, &before)?;
    if before == after {
        return Err(invalid("selected capture already matches checkout"));
    }
    // Validate the eventual patch budget before any filesystem mutation.
    let changes = resulting_changes(&state.snapshot, &resulting);
    let mut total = 0u64;
    for change in &changes {
        total = total
            .checked_add(change.replacement.as_ref().map_or(0, |f| f.bytes))
            .ok_or_else(|| invalid("restore size overflow"))?;
    }
    if changes.len() > 1024 || total > MAX_CAPTURE_BYTES {
        return Err(invalid("restore would exceed capture budget"));
    }
    let mut value = RestorePreview {
        destination: Some(destination(store)),
        operation_id: format!(
            "{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(invalid)?
                .as_nanos(),
            std::process::id()
        ),
        source: source.clone(),
        projection_generation: state.generation,
        projection_digest: state.digest.clone(),
        checkout_digest: checkout_digest(&scan.files)?,
        after_checkout_digest: checkout_digest(&resulting)?,
        before,
        after,
        digest: String::new(),
    };
    value.digest = preview_digest(&value)?;
    Ok((value, changes))
}
