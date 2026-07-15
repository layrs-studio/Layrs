//! Reusable `BEGIN IMMEDIATE` metadata repository operations.

use std::path::{Component, Path};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::error::{StoreError, StoreResult};
use crate::fault::{self, FaultPoint};

pub struct MetadataRepository {
    connection: Connection,
    writable: bool,
}

impl MetadataRepository {
    pub(crate) fn new(connection: Connection, writable: bool) -> Self {
        Self {
            connection,
            writable,
        }
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn with_read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> StoreResult<T>,
    ) -> StoreResult<T> {
        operation(&self.connection)
    }

    /// Runs a metadata mutation under `BEGIN IMMEDIATE`.
    pub fn with_immediate_transaction<T>(
        &mut self,
        operation: impl FnOnce(&Transaction<'_>) -> StoreResult<T>,
    ) -> StoreResult<T> {
        if !self.writable {
            return Err(StoreError::ReadOnlySession);
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = operation(&transaction)?;
        fault::hit(FaultPoint::BeforeMetadataCommit)?;
        transaction.commit()?;
        fault::hit(FaultPoint::AfterMetadataCommit)?;
        Ok(result)
    }

    pub fn insert_space(&mut self, space: &SpaceRecord) -> StoreResult<()> {
        validate_identifier("space_id", &space.space_id)?;
        if space.canonical_root.trim().is_empty() {
            return Err(StoreError::InvalidState(
                "space canonical_root must not be empty".into(),
            ));
        }
        self.with_immediate_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO local_spaces (
                    space_id, canonical_root, display_name, case_sensitive, identity_token,
                    state, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    space.space_id,
                    space.canonical_root,
                    space.display_name,
                    space.case_sensitive,
                    space.identity_token,
                    space.state,
                    space.created_at_ms,
                    space.updated_at_ms,
                ],
            )?;
            Ok(())
        })
    }

    pub fn insert_layer(&mut self, layer: &LayerRecord) -> StoreResult<()> {
        validate_identifier("layer_id", &layer.layer_id)?;
        validate_identifier("space_id", &layer.space_id)?;
        self.with_immediate_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO layers (
                    layer_id, space_id, name, layer_kind, state, base_layer_id,
                    timeline_active, generation, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    layer.layer_id,
                    layer.space_id,
                    layer.name,
                    layer.layer_kind,
                    layer.state,
                    layer.base_layer_id,
                    layer.timeline_active,
                    to_i64(layer.generation, "layer generation")?,
                    layer.created_at_ms,
                    layer.updated_at_ms,
                ],
            )?;
            Ok(())
        })
    }

    pub fn insert_object(&mut self, object: &ObjectRecord) -> StoreResult<()> {
        validate_identifier("object_id", &object.object_id)?;
        validate_relative_path(&object.relative_path)?;
        self.with_immediate_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO object_index (
                    object_id, algorithm, byte_len, object_kind, relative_path,
                    state, verified_at_ms, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    object.object_id,
                    object.algorithm,
                    to_i64(object.byte_len, "object byte length")?,
                    object.object_kind,
                    object.relative_path,
                    object.state,
                    object.verified_at_ms,
                    object.created_at_ms,
                ],
            )?;
            Ok(())
        })
    }

    pub fn append_step(&mut self, step: &StepRecord, paths: &[StepPathRecord]) -> StoreResult<()> {
        validate_identifier("step_id", &step.step_id)?;
        for path in paths {
            if path.step_id != step.step_id {
                return Err(StoreError::InvalidState(format!(
                    "step path {} belongs to a different step",
                    path.path
                )));
            }
            validate_relative_path(&path.path)?;
        }

        self.with_immediate_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO steps (
                    step_id, space_id, layer_id, parent_step_id, root_object_id,
                    sequence, created_at_ms, authored_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    step.step_id,
                    step.space_id,
                    step.layer_id,
                    step.parent_step_id,
                    step.root_object_id,
                    to_i64(step.sequence, "step sequence")?,
                    step.created_at_ms,
                    step.authored_at,
                ],
            )?;
            for path in paths {
                transaction.execute(
                    "INSERT INTO step_paths (
                        step_id, path, object_id, entry_kind, executable
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        path.step_id,
                        path.path,
                        path.object_id,
                        path.entry_kind,
                        path.executable,
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn load_ref(&self, space_id: &str, ref_name: &str) -> StoreResult<Option<RefRecord>> {
        let raw = self
            .connection
            .query_row(
                "SELECT space_id, ref_name, ref_kind, layer_id, step_id, generation, updated_at_ms
                 FROM refs WHERE space_id = ?1 AND ref_name = ?2",
                params![space_id, ref_name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                },
            )
            .optional()?;
        raw.map(ref_from_raw).transpose()
    }

    pub fn compare_and_swap_ref(&mut self, update: &RefUpdate) -> StoreResult<RefRecord> {
        validate_identifier("space_id", &update.space_id)?;
        validate_identifier("ref_name", &update.ref_name)?;
        let expected = to_i64(update.expected_generation, "expected ref generation")?;
        let next = expected
            .checked_add(1)
            .ok_or_else(|| StoreError::InvalidState("ref generation overflow".to_string()))?;
        let entity = format!("ref {}/{}", update.space_id, update.ref_name);

        self.with_immediate_transaction(|transaction| {
            let actual: Option<i64> = transaction
                .query_row(
                    "SELECT generation FROM refs WHERE space_id = ?1 AND ref_name = ?2",
                    params![update.space_id, update.ref_name],
                    |row| row.get(0),
                )
                .optional()?;

            match actual {
                None if expected == 0 => {
                    transaction.execute(
                        "INSERT INTO refs (
                            space_id, ref_name, ref_kind, layer_id, step_id,
                            generation, updated_at_ms
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            update.space_id,
                            update.ref_name,
                            update.ref_kind,
                            update.layer_id,
                            update.step_id,
                            next,
                            update.updated_at_ms,
                        ],
                    )?;
                }
                Some(current) if current == expected => {
                    let changed = transaction.execute(
                        "UPDATE refs
                         SET ref_kind = ?3, layer_id = ?4, step_id = ?5,
                             generation = ?6, updated_at_ms = ?7
                         WHERE space_id = ?1 AND ref_name = ?2 AND generation = ?8",
                        params![
                            update.space_id,
                            update.ref_name,
                            update.ref_kind,
                            update.layer_id,
                            update.step_id,
                            next,
                            update.updated_at_ms,
                            expected,
                        ],
                    )?;
                    if changed != 1 {
                        return Err(StoreError::GenerationConflict {
                            entity: entity.clone(),
                            expected: update.expected_generation,
                            actual: actual.map(to_u64).transpose()?,
                        });
                    }
                }
                _ => {
                    return Err(StoreError::GenerationConflict {
                        entity: entity.clone(),
                        expected: update.expected_generation,
                        actual: actual.map(to_u64).transpose()?,
                    });
                }
            }

            Ok(RefRecord {
                space_id: update.space_id.clone(),
                ref_name: update.ref_name.clone(),
                ref_kind: update.ref_kind.clone(),
                layer_id: update.layer_id.clone(),
                step_id: update.step_id.clone(),
                generation: to_u64(next)?,
                updated_at_ms: update.updated_at_ms,
            })
        })
    }

    pub fn insert_pending_publish(&mut self, pending: &PendingPublishRecord) -> StoreResult<()> {
        validate_identifier("publish_id", &pending.publish_id)?;
        self.with_immediate_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO pending_publish (
                    publish_id, space_id, layer_id, step_id, operation_id,
                    idempotency_key, expected_generation, state, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    pending.publish_id,
                    pending.space_id,
                    pending.layer_id,
                    pending.step_id,
                    pending.operation_id,
                    pending.idempotency_key,
                    to_i64(pending.expected_generation, "pending expected generation")?,
                    pending.state,
                    pending.created_at_ms,
                    pending.updated_at_ms,
                ],
            )?;
            Ok(())
        })
    }

    pub fn update_pending_publish_state(
        &mut self,
        publish_id: &str,
        expected_state: &str,
        next_state: &str,
        updated_at_ms: i64,
    ) -> StoreResult<()> {
        self.with_immediate_transaction(|transaction| {
            let changed = transaction.execute(
                "UPDATE pending_publish SET state = ?3, updated_at_ms = ?4
                 WHERE publish_id = ?1 AND state = ?2",
                params![publish_id, expected_state, next_state, updated_at_ms],
            )?;
            if changed == 1 {
                Ok(())
            } else {
                Err(StoreError::Conflict(format!(
                    "pending publish {publish_id} is not in state {expected_state}"
                )))
            }
        })
    }

    pub fn load_sync_cursor(
        &self,
        space_id: &str,
        remote_id: &str,
    ) -> StoreResult<Option<SyncCursorRecord>> {
        let raw = self
            .connection
            .query_row(
                "SELECT space_id, remote_id, cursor, generation, updated_at_ms
                 FROM sync_cursors WHERE space_id = ?1 AND remote_id = ?2",
                params![space_id, remote_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        raw.map(|(space_id, remote_id, cursor, generation, updated_at_ms)| {
            Ok(SyncCursorRecord {
                space_id,
                remote_id,
                cursor,
                generation: to_u64(generation)?,
                updated_at_ms,
            })
        })
        .transpose()
    }

    pub fn compare_and_swap_sync_cursor(
        &mut self,
        space_id: &str,
        remote_id: &str,
        expected_generation: u64,
        cursor: &str,
        updated_at_ms: i64,
    ) -> StoreResult<SyncCursorRecord> {
        let expected = to_i64(expected_generation, "expected cursor generation")?;
        let next = expected
            .checked_add(1)
            .ok_or_else(|| StoreError::InvalidState("cursor generation overflow".to_string()))?;
        let entity = format!("sync cursor {space_id}/{remote_id}");
        self.with_immediate_transaction(|transaction| {
            let actual: Option<i64> = transaction
                .query_row(
                    "SELECT generation FROM sync_cursors WHERE space_id = ?1 AND remote_id = ?2",
                    params![space_id, remote_id],
                    |row| row.get(0),
                )
                .optional()?;
            match actual {
                None if expected == 0 => {
                    transaction.execute(
                        "INSERT INTO sync_cursors (
                            space_id, remote_id, cursor, generation, updated_at_ms
                         ) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![space_id, remote_id, cursor, next, updated_at_ms],
                    )?;
                }
                Some(current) if current == expected => {
                    let changed = transaction.execute(
                        "UPDATE sync_cursors SET cursor = ?3, generation = ?4, updated_at_ms = ?5
                         WHERE space_id = ?1 AND remote_id = ?2 AND generation = ?6",
                        params![space_id, remote_id, cursor, next, updated_at_ms, expected],
                    )?;
                    if changed != 1 {
                        return Err(StoreError::GenerationConflict {
                            entity: entity.clone(),
                            expected: expected_generation,
                            actual: actual.map(to_u64).transpose()?,
                        });
                    }
                }
                _ => {
                    return Err(StoreError::GenerationConflict {
                        entity: entity.clone(),
                        expected: expected_generation,
                        actual: actual.map(to_u64).transpose()?,
                    });
                }
            }
            Ok(SyncCursorRecord {
                space_id: space_id.to_string(),
                remote_id: remote_id.to_string(),
                cursor: cursor.to_string(),
                generation: to_u64(next)?,
                updated_at_ms,
            })
        })
    }
}

