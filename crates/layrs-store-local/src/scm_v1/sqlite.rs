//! SQLite connection and bootstrap boundary for native SCM V1.

use super::schema::{
    BASELINE_V2, BASELINE_V3, Baseline, REQUIRED_TABLES_V3, SCHEMA_CHECKSUM, SCHEMA_MAGIC,
    SCHEMA_V3_CHECKSUM, schema_checksum, schema_v3_checksum,
};
use layrs_core::scm::{StoreId, canonical_json};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::layout::{create_new_file_no_follow, sync_directory};

pub type Result<T> = std::result::Result<T, ScmSchemaError>;

const APPLICATION_ID: u32 = 0x4c59_5253;

/// Converts an RFC 3339 timestamp to Unix epoch milliseconds using the one
/// authoritative Rust implementation for persisted entity projections.
///
/// Euclidean division is intentional: sub-millisecond instants before the
/// epoch round toward negative infinity, matching the mathematical epoch
/// millisecond containing the instant rather than truncating toward zero.
pub(crate) fn rfc3339_to_epoch_millis(value: &str) -> Result<i64> {
    let timestamp = OffsetDateTime::parse(value, &Rfc3339).map_err(|error| {
        ScmSchemaError::InvalidMetadata(format!("invalid RFC3339 timestamp: {error}"))
    })?;
    let millis = timestamp.unix_timestamp_nanos().div_euclid(1_000_000);
    i64::try_from(millis).map_err(|_| {
        ScmSchemaError::InvalidMetadata("timestamp is outside the i64 millisecond range".into())
    })
}

/// Encodes a replay receipt as RFC 8785 JSON Canonicalization Scheme bytes.
///
/// `applied_transactions.receipt_json` stores this complete receipt so an
/// idempotent retry can reconstruct the original outcome without recomputing
/// it from mutable projections. SQLite guarantees that the value is JSON;
/// this boundary guarantees its exact canonical representation.
pub(crate) fn canonical_receipt_json<T: serde::Serialize>(receipt: &T) -> Result<String> {
    String::from_utf8(
        canonical_json(receipt)
            .map_err(|error| ScmSchemaError::InvalidMetadata(error.to_string()))?,
    )
    .map_err(|error| ScmSchemaError::InvalidMetadata(error.to_string()))
}

/// Validates a receipt loaded from SQLite before it is used for replay.
#[allow(dead_code)] // Wave 3D apply_batch consumes this replay-boundary helper.
pub(crate) fn validate_canonical_receipt_json(receipt_json: &str) -> Result<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(receipt_json).map_err(|error| {
        ScmSchemaError::InvalidMetadata(format!(
            "invalid applied transaction receipt JSON: {error}"
        ))
    })?;
    let canonical = canonical_receipt_json(&value)?;
    if canonical != receipt_json {
        return Err(ScmSchemaError::InvalidMetadata(
            "applied transaction receipt is not RFC 8785 canonical JSON".into(),
        ));
    }
    Ok(value)
}

#[derive(Debug)]
pub enum ScmSchemaError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    AlreadyExists(PathBuf),
    NotFound(PathBuf),
    UncheckpointedWal(PathBuf),
    InvalidStagedDatabase {
        path: PathBuf,
        reason: String,
    },
    ForeignDatabase {
        path: PathBuf,
        tables: Vec<String>,
    },
    MagicMismatch {
        expected: String,
        actual: String,
    },
    UnsupportedSchema {
        found: u32,
        supported: u32,
    },
    ChecksumMismatch {
        expected: String,
        actual: String,
    },
    SchemaStructureMismatch {
        missing_tables: Vec<String>,
        unexpected_tables: Vec<String>,
    },
    SchemaFingerprintMismatch {
        expected: String,
        actual: String,
    },
    EmbeddedChecksumMismatch {
        published: String,
        actual: String,
    },
    InvalidMetadata(String),
    PragmaMismatch {
        name: &'static str,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for ScmSchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::AlreadyExists(path) => {
                write!(
                    formatter,
                    "native SCM database already exists: {}",
                    path.display()
                )
            }
            Self::NotFound(path) => {
                write!(
                    formatter,
                    "native SCM database does not exist: {}",
                    path.display()
                )
            }
            Self::UncheckpointedWal(path) => write!(
                formatter,
                "read-only native SCM open refused because WAL is not checkpointed: {}",
                path.display()
            ),
            Self::InvalidStagedDatabase { path, reason } => write!(
                formatter,
                "invalid staged native SCM database {}: {reason}",
                path.display()
            ),
            Self::ForeignDatabase { path, tables } => write!(
                formatter,
                "database {} is not native SCM V1 (tables: {})",
                path.display(),
                tables.join(", ")
            ),
            Self::MagicMismatch { expected, actual } => {
                write!(
                    formatter,
                    "schema magic mismatch: expected {expected}, got {actual}"
                )
            }
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "unsupported native SCM schema {found}; this binary supports exactly {supported}"
            ),
            Self::ChecksumMismatch { expected, actual } => write!(
                formatter,
                "native SCM schema checksum mismatch: expected {expected}, got {actual}"
            ),
            Self::SchemaStructureMismatch {
                missing_tables,
                unexpected_tables,
            } => write!(
                formatter,
                "native SCM table set mismatch: missing [{}], unexpected [{}]",
                missing_tables.join(", "),
                unexpected_tables.join(", ")
            ),
            Self::SchemaFingerprintMismatch { expected, actual } => write!(
                formatter,
                "native SCM sqlite_schema fingerprint mismatch: expected {expected}, got {actual}"
            ),
            Self::EmbeddedChecksumMismatch { published, actual } => write!(
                formatter,
                "embedded SCM baseline changed without publishing its checksum: expected {published}, got {actual}"
            ),
            Self::InvalidMetadata(message) => write!(formatter, "invalid SCM metadata: {message}"),
            Self::PragmaMismatch {
                name,
                expected,
                actual,
            } => write!(
                formatter,
                "SQLite PRAGMA {name} mismatch: expected {expected}, got {actual}"
            ),
        }
    }
}

