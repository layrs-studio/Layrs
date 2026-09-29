//! Durable relocation of an observed writable directory subtree.
use super::*;
use crate::secure_fs::PinnedDirectory;
use layrs_core::{
    projection::{EntryState, PathSemantics},
    scm::{RepoPath, TreeEntryTarget, canonical_json},
};
use rusqlite::{Connection, params};
use std::{
    collections::BTreeMap,
    fs,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const PREVIEW_SCHEMA: &str = "layrs.projection-directory-move-preview.v1";
const JOURNAL_SCHEMA: &str = "layrs.projection-directory-move-journal.v1";
const MAX_JOURNAL: usize = 8 * 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);
fn unique_id() -> Result<String> {
    Ok(format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(invalid)?
            .as_nanos(),
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryMovePreview {
    pub schema_version: String,
    pub operation_id: String,
    pub projection_generation: u64,
    pub projection_digest: String,
    pub source: RepoPath,
    pub destination: RepoPath,
    pub source_directory_identity: String,
    /// Exact paths and descriptors, including locally created source leaves.
    pub source_files: BTreeMap<String, CapturedFile>,
    pub checkout_digest: String,
    pub after_checkout_digest: String,
    pub digest: String,
}
impl DirectoryMovePreview {
    fn computed_digest(&self) -> Result<String> {
        if self.schema_version != PREVIEW_SCHEMA
            || self.operation_id.is_empty()
            || self.operation_id.len() > 128
            || !self
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-')
        {
            return Err(invalid("invalid directory move preview identity"));
        }
        let mut copy = self.clone();
        copy.digest.clear();
        Ok(hash(
            b"layrs-projection-directory-move-preview-v1\0",
            &canonical_json(&copy).map_err(invalid)?,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: String,
    preview: DirectoryMovePreview,
    capture_digest: Option<String>,
}

impl ProjectionStore {
    pub fn preview_directory_move(
        &self,
        expected_generation: u64,
        source: &RepoPath,
        destination: &RepoPath,
    ) -> Result<DirectoryMovePreview> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_mutable(&access.connection)?;
        let state = database::current(&access.connection)?;
        if state.generation != expected_generation {
            return Err(ProjectionStoreError::Stale {
                expected: expected_generation,
                actual: state.generation,
            });
        }
        let preview = preview(self, &state, source, destination)?;
        access.verify()?;
        Ok(preview)
    }
    pub fn apply_directory_move(&self, preview: &DirectoryMovePreview) -> Result<CapturedPatch> {
        self.apply_directory_move_with_faults(preview, &NoNamespaceFaults)
    }
    pub fn apply_directory_move_with_faults(
        &self,
        expected: &DirectoryMovePreview,
        faults: &dyn NamespaceFaults,
    ) -> Result<CapturedPatch> {
        if expected.digest != expected.computed_digest()? {
            return Err(invalid("directory move preview digest mismatch"));
        }
        let mut access = format::access(self, SessionMode::Exclusive)?;
        database::require_mutable(&access.connection)?;
        if let Some(capture) = completed(&access.connection, self, expected)? {
            return Ok(capture);
        }
        let state = database::current(&access.connection)?;
        if state.generation != expected.projection_generation
            || state.digest != expected.projection_digest
        {
            return Err(invalid("directory move projection changed"));
        }
        let mut actual = preview(self, &state, &expected.source, &expected.destination)?;
        actual.operation_id = expected.operation_id.clone();
        actual.digest = actual.computed_digest()?;
        if &actual != expected {
            return Err(invalid("directory move preview is stale"));
        }
        let journal = Journal {
            schema_version: JOURNAL_SCHEMA.into(),
            preview: expected.clone(),
            capture_digest: None,
        };
        let bytes = canonical_json(&journal).map_err(invalid)?;
        if bytes.len() > MAX_JOURNAL {
            return Err(invalid("directory move journal exceeds8MiB"));
        }
        access.verify()?;
        access.connection.execute(
            "INSERT INTO projection_namespace_pending VALUES(1,?1)",
            [&bytes],
        )?;
        faults.check(NamespacePoint::JournalDurable)?;
        finish(self, &mut access, &journal, faults)
    }
}

fn preview(
    store: &ProjectionStore,
    state: &VersionedProjection,
    source: &RepoPath,
    destination: &RepoPath,
) -> Result<DirectoryMovePreview> {
    validate_roots(&state.snapshot, source, destination)?;
    let root = PinnedDirectory::open(&store.root)?;
    let (source_parent, source_name) = root.existing_parent(Path::new(source.as_str()))?;
    let source_dir = source_parent.open_child_for_traversal(&source_name)?;
    let (destination_parent, _) = root.existing_parent(Path::new(destination.as_str()))?;
    absent(&store.root.join(destination.as_str()))?;
    let scan = capture::scan::scan(store, &state.snapshot, false)?;
    let source_files = source_files(&scan.files, source)?;
    validate_directory_mapping(store, &state.snapshot, source, destination)?;
    let after = mapped_files(
        &state.snapshot,
        &scan.files,
        source,
        destination,
        &source_files,
    )?;
    source_dir.verify_path_identity()?;
    destination_parent.verify_path_identity()?;
    root.verify_path_identity()?;
    let mut result = DirectoryMovePreview {
        schema_version: PREVIEW_SCHEMA.into(),
        operation_id: unique_id()?,
        projection_generation: state.generation,
        projection_digest: state.digest.clone(),
        source: source.clone(),
        destination: destination.clone(),
        source_directory_identity: source_dir.identity_token(),
        source_files,
        checkout_digest: checkout_digest(&scan.files)?,
        after_checkout_digest: checkout_digest(&after)?,
        digest: String::new(),
    };
    result.digest = result.computed_digest()?;
    Ok(result)
}

fn validate_roots(
    snapshot: &ProjectionSnapshot,
    source: &RepoPath,
    destination: &RepoPath,
) -> Result<()> {
    let reserved = snapshot.reserved_paths()?;
    reserved.check_move(source.as_str(), destination.as_str())?;
    let key = |p: &str| match snapshot.path_semantics {
        PathSemantics::PortableWindows => p.to_uppercase(),
        PathSemantics::CaseSensitive => p.to_owned(),
    };
    let from = key(source.as_str());
    let to = key(destination.as_str());
    if from == to || from.starts_with(&format!("{to}/")) || to.starts_with(&format!("{from}/")) {
        return Err(invalid("directory move paths overlap or alias"));
    }
    Ok(())
}
fn source_files(
    files: &BTreeMap<String, Option<CapturedFile>>,
    source: &RepoPath,
) -> Result<BTreeMap<String, CapturedFile>> {
    let prefix = format!("{source}/");
    let result: BTreeMap<_, _> = files
        .iter()
        .filter_map(|(path, file)| {
            (path.starts_with(&prefix))
                .then(|| file.as_ref().map(|file| (path.clone(), file.clone())))
                .flatten()
        })
        .collect();
    if result.is_empty() || result.len() > 1024 {
        return Err(invalid(
            "directory move requires1..1024 observed source files",
        ));
    }
    Ok(result)
}
fn validate_directory_mapping(
    store: &ProjectionStore,
    snapshot: &ProjectionSnapshot,
    source: &RepoPath,
    destination: &RepoPath,
) -> Result<()> {
    let mut pending = vec![(
        String::new(),
        PinnedDirectory::open(&store.root.join(source.as_str()))?,
    )];
    let mut count = 0usize;
    while let Some((relative, directory)) = pending.pop() {
        directory.verify_path_identity()?;
        for item in fs::read_dir(store.root.join(source.as_str()).join(&relative))? {
            let item = item?;
            if !item.file_type()?.is_dir() {
                continue;
            }
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| invalid("non-UTF8 directory source"))?;
            let child = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let mapped = format!("{destination}/{child}");
            let prefix = format!("{mapped}/");
            if !snapshot.entries.iter().any(|entry| {
                entry.path.as_str() == mapped || entry.path.as_str().starts_with(&prefix)
            }) {
                return Err(invalid(
                    "directory destination lacks observation for source subtree",
                ));
            }
            pending.push((child, directory.open_child_for_traversal(name.as_ref())?));
            count += 1;
            if count > 100_000 {
                return Err(invalid("directory subtree exceeds100000 entries"));
            }
        }
        directory.verify_path_identity()?;
    }
    Ok(())
}
fn mapped_files(
    snapshot: &ProjectionSnapshot,
    before: &BTreeMap<String, Option<CapturedFile>>,
    source: &RepoPath,
    destination: &RepoPath,
    source_files: &BTreeMap<String, CapturedFile>,
) -> Result<BTreeMap<String, Option<CapturedFile>>> {
    validate_roots(snapshot, source, destination)?;
    let reserved = snapshot.reserved_paths()?;
    let prefix = format!("{source}/");
    let mut after = before.clone();
    for (path, file) in source_files {
        let suffix = path
            .strip_prefix(&prefix)
            .ok_or_else(|| invalid("directory source file escaped subtree"))?;
        let mapped = RepoPath::parse(format!("{destination}/{suffix}")).map_err(invalid)?;
        if path.len() > 4096
            || mapped.as_str().len() > 4096
            || mapped.as_str().split('/').count() > 128
        {
            return Err(invalid("directory move exceeds capture path budget"));
        }
        reserved.check_move(path, mapped.as_str())?;
        let old = snapshot
            .entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .map_err(|_| invalid("directory source file is unobserved"))?;
        let old = &snapshot.entries[old];
        if !old.writable
            || !matches!(
                (&old.state, &old.target),
                (EntryState::Materialized, Some(TreeEntryTarget::File { .. }))
                    | (EntryState::Absent, None)
            )
        {
            return Err(invalid("directory source file lacks writable observation"));
        }
        let new = snapshot
            .entries
            .binary_search_by(|e| e.path.cmp(&mapped))
            .map_err(|_| invalid("directory destination leaf lacks explicit absence"))?;
        let new = &snapshot.entries[new];
        if !new.writable
            || new.state != EntryState::Absent
            || new.target.is_some()
            || before.get(mapped.as_str()) != Some(&None)
        {
            return Err(invalid(
                "directory destination leaf is not absent and writable",
            ));
        }
        after.insert(path.clone(), None);
        after.insert(mapped.to_string(), Some(file.clone()));
    }
    if source_files.len() > 1024 {
        return Err(invalid("directory move exceeds file budget"));
    }
    let mut changes = 0usize;
    let mut total = 0u64;
    for entry in &snapshot.entries {
        if matches!(entry.state, EntryState::Reserved | EntryState::Unavailable)
            || matches!(entry.target, Some(TreeEntryTarget::Tree { .. }))
        {
            continue;
        }
        let actual = after
            .get(entry.path.as_str())
            .ok_or_else(|| invalid("directory checkout map incomplete"))?;
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id, entry.executable)),
            None if entry.state == EntryState::Absent => None,
            _ => return Err(invalid("directory move requires regular file leaves")),
        };
        if actual.as_ref().map(|f| (&f.file_id, f.executable)) != expected {
            if !entry.writable {
                return Err(invalid("directory move changes read-only entry"));
            }
            changes += 1;
            if let Some(file) = actual {
                total = total
                    .checked_add(file.bytes)
                    .ok_or_else(|| invalid("directory capture size overflow"))?;
            }
        }
    }
    if changes > 1024 || total > capture::MAX_CAPTURE_BYTES {
        return Err(invalid("directory move exceeds capture budget"));
    }
    Ok(after)
}

