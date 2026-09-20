use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use fs2::FileExt;
use layrs_store_local::{LocalStore, LocalStoreConfig, SessionMode, StoreError};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::object_store::{
    read_file_object_bytes, read_file_object_manifest, FileObjectFile, FILE_OBJECT_SCHEMA,
};

use super::super::{
    blake3_id, tree_id_for_files, validate_blake3_id, FileSnapshotEntry, TreeObjectFile,
    TREE_OBJECT_SCHEMA,
};
use super::{create_client_schema, document_kind, projection, unix_now_ms};

const BACKUP_DIR: &str = "legacy-json-v1";
const BACKUP_MANIFEST_SCHEMA: &str = "layrs.legacy_json_backup.v1";
const STAGING_PREFIX: &str = ".metadata-migration-";
const BACKUP_STAGING_PREFIX: &str = ".legacy-json-v1-migration-";
const IMPORT_MARKER: &str = "sqlite-import-required";

#[derive(Clone, Debug)]
struct LegacyDocument {
    key: String,
    bytes: Vec<u8>,
    value: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    schema: String,
    created_at_ms: i64,
    root_blake3: String,
    files: Vec<BackupFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupFile {
    path: String,
    byte_len: u64,
    blake3: String,
}

struct MigrationLock {
    file: File,
}

impl MigrationLock {
    fn acquire(layrs_dir: &Path) -> Result<Self, String> {
        let lock_root = std::env::temp_dir().join("layrs-migration-locks");
        fs::create_dir_all(&lock_root).map_err(|error| {
            format!("Layrs could not create the migration lock directory: {error}")
        })?;
        let identity = blake3::hash(layrs_dir.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        let path = lock_root.join(format!("{identity}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                format!(
                    "Layrs could not open migration lock {}: {error}",
                    path.display()
                )
            })?;
        file.lock_exclusive().map_err(|error| {
            format!("Layrs could not lock migration {}: {error}", path.display())
        })?;
        Ok(Self { file })
    }
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

struct StagingGuard {
    path: PathBuf,
    installed: bool,
}

impl StagingGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            installed: false,
        }
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if !self.installed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub(super) fn migrate_legacy_if_needed(layrs_dir: &Path) -> Result<(), String> {
    let final_metadata = layrs_dir.join("metadata");
    let final_sqlite = final_metadata.join("layrs.sqlite3");
    let _lock = MigrationLock::acquire(layrs_dir)?;
    if final_sqlite.is_file() {
        relocate_embedded_backup(layrs_dir, &final_metadata)?;
        let backup_root = layrs_dir.join(BACKUP_DIR);
        let manifest = if backup_root.exists() {
            verify_installed_backup(layrs_dir)?
        } else {
            let documents = collect_legacy_documents(layrs_dir)?;
            validate_merkle_closure(layrs_dir, &documents)?;
            let manifest = install_backup_atomically(layrs_dir, &documents)?;
            import_into_existing_sqlite(layrs_dir, &documents, &manifest)?;
            clear_import_marker(layrs_dir)?;
            manifest
        };
        if backup_root.join(IMPORT_MARKER).exists() {
            let documents = documents_from_backup(layrs_dir, &manifest)?;
            validate_merkle_closure(layrs_dir, &documents)?;
            import_into_existing_sqlite(layrs_dir, &documents, &manifest)?;
            clear_import_marker(layrs_dir)?;
        }
        return remove_migrated_legacy_documents(layrs_dir, &manifest);
    }
    if final_metadata.exists() {
        return Err(format!(
            "Layrs found an incomplete metadata installation at {} and refused to overwrite it.",
            final_metadata.display()
        ));
    }

    recover_staging_directories(layrs_dir)?;
    let backup_root = layrs_dir.join(BACKUP_DIR);
    let documents = if backup_root.exists() {
        let manifest = verify_backup_root(&backup_root)?;
        documents_from_backup(layrs_dir, &manifest)?
    } else {
        collect_legacy_documents(layrs_dir)?
    };
    validate_merkle_closure(layrs_dir, &documents)?;
    let manifest = install_backup_atomically(layrs_dir, &documents)?;
    install_atomically(layrs_dir, &final_metadata, &documents, &manifest)?;
    clear_import_marker(layrs_dir)?;
    remove_migrated_legacy_documents(layrs_dir, &manifest)
}

fn install_atomically(
    layrs_dir: &Path,
    final_metadata: &Path,
    documents: &[LegacyDocument],
    manifest: &BackupManifest,
) -> Result<(), String> {
    let staging_name = format!("{STAGING_PREFIX}{}-{}", std::process::id(), unique_nonce());
    let staging_metadata = layrs_dir.join(&staging_name);
    let mut guard = StagingGuard::new(staging_metadata.clone());
    let mut config = LocalStoreConfig::new(layrs_dir);
    config.metadata_dir_name = staging_name;
    let store = LocalStore::init(config)
        .map_err(|error| format!("Layrs could not create staged SQLite metadata: {error}"))?;

    let mut session = store.session(SessionMode::Exclusive).map_err(store_error)?;
    session
        .repository_mut()
        .with_immediate_transaction(|transaction| {
            create_client_schema(transaction)?;
            let generation = if documents.is_empty() { 0i64 } else { 1i64 };
            for document in documents {
                let payload = serde_json::to_string(&document.value).map_err(|error| {
                    StoreError::CorruptMetadata(format!(
                        "legacy document {} could not be encoded: {error}",
                        document.key
                    ))
                })?;
                transaction.execute(
                    "INSERT INTO client_documents (
                         path, document_kind, payload_json, generation, updated_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        document.key,
                        document_kind(&document.key),
                        payload,
                        generation,
                        manifest.created_at_ms
                    ],
                )?;
            }
            let created_at_ms: i64 = transaction.query_row(
                "SELECT created_at_ms FROM client_store_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )?;
            let installed_at_ms = manifest.created_at_ms.max(created_at_ms);
            transaction.execute(
                "UPDATE client_store_state
                 SET generation = ?1, legacy_backup_path = ?2, updated_at_ms = ?3
                 WHERE singleton = 1",
                params![generation, BACKUP_DIR, installed_at_ms],
            )?;
            projection::rebuild(transaction, layrs_dir, installed_at_ms)?;
            Ok(())
        })
        .map_err(store_error)?;
    session.checkpoint_full().map_err(store_error)?;
    drop(session);
    drop(store);

    sync_sqlite(&staging_metadata)?;
    fault_before_install()?;
    durable_rename(&staging_metadata, final_metadata).map_err(|error| {
        format!(
            "Layrs could not atomically install SQLite metadata {}: {error}",
            final_metadata.display()
        )
    })?;
    guard.installed = true;
    verify_installed_backup(layrs_dir).map(|_| ())
}

fn collect_legacy_documents(layrs_dir: &Path) -> Result<Vec<LegacyDocument>, String> {
    let mut paths = Vec::new();
    collect_json_paths(layrs_dir, layrs_dir, &mut paths)?;
    paths.sort();
    paths
        .into_iter()
        .map(|source| {
            let key = relative_key(layrs_dir, &source)?;
            let bytes = fs::read(&source).map_err(|error| {
                format!(
                    "Layrs could not read legacy JSON {}: {error}",
                    source.display()
                )
            })?;
            let value = serde_json::from_slice(&bytes).map_err(|error| {
                format!("Layrs legacy JSON {} is invalid: {error}", source.display())
            })?;
            Ok(LegacyDocument { key, bytes, value })
        })
        .collect()
}

fn collect_json_paths(root: &Path, current: &Path, paths: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(current).map_err(|error| {
        format!(
            "Layrs could not inspect legacy metadata {}: {error}",
            current.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Layrs could not inspect legacy metadata {}: {error}",
                current.display()
            )
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "Layrs could not inspect legacy path {}: {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Layrs refused legacy metadata through symbolic link {}.",
                path.display()
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("Layrs legacy path {} escapes the store.", path.display()))?;
        let first = relative
            .components()
            .next()
            .and_then(|component| match component {
                Component::Normal(value) => value.to_str(),
                _ => None,
            });
        if matches!(first, Some("objects" | "metadata" | "tmp" | BACKUP_DIR))
            || first.is_some_and(|name| {
                name.starts_with(STAGING_PREFIX) || name.starts_with(BACKUP_STAGING_PREFIX)
            })
        {
            continue;
        }
        if metadata.is_dir() {
            collect_json_paths(root, &path, paths)?;
        } else if metadata.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("json")
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn validate_merkle_closure(layrs_dir: &Path, documents: &[LegacyDocument]) -> Result<(), String> {
    let mut tree_ids = BTreeSet::new();
    let mut snapshots = BTreeMap::<String, FileSnapshotEntry>::new();
    for document in documents {
        collect_merkle_references(&document.value, &mut tree_ids, &mut snapshots)?;
    }
    for snapshot in snapshots.values() {
        validate_snapshot(layrs_dir, snapshot)?;
    }
    for tree_id in tree_ids {
        validate_tree(layrs_dir, &tree_id)?;
    }
    Ok(())
}

fn collect_merkle_references(
    value: &Value,
    tree_ids: &mut BTreeSet<String>,
    snapshots: &mut BTreeMap<String, FileSnapshotEntry>,
) -> Result<(), String> {
    if let Some(object) = value.as_object() {
        for key in ["rootTreeId", "baseTreeId", "preWeaveTargetTreeId"] {
            if let Some(tree_id) = object.get(key).and_then(Value::as_str) {
                validate_blake3_id(tree_id)?;
                tree_ids.insert(tree_id.to_string());
            }
        }
        if object.contains_key("path")
            && object.contains_key("object")
            && object.contains_key("hash")
        {
            let snapshot: FileSnapshotEntry = serde_json::from_value(value.clone())
                .map_err(|error| format!("Layrs legacy file snapshot is invalid: {error}"))?;
            snapshots.insert(snapshot.hash.clone(), snapshot);
        }
        for child in object.values() {
            collect_merkle_references(child, tree_ids, snapshots)?;
        }
    } else if let Some(array) = value.as_array() {
        for child in array {
            collect_merkle_references(child, tree_ids, snapshots)?;
        }
    }
    Ok(())
}

fn validate_tree(layrs_dir: &Path, tree_id: &str) -> Result<(), String> {
    validate_blake3_id(tree_id)?;
    let path = layrs_dir
        .join("objects")
        .join("trees")
        .join(format!("{}.json", object_stem(tree_id)));
    let bytes = fs::read(&path).map_err(|error| {
        format!(
            "Layrs legacy tree object {} is missing or unreadable: {error}",
            path.display()
        )
    })?;
    let tree: TreeObjectFile = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "Layrs legacy tree object {} is invalid: {error}",
            path.display()
        )
    })?;
    if tree.schema != TREE_OBJECT_SCHEMA || tree.tree_id != tree_id {
        return Err(format!(
            "Layrs legacy tree object {} has inconsistent identity.",
            path.display()
        ));
    }
    if tree_id_for_files(&tree.files) != tree_id {
        return Err(format!(
            "Layrs legacy tree object {tree_id} failed BLAKE3 verification."
        ));
    }
    for snapshot in &tree.files {
        validate_snapshot(layrs_dir, snapshot)?;
    }
    Ok(())
}

