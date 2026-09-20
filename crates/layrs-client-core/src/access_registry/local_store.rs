use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_store_local::{
    LocalStore, LocalStoreConfig, LocalStoreSession as StoreSession, SessionMode, StoreError,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use serde::Serialize;

mod materialization;
mod migration;
mod projection;
#[cfg(test)]
mod tests;

pub(super) use materialization::{
    remove_materialized_file, write_materialized_file, MaterializationLease,
};

const CLIENT_LAYOUT_VERSION: i64 = 1;

static STORES: OnceLock<Mutex<BTreeMap<PathBuf, LocalStore>>> = OnceLock::new();
static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(0);

/// One non-reentrant lock and SQLite connection for a complete local API call.
pub(super) struct LocalStoreSession {
    layrs_dir: PathBuf,
    inner: StoreSession,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct NetworkLease {
    pub operation_id: String,
    pub idempotency_key: String,
    pub expected_generation: u64,
}

impl LocalStoreSession {
    pub(super) fn open_exclusive(layrs_dir: &Path) -> Result<Self, String> {
        migration::migrate_legacy_if_needed(layrs_dir)?;
        let store = cached_store(layrs_dir)?;
        let inner = store.session(SessionMode::Exclusive).map_err(store_error)?;
        let mut session = Self {
            layrs_dir: layrs_dir.to_path_buf(),
            inner,
        };
        session.ensure_client_schema()?;
        session.recover_materializations()?;
        Ok(session)
    }

    pub(super) fn read<T: DeserializeOwned>(&self, path: &Path) -> Result<T, String> {
        let key = self.document_key(path)?;
        let body = self
            .inner
            .repository()
            .with_read(|connection| {
                connection
                    .query_row(
                        "SELECT payload_json FROM client_documents WHERE path = ?1",
                        [&key],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(StoreError::from)
            })
            .map_err(store_error)?
            .ok_or_else(|| format!("Layrs metadata document {key} does not exist in SQLite."))?;
        serde_json::from_str(&body)
            .map_err(|error| format!("Layrs SQLite metadata document {key} is invalid: {error}"))
    }

    pub(super) fn read_value(&self, path: &Path) -> Result<serde_json::Value, String> {
        self.read(path)
    }

    pub(super) fn document_snapshot(&self) -> Result<BTreeMap<String, serde_json::Value>, String> {
        self.inner
            .repository()
            .with_read(|connection| {
                let mut statement = connection
                    .prepare("SELECT path, payload_json FROM client_documents ORDER BY path")?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                rows.into_iter()
                    .map(|(path, payload)| {
                        serde_json::from_str(&payload)
                            .map(|value| (path.clone(), value))
                            .map_err(|error| {
                                StoreError::CorruptMetadata(format!(
                                    "client document {path} is invalid: {error}"
                                ))
                            })
                    })
                    .collect()
            })
            .map_err(store_error)
    }

    pub(super) fn replace_document_snapshot(
        &mut self,
        documents: BTreeMap<String, serde_json::Value>,
    ) -> Result<u64, String> {
        let encoded = documents
            .into_iter()
            .map(|(path, value)| {
                let normalized = normalized_relative_key(Path::new(&path))?;
                if normalized != path {
                    return Err(format!(
                        "Layrs metadata snapshot path {path} is not normalized."
                    ));
                }
                let payload = serde_json::to_string(&value).map_err(|error| {
                    format!("Layrs could not encode SQLite metadata {path}: {error}")
                })?;
                Ok((path, payload))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let now_ms = unix_now_ms();
        let layrs_dir = self.layrs_dir.clone();
        self.inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                let generation = bump_generation(transaction, now_ms)?;
                transaction.execute("DELETE FROM client_documents", [])?;
                for (path, payload) in &encoded {
                    transaction.execute(
                        "INSERT INTO client_documents (
                             path, document_kind, payload_json, generation, updated_at_ms
                         ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            path,
                            document_kind(path),
                            payload,
                            u64_to_i64(generation)?,
                            now_ms
                        ],
                    )?;
                }
                projection::rebuild(transaction, &layrs_dir, now_ms)?;
                Ok(generation)
            })
            .map_err(store_error)
    }

    pub(super) fn exists(&self, path: &Path) -> Result<bool, String> {
        let key = self.document_key(path)?;
        self.inner
            .repository()
            .with_read(|connection| {
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM client_documents WHERE path = ?1)",
                        [&key],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(StoreError::from)
            })
            .map_err(store_error)
    }

    pub(super) fn write<T: Serialize + ?Sized>(
        &mut self,
        path: &Path,
        value: &T,
    ) -> Result<u64, String> {
        let key = self.document_key(path)?;
        let payload = serde_json::to_string(value)
            .map_err(|error| format!("Layrs could not encode SQLite metadata {key}: {error}"))?;
        let kind = document_kind(&key);
        let now_ms = unix_now_ms();
        let layrs_dir = self.layrs_dir.clone();
        self.inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                let generation = bump_generation(transaction, now_ms)?;
                transaction.execute(
                    "INSERT INTO client_documents (
                         path, document_kind, payload_json, generation, updated_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(path) DO UPDATE SET
                         document_kind = excluded.document_kind,
                         payload_json = excluded.payload_json,
                         generation = excluded.generation,
                         updated_at_ms = excluded.updated_at_ms",
                    params![key, kind, payload, u64_to_i64(generation)?, now_ms],
                )?;
                projection::rebuild(transaction, &layrs_dir, now_ms)?;
                Ok(generation)
            })
            .map_err(store_error)
    }

    pub(super) fn remove(&mut self, path: &Path) -> Result<bool, String> {
        let key = self.document_key(path)?;
        let now_ms = unix_now_ms();
        let layrs_dir = self.layrs_dir.clone();
        self.inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                let removed = transaction
                    .execute("DELETE FROM client_documents WHERE path = ?1", [&key])?
                    > 0;
                if removed {
                    bump_generation(transaction, now_ms)?;
                    projection::rebuild(transaction, &layrs_dir, now_ms)?;
                }
                Ok(removed)
            })
            .map_err(store_error)
    }

    pub(super) fn rename_prefix(&mut self, from: &Path, to: &Path) -> Result<usize, String> {
        let mut from_key = self.document_key(from)?;
        let mut to_key = self.document_key(to)?;
        if !from_key.ends_with('/') {
            from_key.push('/');
        }
        if !to_key.ends_with('/') {
            to_key.push('/');
        }
        let pattern = format!("{}%", escape_like_prefix(&from_key));
        let now_ms = unix_now_ms();
        let layrs_dir = self.layrs_dir.clone();
        self.inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                let rows = {
                    let mut statement = transaction.prepare(
                        "SELECT path FROM client_documents
                         WHERE path LIKE ?1 ESCAPE '\\' ORDER BY path",
                    )?;
                    let rows = statement.query_map([pattern], |row| row.get::<_, String>(0))?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                for source in &rows {
                    let suffix = source.strip_prefix(&from_key).ok_or_else(|| {
                        StoreError::InvalidState(format!(
                            "document {source} escaped rename prefix {from_key}"
                        ))
                    })?;
                    let destination = format!("{to_key}{suffix}");
                    transaction.execute(
                        "UPDATE client_documents
                         SET path = ?2, generation = generation + 1, updated_at_ms = ?3
                         WHERE path = ?1",
                        params![source, destination, now_ms],
                    )?;
                }
                if !rows.is_empty() {
                    bump_generation(transaction, now_ms)?;
                    projection::rebuild(transaction, &layrs_dir, now_ms)?;
                }
                Ok(rows.len())
            })
            .map_err(store_error)
    }

    pub(super) fn remove_listed<T: DeserializeOwned>(
        &mut self,
        directory: &Path,
        suffix: &str,
    ) -> Result<Vec<(String, T)>, String> {
        let rows = self.list(directory, suffix)?;
        if rows.is_empty() {
            return Ok(rows);
        }
        let now_ms = unix_now_ms();
        let layrs_dir = self.layrs_dir.clone();
        self.inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                for (path, _) in &rows {
                    transaction.execute("DELETE FROM client_documents WHERE path = ?1", [path])?;
                }
                bump_generation(transaction, now_ms)?;
                projection::rebuild(transaction, &layrs_dir, now_ms)?;
                Ok(())
            })
            .map_err(store_error)?;
        Ok(rows)
    }

    pub(super) fn list<T: DeserializeOwned>(
        &self,
        directory: &Path,
        suffix: &str,
    ) -> Result<Vec<(String, T)>, String> {
        let mut prefix = self.document_key(directory)?;
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        let pattern = format!("{}%", escape_like_prefix(&prefix));
        let rows = self
            .inner
            .repository()
            .with_read(|connection| {
                let mut statement = connection.prepare(
                    "SELECT path, payload_json FROM client_documents
                     WHERE path LIKE ?1 ESCAPE '\\' ORDER BY path",
                )?;
                let values = statement
                    .query_map([pattern], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(values)
            })
            .map_err(store_error)?;

        rows.into_iter()
            .filter(|(path, _)| path.ends_with(suffix))
            .map(|(path, body)| {
                serde_json::from_str(&body)
                    .map(|value| (path.clone(), value))
                    .map_err(|error| {
                        format!("Layrs SQLite metadata document {path} is invalid: {error}")
                    })
            })
            .collect()
    }

    pub(super) fn generation(&self) -> Result<u64, String> {
        self.inner
            .repository()
            .with_read(|connection| {
                connection
                    .query_row(
                        "SELECT generation FROM client_store_state WHERE singleton = 1",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(StoreError::from)
            })
            .map_err(store_error)
            .and_then(i64_to_u64)
    }

    pub(super) fn prepare_network(
        &mut self,
        space_id: &str,
        operation_kind: &str,
        payload: &serde_json::Value,
    ) -> Result<NetworkLease, String> {
        let now_ms = unix_now_ms();
        let expected_generation = self
            .generation()?
            .checked_add(1)
            .ok_or_else(|| "Layrs metadata generation overflowed.".to_string())?;
        let mut payload = payload.clone();
        payload
            .as_object_mut()
            .ok_or_else(|| "Layrs network journal payload must be an object.".to_string())?
            .insert(
                "targetGeneration".to_string(),
                serde_json::Value::from(expected_generation),
            );
        let payload_json = serde_json::to_string(&payload)
            .map_err(|error| format!("Layrs could not encode network journal: {error}"))?;
        let operation_id = unique_operation_id(space_id, operation_kind, &payload_json);
        let idempotency_key =
            blake3::hash(format!("{space_id}\0{operation_kind}\0{payload_json}").as_bytes())
                .to_hex()
                .to_string();
        let operation_id_for_tx = operation_id.clone();
        let idempotency_for_tx = idempotency_key.clone();

        let prepared_generation = self
            .inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                let generation = bump_generation(transaction, now_ms)?;
                if generation != expected_generation {
                    return Err(StoreError::GenerationConflict {
                        entity: "client network preparation".to_string(),
                        expected: expected_generation,
                        actual: Some(generation),
                    });
                }
                transaction.execute(
                    "INSERT INTO client_network_operations (
                         operation_id, space_id, operation_kind, idempotency_key,
                         expected_generation, state, payload_json, created_at_ms, updated_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'external_pending', ?6, ?7, ?7)",
                    params![
                        operation_id_for_tx,
                        space_id,
                        operation_kind,
                        idempotency_for_tx,
                        u64_to_i64(generation)?,
                        payload_json,
                        now_ms
                    ],
                )?;
                Ok(generation)
            })
            .map_err(store_error)?;
        debug_assert_eq!(prepared_generation, expected_generation);

        Ok(NetworkLease {
            operation_id,
            idempotency_key,
            expected_generation,
        })
    }

    pub(super) fn begin_network_finalize(&mut self, lease: &NetworkLease) -> Result<(), String> {
        let now_ms = unix_now_ms();
        let stale_generation = self
            .inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                let actual = current_generation(transaction)?;
                let expected = u64_to_i64(lease.expected_generation)?;
                if actual != expected {
                    transaction.execute(
                        "UPDATE client_network_operations
                         SET state = 'stale', updated_at_ms = ?2
                         WHERE operation_id = ?1 AND state = 'external_pending'",
                        params![lease.operation_id, now_ms],
                    )?;
                    return Ok(Some(actual));
                }
                let changed = transaction.execute(
                    "UPDATE client_network_operations
                     SET state = 'applying', updated_at_ms = ?2
                     WHERE operation_id = ?1
                       AND idempotency_key = ?3
                       AND expected_generation = ?4
                       AND state = 'external_pending'",
                    params![lease.operation_id, now_ms, lease.idempotency_key, expected],
                )?;
                if changed != 1 {
                    return Err(StoreError::InvalidState(format!(
                        "network operation {} is not pending",
                        lease.operation_id
                    )));
                }
                Ok(None)
            })
            .map_err(store_error)?;
        if stale_generation.is_some() {
            return Err(format!(
                "Layrs local state changed while the network request was in flight; operation {} was refused and pending work was preserved.",
                lease.operation_id
            ));
        }
        Ok(())
    }

    pub(super) fn amend_network(
        &mut self,
        lease: &NetworkLease,
        amendment: &serde_json::Value,
    ) -> Result<(), String> {
        let amendment_json = serde_json::to_string(amendment)
            .map_err(|error| format!("Layrs could not encode network journal update: {error}"))?;
        let now_ms = unix_now_ms();
        self.inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                let actual = current_generation(transaction)?;
                let expected = u64_to_i64(lease.expected_generation)?;
                if actual != expected {
                    return Err(StoreError::GenerationConflict {
                        entity: "client network generation".to_string(),
                        expected: lease.expected_generation,
                        actual: i64_to_u64(actual).ok(),
                    });
                }
                let changed = transaction.execute(
                    "UPDATE client_network_operations
                     SET payload_json = json_patch(payload_json, json(?2)),
                         updated_at_ms = ?3
                     WHERE operation_id = ?1
                       AND idempotency_key = ?4
                       AND state = 'external_pending'",
                    params![
                        lease.operation_id,
                        amendment_json,
                        now_ms,
                        lease.idempotency_key
                    ],
                )?;
                if changed != 1 {
                    return Err(StoreError::InvalidState(format!(
                        "network operation {} cannot be amended",
                        lease.operation_id
                    )));
                }
                Ok(())
            })
            .map_err(|error| match error {
                StoreError::GenerationConflict { .. } => format!(
                    "Layrs local state changed while the network request was in flight; operation {} was refused and pending work was preserved.",
                    lease.operation_id
                ),
                other => store_error(other),
            })
    }

    pub(super) fn finish_network(
        &mut self,
        lease: &NetworkLease,
        result: &serde_json::Value,
    ) -> Result<(), String> {
        let now_ms = unix_now_ms();
        let result_json = serde_json::to_string(result)
            .map_err(|error| format!("Layrs could not encode network result journal: {error}"))?;
        self.inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                let changed = transaction.execute(
                    "UPDATE client_network_operations
                     SET state = 'finalized',
                         payload_json = json_set(payload_json, '$.result', json(?3)),
                         updated_at_ms = ?2
                     WHERE operation_id = ?1 AND state = 'applying'",
                    params![lease.operation_id, now_ms, result_json],
                )?;
                if changed != 1 {
                    return Err(StoreError::InvalidState(format!(
                        "network operation {} did not finalize from applying",
                        lease.operation_id
                    )));
                }
                Ok(())
            })
            .map_err(store_error)
    }

    fn ensure_client_schema(&mut self) -> Result<(), String> {
        self.inner
            .repository_mut()
            .with_immediate_transaction(|transaction| {
                create_client_schema(transaction)?;
                Ok(())
            })
            .map_err(store_error)
    }

    pub(super) fn begin_materialization(
        &mut self,
        root: &Path,
        paths: impl IntoIterator<Item = String>,
    ) -> Result<MaterializationLease, String> {
        materialization::begin(self, root, paths)
    }

    pub(super) fn finish_materialization(
        &mut self,
        lease: &MaterializationLease,
    ) -> Result<(), String> {
        materialization::finish(self, lease)
    }

    pub(super) fn rollback_materialization(
        &mut self,
        lease: &MaterializationLease,
    ) -> Result<(), String> {
        materialization::rollback(self, lease)
    }

    fn recover_materializations(&mut self) -> Result<(), String> {
        materialization::recover(self)
    }

    pub(super) fn document_key(&self, path: &Path) -> Result<String, String> {
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.layrs_dir).map_err(|_| {
                format!(
                    "Layrs metadata path {} is outside {}.",
                    path.display(),
                    self.layrs_dir.display()
                )
            })?
        } else {
            path
        };
        normalized_relative_key(relative)
    }
}