fn finish(
    store: &ProjectionStore,
    access: &mut format::Access,
    journal: &Journal,
    faults: &dyn NamespaceFaults,
) -> Result<CapturedPatch> {
    let expected = &journal.preview;
    let state = database::current(&access.connection)?;
    if journal.schema_version != JOURNAL_SCHEMA
        || journal.capture_digest.is_some()
        || expected.digest != expected.computed_digest()?
        || state.generation != expected.projection_generation
        || state.digest != expected.projection_digest
    {
        return Err(invalid("directory move recovery identity mismatch"));
    }
    validate_record_plan(&state.snapshot, expected)?;
    let root = PinnedDirectory::open(&store.root)?;
    let (source_parent, source_name) = root.existing_parent(Path::new(expected.source.as_str()))?;
    let (destination_parent, destination_name) =
        root.existing_parent(Path::new(expected.destination.as_str()))?;
    match fs::symlink_metadata(store.root.join(expected.source.as_str())) {
        Ok(_) => {
            let source_dir = source_parent.open_child_for_traversal(&source_name)?;
            if source_dir.identity_token() != expected.source_directory_identity {
                return Err(invalid("directory source identity changed"));
            }
            absent(&store.root.join(expected.destination.as_str()))?;
            let before = capture::scan::scan(store, &state.snapshot, false)?;
            if checkout_digest(&before.files)? != expected.checkout_digest
                || source_files(&before.files, &expected.source)? != expected.source_files
            {
                return Err(invalid(
                    "directory source checkout changed before relocation",
                ));
            }
            validate_directory_mapping(
                store,
                &state.snapshot,
                &expected.source,
                &expected.destination,
            )?;
            source_dir.verify_path_identity()?;
            if source_parent.move_child_directory(
                &source_name,
                &expected.source_directory_identity,
                &destination_parent,
                &destination_name,
            )? {
                return Err(invalid(
                    "directory destination appeared; nothing overwritten",
                ));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    absent(&store.root.join(expected.source.as_str()))?;
    let destination_dir = destination_parent.open_child_for_traversal(&destination_name)?;
    if destination_dir.identity_token() != expected.source_directory_identity {
        return Err(invalid("directory destination identity changed"));
    }
    let first = capture::scan::scan(store, &state.snapshot, false)?;
    if checkout_digest(&first.files)? != expected.after_checkout_digest {
        return Err(invalid("directory destination content changed"));
    }
    faults.check(NamespacePoint::PathApplied)?;
    let second = capture::scan::scan(store, &state.snapshot, true)?;
    if first.files != second.files
        || checkout_digest(&second.files)? != expected.after_checkout_digest
    {
        return Err(invalid("directory checkout changed during finalization"));
    }
    destination_dir.verify_path_identity()?;
    root.verify_path_identity()?;
    access.verify()?;
    let tx = access
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let capture = capture::storage::persist_in_transaction(&tx, store, &state, second)?;
    destination_dir.verify_path_identity()?;
    absent(&store.root.join(expected.source.as_str()))?;
    let mut completed = journal.clone();
    completed.capture_digest = Some(capture.digest.clone());
    let bytes = canonical_json(&completed).map_err(invalid)?;
    tx.execute(
        "INSERT INTO projection_namespace_operations VALUES(?1,?2,?3)",
        params![expected.digest, capture.sequence, bytes],
    )?;
    tx.execute(
        "DELETE FROM projection_namespace_pending WHERE singleton=1",
        [],
    )?;
    faults.check(NamespacePoint::BeforeCommit)?;
    let final_scan = capture::scan::scan(store, &state.snapshot, false)?;
    if checkout_digest(&final_scan.files)? != expected.after_checkout_digest {
        return Err(invalid("directory checkout changed before commit"));
    }
    destination_dir.verify_path_identity()?;
    absent(&store.root.join(expected.source.as_str()))?;
    root.verify_path_identity()?;
    format::verify_boundaries(
        &store.root,
        &store.marker,
        &access.metadata,
        &access.database,
    )?;
    tx.commit()?;
    faults.check(NamespacePoint::AfterCommit)?;
    access.verify()?;
    Ok(capture)
}

fn validate_record_plan(
    snapshot: &ProjectionSnapshot,
    preview: &DirectoryMovePreview,
) -> Result<()> {
    validate_roots(snapshot, &preview.source, &preview.destination)?;
    if preview.source_files.is_empty()
        || preview.source_files.len() > 1024
        || !preview.source_directory_identity.starts_with("fs:v1:")
        || preview.source_directory_identity.len() != 39
    {
        return Err(invalid("directory move journal source is invalid"));
    }
    let mut minimal = BTreeMap::new();
    for entry in &snapshot.entries {
        if matches!(entry.state, EntryState::Reserved | EntryState::Unavailable)
            || matches!(entry.target, Some(TreeEntryTarget::Tree { .. }))
        {
            continue;
        }
        let actual = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some(CapturedFile {
                file_id: file_id.clone(),
                executable: entry.executable,
                bytes: 0,
            }),
            None => None,
            _ => return Err(invalid("directory move requires regular file leaves")),
        };
        minimal.insert(entry.path.to_string(), actual);
    }
    for (path, file) in &preview.source_files {
        minimal.insert(path.clone(), Some(file.clone()));
    }
    mapped_files(
        snapshot,
        &minimal,
        &preview.source,
        &preview.destination,
        &preview.source_files,
    )?;
    Ok(())
}

pub(super) fn resume_payload(
    store: &ProjectionStore,
    access: &mut format::Access,
    bytes: &[u8],
) -> Result<CapturedPatch> {
    if bytes.len() > MAX_JOURNAL {
        return Err(invalid("directory move journal exceeds8MiB"));
    }
    let journal: Journal = serde_json::from_slice(bytes).map_err(invalid)?;
    if canonical_json(&journal).map_err(invalid)? != bytes {
        return Err(invalid("directory move journal is not canonical"));
    }
    finish(store, access, &journal, &NoNamespaceFaults)
}

fn completed(
    c: &Connection,
    store: &ProjectionStore,
    expected: &DirectoryMovePreview,
) -> Result<Option<CapturedPatch>> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_namespace_operations WHERE preview_digest=?1)",
        [&expected.digest],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let (sequence, length): (u64, u64) = c.query_row("SELECT capture_sequence,length(payload) FROM projection_namespace_operations WHERE preview_digest=?1", [&expected.digest], |r| Ok((r.get(0)?,r.get(1)?)))?;
    if length > MAX_JOURNAL as u64 {
        return Err(invalid("directory move history exceeds8MiB"));
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT payload FROM projection_namespace_operations WHERE preview_digest=?1",
        [&expected.digest],
        |r| r.get(0),
    )?;
    let journal = verify_record(c, store, &expected.digest, sequence, &bytes)?;
    if journal.preview != *expected {
        return Err(invalid("directory move replay identity changed"));
    }
    Ok(Some(capture::storage::load(c, store, sequence)?))
}