fn validate_snapshot(layrs_dir: &Path, snapshot: &FileSnapshotEntry) -> Result<(), String> {
    validate_blake3_id(&snapshot.hash)?;
    let object_path = safe_object_path(layrs_dir, &snapshot.object)?;
    if snapshot.object.starts_with("objects/files/") {
        let manifest: FileObjectFile =
            read_file_object_manifest(layrs_dir, Path::new(&snapshot.object))?;
        if manifest.schema != FILE_OBJECT_SCHEMA
            || manifest.hash != snapshot.hash
            || manifest.size != snapshot.size
        {
            return Err(format!(
                "Layrs legacy file object {} has inconsistent metadata.",
                object_path.display()
            ));
        }
        let bytes = read_file_object_bytes(layrs_dir, &manifest)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != snapshot.size {
            return Err(format!(
                "Layrs legacy file object {} has the wrong size.",
                object_path.display()
            ));
        }
    } else {
        let bytes = fs::read(&object_path).map_err(|error| {
            format!(
                "Layrs legacy object {} is missing or unreadable: {error}",
                object_path.display()
            )
        })?;
        if blake3_id(&bytes) != snapshot.hash
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) != snapshot.size
        {
            return Err(format!(
                "Layrs legacy object {} failed BLAKE3 verification.",
                object_path.display()
            ));
        }
    }
    Ok(())
}

