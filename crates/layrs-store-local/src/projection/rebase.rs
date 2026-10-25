//! Resolved regular-file rebase onto an exact authorized remote projection.
mod engine;
pub(super) use engine::resume_with_access;
mod history;
use super::*;
use crate::secure_fs::PinnedDirectory;
pub(super) use history::verify_history;
use layrs_core::{
    projection::EntryState,
    scm::{RepoPath, TreeEntryTarget, canonical_json},
};
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const PREVIEW_SCHEMA: &str = "layrs.projection-rebase-preview.v1";
const JOURNAL_SCHEMA: &str = "layrs.projection-rebase-journal.v1";
const MAX_JOURNAL: usize = 64 * 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RebasePreview {
    pub schema_version: String,
    pub operation_id: String,
    pub projection_generation: u64,
    pub projection_digest: String,
    pub capture_sequence: u64,
    pub capture_digest: String,
    pub target: ProjectionSnapshot,
    /// Exact current values at every target writable regular leaf.
    pub before: BTreeMap<RepoPath, Option<CapturedFile>>,
    /// Complete resolved checkout values at those same leaves.
    pub resolutions: BTreeMap<RepoPath, Option<CapturedFile>>,
    pub checkout_digest: String,
    pub after_checkout_digest: String,
    pub digest: String,
}
impl RebasePreview {
    /// Bind a prepared preview to a stable client operation identity.
    pub fn with_operation_id(mut self, id: &str) -> Result<Self> {
        self.operation_id = id.to_owned();
        self.digest = self.computed_digest()?;
        Ok(self)
    }
    fn computed_digest(&self) -> Result<String> {
        if self.schema_version != PREVIEW_SCHEMA
            || self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || !self
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid("invalid rebase preview identity"));
        }
        let mut copy = self.clone();
        copy.digest.clear();
        hash(b"layrs-projection-rebase-preview-v1\0", &copy)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebasePoint {
    IntentDurable,
    Staged,
    JournalDurable,
    PathQuarantined,
    PathInstalled,
    BeforeCommit,
    AfterCommit,
}
pub trait RebaseFaults {
    fn check(&self, point: RebasePoint) -> io::Result<()>;
}
pub struct NoRebaseFaults;
impl RebaseFaults for NoRebaseFaults {
    fn check(&self, _: RebasePoint) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileRecord {
    path: RepoPath,
    file: CapturedFile,
    identity: String,
    name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: String,
    preview: RebasePreview,
    directory: String,
    directory_identity: Option<String>,
    ready: bool,
    old: Vec<FileRecord>,
    new: Vec<FileRecord>,
}

impl ProjectionStore {
    pub fn preview_rebase(
        &self,
        expected_generation: u64,
        capture_sequence: u64,
        target: &ProjectionSnapshot,
        resolutions: &BTreeMap<RepoPath, Option<CapturedFile>>,
    ) -> Result<RebasePreview> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_mutable(&access.connection)?;
        let current = database::current(&access.connection)?;
        if current.generation != expected_generation {
            return Err(ProjectionStoreError::Stale {
                expected: expected_generation,
                actual: current.generation,
            });
        }
        let value = preview(
            self,
            &access.connection,
            &current,
            capture_sequence,
            target,
            resolutions,
        )?;
        access.verify()?;
        Ok(value)
    }
    /// `write_file` may only read prepared local bytes; no network I/O under the lock.
    pub fn apply_rebase(
        &self,
        preview: &RebasePreview,
        write_file: &mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>,
    ) -> Result<CapturedPatch> {
        self.apply_rebase_with_faults(preview, write_file, &NoRebaseFaults)
    }
    pub fn apply_rebase_with_faults(
        &self,
        preview: &RebasePreview,
        write_file: &mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>,
        faults: &dyn RebaseFaults,
    ) -> Result<CapturedPatch> {
        engine::apply(self, preview, write_file, faults)
    }
    pub fn resume_rebase(root: impl AsRef<Path>) -> Result<CapturedPatch> {
        let root = std::path::absolute(root)?;
        let marker = format::read_marker(&root)?;
        engine::resume(&Self { root, marker })
    }
    /// Validate and return a durable completed result without reading live checkout files.
    pub fn completed_rebase(&self, operation_id: &str) -> Result<Option<CapturedPatch>> {
        let access = format::access(self, SessionMode::Shared)?;
        let result = history::completed_id(&access.connection, self, operation_id)?;
        access.verify()?;
        Ok(result)
    }
}