pub(in crate::projection) fn verify_history_record(
    c: &Connection,
    store: &ProjectionStore,
    key: &str,
    sequence: u64,
    bytes: &[u8],
) -> Result<()> {
    verify_record(c, store, key, sequence, bytes)?;
    Ok(())
}
fn verify_record(
    c: &Connection,
    store: &ProjectionStore,
    key: &str,
    sequence: u64,
    bytes: &[u8],
) -> Result<Journal> {
    if bytes.len() > MAX_JOURNAL {
        return Err(invalid("directory move history exceeds8MiB"));
    }
    let journal: Journal = serde_json::from_slice(bytes).map_err(invalid)?;
    let preview = &journal.preview;
    if journal.schema_version != JOURNAL_SCHEMA
        || canonical_json(&journal).map_err(invalid)? != bytes
        || preview.digest != key
        || preview.computed_digest()? != key
    {
        return Err(invalid("directory move history identity mismatch"));
    }
    let capture = capture::storage::load(c, store, sequence)?;
    if capture.projection_generation != preview.projection_generation
        || capture.projection_digest != preview.projection_digest
        || journal.capture_digest.as_deref() != Some(capture.digest.as_str())
    {
        return Err(invalid("directory move history capture base mismatch"));
    }
    let length: u64 = c.query_row(
        "SELECT length(payload) FROM projection_snapshots WHERE generation=?1",
        [preview.projection_generation],
        |r| r.get(0),
    )?;
    if length > 64 * 1024 * 1024 {
        return Err(invalid("directory move base snapshot exceeds64MiB"));
    }
    let base_bytes: Vec<u8> = c.query_row(
        "SELECT payload FROM projection_snapshots WHERE generation=?1",
        [preview.projection_generation],
        |r| r.get(0),
    )?;
    let base: ProjectionSnapshot = serde_json::from_slice(&base_bytes).map_err(invalid)?;
    if base.canonical_bytes()? != base_bytes {
        return Err(invalid("directory move base snapshot changed"));
    }
    validate_record_plan(&base, preview)?;
    for (source, file) in &preview.source_files {
        let suffix = source
            .strip_prefix(&format!("{}/", preview.source))
            .ok_or_else(|| invalid("directory source path invalid"))?;
        let destination =
            RepoPath::parse(format!("{}/{suffix}", preview.destination)).map_err(invalid)?;
        let dest = capture
            .changes
            .iter()
            .find(|change| change.path == destination)
            .ok_or_else(|| invalid("directory capture missing destination"))?;
        if dest.expected.is_some() || dest.replacement.as_ref() != Some(file) {
            return Err(invalid("directory capture destination mismatch"));
        }
        let original = base
            .entries
            .iter()
            .find(|entry| entry.path.as_str() == source)
            .ok_or_else(|| invalid("directory capture source missing"))?;
        let from = capture
            .changes
            .iter()
            .find(|change| change.path.as_str() == source);
        if (original.state == EntryState::Absent && from.is_some())
            || (original.state == EntryState::Materialized
                && !from.is_some_and(|change| {
                    change.expected.is_some() && change.replacement.is_none()
                }))
        {
            return Err(invalid("directory capture source mismatch"));
        }
    }
    Ok(journal)
}

fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(invalid("directory path is occupied")),
        Err(e) => Err(e.into()),
    }
}
fn checkout_digest(files: &BTreeMap<String, Option<CapturedFile>>) -> Result<String> {
    Ok(hash(
        b"layrs-projection-checkout-v1\0",
        &canonical_json(files).map_err(invalid)?,
    ))
}
fn hash(domain: &[u8], bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(bytes);
    format!("blake3:{}", hasher.finalize())
}