fn safe_object_path(layrs_dir: &Path, key: &str) -> Result<PathBuf, String> {
    if !key.starts_with("objects/") || key.contains('\\') {
        return Err(format!("Layrs legacy object path {key} is invalid."));
    }
    let mut path = layrs_dir.to_path_buf();
    for segment in key.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") {
            return Err(format!("Layrs legacy object path {key} is invalid."));
        }
        path.push(segment);
    }
    Ok(path)
}

fn install_backup_atomically(
    layrs_dir: &Path,
    documents: &[LegacyDocument],
) -> Result<BackupManifest, String> {
    let backup_root = layrs_dir.join(BACKUP_DIR);
    if backup_root.exists() {
        let manifest = verify_backup_root(&backup_root)?;
        verify_backup_matches_documents(&manifest, documents)?;
        return Ok(manifest);
    }

    let staging = layrs_dir.join(format!(
        "{BACKUP_STAGING_PREFIX}{}-{}",
        std::process::id(),
        unique_nonce()
    ));
    let mut guard = StagingGuard::new(staging.clone());
    let manifest = write_backup(&staging, documents)?;
    verify_backup_root(&staging)?;
    durable_rename(&staging, &backup_root).map_err(|error| {
        format!(
            "Layrs could not atomically install legacy backup {}: {error}",
            backup_root.display()
        )
    })?;
    guard.installed = true;
    let installed = verify_backup_root(&backup_root)?;
    verify_backup_matches_documents(&installed, documents)?;
    Ok(manifest)
}

