//! Explicit local restoration of selected captured leaves. Remote bases and
//! permissions never roll back; every overwritten byte remains recoverable.
mod engine;
mod files;
use super::*;
use crate::secure_fs::PinnedDirectory;
pub(super) use engine::verify_history;
use layrs_core::{
    projection::EntryState,
    scm::{FileId, RepoPath, canonical_json},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RestoreSource {
    Capture { sequence: u64, paths: Vec<RepoPath> },
    Inverse { operation_id: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestorePreview {
    pub operation_id: String,
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
        database::require_mutable(&access.connection)?;
        let state = database::current(&access.connection)?;
        if state.generation != expected_generation {
            return Err(invalid("restore projection changed"));
        }
        let preview = preview(self, &access.connection, &state, source)?;
        access.verify()?;
        Ok(preview)
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
            total = total
                .checked_add(file.bytes)
                .filter(|n| *n <= MAX_PATCH_BYTES as u64)
                .ok_or_else(|| invalid("restore bytes exceed16MiB"))?;
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
) -> Result<BTreeMap<RepoPath, Option<CapturedFile>>> {
    match source {
        RestoreSource::Capture { sequence, paths } => {
            if paths.is_empty() || paths.len() > 1024 {
                return Err(invalid("restore requires1..1024 paths"));
            }
            let patch = capture::storage::load(c, store, *sequence)?;
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
            let journal = engine::history(c, store, operation_id)?
                .ok_or_else(|| invalid("restore operation unknown"))?;
            Ok(journal.0.preview.before)
        }
    }
}
fn preview(
    store: &ProjectionStore,
    c: &Connection,
    state: &VersionedProjection,
    source: &RestoreSource,
) -> Result<RestorePreview> {
    let after = targets(c, store, source)?;
    validate_paths(&state.snapshot, &after)?;
    let scan = capture::scan::scan(store, &state.snapshot, false)?;
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
            capture::storage::file_bytes(c, file)?;
        }
    }
    validate_paths(&state.snapshot, &before)?;
    if before == after {
        return Err(invalid("selected capture already matches checkout"));
    }
    // Validate the eventual patch budget before any filesystem mutation.
    let mut changed = 0usize;
    let mut total = 0u64;
    for entry in &state.snapshot.entries {
        if let Some(actual) = resulting.get(entry.path.as_str()) {
            let expected = entry.target.as_ref().and_then(|t| match t {
                layrs_core::scm::TreeEntryTarget::File { file_id } => {
                    Some((file_id, entry.executable))
                }
                _ => None,
            });
            if actual.as_ref().map(|f| (&f.file_id, f.executable)) != expected {
                changed += 1;
                total = total
                    .checked_add(actual.as_ref().map_or(0, |f| f.bytes))
                    .ok_or_else(|| invalid("restore size overflow"))?;
            }
        }
    }
    if changed > 1024 || total > MAX_PATCH_BYTES as u64 {
        return Err(invalid("restore would exceed capture budget"));
    }
    let mut value = RestorePreview {
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
    Ok(value)
}
