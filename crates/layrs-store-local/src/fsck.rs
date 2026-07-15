//! Read-only consistency checks and metadata-only quarantine repair.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::error::{StoreError, StoreResult};
use crate::layout::{
    StoreLayout, contains_reparse_component, create_dir_all_durable, sync_directory,
};
use crate::lock::{SessionMode, StoreLock};
use crate::schema::CURRENT_SCHEMA_VERSION;
use crate::sqlite::{self, SqliteOptions};
use crate::{LocalStore, MetadataRepository};

const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FsckCode {
    Integrity,
    ForeignKey,
    Migration,
    SchemaVersion,
    RefInvariant,
    StepInvariant,
    PendingInvariant,
    JournalInvariant,
    TimelineInvariant,
    WeaveInvariant,
    GcInvariant,
    ObjectMissing,
    ObjectCorrupt,
    ObjectPathUnsafe,
    ObjectGraphCycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsckIssue {
    pub code: FsckCode,
    pub entity: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsckReport {
    pub sqlite_version: String,
    pub schema_version: Option<u32>,
    pub checked_objects: u64,
    pub issues: Vec<FsckIssue>,
}

impl FsckReport {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn issues_with_code(&self, code: FsckCode) -> impl Iterator<Item = &FsckIssue> {
        self.issues.iter().filter(move |issue| issue.code == code)
    }
}

/// Opens only an existing database and lock file. This path never creates or
/// migrates the store.
pub(crate) fn fsck_read_only(
    layout: &StoreLayout,
    options: SqliteOptions,
) -> StoreResult<FsckReport> {
    let _lock = StoreLock::acquire_existing(&layout.lock_path(), SessionMode::Shared)?;
    let connection = sqlite::open_read_only_unchecked(&layout.sqlite_path, options)?;
    check_connection(layout, &connection)
}

pub fn fsck_existing(config: &crate::LocalStoreConfig) -> StoreResult<FsckReport> {
    let layout = config.layout();
    fsck_read_only(&layout, config.sqlite_options())
}

fn check_connection(layout: &StoreLayout, connection: &Connection) -> StoreResult<FsckReport> {
    let sqlite_version = connection.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    let schema_version = connection
        .query_row(
            "SELECT schema_version FROM store_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap_or(None);
    let mut report = FsckReport {
        sqlite_version,
        schema_version,
        checked_objects: 0,
        issues: Vec::new(),
    };

    check_integrity(connection, &mut report)?;
    check_foreign_keys(connection, &mut report)?;
    if let Err(error) = sqlite::verify_applied_migrations(connection, CURRENT_SCHEMA_VERSION) {
        push_issue(&mut report, FsckCode::Migration, None, error.to_string());
    }
    if schema_version != Some(CURRENT_SCHEMA_VERSION) {
        push_issue(
            &mut report,
            FsckCode::SchemaVersion,
            None,
            format!(
                "store_meta schema version {:?} does not equal {}",
                schema_version, CURRENT_SCHEMA_VERSION
            ),
        );
    }

    if has_table(connection, "refs")? {
        check_relational_invariants(connection, &mut report)?;
        check_object_graph(connection, &mut report)?;
        check_indexed_objects(layout, connection, &mut report)?;
    }
    Ok(report)
}

fn check_integrity(connection: &Connection, report: &mut FsckReport) -> StoreResult<()> {
    let mut statement = connection.prepare("PRAGMA integrity_check")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let message = row?;
        if !message.eq_ignore_ascii_case("ok") {
            push_issue(report, FsckCode::Integrity, None, message);
        }
    }
    Ok(())
}

fn check_foreign_keys(connection: &Connection, report: &mut FsckReport) -> StoreResult<()> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    for row in rows {
        let (table, row_id, parent, fk_id) = row?;
        push_issue(
            report,
            FsckCode::ForeignKey,
            Some(table.clone()),
            format!("table {table} row {row_id:?} violates FK {fk_id} to {parent}"),
        );
    }
    Ok(())
}

fn check_relational_invariants(
    connection: &Connection,
    report: &mut FsckReport,
) -> StoreResult<()> {
    collect_space_query(
        connection,
        report,
        FsckCode::RefInvariant,
        "SELECT r.space_id, r.ref_name
         FROM refs r
         LEFT JOIN steps s ON s.step_id = r.step_id
         WHERE r.generation <= 0 OR s.step_id IS NULL
            OR s.space_id <> r.space_id OR s.layer_id <> r.layer_id",
        "invalid ref",
    )?;
    collect_space_query(
        connection,
        report,
        FsckCode::StepInvariant,
        "SELECT s.space_id, s.step_id
         FROM steps s
         LEFT JOIN steps p ON p.step_id = s.parent_step_id
         WHERE s.parent_step_id IS NOT NULL
           AND (p.step_id IS NULL OR p.space_id <> s.space_id OR p.layer_id <> s.layer_id)",
        "invalid parent for step",
    )?;
    collect_space_query(
        connection,
        report,
        FsckCode::PendingInvariant,
        "SELECT p.space_id, p.publish_id
         FROM pending_publish p
         LEFT JOIN steps s ON s.step_id = p.step_id
         LEFT JOIN layers l ON l.layer_id = p.layer_id
         WHERE s.step_id IS NULL OR l.layer_id IS NULL
            OR s.space_id <> p.space_id OR s.layer_id <> p.layer_id
            OR l.space_id <> p.space_id",
        "invalid pending publish",
    )?;
    collect_space_query(
        connection,
        report,
        FsckCode::JournalInvariant,
        "SELECT space_id, CAST(COUNT(*) AS TEXT)
         FROM operation_journal
         WHERE state IN (
            'prepared', 'external_pending', 'applying', 'metadata_committed', 'rolling_back'
         )
         GROUP BY space_id HAVING COUNT(*) > 1",
        "multiple non-terminal journal operations",
    )?;
    collect_space_query(
        connection,
        report,
        FsckCode::TimelineInvariant,
        "SELECT space_id, CAST(COUNT(*) AS TEXT)
         FROM layers WHERE timeline_active = 1 AND state = 'active'
         GROUP BY space_id HAVING COUNT(*) > 1",
        "multiple active timeline layers",
    )?;
    if has_table(connection, "weave_sessions")? {
        collect_space_query(
            connection,
            report,
            FsckCode::WeaveInvariant,
            "SELECT space_id, CAST(COUNT(*) AS TEXT)
             FROM weave_sessions
             WHERE state IN ('prepared', 'in_progress', 'conflicted', 'resolved', 'applying')
             GROUP BY space_id HAVING COUNT(*) > 1",
            "multiple active weave sessions",
        )?;
    }
    collect_space_query(
        connection,
        report,
        FsckCode::GcInvariant,
        "SELECT g.space_id, g.root_id
         FROM gc_roots g
         LEFT JOIN object_index o ON o.object_id = g.object_id
         WHERE o.object_id IS NULL OR o.state <> 'available'",
        "GC root references unavailable object",
    )?;
    Ok(())
}

fn collect_space_query(
    connection: &Connection,
    report: &mut FsckReport,
    code: FsckCode,
    sql: &str,
    label: &str,
) -> StoreResult<()> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (space_id, detail) = row?;
        push_issue(report, code, Some(space_id), format!("{label}: {detail}"));
    }
    Ok(())
}