fn write_backup(
    backup_root: &Path,
    documents: &[LegacyDocument],
) -> Result<BackupManifest, String> {
    let files_root = backup_root.join("files");
    fs::create_dir_all(&files_root).map_err(|error| {
        format!(
            "Layrs could not create legacy backup {}: {error}",
            files_root.display()
        )
    })?;
    let mut files = Vec::with_capacity(documents.len());
    for document in documents {
        let destination = backup_path(&files_root, &document.key)?;
        write_durable_file(&destination, &document.bytes)?;
        files.push(BackupFile {
            path: document.key.clone(),
            byte_len: u64::try_from(document.bytes.len()).unwrap_or(u64::MAX),
            blake3: blake3::hash(&document.bytes).to_hex().to_string(),
        });
    }
    let created_at_ms = unix_now_ms();
    let root_blake3 = backup_root_digest(&files);
    let manifest = BackupManifest {
        schema: BACKUP_MANIFEST_SCHEMA.to_string(),
        created_at_ms,
        root_blake3,
        files,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("Layrs could not encode legacy backup manifest: {error}"))?;
    write_durable_file(&backup_root.join("manifest.json"), &bytes)?;
    write_durable_file(
        &backup_root.join(IMPORT_MARKER),
        b"SQLite import must complete before legacy JSON is retired.\n",
    )?;
    Ok(manifest)
}

fn verify_installed_backup(layrs_dir: &Path) -> Result<BackupManifest, String> {
    let backup_root = layrs_dir.join(BACKUP_DIR);
    if !backup_root.exists() {
        return Err(format!(
            "Layrs SQLite metadata exists without its legacy backup at {}.",
            backup_root.display()
        ));
    }
    verify_backup_root(&backup_root)
}

fn relocate_embedded_backup(layrs_dir: &Path, metadata_dir: &Path) -> Result<(), String> {
    let backup_root = layrs_dir.join(BACKUP_DIR);
    if backup_root.exists() {
        return Ok(());
    }
    let embedded = metadata_dir.join(BACKUP_DIR);
    if !embedded.exists() {
        return Ok(());
    }
    verify_backup_root(&embedded)?;
    durable_rename(&embedded, &backup_root).map_err(|error| {
        format!(
            "Layrs could not relocate legacy backup to {}: {error}",
            backup_root.display()
        )
    })
}

