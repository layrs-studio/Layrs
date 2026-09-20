use std::collections::BTreeSet;
#[cfg(not(windows))]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use layrs_store_local::StoreError;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{
    bump_generation, normalized_relative_key, store_error, unix_now_ms, LocalStoreSession,
};

const BACKUP_ROOT: &str = "materialization-backups";

#[derive(Clone, Debug)]
pub(in crate::access_registry) struct MaterializationLease {
    operation_id: String,
    root: PathBuf,
    entries: Vec<BackupEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MaterializationJournal {
    schema: String,
    operation_id: String,
    root_path: String,
    entries: Vec<BackupEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupEntry {
    path: String,
    existed: bool,
    byte_len: u64,
    blake3: Option<String>,
}

pub(super) fn begin(
    store: &mut LocalStoreSession,
    root: &Path,
    paths: impl IntoIterator<Item = String>,
) -> Result<MaterializationLease, String> {
    ensure_matching_root(store, root)?;
    let paths = paths
        .into_iter()
        .map(|path| {
            let normalized = normalized_relative_key(Path::new(&path))?;
            if normalized.is_empty() || normalized != path {
                return Err(format!(
                    "Layrs materialization path {path} is not normalized."
                ));
            }
            Ok(normalized)
        })
        .collect::<Result<BTreeSet<_>, String>>()?;
    let operation_id =
        super::unique_operation_id("working-tree", "materialize", &paths.len().to_string());
    let backup_dir = backup_dir(store, &operation_id);
    let mut entries = Vec::with_capacity(paths.len());

    for path in paths {
        let target = safe_target(root, &path)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                cleanup_dir(&backup_dir);
                return Err(format!(
                    "Layrs refused to materialize non-file path {}.",
                    target.display()
                ));
            }
            Ok(_) => {
                let bytes = fs::read(&target).map_err(|error| {
                    format!(
                        "Layrs could not back up working file {}: {error}",
                        target.display()
                    )
                })?;
                write_backup_file(&backup_dir, &path, &bytes)?;
                entries.push(BackupEntry {
                    path,
                    existed: true,
                    byte_len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                    blake3: Some(blake3::hash(&bytes).to_hex().to_string()),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                entries.push(BackupEntry {
                    path,
                    existed: false,
                    byte_len: 0,
                    blake3: None,
                });
            }
            Err(error) => {
                cleanup_dir(&backup_dir);
                return Err(format!(
                    "Layrs could not inspect working file {}: {error}",
                    target.display()
                ));
            }
        }
    }

    let journal = MaterializationJournal {
        schema: "layrs.client_materialization.v1".to_string(),
        operation_id: operation_id.clone(),
        root_path: root.display().to_string(),
        entries: entries.clone(),
    };
    let payload = serde_json::to_string(&journal)
        .map_err(|error| format!("Layrs could not encode materialization journal: {error}"))?;
    let now_ms = unix_now_ms();
    let insert = store
        .inner
        .repository_mut()
        .with_immediate_transaction(|transaction| {
            bump_generation(transaction, now_ms)?;
            transaction.execute(
                "INSERT INTO client_materialization_operations (
                     operation_id, state, payload_json, created_at_ms, updated_at_ms
                 ) VALUES (?1, 'applying', ?2, ?3, ?3)",
                params![operation_id, payload, now_ms],
            )?;
            Ok(())
        })
        .map_err(store_error);
    if insert.is_err() {
        cleanup_dir(&backup_dir);
    }
    insert?;

    Ok(MaterializationLease {
        operation_id,
        root: root.to_path_buf(),
        entries,
    })
}

pub(super) fn finish(
    store: &mut LocalStoreSession,
    lease: &MaterializationLease,
) -> Result<(), String> {
    set_state(store, &lease.operation_id, "applying", "completed")?;
    cleanup_dir(&backup_dir(store, &lease.operation_id));
    Ok(())
}

pub(super) fn rollback(
    store: &mut LocalStoreSession,
    lease: &MaterializationLease,
) -> Result<(), String> {
    restore_entries(store, &lease.root, &lease.operation_id, &lease.entries)?;
    set_state(store, &lease.operation_id, "applying", "rolled_back")?;
    cleanup_dir(&backup_dir(store, &lease.operation_id));
    Ok(())
}

pub(super) fn recover(store: &mut LocalStoreSession) -> Result<(), String> {
    let rows = store
        .inner
        .repository()
        .with_read(|connection| {
            let mut statement = connection.prepare(
                "SELECT operation_id, state, payload_json
                 FROM client_materialization_operations
                 ORDER BY created_at_ms, operation_id",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::from)?;
            Ok(rows)
        })
        .map_err(store_error)?;

    let root = store
        .layrs_dir
        .parent()
        .ok_or_else(|| "Layrs metadata directory has no working root.".to_string())?
        .to_path_buf();
    let mut referenced = BTreeSet::new();
    for (operation_id, state, payload) in rows {
        referenced.insert(backup_name(&operation_id));
        if state == "applying" {
            let journal: MaterializationJournal =
                serde_json::from_str(&payload).map_err(|error| {
                    format!("Layrs materialization journal {operation_id} is invalid: {error}")
                })?;
            if journal.operation_id != operation_id
                || journal.root_path != root.display().to_string()
            {
                return Err(format!(
                    "Layrs materialization journal {operation_id} does not match this Local Space."
                ));
            }
            restore_entries(store, &root, &operation_id, &journal.entries)?;
            set_state(store, &operation_id, "applying", "rolled_back")?;
        }
        cleanup_dir(&backup_dir(store, &operation_id));
    }
    cleanup_orphan_backups(store, &referenced)
}

fn set_state(
    store: &mut LocalStoreSession,
    operation_id: &str,
    expected: &str,
    next: &str,
) -> Result<(), String> {
    let now_ms = unix_now_ms();
    store
        .inner
        .repository_mut()
        .with_immediate_transaction(|transaction| {
            let changed = transaction.execute(
                "UPDATE client_materialization_operations
                 SET state = ?3, updated_at_ms = ?4
                 WHERE operation_id = ?1 AND state = ?2",
                params![operation_id, expected, next, now_ms],
            )?;
            if changed != 1 {
                return Err(StoreError::InvalidState(format!(
                    "materialization {operation_id} is not {expected}"
                )));
            }
            bump_generation(transaction, now_ms)?;
            Ok(())
        })
        .map_err(store_error)
}

fn restore_entries(
    store: &LocalStoreSession,
    root: &Path,
    operation_id: &str,
    entries: &[BackupEntry],
) -> Result<(), String> {
    let backup_dir = backup_dir(store, operation_id);
    for entry in entries.iter().rev() {
        let target = safe_target(root, &entry.path)?;
        if entry.existed {
            let source = safe_target(&backup_dir.join("files"), &entry.path)?;
            let bytes = fs::read(&source).map_err(|error| {
                format!(
                    "Layrs could not recover materialization backup {}: {error}",
                    source.display()
                )
            })?;
            let digest = blake3::hash(&bytes).to_hex().to_string();
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != entry.byte_len
                || entry.blake3.as_deref() != Some(digest.as_str())
            {
                return Err(format!(
                    "Layrs materialization backup {} failed BLAKE3 verification.",
                    source.display()
                ));
            }
            write_materialized_file(&target, &bytes)?;
        } else {
            remove_materialized_file(&target)?;
            prune_empty_parents(root, target.parent());
        }
    }
    Ok(())
}

pub(in crate::access_registry) fn write_materialized_file(
    path: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Layrs materialization path {} has no parent.",
            path.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Layrs could not create materialization directory {}: {error}",
            parent.display()
        )
    })?;
    let temp = parent.join(format!(
        ".layrs-materialize-{}-{}",
        std::process::id(),
        super::NEXT_OPERATION_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|error| format!("Layrs could not create {}: {error}", temp.display()))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "Layrs could not persist {}: {error}",
            temp.display()
        ));
    }
    drop(file);
    if let Err(error) = durable_replace(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "Layrs could not replace materialized file {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

pub(in crate::access_registry) fn remove_materialized_file(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Layrs could not remove materialized file {}: {error}",
            path.display()
        )),
    }
}

