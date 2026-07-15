//! Transactional operation journal records and legal state transitions.

use std::fmt;

use rusqlite::{OptionalExtension, params};

use crate::error::{StoreError, StoreResult};
use crate::repository::MetadataRepository;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Publish,
    LayerDelete,
    Scan,
    Weave,
    Gc,
    ObjectInstall,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::LayerDelete => "layer_delete",
            Self::Scan => "scan",
            Self::Weave => "weave",
            Self::Gc => "gc",
            Self::ObjectInstall => "object_install",
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "publish" => Ok(Self::Publish),
            "layer_delete" => Ok(Self::LayerDelete),
            "scan" => Ok(Self::Scan),
            "weave" => Ok(Self::Weave),
            "gc" => Ok(Self::Gc),
            "object_install" => Ok(Self::ObjectInstall),
            other => Err(StoreError::CorruptMetadata(format!(
                "unknown journal operation kind {other:?}"
            ))),
        }
    }
}

impl fmt::Display for OperationKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalState {
    Prepared,
    ExternalPending,
    Applying,
    MetadataCommitted,
    Finalized,
    RollingBack,
    RolledBack,
    NeedsRepair,
}

impl JournalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::ExternalPending => "external_pending",
            Self::Applying => "applying",
            Self::MetadataCommitted => "metadata_committed",
            Self::Finalized => "finalized",
            Self::RollingBack => "rolling_back",
            Self::RolledBack => "rolled_back",
            Self::NeedsRepair => "needs_repair",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Finalized | Self::RolledBack | Self::NeedsRepair)
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        match self {
            Self::Prepared => matches!(
                next,
                Self::ExternalPending | Self::Applying | Self::RollingBack | Self::NeedsRepair
            ),
            Self::ExternalPending => {
                matches!(next, Self::Applying | Self::RollingBack | Self::NeedsRepair)
            }
            Self::Applying => matches!(
                next,
                Self::MetadataCommitted | Self::RollingBack | Self::NeedsRepair
            ),
            Self::MetadataCommitted => {
                matches!(
                    next,
                    Self::Finalized | Self::RollingBack | Self::NeedsRepair
                )
            }
            Self::RollingBack => matches!(next, Self::RolledBack | Self::NeedsRepair),
            Self::Finalized | Self::RolledBack | Self::NeedsRepair => false,
        }
    }

    fn parse(value: &str) -> StoreResult<Self> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "external_pending" => Ok(Self::ExternalPending),
            "applying" => Ok(Self::Applying),
            "metadata_committed" => Ok(Self::MetadataCommitted),
            "finalized" => Ok(Self::Finalized),
            "rolling_back" => Ok(Self::RollingBack),
            "rolled_back" => Ok(Self::RolledBack),
            "needs_repair" => Ok(Self::NeedsRepair),
            other => Err(StoreError::CorruptMetadata(format!(
                "unknown journal state {other:?}"
            ))),
        }
    }
}