fn verify_backup_root(backup_root: &Path) -> Result<BackupManifest, String> {
    let manifest_path = backup_root.join("manifest.json");
    let body = fs::read(&manifest_path).map_err(|error| {
        format!(
            "Layrs could not read legacy backup manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let manifest: BackupManifest = serde_json::from_slice(&body).map_err(|error| {
        format!(
            "Layrs legacy backup manifest {} is invalid: {error}",
            manifest_path.display()
        )
    })?;
    if manifest.schema != BACKUP_MANIFEST_SCHEMA
        || manifest.root_blake3 != backup_root_digest(&manifest.files)
    {
        return Err("Layrs legacy backup manifest failed BLAKE3 verification.".to_string());
    }
    for file in &manifest.files {
        let path = backup_path(&backup_root.join("files"), &file.path)?;
        let bytes = fs::read(&path).map_err(|error| {
            format!(
                "Layrs legacy backup file {} is missing: {error}",
                path.display()
            )
        })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != file.byte_len
            || blake3::hash(&bytes).to_hex().as_str() != file.blake3
        {
            return Err(format!(
                "Layrs legacy backup file {} failed BLAKE3 verification.",
                path.display()
            ));
        }
    }
    Ok(manifest)
}

fn verify_backup_matches_documents(
    manifest: &BackupManifest,
    documents: &[LegacyDocument],
) -> Result<(), String> {
    if manifest.files.len() != documents.len() {
        return Err(
            "Layrs legacy backup does not match the metadata selected for migration.".to_string(),
        );
    }
    for (file, document) in manifest.files.iter().zip(documents) {
        if file.path != document.key
            || file.byte_len != u64::try_from(document.bytes.len()).unwrap_or(u64::MAX)
            || file.blake3 != blake3::hash(&document.bytes).to_hex().as_str()
        {
            return Err(format!(
                "Layrs legacy backup entry {} does not match the source metadata.",
                file.path
            ));
        }
    }
    Ok(())
}

fn documents_from_backup(
    layrs_dir: &Path,
    manifest: &BackupManifest,
) -> Result<Vec<LegacyDocument>, String> {
    let files_root = layrs_dir.join(BACKUP_DIR).join("files");
    manifest
        .files
        .iter()
        .map(|file| {
            let path = backup_path(&files_root, &file.path)?;
            let bytes = fs::read(&path).map_err(|error| {
                format!(
                    "Layrs could not read recovery backup {}: {error}",
                    path.display()
                )
            })?;
            let value = serde_json::from_slice(&bytes).map_err(|error| {
                format!(
                    "Layrs recovery backup {} is invalid: {error}",
                    path.display()
                )
            })?;
            Ok(LegacyDocument {
                key: file.path.clone(),
                bytes,
                value,
            })
        })
        .collect()
}

fn import_into_existing_sqlite(
    layrs_dir: &Path,
    documents: &[LegacyDocument],
    manifest: &BackupManifest,
) -> Result<(), String> {
    let store = LocalStore::open(LocalStoreConfig::new(layrs_dir))
        .map_err(|error| format!("Layrs could not open existing SQLite metadata: {error}"))?;
    let mut session = store.session(SessionMode::Exclusive).map_err(store_error)?;
    session
        .repository_mut()
        .with_immediate_transaction(|transaction| {
            create_client_schema(transaction)?;
            let existing: i64 =
                transaction.query_row("SELECT COUNT(*) FROM client_documents", [], |row| {
                    row.get(0)
                })?;
            if existing == 0 {
                let generation = if documents.is_empty() { 0i64 } else { 1i64 };
                for document in documents {
                    let payload = serde_json::to_string(&document.value).map_err(|error| {
                        StoreError::CorruptMetadata(format!(
                            "backup document {} could not be encoded: {error}",
                            document.key
                        ))
                    })?;
                    transaction.execute(
                        "INSERT INTO client_documents (
                             path, document_kind, payload_json, generation, updated_at_ms
                         ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            document.key,
                            document_kind(&document.key),
                            payload,
                            generation,
                            manifest.created_at_ms
                        ],
                    )?;
                }
                let created_at_ms: i64 = transaction.query_row(
                    "SELECT created_at_ms FROM client_store_state WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )?;
                let installed_at_ms = manifest.created_at_ms.max(created_at_ms);
                transaction.execute(
                    "UPDATE client_store_state
                     SET generation = ?1, legacy_backup_path = ?2, updated_at_ms = ?3
                     WHERE singleton = 1",
                    params![generation, BACKUP_DIR, installed_at_ms],
                )?;
                projection::rebuild(transaction, layrs_dir, installed_at_ms)?;
            }
            Ok(())
        })
        .map_err(store_error)?;
    session.checkpoint_full().map_err(store_error)
}

fn clear_import_marker(layrs_dir: &Path) -> Result<(), String> {
    let path = layrs_dir.join(BACKUP_DIR).join(IMPORT_MARKER);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Layrs could not complete backup import marker {}: {error}",
            path.display()
        )),
    }
}