fn check_object_graph(connection: &Connection, report: &mut FsckReport) -> StoreResult<()> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE walk(root, node, visited, cycle) AS (
            SELECT parent_object_id, child_object_id,
                   ',' || parent_object_id || ',' || child_object_id || ',', 0
            FROM object_edges
            UNION ALL
            SELECT walk.root, edge.child_object_id,
                   walk.visited || edge.child_object_id || ',',
                   instr(walk.visited, ',' || edge.child_object_id || ',') > 0
            FROM walk
            JOIN object_edges edge ON edge.parent_object_id = walk.node
            WHERE walk.cycle = 0
         )
         SELECT DISTINCT root, node FROM walk WHERE cycle = 1 LIMIT 100",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (root, node) = row?;
        push_issue(
            report,
            FsckCode::ObjectGraphCycle,
            Some(root),
            format!("object graph cycle reaches {node}"),
        );
    }
    Ok(())
}

fn check_indexed_objects(
    layout: &StoreLayout,
    connection: &Connection,
    report: &mut FsckReport,
) -> StoreResult<()> {
    let mut statement = connection.prepare(
        "SELECT object_id, algorithm, byte_len, relative_path, state
         FROM object_index ORDER BY object_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    for row in rows {
        let (object_id, algorithm, expected_len, relative_path, state) = row?;
        report.checked_objects += 1;
        let Some(path) = safe_object_path(&layout.cas_dir, &relative_path) else {
            push_issue(
                report,
                FsckCode::ObjectPathUnsafe,
                Some(object_id),
                format!("unsafe object path {relative_path:?}"),
            );
            continue;
        };
        if contains_reparse_component(&path).unwrap_or(true) {
            push_issue(
                report,
                FsckCode::ObjectPathUnsafe,
                Some(object_id),
                format!("object path crosses a reparse point: {}", path.display()),
            );
            continue;
        }
        if !path.is_file() {
            if state == "available" {
                push_issue(
                    report,
                    FsckCode::ObjectMissing,
                    Some(object_id),
                    format!("indexed object is missing: {}", path.display()),
                );
            }
            continue;
        }
        let Ok(expected_len) = u64::try_from(expected_len) else {
            push_issue(
                report,
                FsckCode::ObjectCorrupt,
                Some(object_id),
                format!("negative object length at {}", path.display()),
            );
            continue;
        };
        match stream_verify_object(&path, &algorithm, &object_id, expected_len)? {
            StreamVerification::Verified => {}
            StreamVerification::LengthMismatch { actual } => push_issue(
                report,
                FsckCode::ObjectCorrupt,
                Some(object_id),
                format!(
                    "object length mismatch at {}: expected {expected_len}, got {actual}",
                    path.display()
                ),
            ),
            StreamVerification::DigestMismatch => push_issue(
                report,
                FsckCode::ObjectCorrupt,
                Some(object_id),
                format!("object digest mismatch at {}", path.display()),
            ),
        }
    }
    Ok(())
}

fn safe_object_path(cas_dir: &Path, relative_path: &str) -> Option<std::path::PathBuf> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        None
    } else {
        Some(cas_dir.join(relative))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamVerification {
    Verified,
    LengthMismatch { actual: u64 },
    DigestMismatch,
}

fn stream_verify_object(
    path: &Path,
    algorithm: &str,
    object_id: &str,
    expected_len: u64,
) -> io::Result<StreamVerification> {
    let expected_hex = object_id
        .strip_prefix(&format!("{algorithm}:"))
        .unwrap_or(object_id);
    let mut file = File::open(path)?;
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    let mut byte_len = 0_u64;
    let mut blake3 = blake3::Hasher::new();
    let mut placeholder = 0xcbf2_9ce4_8422_2325_u64;

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        byte_len = byte_len
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("object length overflow during fsck"))?;
        match algorithm {
            "blake3" => {
                blake3.update(&buffer[..read]);
            }
            "placeholder" => {
                for byte in &buffer[..read] {
                    placeholder ^= u64::from(*byte);
                    placeholder = placeholder.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            _ => return Ok(StreamVerification::DigestMismatch),
        }
    }

    if byte_len != expected_len {
        return Ok(StreamVerification::LengthMismatch { actual: byte_len });
    }
    let actual = match algorithm {
        "blake3" => blake3.finalize().to_hex().to_string(),
        "placeholder" => format!("{placeholder:016x}{byte_len:016x}"),
        _ => return Ok(StreamVerification::DigestMismatch),
    };
    if actual == expected_hex {
        Ok(StreamVerification::Verified)
    } else {
        Ok(StreamVerification::DigestMismatch)
    }
}

fn has_table(connection: &Connection, name: &str) -> StoreResult<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some())
}