impl Error for ScmSchemaError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ScmSchemaError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for ScmSchemaError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// Creates and durably publishes the one supported schema at a path that does
/// not exist.
///
/// The caller must hold the exclusive Store lock and have authenticated the
/// Store's `Initializing` FORMAT marker. Creation happens in the deterministic
/// path returned by [`staged_database_path`] in the final file's directory.
/// The baseline is committed, checkpointed, synced, and only then atomically
/// published without clobbering an existing final file. A retry removes and
/// durably recreates only this exact authenticated staging path. This function
/// never opens, upgrades, or replaces an existing final database.
#[cfg(test)]
pub fn create_fresh(path: &Path, busy_timeout: Duration, store_id: &StoreId) -> Result<Connection> {
    create_fresh_baseline(path, busy_timeout, store_id, &BASELINE_V2)
}

/// `create_fresh` with an explicit baseline (format 6 creates v3).
pub fn create_fresh_baseline(
    path: &Path,
    busy_timeout: Duration,
    store_id: &StoreId,
    baseline: &Baseline,
) -> Result<Connection> {
    validate_embedded_checksum()?;
    if path.exists() {
        return Err(ScmSchemaError::AlreadyExists(path.to_path_buf()));
    }
    let parent = path.parent().ok_or_else(|| {
        ScmSchemaError::InvalidMetadata("database path has no parent directory".into())
    })?;
    if !parent.is_dir() {
        return Err(ScmSchemaError::NotFound(parent.to_path_buf()));
    }

    let staged = staged_database_path(path)?;
    remove_staged_for_retry(&staged)?;
    let staged_file = create_new_file_no_follow(&staged)?;
    staged_file.sync_all()?;
    drop(staged_file);
    sync_directory(parent)?;

    let mut connection = open_guarded_writer(
        &staged,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    // The switch to WAL writes the header in a transaction that uses a
    // rollback journal unless the journal is off: kept in memory (the
    // guarded VFS opens no journal file), as SQLite moves from MEMORY to WAL
    // through OFF. A staged file: a crash only means a retry.
    let journal: String =
        connection.query_row("PRAGMA journal_mode = MEMORY", [], |row| row.get(0))?;
    require_pragma("journal_mode", "memory", journal.to_ascii_lowercase())?;
    if let Some(page_size) = baseline.page_size {
        // Only an empty database takes a page size, before WAL is enabled.
        connection.pragma_update(None, "page_size", page_size)?;
    }
    if baseline.auto_vacuum {
        // Only an empty database takes a vacuum mode, before any table.
        connection.pragma_update(None, "auto_vacuum", "FULL")?;
    }
    configure_writer(&connection, busy_timeout)?;
    if !table_names(&connection)?.is_empty() {
        return Err(ScmSchemaError::InvalidStagedDatabase {
            path: staged,
            reason: "exclusive staging file was not empty".into(),
        });
    }

    let now_ms = unix_time_ms()?;
    let store_payload = String::from_utf8(
        canonical_json(&serde_json::json!({"storeId": store_id.as_str()}))
            .map_err(|error| ScmSchemaError::InvalidMetadata(error.to_string()))?,
    )
    .map_err(|error| ScmSchemaError::InvalidMetadata(error.to_string()))?;
    with_immediate_transaction(&mut connection, |transaction| {
        transaction.execute_batch(baseline.sql)?;
        transaction.execute(
            "INSERT INTO meta(singleton, magic, schema_version, schema_checksum, created_at_ms)\
             VALUES (1, ?1, ?2, ?3, ?4)",
            params![SCHEMA_MAGIC, baseline.version, baseline.checksum, now_ms],
        )?;
        transaction.execute(
            "INSERT INTO migrations(version, name, checksum, applied_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![baseline.version, baseline.name, baseline.checksum, now_ms],
        )?;
        transaction.execute(
            "INSERT INTO stores(store_id, payload_json, created_at_ms) VALUES (?1, ?2, ?3)",
            params![store_id.as_str(), store_payload, now_ms],
        )?;
        transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
        transaction.pragma_update(None, "user_version", baseline.version)?;
        Ok(())
    })?;
    validate_schema(&connection, &staged)?;
    validate_writer_pragmas(&connection, busy_timeout)?;
    checkpoint_clean(&connection, &staged)?;
    drop(connection);
    require_clean_stage_sidecars(&staged)?;

    let staged_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&staged)?;
    staged_file.sync_all()?;
    drop(staged_file);
    sync_directory(parent)?;
    publish_no_clobber(&staged, path)?;
    sync_directory(parent)?;
    remove_regular_staged_file(&staged)?;
    sync_directory(parent)?;
    open_existing(path, busy_timeout)
}