fn backup_root_digest(files: &[BackupFile]) -> String {
    let mut material = Vec::new();
    for file in files {
        material.extend_from_slice(file.path.as_bytes());
        material.push(0);
        material.extend_from_slice(file.blake3.as_bytes());
        material.push(0);
        material.extend_from_slice(file.byte_len.to_string().as_bytes());
        material.push(b'\n');
    }
    blake3::hash(&material).to_hex().to_string()
}

fn backup_path(root: &Path, key: &str) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for segment in key.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") || segment.contains('\\') {
            return Err(format!("Layrs legacy backup path {key} is invalid."));
        }
        path.push(segment);
    }
    Ok(path)
}

fn write_durable_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Layrs could not create backup directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "Layrs could not create backup file {}: {error}",
                path.display()
            )
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "Layrs could not persist backup file {}: {error}",
                path.display()
            )
        })
}

fn sync_sqlite(metadata_dir: &Path) -> Result<(), String> {
    let path = metadata_dir.join("layrs.sqlite3");
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            format!(
                "Layrs could not sync migration artifact {}: {error}",
                path.display()
            )
        })
}

fn recover_staging_directories(layrs_dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(layrs_dir)
        .map_err(|error| format!("Layrs could not inspect migration recovery state: {error}"))?
    {
        let path = entry
            .map_err(|error| format!("Layrs could not inspect migration recovery state: {error}"))?
            .path();
        let matches = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(STAGING_PREFIX) || name.starts_with(BACKUP_STAGING_PREFIX)
            });
        if matches && path.is_dir() {
            fs::remove_dir_all(&path).map_err(|error| {
                format!(
                    "Layrs could not recover stale migration {}: {error}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn remove_migrated_legacy_documents(
    layrs_dir: &Path,
    manifest: &BackupManifest,
) -> Result<(), String> {
    for file in &manifest.files {
        let path = backup_path(layrs_dir, &file.path)?;
        match fs::remove_file(&path) {
            Ok(()) => prune_empty_legacy_parents(layrs_dir, path.parent()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Layrs could not retire migrated legacy metadata {}: {error}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn prune_empty_legacy_parents(layrs_dir: &Path, mut current: Option<&Path>) {
    while let Some(path) = current {
        if path == layrs_dir || !path.starts_with(layrs_dir) {
            break;
        }
        match fs::remove_dir(path) {
            Ok(()) => current = path.parent(),
            Err(_) => break,
        }
    }
}

fn relative_key(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("Layrs legacy path {} escapes the store.", path.display()))?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            _ => return Err(format!("Layrs legacy path {} is invalid.", path.display())),
        }
    }
    Ok(parts.join("/"))
}

fn object_stem(object_id: &str) -> &str {
    object_id.strip_prefix("blake3:").unwrap_or(object_id)
}

fn unique_nonce() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{nanos}-{sequence}")
}

fn store_error(error: StoreError) -> String {
    format!("Layrs local store migration error: {error}")
}

#[cfg(test)]
thread_local! {
    static FAIL_BEFORE_INSTALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(super) fn fail_before_install_once() {
    FAIL_BEFORE_INSTALL.with(|fault| fault.set(true));
}

fn fault_before_install() -> Result<(), String> {
    #[cfg(test)]
    if FAIL_BEFORE_INSTALL.with(|fault| fault.replace(false)) {
        return Err("injected migration fault before atomic install".to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn durable_rename(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

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
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn durable_rename(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)?;
    if let Some(parent) = destination.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}