fn preview(
    store: &ProjectionStore,
    c: &Connection,
    current: &VersionedProjection,
    capture_sequence: u64,
    target: &ProjectionSnapshot,
    resolutions: &BTreeMap<RepoPath, Option<CapturedFile>>,
) -> Result<RebasePreview> {
    selection::verify_target(c, target)?;
    validate_target(&current.snapshot, target, &selection::current(c)?)?;
    validate_resolutions(target, resolutions)?;
    let scan = capture::scan::scan(store, c, &current.snapshot, false)?;
    let captured = exact_current_capture(c, store, current, capture_sequence, &scan)?;
    let root = PinnedDirectory::open(&store.root)?;
    let mut before = BTreeMap::new();
    let mut after = scan.files.clone();
    for (path, resolved) in resolutions {
        let actual = scan.files.get(path.as_str()).cloned().unwrap_or(None);
        if !scan.files.contains_key(path.as_str()) {
            install::files::destination_absent(&root, &store.root, path.as_str())?;
        }
        before.insert(path.clone(), actual);
        after.insert(path.to_string(), resolved.clone());
    }
    validate_after(target, &after)?;
    root.verify_path_identity()?;
    let mut value = RebasePreview {
        schema_version: PREVIEW_SCHEMA.into(),
        operation_id: format!(
            "{}-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(invalid)?
                .as_nanos(),
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ),
        projection_generation: current.generation,
        projection_digest: current.digest.clone(),
        capture_sequence,
        capture_digest: captured.digest,
        target: target.clone(),
        before,
        resolutions: resolutions.clone(),
        checkout_digest: checkout_digest(&scan.files)?,
        after_checkout_digest: checkout_digest(&after)?,
        digest: String::new(),
    };
    value.digest = value.computed_digest()?;
    if canonical_json(&value).map_err(invalid)?.len() > MAX_JOURNAL / 2 {
        return Err(invalid("rebase preview exceeds32MiB"));
    }
    Ok(value)
}