/// Deterministic, same-directory path reserved for fresh SQLite bootstrap.
pub fn staged_database_path(final_path: &Path) -> Result<PathBuf> {
    let parent = final_path.parent().ok_or_else(|| {
        ScmSchemaError::InvalidMetadata("database path has no parent directory".into())
    })?;
    let name = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            ScmSchemaError::InvalidMetadata(format!(
                "database filename is not valid UTF-8: {}",
                final_path.display()
            ))
        })?;
    Ok(parent.join(format!(".{name}.initializing")))
}

fn remove_staged_for_retry(staged: &Path) -> Result<()> {
    for candidate in [
        staged.to_path_buf(),
        sidecar_path(staged, "-wal"),
        sidecar_path(staged, "-shm"),
        sidecar_path(staged, "-journal"),
    ] {
        remove_regular_staged_file(&candidate)?;
    }
    let parent = staged.parent().ok_or_else(|| {
        ScmSchemaError::InvalidMetadata("staged database has no parent directory".into())
    })?;
    sync_directory(parent)?;
    Ok(())
}

fn remove_regular_staged_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path)?;
            Ok(())
        }
        Ok(_) => Err(ScmSchemaError::InvalidStagedDatabase {
            path: path.to_path_buf(),
            reason: "expected a regular non-link file".into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn checkpoint_clean(connection: &Connection, staged: &Path) -> Result<()> {
    let (busy, remaining, checkpointed): (u32, u32, u32) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 || remaining != checkpointed {
        return Err(ScmSchemaError::InvalidStagedDatabase {
            path: staged.to_path_buf(),
            reason: format!(
                "WAL checkpoint was incomplete: busy={busy}, remaining={remaining}, checkpointed={checkpointed}"
            ),
        });
    }
    Ok(())
}

fn require_clean_stage_sidecars(staged: &Path) -> Result<()> {
    for sidecar in [sidecar_path(staged, "-wal"), sidecar_path(staged, "-shm")] {
        match fs::symlink_metadata(&sidecar) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() == 0 =>
            {
                fs::remove_file(&sidecar)?;
            }
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Err(ScmSchemaError::InvalidStagedDatabase {
                    path: sidecar,
                    reason: "checkpoint left a non-empty SQLite sidecar".into(),
                });
            }
            Ok(_) => {
                return Err(ScmSchemaError::InvalidStagedDatabase {
                    path: sidecar,
                    reason: "SQLite sidecar is not a regular non-link file".into(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn publish_no_clobber(staged: &Path, final_path: &Path) -> Result<()> {
    match fs::hard_link(staged, final_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(ScmSchemaError::AlreadyExists(final_path.to_path_buf()))
        }
        Err(error) => Err(error.into()),
    }
}

/// Opens an existing native SCM V1 database for writes without creating or
/// migrating anything.
pub fn open_existing(path: &Path, busy_timeout: Duration) -> Result<Connection> {
    validate_embedded_checksum()?;
    require_existing_file(path)?;
    // A creation interrupted after publishing leaves the staged name as a
    // second link to the database, which the guarded VFS refuses to write.
    let staged = staged_database_path(path)?;
    if fs::symlink_metadata(&staged).is_ok() {
        remove_staged_for_retry(&staged)?;
    }
    let connection = open_guarded_writer(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    configure_common(&connection, busy_timeout)?;
    validate_schema_once(&connection, path)?;
    configure_writer(&connection, busy_timeout)?;
    validate_writer_pragmas(&connection, busy_timeout)?;
    // Memory-mapped reads of the database file during the write transaction
    // (admission reads many rows); writes still go through the WAL.
    connection.pragma_update(None, "mmap_size", 256i64 * 1024 * 1024)?;
    Ok(connection)
}

/// Verifies the row-level identity bound by the authenticated FORMAT marker.
/// Fresh creation writes this row in the baseline transaction; initialization
/// rechecks it before publishing Ready, including after an interrupted resume.
pub(crate) fn validate_store_identity(connection: &Connection, store_id: &StoreId) -> Result<()> {
    let expected_payload = String::from_utf8(
        canonical_json(&serde_json::json!({"storeId": store_id.as_str()}))
            .map_err(|error| ScmSchemaError::InvalidMetadata(error.to_string()))?,
    )
    .map_err(|error| ScmSchemaError::InvalidMetadata(error.to_string()))?;
    let rows = {
        let mut statement =
            connection.prepare("SELECT store_id, payload_json FROM stores ORDER BY store_id")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if rows == [(store_id.as_str().to_owned(), expected_payload)] {
        Ok(())
    } else {
        Err(ScmSchemaError::InvalidMetadata(format!(
            "stores row does not exactly match FORMAT StoreId {store_id}"
        )))
    }
}

/// Opens an existing validated database for an exclusively locked writer
/// without creating schema, running migrations, or changing persistent
/// pragmas. `busy_timeout` and `foreign_keys` are connection-local settings.
///
/// The caller must already hold the native Store's exclusive lock. SQLite may
/// maintain its normal WAL sidecars when the returned connection is used.
pub(crate) fn open_writer_existing_no_mutation(
    path: &Path,
    busy_timeout: Duration,
    store_id: &StoreId,
    durability: WriterDurability,
) -> Result<Connection> {
    // Validate through the immutable reader before constructing the writer.
    // A normal WAL-mode read would create `-wal`/`-shm` sidecars merely by
    // inspecting schema; the writer connection is therefore returned before
    // issuing any SQL against it.
    let reader = open_read_only(path, busy_timeout)?;
    validate_store_identity(&reader, store_id)?;
    drop(reader);
    let connection = open_guarded_writer(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(busy_timeout)?;
    configure_session_writer(&connection, durability)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(connection)
}

/// The session settings of a writer on an exclusively locked Store (council
/// s3, D2). All are connection-local; none changes the database file.
///
/// - Durability (`WriterDurability`): `synchronous=FULL` (each COMMIT
///   flushes the WAL) for every writer, except a session that ends with
///   `finish_writer` on every successful path: `synchronous=NORMAL`, a
///   COMMIT appends to the WAL without flushing it, and the session's
///   durability point is that checked checkpoint, which flushes the WAL,
///   copies it into the database and flushes the database before the
///   session reports success. Nothing that depends on a commit happens
///   between it and that checkpoint, so a power loss there equals one just
///   before the commit, which recovery covers (council s5 r1: only such
///   sessions may use NORMAL).
/// - `locking_mode=EXCLUSIVE`, set before the first read: the WAL index
///   lives in this connection's memory, so no `-shm` file is created and
///   deleted per session. Readers open the database `immutable=1` and take
///   no SQLite lock; the Store lock already excludes them.
/// - A persistent WAL truncated to zero bytes (`journal_size_limit=0`) at
///   every checkpoint and close: no `-wal` file is created and deleted per
///   session. Readers already accept an empty WAL.
///
/// Measured on a fast-forward Room merge (three sessions): a commit costs
/// 1.5–2.5 ms instead of 3.5–6 ms; the checkpoint is unchanged.
fn configure_session_writer(connection: &Connection, durability: WriterDurability) -> Result<()> {
    let mode: String =
        connection.query_row("PRAGMA locking_mode = EXCLUSIVE", [], |row| row.get(0))?;
    require_pragma("locking_mode", "exclusive", mode.to_ascii_lowercase())?;
    connection.pragma_update(
        None,
        "synchronous",
        match durability {
            WriterDurability::EachCommit => "FULL",
            WriterDurability::CheckedFinish => "NORMAL",
        },
    )?;
    connection.pragma_update(None, "journal_size_limit", 0)?;
    let mut persist: std::os::raw::c_int = 1;
    // SAFETY: the handle belongs to this open connection, "main" names its
    // database, and SQLITE_FCNTL_PERSIST_WAL reads and writes one int
    // through the pointer, which outlives the call.
    let code = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            connection.handle(),
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_PERSIST_WAL,
            (&raw mut persist).cast(),
        )
    };
    if code != rusqlite::ffi::SQLITE_OK {
        return Err(ScmSchemaError::InvalidMetadata(format!(
            "SQLite refused a persistent WAL (code {code})"
        )));
    }
    Ok(())
}

/// When a writer's commits become durable (`configure_session_writer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriterDurability {
    /// At each COMMIT (`synchronous=FULL`).
    EachCommit,
    /// At `finish_writer`, which every successful path of the session must
    /// call before acting on its commits (`synchronous=NORMAL`).
    CheckedFinish,
}

/// Ends a writer session: the checked durability point of every commit it
/// made (`configure_session_writer`). A `TRUNCATE` checkpoint flushes the
/// WAL, copies every frame into the database, flushes it and empties the
/// WAL; it must report no busy reader and every frame copied, the close
/// must succeed, and no frame may remain. Only then may the caller act on
/// the commits (touch a checkout, acknowledge).
pub(crate) fn finish_writer(connection: Connection, path: &Path) -> Result<()> {
    let _span = crate::perf::span("sqlite.finish_writer");
    let (busy, log, checkpointed): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    connection.close().map_err(|(_, error)| error)?;
    if busy != 0 || log != checkpointed || has_uncheckpointed_wal(path)? {
        return Err(ScmSchemaError::UncheckpointedWal(sidecar_path(
            path, "-wal",
        )));
    }
    Ok(())
}

/// Opens an existing database read-only. Missing files and foreign/legacy
/// databases are refused; this function never creates schema, directories, or
/// WAL sidecars and never checkpoints. The caller must hold the store's read
/// lock so no writer can appear between the WAL check and immutable open. A
/// non-empty WAL is refused. Because immutable SQLite reports connection-local
/// `journal_mode=delete`, persistent WAL is verified directly from the
/// database header instead of issuing a mutating PRAGMA.
pub fn open_read_only(path: &Path, busy_timeout: Duration) -> Result<Connection> {
    validate_embedded_checksum()?;
    require_existing_file(path)?;
    refuse_uncheckpointed_wal(path)?;
    verify_persistent_wal(path)?;
    let uri = immutable_file_uri(path)?;
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(busy_timeout)?;
    // Memory-mapped reads: a fresh read connection otherwise pays one read
    // syscall per page it touches. Read-only and immutable, so no writes.
    connection.pragma_update(None, "mmap_size", 256i64 * 1024 * 1024)?;
    validate_schema_once(&connection, path)?;
    let synchronous: u32 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    require_pragma("synchronous", "2", synchronous.to_string())?;
    Ok(connection)
}

/// Opens raw SQLite metadata for audit without trusting or validating the SCM
/// schema. The immutable URI prevents SQLite from creating sidecars or
/// checkpointing. A non-empty WAL is refused before SQLite is opened so audit
/// callers can report the unsafe snapshot as a typed finding.
pub(crate) fn open_audit_read_only(path: &Path, busy_timeout: Duration) -> Result<Connection> {
    require_existing_file(path)?;
    refuse_uncheckpointed_wal(path)?;
    let uri = immutable_file_uri(path)?;
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(busy_timeout)?;
    Ok(connection)
}

/// The version (length, modification time) of each database file a check
/// last passed for, one entry per path: a new version replaces the old one,
/// so a long-lived process keeps one entry per Store, not one per commit.
type FileVersions = std::sync::Mutex<
    std::collections::HashMap<std::path::PathBuf, (u64, Option<std::time::SystemTime>)>,
>;

fn current_version(path: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    fs::metadata(path)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok()))
}

/// Runs `check` unless it last passed for this very version of `path`.
fn once_per_version(
    versions: &FileVersions,
    path: &Path,
    check: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let version = current_version(path);
    if version.is_some()
        && versions
            .lock()
            .is_ok_and(|map| map.get(path) == version.as_ref())
    {
        return Ok(());
    }
    check()?;
    if let (Some(version), Ok(mut map)) = (version, versions.lock()) {
        map.insert(path.to_path_buf(), version);
    }
    Ok(())
}

fn verify_persistent_wal(path: &Path) -> Result<()> {
    // The header's journal-mode bytes only change with the file.
    static VERIFIED: std::sync::OnceLock<FileVersions> = std::sync::OnceLock::new();
    once_per_version(VERIFIED.get_or_init(Default::default), path, || {
        verify_persistent_wal_header(path)
    })
}

fn verify_persistent_wal_header(path: &Path) -> Result<()> {
    let mut header = [0_u8; 20];
    fs::File::open(path)?.read_exact(&mut header)?;
    let actual = if header.starts_with(b"SQLite format 3\0") && header[18..20] == [2, 2] {
        "wal"
    } else {
        "not-wal"
    };
    require_pragma("journal_mode", "wal", actual.into())
}

fn refuse_uncheckpointed_wal(path: &Path) -> Result<()> {
    if has_uncheckpointed_wal(path)? {
        return Err(ScmSchemaError::UncheckpointedWal(sidecar_path(
            path, "-wal",
        )));
    }
    Ok(())
}

pub(crate) fn has_uncheckpointed_wal(path: &Path) -> Result<bool> {
    let wal_path = sidecar_path(path, "-wal");
    Ok(wal_path.is_file() && fs::metadata(&wal_path)?.len() > 0)
}

/// Folds a WAL left by a writer that died before closing back into the
/// database. SQLite replays committed frames and discards a torn tail, exactly
/// as it would for the next writer. The caller must hold the Store's exclusive
/// lock, so no live connection can own those frames.
pub(crate) fn checkpoint_orphaned_wal(path: &Path, busy_timeout: Duration) -> Result<()> {
    validate_embedded_checksum()?;
    require_existing_file(path)?;
    verify_persistent_wal(path)?;
    let connection = open_guarded_writer(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(busy_timeout)?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    let busy: i64 =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    drop(connection);
    if busy != 0 || has_uncheckpointed_wal(path)? {
        return Err(ScmSchemaError::UncheckpointedWal(sidecar_path(
            path, "-wal",
        )));
    }
    Ok(())
}

/// Opens the Store's database for a writer through the guarded VFS (council
/// s13 Q7): it opens the database and its `-wal` only, each a regular file
/// with a single link (checked on the handle SQLite opened, on Windows), and
/// never sees a rollback journal, so a journal planted beside the database
/// is never replayed (nor the super-journal it names deleted). The
/// connection takes `locking_mode=EXCLUSIVE` before its first read, so its
/// WAL index lives in its memory and no `-shm` is ever opened (SQLite opens
/// one through the file's methods, which the VFS does not see). Every writer
/// holds the Store's exclusive lock; readers open the database `immutable=1`
/// and take no SQLite lock.
///
/// Always `SQLITE_OPEN_NOFOLLOW`: SQLite refuses a path whose resolution
/// follows a link (council s15 r1). A database or `-wal` name with another
/// link (a hard-link backup) is first given a file of its own
/// (`take_back_linked_names`).
fn open_guarded_writer(path: &Path, flags: OpenFlags) -> Result<Connection> {
    if !super::guarded_vfs::register() {
        return Err(ScmSchemaError::InvalidMetadata(
            "the guarded SQLite VFS is unavailable".into(),
        ));
    }
    take_back_linked_names(path)?;
    let connection = Connection::open_with_flags_and_vfs(
        path,
        flags | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        super::guarded_vfs::MAIN_AND_WAL,
    )?;
    let mode: String =
        connection.query_row("PRAGMA locking_mode = EXCLUSIVE", [], |row| row.get(0))?;
    require_pragma("locking_mode", "exclusive", mode.to_ascii_lowercase())?;
    Ok(connection)
}

/// Gives the database at `path` and its `-wal` names of their own when
/// another link shares their file (a hard-link backup, `cp -al`), as git,
/// which never writes into an existing file, keeps such a backup intact
/// (council s15 r1). The caller holds the Store's exclusive lock.
/// - The database: its bytes are copied to a staged file in the same
///   folder, synced, and renamed over its name; the other link keeps its
///   bytes.
/// - An empty `-wal`: its name is removed (SQLite makes a new one).
/// - A `-wal` holding frames is refused: they belong to the database the
///   other link also names.
fn take_back_linked_names(path: &Path) -> Result<()> {
    let _span = crate::perf::span("sqlite.take_back");
    let linked = |path: &Path| -> Result<bool> {
        Ok(super::guarded_vfs::link_count(path)?.is_some_and(|links| links > 1))
    };
    let wal = sidecar_path(path, "-wal");
    if linked(&wal)? {
        if fs::symlink_metadata(&wal)?.len() != 0 {
            return Err(ScmSchemaError::InvalidMetadata(format!(
                "{} holds frames and has another hard link (a hard-link backup?): \
                 copy the folder instead of linking it",
                wal.display()
            )));
        }
        fs::remove_file(&wal)?;
    }
    if !linked(path)? {
        return Ok(());
    }
    let parent = path.parent().ok_or_else(|| {
        ScmSchemaError::InvalidMetadata("database path has no parent directory".into())
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ScmSchemaError::InvalidMetadata("database name is not UTF-8".into()))?;
    let staged = parent.join(format!(".{name}.unlinking"));
    remove_regular_staged_file(&staged)?;
    {
        let mut source = crate::layout::open_read_no_follow(path)?;
        let mut copy = create_new_file_no_follow(&staged)?;
        std::io::copy(&mut source, &mut copy)?;
        copy.sync_all()?;
    }
    super::format::atomic_replace(&staged, path)?;
    sync_directory(parent)?;
    if linked(path)? {
        return Err(ScmSchemaError::InvalidMetadata(format!(
            "{} still has another hard link",
            path.display()
        )));
    }
    Ok(())
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn immutable_file_uri(path: &Path) -> Result<String> {
    // The canonical form of a Store's database path, resolved once per
    // process (its Store root is pinned and its identity checked by every
    // session that uses the connection).
    static URIS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, String>>,
    > = std::sync::OnceLock::new();
    let uris = URIS.get_or_init(Default::default);
    if let Some(uri) = uris.lock().ok().and_then(|map| map.get(path).cloned()) {
        return Ok(uri);
    }
    let uri = immutable_file_uri_uncached(path)?;
    if let Ok(mut map) = uris.lock() {
        map.insert(path.to_path_buf(), uri.clone());
    }
    Ok(uri)
}

fn immutable_file_uri_uncached(path: &Path) -> Result<String> {
    let canonical = fs::canonicalize(path)?;
    let normalized = canonical
        .to_str()
        .ok_or_else(|| {
            ScmSchemaError::InvalidMetadata(format!(
                "SQLite path is not valid UTF-8: {}",
                canonical.display()
            ))
        })?
        .replace('\\', "/");
    let normalized = if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = normalized.strip_prefix("//?/") {
        rest.to_owned()
    } else {
        normalized
    };
    let mut uri = String::with_capacity(normalized.len() + 24);
    uri.push_str("file:");
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in normalized.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(byte));
        } else {
            uri.push('%');
            uri.push(char::from(HEX[usize::from(byte >> 4)]));
            uri.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    uri.push_str("?immutable=1");
    Ok(uri)
}

/// Runs one closure under `BEGIN IMMEDIATE`, committing only on success.
pub fn with_immediate_transaction<T>(
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let value = operation(&transaction)?;
    transaction.commit()?;
    Ok(value)
}

fn validate_embedded_checksum() -> Result<()> {
    // A pure function of this binary's embedded SQL: computed once.
    static VALID: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *VALID.get_or_init(|| {
        schema_checksum() == SCHEMA_CHECKSUM && schema_v3_checksum() == SCHEMA_V3_CHECKSUM
    }) {
        return Ok(());
    }
    for (actual, published) in [
        (schema_checksum(), SCHEMA_CHECKSUM),
        (schema_v3_checksum(), SCHEMA_V3_CHECKSUM),
    ] {
        if actual != published {
            return Err(ScmSchemaError::EmbeddedChecksumMismatch {
                published: published.into(),
                actual,
            });
        }
    }
    Ok(())
}

/// The baseline version of a validated database (2, or 3 for format 6).
pub(crate) fn schema_version(connection: &Connection) -> Result<u32> {
    Ok(connection.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Refuses a database whose baseline is not the one of the Store format
/// that names it: a format 6 Store holds v3, formats 2 and 5 hold v2.
pub(crate) fn require_baseline_for_format(
    connection: &Connection,
    format_version: u32,
) -> Result<()> {
    let expected = super::schema::baseline_for_format(format_version).version;
    let found = schema_version(connection)?;
    if found != expected {
        return Err(ScmSchemaError::UnsupportedSchema {
            found,
            supported: expected,
        });
    }
    Ok(())
}

fn require_existing_file(path: &Path) -> Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(ScmSchemaError::NotFound(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(ScmSchemaError::NotFound(path.to_path_buf()))
        }
        Err(error) => Err(error.into()),
    }
}

fn configure_common(connection: &Connection, busy_timeout: Duration) -> Result<()> {
    connection.busy_timeout(busy_timeout)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    let foreign_keys: u32 =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    require_pragma("foreign_keys", "1", foreign_keys.to_string())
}

fn configure_writer(connection: &Connection, busy_timeout: Duration) -> Result<()> {
    configure_common(connection, busy_timeout)?;
    let journal_mode: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    require_pragma("journal_mode", "wal", journal_mode.to_ascii_lowercase())?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn validate_writer_pragmas(connection: &Connection, busy_timeout: Duration) -> Result<()> {
    let foreign_keys: u32 =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    require_pragma("foreign_keys", "1", foreign_keys.to_string())?;
    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    require_pragma("journal_mode", "wal", journal_mode.to_ascii_lowercase())?;
    let synchronous: u32 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    require_pragma("synchronous", "2", synchronous.to_string())?;
    let actual_timeout: u64 =
        connection.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
    let expected_timeout = u64::try_from(busy_timeout.as_millis()).map_err(|_| {
        ScmSchemaError::InvalidMetadata("busy timeout exceeds u64 milliseconds".into())
    })?;
    require_pragma(
        "busy_timeout",
        &expected_timeout.to_string(),
        actual_timeout.to_string(),
    )
}

/// `validate_schema` once per database file state (path, length, last
/// write): the schema only changes through a write, which moves both. A
/// read-only opening of an unchanged file needs no new fingerprint.
fn validate_schema_once(connection: &Connection, path: &Path) -> Result<()> {
    static VALIDATED: std::sync::OnceLock<FileVersions> = std::sync::OnceLock::new();
    once_per_version(VALIDATED.get_or_init(Default::default), path, || {
        let _span = crate::perf::span("sqlite.validate_schema");
        validate_schema(connection, path)
    })
}

fn validate_schema(connection: &Connection, path: &Path) -> Result<()> {
    let tables = table_names(connection)?;
    if !tables.iter().any(|table| table == "meta") {
        return Err(ScmSchemaError::ForeignDatabase {
            path: path.to_path_buf(),
            tables,
        });
    }
    // A database is checked against the baseline whose exact table set it
    // has; anything else is checked (and refused) as v2, as before format 6.
    let baseline = if tables.len() == REQUIRED_TABLES_V3.len()
        && REQUIRED_TABLES_V3
            .iter()
            .all(|table| tables.iter().any(|actual| actual == table))
    {
        &BASELINE_V3
    } else {
        &BASELINE_V2
    };
    validate_schema_against(connection, &tables, baseline)
}

fn validate_schema_against(
    connection: &Connection,
    tables: &[String],
    baseline: &Baseline,
) -> Result<()> {
    validate_required_tables(tables, baseline.required_tables)?;
    let fingerprint = schema_fingerprint(connection)?;
    if fingerprint != baseline.fingerprint {
        return Err(ScmSchemaError::SchemaFingerprintMismatch {
            expected: baseline.fingerprint.into(),
            actual: fingerprint,
        });
    }

    let meta_count: u32 =
        connection.query_row("SELECT COUNT(*) FROM meta", [], |row| row.get(0))?;
    if meta_count != 1 {
        return Err(ScmSchemaError::InvalidMetadata(format!(
            "meta must contain exactly one row, found {meta_count}"
        )));
    }
    let (magic, version, checksum): (String, u32, String) = connection.query_row(
        "SELECT magic, schema_version, schema_checksum FROM meta WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if magic != SCHEMA_MAGIC {
        return Err(ScmSchemaError::MagicMismatch {
            expected: SCHEMA_MAGIC.into(),
            actual: magic,
        });
    }
    if version != baseline.version {
        return Err(ScmSchemaError::UnsupportedSchema {
            found: version,
            supported: baseline.version,
        });
    }
    if checksum != baseline.checksum {
        return Err(ScmSchemaError::ChecksumMismatch {
            expected: baseline.checksum.into(),
            actual: checksum,
        });
    }

    let application_id: u32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    require_pragma(
        "application_id",
        &APPLICATION_ID.to_string(),
        application_id.to_string(),
    )?;
    let user_version: u32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if user_version != baseline.version {
        return Err(ScmSchemaError::UnsupportedSchema {
            found: user_version,
            supported: baseline.version,
        });
    }

    let migration_count: u32 =
        connection.query_row("SELECT COUNT(*) FROM migrations", [], |row| row.get(0))?;
    if migration_count != 1 {
        return Err(ScmSchemaError::InvalidMetadata(format!(
            "migrations must contain only the V1 baseline, found {migration_count} rows"
        )));
    }
    let (name, migration_checksum): (String, String) = connection.query_row(
        "SELECT name, checksum FROM migrations WHERE version = ?1",
        [baseline.version],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if name != baseline.name || migration_checksum != baseline.checksum {
        return Err(ScmSchemaError::InvalidMetadata(
            "baseline migration ledger does not match the embedded schema".into(),
        ));
    }
    Ok(())
}

fn validate_required_tables(actual: &[String], required: &[&str]) -> Result<()> {
    let expected: std::collections::BTreeSet<_> =
        required.iter().map(|name| (*name).to_owned()).collect();
    let actual: std::collections::BTreeSet<_> = actual.iter().cloned().collect();
    let missing_tables = expected.difference(&actual).cloned().collect::<Vec<_>>();
    let unexpected_tables = actual.difference(&expected).cloned().collect::<Vec<_>>();
    if missing_tables.is_empty() && unexpected_tables.is_empty() {
        Ok(())
    } else {
        Err(ScmSchemaError::SchemaStructureMismatch {
            missing_tables,
            unexpected_tables,
        })
    }
}

/// Fingerprints the real ordered SQLite catalog, not just the metadata claim.
pub fn schema_fingerprint(connection: &Connection) -> Result<String> {
    let mut statement = connection.prepare(
        "SELECT type, name, tbl_name, sql FROM sqlite_schema \
         WHERE type IN ('table', 'index', 'trigger', 'view') \
           AND name NOT LIKE 'sqlite_%' \
         ORDER BY type, name",
    )?;
    let mut rows = statement.query([])?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-sqlite-schema-v1");
    hasher.update(&[0]);
    while let Some(row) = rows.next()? {
        for index in 0..4 {
            let value: Option<String> = row.get(index)?;
            match value {
                Some(value) => {
                    hasher.update(&(value.len() as u64).to_le_bytes());
                    hasher.update(value.as_bytes());
                }
                None => {
                    hasher.update(&u64::MAX.to_le_bytes());
                }
            }
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn table_names(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_schema \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| row.get(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn require_pragma(name: &'static str, expected: &str, actual: String) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(ScmSchemaError::PragmaMismatch {
            name,
            expected: expected.into(),
            actual,
        })
    }
}

fn unix_time_ms() -> Result<i64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            ScmSchemaError::InvalidMetadata(format!("system clock precedes Unix epoch: {error}"))
        })?;
    i64::try_from(duration.as_millis())
        .map_err(|_| ScmSchemaError::InvalidMetadata("system time exceeds SQLite INTEGER".into()))
}

#[cfg(test)]
mod tests;