fn write_backup_file(root: &Path, key: &str, bytes: &[u8]) -> Result<(), String> {
    let path = safe_target(&root.join("files"), key)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Layrs could not create materialization backup: {error}"))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("Layrs could not create backup {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Layrs could not persist backup {}: {error}", path.display()))
}

fn ensure_matching_root(store: &LocalStoreSession, root: &Path) -> Result<(), String> {
    if root.join(".layrs") != store.layrs_dir {
        return Err(format!(
            "Layrs materialization root {} does not own {}.",
            root.display(),
            store.layrs_dir.display()
        ));
    }
    Ok(())
}

fn backup_dir(store: &LocalStoreSession, operation_id: &str) -> PathBuf {
    store
        .layrs_dir
        .join("metadata")
        .join(BACKUP_ROOT)
        .join(backup_name(operation_id))
}

fn backup_name(operation_id: &str) -> String {
    blake3::hash(operation_id.as_bytes()).to_hex().to_string()
}

fn safe_target(root: &Path, key: &str) -> Result<PathBuf, String> {
    let normalized = normalized_relative_key(Path::new(key))?;
    if normalized.is_empty() || normalized != key {
        return Err(format!("Layrs materialization path {key} is invalid."));
    }
    Ok(root.join(Path::new(key)))
}

fn cleanup_orphan_backups(
    store: &LocalStoreSession,
    referenced: &BTreeSet<String>,
) -> Result<(), String> {
    let root = store.layrs_dir.join("metadata").join(BACKUP_ROOT);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Layrs could not inspect materialization backups: {error}"
            ))
        }
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("Layrs could not inspect materialization backup: {error}"))?
            .path();
        let name = path.file_name().and_then(|name| name.to_str());
        if name.is_some_and(|name| !referenced.contains(name)) {
            cleanup_dir(&path);
        }
    }
    Ok(())
}

fn cleanup_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn prune_empty_parents(root: &Path, mut current: Option<&Path>) {
    while let Some(path) = current {
        if path == root || !path.starts_with(root) {
            break;
        }
        match fs::remove_dir(path) {
            Ok(()) => current = path.parent(),
            Err(_) => break,
        }
    }
}

#[cfg(windows)]
fn durable_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
    }
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn durable_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)?;
    if let Some(parent) = destination.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
