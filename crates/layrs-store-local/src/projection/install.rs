//! Selective hydration: verified newly readable files, absent destinations only.
mod files;
use super::*;
use crate::secure_fs::PinnedDirectory;
use layrs_core::{
    projection::{EntryState, ReservedPaths},
    scm::{FileId, TreeEntryTarget},
};
use std::{
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
const MAX_FILE: u64 = 1024 * 1024 * 1024;
const MAX_TOTAL: u64 = 4 * MAX_FILE;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPoint {
    Staged,
    JournalDurable,
    PathInstalled,
    BeforeCommit,
    AfterCommit,
}
pub trait InstallFaults {
    fn check(&self, point: InstallPoint) -> io::Result<()>;
}
pub struct NoInstallFaults;
impl InstallFaults for NoInstallFaults {
    fn check(&self, _: InstallPoint) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileRecord {
    path: String,
    file_id: FileId,
    stage_name: String,
    identity: String,
    bytes: u64,
    executable: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    schema_version: String,
    expected_generation: u64,
    expected_digest: String,
    stage_directory: String,
    target: ProjectionSnapshot,
    files: Vec<FileRecord>,
}

pub(super) fn install(
    store: &ProjectionStore,
    expected: u64,
    target: &ProjectionSnapshot,
    write_file: &mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>,
    faults: &dyn InstallFaults,
) -> Result<VersionedProjection> {
    target.canonical_bytes()?;
    let mut access = format::access(store, SessionMode::Exclusive)?;
    database::require_mutable(&access.connection)?;
    let current = database::current(&access.connection)?;
    if current.generation == expected.saturating_add(1) && current.snapshot == *target {
        return Ok(current);
    }
    if current.generation != expected {
        return Err(ProjectionStoreError::Stale {
            expected,
            actual: current.generation,
        });
    }
    let paths = validate_transition(&current.snapshot, target)?;
    if paths.is_empty() {
        return Ok(current);
    }
    let root = PinnedDirectory::open(&store.root)?;
    for path in &paths {
        files::destination_absent(&root, &store.root, path)?;
    }
    let staging = PinnedDirectory::open(&store.root.join(".layrs"))?
        .ensure_child("projection-staging".as_ref())?;
    let stage_directory = format!(
        "install-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(invalid)?
            .as_nanos(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    );
    let stage_path = store
        .root
        .join(".layrs/projection-staging")
        .join(&stage_directory);
    fs::create_dir(&stage_path)?;
    staging.sync()?;
    let stage = staging.open_child(stage_directory.as_ref())?;
    let mut records = Vec::new();
    let mut total = 0u64;
    let entries: std::collections::BTreeMap<_, _> = target
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();
    for (index, path) in paths.iter().enumerate() {
        let entry = entries[path.as_str()];
        let Some(TreeEntryTarget::File { file_id }) = &entry.target else {
            return Err(invalid(
                "initial projection hydration supports regular files only",
            ));
        };
        if cfg!(windows) && entry.executable {
            return Err(invalid(
                "Windows cannot faithfully materialize executable mode",
            ));
        }
        let stage_name = format!("{index:08}");
        let mut staged = stage.create_file(stage_name.as_ref())?;
        let mut writer = files::FileWriter::new(staged.file_mut(), MAX_FILE.min(MAX_TOTAL - total));
        write_file(path, &mut writer)?;
        let length = writer.finish(file_id)?;
        total += length;
        files::set_mode(staged.file(), entry.executable)?;
        staged.file().sync_all()?;
        stage.sync()?;
        let identity = super::file::DatabasePin::open(&stage_path.join(&stage_name))?
            .identity()
            .to_owned();
        records.push(FileRecord {
            path: path.clone(),
            file_id: file_id.clone(),
            stage_name,
            identity,
            bytes: length,
            executable: entry.executable,
        });
    }
    faults.check(InstallPoint::Staged)?;
    let journal = Journal {
        schema_version: "layrs.projection-install.v1".into(),
        expected_generation: expected,
        expected_digest: current.digest,
        stage_directory,
        target: target.clone(),
        files: records,
    };
    let bytes = layrs_core::scm::canonical_json(&journal).map_err(invalid)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(invalid("projection install journal exceeds64MiB"));
    }
    access.verify()?;
    access
        .connection
        .execute("INSERT INTO projection_install VALUES(1,?1)", [bytes])?;
    faults.check(InstallPoint::JournalDurable)?;
    finish(store, &mut access, &journal, faults)
}

fn validate_transition(
    before: &ProjectionSnapshot,
    after: &ProjectionSnapshot,
) -> Result<Vec<String>> {
    before.validate()?;
    after.validate()?;
    if before.base_step_id != after.base_step_id
        || before.base_root_tree_id != after.base_root_tree_id
        || before.observed_policy_set_id != after.observed_policy_set_id
        || before.observed_authorization_epoch != after.observed_authorization_epoch
        || before.path_semantics != after.path_semantics
        || before.entries.len() != after.entries.len()
    {
        return Err(invalid(
            "hydration must keep the exact observed base and authority",
        ));
    }
    let mut changed = Vec::new();
    for (old, new) in before.entries.iter().zip(&after.entries) {
        let mut expected = old.clone();
        if old.state == EntryState::Unavailable && new.state == EntryState::Materialized {
            expected.state = EntryState::Materialized;
            changed.push(old.path.as_str().to_owned());
        }
        if expected != *new {
            return Err(invalid(
                "hydration cannot mutate targets, permissions or reserved entries",
            ));
        }
    }
    let fences = ReservedPaths::new(
        before
            .entries
            .iter()
            .filter(|e| e.state == EntryState::Reserved)
            .map(|e| e.path.as_str()),
        before.path_semantics,
    )?;
    for path in &changed {
        fences.check_path(path)?;
    }
    Ok(changed)
}

pub(super) fn resume(store: &ProjectionStore) -> Result<VersionedProjection> {
    let mut access = format::access(store, SessionMode::Exclusive)?;
    let length: u64 = access.connection.query_row(
        "SELECT length(payload) FROM projection_install WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if length > 64 * 1024 * 1024 {
        return Err(invalid("installation journal exceeds limit"));
    }
    let bytes: Vec<u8> = access.connection.query_row(
        "SELECT payload FROM projection_install WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(invalid)?;
    if layrs_core::scm::canonical_json(&journal).map_err(invalid)? != bytes {
        return Err(invalid("installation journal is not canonical"));
    }
    finish(store, &mut access, &journal, &NoInstallFaults)
}

fn finish(
    store: &ProjectionStore,
    access: &mut format::Access,
    journal: &Journal,
    faults: &dyn InstallFaults,
) -> Result<VersionedProjection> {
    let current = database::current(&access.connection)?;
    if journal.schema_version != "layrs.projection-install.v1"
        || journal.expected_generation != current.generation
        || journal.expected_digest != current.digest
    {
        return Err(invalid("installation journal has a stale or invalid base"));
    }
    let changed = validate_transition(&current.snapshot, &journal.target)?;
    let suffix = journal
        .stage_directory
        .strip_prefix("install-")
        .unwrap_or("");
    if changed.len() != journal.files.len()
        || suffix.is_empty()
        || journal.stage_directory.len() > 96
        || !suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-')
    {
        return Err(invalid("invalid installation journal paths"));
    }
    let stage_path = store
        .root
        .join(".layrs/projection-staging")
        .join(&journal.stage_directory);
    let stage = PinnedDirectory::open(&stage_path)?;
    let root = PinnedDirectory::open(&store.root)?;
    let targets: std::collections::BTreeMap<_, _> = journal
        .target
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();
    let mut total = 0u64;
    // Verify every remaining staged/installed inode before touching any path.
    for (index, (path, record)) in changed.iter().zip(&journal.files).enumerate() {
        if path != &record.path
            || record.stage_name != format!("{index:08}")
            || record.bytes > MAX_FILE
        {
            return Err(invalid(
                "installation journal does not match its target paths",
            ));
        }
        let target = targets[path.as_str()];
        if target.target != Some(TreeEntryTarget::file(record.file_id.clone()))
            || target.executable != record.executable
        {
            return Err(invalid("installation journal target identity mismatch"));
        }
        total = total
            .checked_add(record.bytes)
            .filter(|n| *n <= MAX_TOTAL)
            .ok_or_else(|| invalid("installation exceeds4GiB"))?;
        let staged = stage_path.join(&record.stage_name);
        match fs::symlink_metadata(&staged) {
            Ok(_) => {
                files::verify_file(&staged, record)?;
                files::destination_absent(&root, &store.root, path)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                files::verify_file(&store.root.join(path), record)?
            }
            Err(e) => return Err(e.into()),
        }
    }
    for record in &journal.files {
        let staged = stage_path.join(&record.stage_name);
        match fs::symlink_metadata(&staged) {
            Ok(_) => {
                let source = stage.open_file(record.stage_name.as_ref())?;
                files::verify_file(&staged, record)?;
                let (parent, name) = root.components_to_parent(Path::new(&record.path))?;
                if source.rename_to(&parent, &name)? {
                    return Err(invalid(
                        "installation destination appeared; no overwrite performed",
                    ));
                }
                stage.sync()?;
                parent.sync()?;
                root.verify_path_identity()?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        files::verify_file(&store.root.join(&record.path), record)?;
        faults.check(InstallPoint::PathInstalled)?;
    }
    faults.check(InstallPoint::BeforeCommit)?;
    for record in &journal.files {
        files::verify_file(&store.root.join(&record.path), record)?;
    }
    access.verify()?;
    let generation = current
        .generation
        .checked_add(1)
        .filter(|g| *g <= i64::MAX as u64)
        .ok_or_else(|| invalid("projection generation exhausted"))?;
    let payload = journal.target.canonical_bytes()?;
    let digest = database::digest(generation, Some(&current.digest), &payload);
    let tx = access
        .connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO projection_snapshots VALUES(?1,?2,?3,?4)",
        rusqlite::params![generation, digest, current.digest, payload],
    )?;
    tx.execute(
        "UPDATE projection_head SET generation=?1 WHERE singleton=1",
        [generation],
    )?;
    tx.execute("DELETE FROM projection_install WHERE singleton=1", [])?;
    tx.commit()?;
    faults.check(InstallPoint::AfterCommit)?;
    access.verify()?;
    Ok(VersionedProjection {
        generation,
        digest,
        snapshot: journal.target.clone(),
    })
}