fn push_issue(report: &mut FsckReport, code: FsckCode, entity: Option<String>, detail: String) {
    report.issues.push(FsckIssue {
        code,
        entity,
        detail,
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairAction {
    ObjectQuarantined(String),
    SpaceQuarantined(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RepairReport {
    pub backup_path: std::path::PathBuf,
    pub actions: Vec<RepairAction>,
    pub skipped_issues: usize,
}

/// Applies metadata-only quarantine actions. Object bytes are never created,
/// moved, rewritten, or removed by repair.
pub fn repair(
    store: &LocalStore,
    findings: &FsckReport,
    updated_at_ms: i64,
) -> StoreResult<RepairReport> {
    if updated_at_ms < 0 {
        return Err(StoreError::InvalidState(
            "repair timestamp must not be negative".to_string(),
        ));
    }
    let mut session = store.session(SessionMode::Exclusive)?;
    let backup_path = create_repair_backup(store.layout(), session.repository(), updated_at_ms)?;
    let mut report = repair_repository(session.repository_mut(), findings, updated_at_ms)?;
    report.backup_path = backup_path;
    Ok(report)
}

fn create_repair_backup(
    layout: &StoreLayout,
    repository: &MetadataRepository,
    timestamp_ms: i64,
) -> StoreResult<std::path::PathBuf> {
    let source_signature: (u32, u64) = repository.connection().query_row(
        "SELECT schema_version, quarantine_generation FROM store_meta WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let backup_dir = layout.repair_backup_dir();
    create_dir_all_durable(&backup_dir)?;
    let backup_path = backup_dir.join(format!(
        "layrs-repair-{timestamp_ms}-q{}.sqlite3",
        source_signature.1
    ));
    if backup_path.exists() {
        return Err(StoreError::Conflict(format!(
            "repair backup already exists: {}",
            backup_path.display()
        )));
    }
    let backup_name = backup_path.to_str().ok_or_else(|| {
        StoreError::InvalidState("repair backup path is not valid UTF-8".to_string())
    })?;

    let backup_result = (|| -> StoreResult<()> {
        repository
            .connection()
            .execute("VACUUM main INTO ?1", [backup_name])?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&backup_path)?
            .sync_all()?;
        sync_directory(&backup_dir)?;
        validate_repair_backup(&backup_path, source_signature)?;
        Ok(())
    })();
    if let Err(error) = backup_result {
        if backup_path.exists() {
            let _ = fs::remove_file(&backup_path);
            let _ = sync_directory(&backup_dir);
        }
        return Err(error);
    }
    Ok(backup_path)
}

fn validate_repair_backup(backup_path: &Path, source_signature: (u32, u64)) -> StoreResult<()> {
    let connection = Connection::open_with_flags(
        backup_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    connection.pragma_update(None, "query_only", "ON")?;
    let mut integrity = connection.prepare("PRAGMA integrity_check")?;
    let rows = integrity.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let message = row?;
        if !message.eq_ignore_ascii_case("ok") {
            return Err(StoreError::CorruptMetadata(format!(
                "repair backup integrity check failed: {message}"
            )));
        }
    }
    drop(integrity);
    sqlite::verify_applied_migrations(&connection, CURRENT_SCHEMA_VERSION)?;
    let backup_signature: (u32, u64) = connection.query_row(
        "SELECT schema_version, quarantine_generation FROM store_meta WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if backup_signature != source_signature {
        return Err(StoreError::CorruptMetadata(format!(
            "repair backup signature mismatch: expected {source_signature:?}, got {backup_signature:?}"
        )));
    }
    Ok(())
}

fn repair_repository(
    repository: &mut MetadataRepository,
    findings: &FsckReport,
    updated_at_ms: i64,
) -> StoreResult<RepairReport> {
    let object_codes = [
        FsckCode::ObjectMissing,
        FsckCode::ObjectCorrupt,
        FsckCode::ObjectPathUnsafe,
    ];
    let space_codes = [
        FsckCode::RefInvariant,
        FsckCode::StepInvariant,
        FsckCode::PendingInvariant,
        FsckCode::JournalInvariant,
        FsckCode::TimelineInvariant,
        FsckCode::WeaveInvariant,
        FsckCode::GcInvariant,
    ];
    let objects: BTreeSet<_> = findings
        .issues
        .iter()
        .filter(|issue| object_codes.contains(&issue.code))
        .filter_map(|issue| issue.entity.clone())
        .collect();
    let spaces: BTreeSet<_> = findings
        .issues
        .iter()
        .filter(|issue| space_codes.contains(&issue.code))
        .filter_map(|issue| issue.entity.clone())
        .collect();
    let actionable = objects.len() + spaces.len();

    let actions = repository.with_immediate_transaction(|transaction| {
        let mut actions = Vec::new();
        for object_id in &objects {
            let changed = transaction.execute(
                "UPDATE object_index SET state = 'quarantined'
                 WHERE object_id = ?1 AND state <> 'quarantined'",
                [object_id],
            )?;
            if changed == 1 {
                actions.push(RepairAction::ObjectQuarantined(object_id.clone()));
            }
        }
        for space_id in &spaces {
            let changed = transaction.execute(
                "UPDATE local_spaces SET state = 'quarantined', updated_at_ms = ?2
                 WHERE space_id = ?1 AND state <> 'quarantined'",
                params![space_id, updated_at_ms],
            )?;
            if changed == 1 {
                actions.push(RepairAction::SpaceQuarantined(space_id.clone()));
            }
        }
        if !actions.is_empty() {
            transaction.execute(
                "UPDATE store_meta
                 SET quarantine_generation = quarantine_generation + 1,
                     updated_at_ms = MAX(updated_at_ms, ?1)
                 WHERE singleton = 1",
                [updated_at_ms],
            )?;
        }
        Ok(actions)
    })?;

    Ok(RepairReport {
        backup_path: std::path::PathBuf::new(),
        skipped_issues: findings.issues.len().saturating_sub(actionable),
        actions,
    })
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn rejects_parent_components_in_indexed_paths() {
        assert!(safe_object_path(Path::new("objects"), "../outside").is_none());
        assert!(safe_object_path(Path::new("objects"), "blake3/ab/cd/hash").is_some());
    }

    #[test]
    fn object_verification_streams_across_multiple_fixed_buffers() {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "layrs-store-local-stream-{}-{timestamp}",
            std::process::id()
        ));
        let bytes = vec![0x5a; HASH_BUFFER_BYTES * 3 + 17];
        fs::write(&path, &bytes).expect("write streamed fixture");
        let digest = blake3::hash(&bytes).to_hex().to_string();

        assert_eq!(
            stream_verify_object(&path, "blake3", &digest, bytes.len() as u64)
                .expect("stream object"),
            StreamVerification::Verified
        );

        fs::remove_file(path).expect("remove streamed fixture");
    }
}