fn validate_target(
    before: &ProjectionSnapshot,
    target: &ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
) -> Result<()> {
    before.validate()?;
    target.validate()?;
    let reserved = before.reserved_paths()?;
    if before.path_semantics != target.path_semantics
        || target.observed_authorization_epoch < before.observed_authorization_epoch
        || (before.base_step_id == target.base_step_id
            && before.base_root_tree_id != target.base_root_tree_id)
        || (before.observed_authorization_epoch == target.observed_authorization_epoch
            && before.observed_policy_set_id != target.observed_policy_set_id)
    {
        return Err(invalid(
            "rebase base or authorization observation regressed",
        ));
    }
    let next: BTreeMap<_, _> = target.entries.iter().map(|e| (&e.path, e)).collect();
    for old in &before.entries {
        // Structural changes outside local intent do not become local edits.
        if !selection.includes(&old.path) {
            continue;
        }
        let new = next
            .get(&old.path)
            .ok_or_else(|| invalid("rebase target must retain old observed paths"))?;
        if old.writable && regular(old) {
            if !new.writable || !regular(new) {
                return Err(invalid("rebase changes writable regular path authority"));
            }
        } else if matches!(old.state, EntryState::Reserved | EntryState::Unavailable) {
            if new.state != old.state
                || new.writable != old.writable
                || new.executable != old.executable
            {
                return Err(invalid("rebase changes protected path authority"));
            }
        } else if matches!(old.target, Some(TreeEntryTarget::Tree { .. })) {
            if new.state != old.state
                || new.writable != old.writable
                || !matches!(new.target, Some(TreeEntryTarget::Tree { .. }))
            {
                return Err(invalid("rebase changes directory structure"));
            }
        } else if *new != old {
            return Err(invalid("rebase changes read-only materialized path"));
        }
    }
    let old: BTreeMap<_, _> = before.entries.iter().map(|e| (&e.path, e)).collect();
    for entry in &target.entries {
        if !selection.includes(&entry.path) {
            continue;
        }
        if entry.writable && regular(entry) {
            reserved.check_path(entry.path.as_str())?;
        }
        if !old.contains_key(&entry.path) && (!entry.writable || !regular(entry)) {
            return Err(invalid("rebase adds an unsupported path"));
        }
    }
    Ok(())
}
fn regular(entry: &layrs_core::projection::ProjectionEntry) -> bool {
    matches!(
        (&entry.state, &entry.target),
        (EntryState::Materialized, Some(TreeEntryTarget::File { .. })) | (EntryState::Absent, None)
    )
}
fn validate_resolutions(
    target: &ProjectionSnapshot,
    resolutions: &BTreeMap<RepoPath, Option<CapturedFile>>,
) -> Result<()> {
    let required: Vec<_> = target
        .entries
        .iter()
        .filter(|e| e.writable && regular(e))
        .map(|e| &e.path)
        .collect();
    if required.iter().copied().ne(resolutions.keys()) {
        return Err(invalid(
            "rebase needs one resolution per writable regular path",
        ));
    }
    let mut total = 0u64;
    for file in resolutions.values().flatten() {
        if file.bytes > MAX_CAPTURE_FILE_BYTES || (cfg!(windows) && file.executable) {
            return Err(invalid("rebase resolution file or mode exceeds limit"));
        }
        total = total
            .checked_add(file.bytes)
            .filter(|n| *n <= MAX_CAPTURE_BYTES)
            .ok_or_else(|| invalid("rebase resolved checkout exceeds4GiB"))?;
    }
    Ok(())
}
fn validate_after(
    target: &ProjectionSnapshot,
    after: &BTreeMap<String, Option<CapturedFile>>,
) -> Result<()> {
    let mut changes = 0usize;
    let mut total = 0u64;
    for entry in &target.entries {
        if matches!(entry.state, EntryState::Reserved | EntryState::Unavailable)
            || matches!(entry.target, Some(TreeEntryTarget::Tree { .. }))
        {
            continue;
        }
        let actual = after
            .get(entry.path.as_str())
            .ok_or_else(|| invalid("rebase checkout map incomplete"))?;
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id, entry.executable)),
            None if entry.state == EntryState::Absent => None,
            _ => return Err(invalid("rebase target is not a regular file leaf")),
        };
        if actual.as_ref().map(|f| (&f.file_id, f.executable)) != expected {
            if !entry.writable {
                return Err(invalid("rebase would change read-only path"));
            }
            changes += 1;
            total = total
                .checked_add(actual.as_ref().map_or(0, |f| f.bytes))
                .ok_or_else(|| invalid("rebase capture size overflow"))?;
        }
    }
    if changes > 1024 || total > MAX_CAPTURE_BYTES {
        return Err(invalid("rebase exceeds capture budget"));
    }
    Ok(())
}
fn exact_current_capture(
    c: &Connection,
    store: &ProjectionStore,
    current: &VersionedProjection,
    sequence: u64,
    scan: &capture::scan::Scan,
) -> Result<CapturedPatch> {
    let latest: Option<u64> =
        c.query_row("SELECT MAX(sequence) FROM projection_captures", [], |r| {
            r.get(0)
        })?;
    if latest != Some(sequence) {
        return Err(invalid("rebase capture is not the latest"));
    }
    let patch = capture::storage::load(c, store, sequence)?;
    if patch.projection_generation != current.generation
        || patch.projection_digest != current.digest
        || patch.changes != scan.changes
    {
        return Err(invalid("rebase capture no longer matches current checkout"));
    }
    Ok(patch)
}
fn checkout_digest(files: &BTreeMap<String, Option<CapturedFile>>) -> Result<String> {
    hash(b"layrs-projection-rebase-checkout-v1\0", files)
}
fn hash<T: Serialize>(domain: &[u8], value: &T) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&canonical_json(value).map_err(invalid)?);
    Ok(format!("blake3:{}", hasher.finalize()))
}