impl fmt::Display for JournalState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJournalOperation {
    pub operation_id: String,
    pub space_id: String,
    pub kind: OperationKind,
    pub payload_json: String,
    pub created_at_ms: i64,
    pub paths: Vec<JournalPathInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPathInput {
    pub path: String,
    pub role: String,
    pub expected_object_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalOperation {
    pub operation_id: String,
    pub space_id: String,
    pub kind: OperationKind,
    pub state: JournalState,
    pub payload_json: String,
    pub attempt: u64,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPath {
    pub operation_id: String,
    pub path: String,
    pub role: String,
    pub expected_object_id: Option<String>,
    pub state: String,
}

impl MetadataRepository {
    pub fn prepare_operation(
        &mut self,
        operation: &NewJournalOperation,
    ) -> StoreResult<JournalOperation> {
        if operation.operation_id.trim().is_empty() || operation.space_id.trim().is_empty() {
            return Err(StoreError::InvalidState(
                "journal operation and space IDs must not be empty".into(),
            ));
        }
        self.with_immediate_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO operation_journal (
                    operation_id, space_id, operation_kind, state, payload_json,
                    attempt, last_error, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, 'prepared', ?4, 0, NULL, ?5, ?5)",
                params![
                    operation.operation_id,
                    operation.space_id,
                    operation.kind.as_str(),
                    operation.payload_json,
                    operation.created_at_ms,
                ],
            )?;
            for path in &operation.paths {
                transaction.execute(
                    "INSERT INTO operation_paths (
                        operation_id, path, path_role, expected_object_id, path_state
                     ) VALUES (?1, ?2, ?3, ?4, 'pending')",
                    params![
                        operation.operation_id,
                        path.path,
                        path.role,
                        path.expected_object_id,
                    ],
                )?;
            }
            Ok(JournalOperation {
                operation_id: operation.operation_id.clone(),
                space_id: operation.space_id.clone(),
                kind: operation.kind,
                state: JournalState::Prepared,
                payload_json: operation.payload_json.clone(),
                attempt: 0,
                last_error: None,
                created_at_ms: operation.created_at_ms,
                updated_at_ms: operation.created_at_ms,
            })
        })
    }

    pub fn load_operation(&self, operation_id: &str) -> StoreResult<Option<JournalOperation>> {
        let raw = self
            .connection()
            .query_row(
                "SELECT operation_id, space_id, operation_kind, state, payload_json,
                        attempt, last_error, created_at_ms, updated_at_ms
                 FROM operation_journal WHERE operation_id = ?1",
                [operation_id],
                operation_row,
            )
            .optional()?;
        raw.map(operation_from_raw).transpose()
    }

    pub fn load_operation_paths(&self, operation_id: &str) -> StoreResult<Vec<JournalPath>> {
        let mut statement = self.connection().prepare(
            "SELECT operation_id, path, path_role, expected_object_id, path_state
             FROM operation_paths WHERE operation_id = ?1 ORDER BY path_role, path",
        )?;
        let rows = statement.query_map([operation_id], |row| {
            Ok(JournalPath {
                operation_id: row.get(0)?,
                path: row.get(1)?,
                role: row.get(2)?,
                expected_object_id: row.get(3)?,
                state: row.get(4)?,
            })
        })?;
        let mut paths = Vec::new();
        for row in rows {
            paths.push(row?);
        }
        Ok(paths)
    }

    pub fn list_recoverable_operations(&self) -> StoreResult<Vec<JournalOperation>> {
        let mut statement = self.connection().prepare(
            "SELECT operation_id, space_id, operation_kind, state, payload_json,
                    attempt, last_error, created_at_ms, updated_at_ms
             FROM operation_journal
             WHERE state IN (
                'prepared', 'external_pending', 'applying', 'metadata_committed', 'rolling_back'
             )
             ORDER BY created_at_ms, operation_id",
        )?;
        let rows = statement.query_map([], operation_row)?;
        let mut operations = Vec::new();
        for row in rows {
            operations.push(operation_from_raw(row?)?);
        }
        Ok(operations)
    }

    pub fn transition_operation(
        &mut self,
        operation_id: &str,
        expected: JournalState,
        next: JournalState,
        updated_at_ms: i64,
        last_error: Option<&str>,
    ) -> StoreResult<JournalOperation> {
        if !expected.can_transition_to(next) {
            return Err(StoreError::InvalidState(format!(
                "journal transition {expected} -> {next} is not allowed"
            )));
        }
        self.with_immediate_transaction(|transaction| {
            let changed = transaction.execute(
                "UPDATE operation_journal
                 SET state = ?3,
                     attempt = attempt + CASE WHEN ?3 IN ('applying', 'rolling_back') THEN 1 ELSE 0 END,
                     last_error = ?4,
                     updated_at_ms = ?5
                 WHERE operation_id = ?1 AND state = ?2",
                params![
                    operation_id,
                    expected.as_str(),
                    next.as_str(),
                    last_error,
                    updated_at_ms,
                ],
            )?;
            if changed != 1 {
                return Err(StoreError::Conflict(format!(
                    "journal operation {operation_id} is not in state {expected}"
                )));
            }
            let raw = transaction.query_row(
                "SELECT operation_id, space_id, operation_kind, state, payload_json,
                        attempt, last_error, created_at_ms, updated_at_ms
                 FROM operation_journal WHERE operation_id = ?1",
                [operation_id],
                operation_row,
            )?;
            operation_from_raw(raw)
        })
    }
}

type RawOperation = (
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    i64,
    i64,
);

fn operation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawOperation> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn operation_from_raw(raw: RawOperation) -> StoreResult<JournalOperation> {
    Ok(JournalOperation {
        operation_id: raw.0,
        space_id: raw.1,
        kind: OperationKind::parse(&raw.2)?,
        state: JournalState::parse(&raw.3)?,
        payload_json: raw.4,
        attempt: u64::try_from(raw.5)
            .map_err(|_| StoreError::CorruptMetadata("negative journal attempt".to_string()))?,
        last_error: raw.6,
        created_at_ms: raw.7,
        updated_at_ms: raw.8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_journal_states_cannot_transition() {
        assert!(JournalState::Prepared.can_transition_to(JournalState::Applying));
        assert!(JournalState::Applying.can_transition_to(JournalState::MetadataCommitted));
        assert!(JournalState::MetadataCommitted.can_transition_to(JournalState::Finalized));
        assert!(!JournalState::Finalized.can_transition_to(JournalState::Applying));
        assert!(!JournalState::NeedsRepair.can_transition_to(JournalState::RollingBack));
    }
}