pub(super) fn create_client_schema(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute_batch(include_str!("local_store/schema.sql"))?;
    let now_ms = unix_now_ms();
    transaction.execute(
        "INSERT INTO client_store_state (
             singleton, layout_version, generation, created_at_ms, updated_at_ms
         ) VALUES (1, ?1, 0, ?2, ?2)
         ON CONFLICT(singleton) DO NOTHING",
        params![CLIENT_LAYOUT_VERSION, now_ms],
    )?;
    let actual: i64 = transaction.query_row(
        "SELECT layout_version FROM client_store_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if actual != CLIENT_LAYOUT_VERSION {
        return Err(StoreError::InvalidState(format!(
            "client metadata layout {actual} is unsupported"
        )));
    }
    Ok(())
}

fn cached_store(layrs_dir: &Path) -> Result<LocalStore, String> {
    let key = absolute_store_key(layrs_dir)?;
    let stores = STORES.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut stores = stores
        .lock()
        .map_err(|_| "Layrs local store cache lock is poisoned.".to_string())?;
    if let Some(store) = stores.get(&key) {
        return Ok(store.clone());
    }

    let config = LocalStoreConfig::new(&key);
    let layout = config.layout();
    let store = if layout.sqlite_path.exists() {
        LocalStore::open(config)
    } else {
        LocalStore::init(config)
    }
    .map_err(|error| format!("Layrs could not initialize local SQLite store: {error}"))?;
    stores.insert(key, store.clone());
    Ok(store)
}