type RawRef = (String, String, String, String, String, i64, i64);

fn ref_from_raw(raw: RawRef) -> StoreResult<RefRecord> {
    Ok(RefRecord {
        space_id: raw.0,
        ref_name: raw.1,
        ref_kind: raw.2,
        layer_id: raw.3,
        step_id: raw.4,
        generation: to_u64(raw.5)?,
        updated_at_ms: raw.6,
    })
}

pub(crate) fn to_i64(value: u64, label: &str) -> StoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| StoreError::InvalidState(format!("{label} exceeds SQLite INTEGER")))
}

pub(crate) fn to_u64(value: i64) -> StoreResult<u64> {
    u64::try_from(value)
        .map_err(|_| StoreError::CorruptMetadata(format!("negative generation {value}")))
}

fn validate_identifier(label: &str, value: &str) -> StoreResult<()> {
    if value.trim().is_empty() || value.contains('\0') {
        Err(StoreError::InvalidState(format!(
            "{label} must be non-empty and contain no NUL"
        )))
    } else {
        Ok(())
    }
}

fn validate_relative_path(value: &str) -> StoreResult<()> {
    if value.is_empty() || value.contains('\0') || value.contains('\\') {
        return Err(StoreError::InvalidState(format!(
            "path must be normalized with forward slashes: {value:?}"
        )));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(StoreError::InvalidState(format!(
            "path escapes the store namespace: {value:?}"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceRecord {
    pub space_id: String,
    pub canonical_root: String,
    pub display_name: Option<String>,
    pub case_sensitive: bool,
    pub identity_token: Option<String>,
    pub state: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerRecord {
    pub layer_id: String,
    pub space_id: String,
    pub name: String,
    pub layer_kind: String,
    pub state: String,
    pub base_layer_id: Option<String>,
    pub timeline_active: bool,
    pub generation: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRecord {
    pub object_id: String,
    pub algorithm: String,
    pub byte_len: u64,
    pub object_kind: String,
    pub relative_path: String,
    pub state: String,
    pub verified_at_ms: Option<i64>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub step_id: String,
    pub space_id: String,
    pub layer_id: String,
    pub parent_step_id: Option<String>,
    pub root_object_id: String,
    pub sequence: u64,
    pub created_at_ms: i64,
    pub authored_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepPathRecord {
    pub step_id: String,
    pub path: String,
    pub object_id: String,
    pub entry_kind: String,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate {
    pub space_id: String,
    pub ref_name: String,
    pub ref_kind: String,
    pub layer_id: String,
    pub step_id: String,
    pub expected_generation: u64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRecord {
    pub space_id: String,
    pub ref_name: String,
    pub ref_kind: String,
    pub layer_id: String,
    pub step_id: String,
    pub generation: u64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPublishRecord {
    pub publish_id: String,
    pub space_id: String,
    pub layer_id: String,
    pub step_id: String,
    pub operation_id: Option<String>,
    pub idempotency_key: String,
    pub expected_generation: u64,
    pub state: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCursorRecord {
    pub space_id: String,
    pub remote_id: String,
    pub cursor: String,
    pub generation: u64,
    pub updated_at_ms: i64,
}