fn absolute_store_key(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| format!("Layrs could not resolve local store path: {error}"))
    }
}

fn normalized_relative_key(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Layrs metadata path {} escapes the local store.",
                    path.display()
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

fn document_kind(path: &str) -> &'static str {
    if path == "local-space.json" {
        "local_space"
    } else if path == "active-layer.json" {
        "active_layer"
    } else if path.ends_with("/steps.json") || path.contains("/steps/") {
        "step"
    } else if path.contains("/pending-publish/") {
        "pending_publish"
    } else if path.ends_with("sync-state.json") {
        "sync_state"
    } else if path == "scan-cache.json" {
        "scan_cache"
    } else if path.contains("weaves/") {
        "weave"
    } else if path.ends_with("access.json") {
        "access"
    } else {
        "metadata"
    }
}

fn escape_like_prefix(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn unique_operation_id(space_id: &str, operation_kind: &str, payload_json: &str) -> String {
    let sequence = NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let material = format!(
        "{space_id}\0{operation_kind}\0{payload_json}\0{}\0{nanos}\0{sequence}",
        std::process::id()
    );
    format!(
        "client-{operation_kind}-{}",
        &blake3::hash(material.as_bytes()).to_hex()[..32]
    )
}

fn bump_generation(transaction: &Transaction<'_>, now_ms: i64) -> Result<u64, StoreError> {
    transaction.execute(
        "UPDATE client_store_state
         SET generation = generation + 1, updated_at_ms = ?1
         WHERE singleton = 1",
        [now_ms],
    )?;
    i64_to_u64(current_generation(transaction)?).map_err(StoreError::InvalidState)
}

fn current_generation(transaction: &Transaction<'_>) -> Result<i64, StoreError> {
    transaction
        .query_row(
            "SELECT generation FROM client_store_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
}

fn u64_to_i64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidState("metadata generation exceeds SQLite INTEGER".into()))
}

fn i64_to_u64(value: i64) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("negative SQLite metadata generation {value}"))
}

fn unix_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn store_error(error: StoreError) -> String {
    format!("Layrs local store error: {error}")
}

#[cfg(test)]
pub(super) fn forget_cached_store(layrs_dir: &Path) {
    if let Some(stores) = STORES.get() {
        if let Ok(mut stores) = stores.lock() {
            let key = absolute_store_key(layrs_dir).unwrap_or_else(|_| layrs_dir.to_path_buf());
            stores.remove(&key);
        }
    }
}
