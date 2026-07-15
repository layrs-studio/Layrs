//! SQLite connection policy, runtime checks, and migration runner.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use crate::error::{StoreError, StoreResult};
use crate::fault::{self, FaultPoint};
use crate::layout::open_read_no_follow;
use crate::schema::{CURRENT_SCHEMA_VERSION, MIGRATIONS, Migration};

const MIN_SQLITE_VERSION: (u32, u32, u32) = (3, 46, 0);

#[derive(Debug, Clone, Copy)]
pub(crate) struct SqliteOptions {
    pub wal: bool,
    pub busy_timeout: Duration,
    pub wal_autocheckpoint_pages: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PragmaSnapshot {
    pub sqlite_version: String,
    pub journal_mode: String,
    pub synchronous: i64,
    pub foreign_keys: bool,
    pub busy_timeout_ms: u64,
    pub trusted_schema: bool,
    pub wal_autocheckpoint_pages: u32,
    pub query_only: bool,
}

pub(crate) fn open_initialized(path: &Path, options: SqliteOptions) -> StoreResult<Connection> {
    open_initialized_to(path, options, CURRENT_SCHEMA_VERSION)
}

fn open_initialized_to(
    path: &Path,
    options: SqliteOptions,
    target_version: u32,
) -> StoreResult<Connection> {
    validate_options(options)?;
    verify_sqlite_version()?;

    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    configure(&connection, options, true, false)?;
    apply_migrations(&mut connection, target_version)?;
    verify_applied_migrations(&connection, target_version)?;
    let snapshot = pragma_snapshot(&connection)?;
    verify_snapshot(&snapshot, options, false)?;
    Ok(connection)
}

pub(crate) fn open_read_only(path: &Path, options: SqliteOptions) -> StoreResult<Connection> {
    if !path.is_file() {
        return Err(StoreError::NotFound(format!(
            "metadata database {}",
            path.display()
        )));
    }

    validate_options(options)?;
    verify_sqlite_version()?;
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
    configure(&connection, options, false, true)?;
    verify_applied_migrations(&connection, CURRENT_SCHEMA_VERSION)?;
    let snapshot = pragma_snapshot(&connection)?;
    verify_snapshot(&snapshot, options, true)?;
    Ok(connection)
}

pub(crate) fn open_read_only_unchecked(
    path: &Path,
    options: SqliteOptions,
) -> StoreResult<Connection> {
    if !path.is_file() {
        return Err(StoreError::NotFound(format!(
            "metadata database {}",
            path.display()
        )));
    }

    validate_options(options)?;
    verify_sqlite_version()?;
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
    configure(&connection, options, false, true)?;
    Ok(connection)
}

fn refuse_uncheckpointed_wal(path: &Path) -> StoreResult<()> {
    let wal_path = sidecar_path(path, "-wal");
    if wal_path.is_file() && fs::metadata(&wal_path)?.len() > 0 {
        return Err(StoreError::Conflict(format!(
            "read-only metadata open refused because WAL is not checkpointed: {}",
            wal_path.display()
        )));
    }
    Ok(())
}

fn verify_persistent_wal(path: &Path) -> StoreResult<()> {
    let mut header = [0_u8; 20];
    open_read_no_follow(path)?.read_exact(&mut header)?;
    let actual = if header.starts_with(b"SQLite format 3\0") && header[18..20] == [2, 2] {
        "wal"
    } else {
        "not-wal"
    };
    if actual != "wal" {
        return Err(StoreError::PragmaMismatch {
            name: "journal_mode",
            expected: "wal".to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}

pub(crate) fn persistent_journal_mode(path: &Path) -> StoreResult<String> {
    verify_persistent_wal(path)?;
    Ok("wal".to_string())
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn immutable_file_uri(path: &Path) -> StoreResult<String> {
    let canonical = fs::canonicalize(path)?;
    let path = canonical
        .to_str()
        .ok_or_else(|| StoreError::InvalidState("SQLite path is not valid UTF-8".to_string()))?;
    let normalized = path.replace('\\', "/");
    #[cfg(windows)]
    let normalized = if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = normalized.strip_prefix("//?/") {
        rest.to_string()
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

fn validate_options(options: SqliteOptions) -> StoreResult<()> {
    if !options.wal {
        return Err(StoreError::InvalidConfig(
            "WAL cannot be disabled for the durable local store".to_string(),
        ));
    }
    if options.busy_timeout.is_zero() {
        return Err(StoreError::InvalidConfig(
            "SQLite busy timeout must be non-zero".to_string(),
        ));
    }
    if options.wal_autocheckpoint_pages == 0 {
        return Err(StoreError::InvalidConfig(
            "wal_autocheckpoint must be non-zero".to_string(),
        ));
    }
    Ok(())
}

fn verify_sqlite_version() -> StoreResult<()> {
    let version = rusqlite::version();
    let parsed = parse_version(version).ok_or_else(|| {
        StoreError::InvalidConfig(format!("cannot parse bundled SQLite version {version}"))
    })?;
    if parsed < MIN_SQLITE_VERSION {
        return Err(StoreError::InvalidConfig(format!(
            "SQLite {version} is older than required 3.46.0"
        )));
    }
    Ok(())
}

fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

fn configure(
    connection: &Connection,
    options: SqliteOptions,
    set_wal: bool,
    query_only: bool,
) -> StoreResult<()> {
    connection.busy_timeout(options.busy_timeout)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "wal_autocheckpoint", options.wal_autocheckpoint_pages)?;

    if set_wal {
        let journal_mode: String =
            connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::PragmaMismatch {
                name: "journal_mode",
                expected: "wal".to_string(),
                actual: journal_mode,
            });
        }
    }

    if query_only {
        connection.pragma_update(None, "query_only", "ON")?;
    }
    Ok(())
}

pub(crate) fn pragma_snapshot(connection: &Connection) -> StoreResult<PragmaSnapshot> {
    let sqlite_version = connection.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    let journal_mode = connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    let synchronous = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    let foreign_keys: i64 =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    let busy_timeout_ms: i64 =
        connection.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
    let trusted_schema: i64 =
        connection.pragma_query_value(None, "trusted_schema", |row| row.get(0))?;
    let wal_autocheckpoint_pages: i64 =
        connection.pragma_query_value(None, "wal_autocheckpoint", |row| row.get(0))?;
    let query_only: i64 = connection.pragma_query_value(None, "query_only", |row| row.get(0))?;

    let busy_timeout_ms = u64::try_from(busy_timeout_ms)
        .map_err(|_| StoreError::CorruptMetadata("negative PRAGMA busy_timeout".to_string()))?;
    let wal_autocheckpoint_pages = u32::try_from(wal_autocheckpoint_pages).map_err(|_| {
        StoreError::CorruptMetadata("invalid PRAGMA wal_autocheckpoint".to_string())
    })?;

    Ok(PragmaSnapshot {
        sqlite_version,
        journal_mode,
        synchronous,
        foreign_keys: foreign_keys == 1,
        busy_timeout_ms,
        trusted_schema: trusted_schema == 1,
        wal_autocheckpoint_pages,
        query_only: query_only == 1,
    })
}

fn verify_snapshot(
    snapshot: &PragmaSnapshot,
    options: SqliteOptions,
    query_only: bool,
) -> StoreResult<()> {
    let expected_journal_mode = if query_only { "delete" } else { "wal" };
    verify_value(
        "journal_mode",
        expected_journal_mode,
        snapshot.journal_mode.to_ascii_lowercase(),
    )?;
    verify_value("synchronous", "2", snapshot.synchronous.to_string())?;
    verify_value("foreign_keys", "true", snapshot.foreign_keys.to_string())?;
    verify_value(
        "trusted_schema",
        "false",
        snapshot.trusted_schema.to_string(),
    )?;
    verify_value(
        "query_only",
        &query_only.to_string(),
        snapshot.query_only.to_string(),
    )?;
    verify_value(
        "busy_timeout",
        &duration_millis(options.busy_timeout)?.to_string(),
        snapshot.busy_timeout_ms.to_string(),
    )?;
    verify_value(
        "wal_autocheckpoint",
        &options.wal_autocheckpoint_pages.to_string(),
        snapshot.wal_autocheckpoint_pages.to_string(),
    )?;
    Ok(())
}

fn duration_millis(duration: Duration) -> StoreResult<u64> {
    u64::try_from(duration.as_millis()).map_err(|_| {
        StoreError::InvalidConfig("SQLite busy timeout exceeds u64 milliseconds".to_string())
    })
}

fn verify_value(name: &'static str, expected: &str, actual: String) -> StoreResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(StoreError::PragmaMismatch {
            name,
            expected: expected.to_string(),
            actual,
        })
    }
}

fn bootstrap_migrations(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY CHECK (version > 0),
            name TEXT NOT NULL UNIQUE CHECK (length(trim(name)) > 0),
            checksum TEXT NOT NULL CHECK (length(checksum) = 64),
            applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0)
        ) STRICT;",
    )?;
    transaction.commit()?;
    Ok(())
}

fn apply_migrations(connection: &mut Connection, target_version: u32) -> StoreResult<()> {
    if target_version > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema {
            found: target_version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    bootstrap_migrations(connection)?;
    let applied = load_applied_migrations(connection)?;
    if let Some((version, _, _)) = applied.last() {
        if *version > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchema {
                found: *version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }
    }

    for migration in MIGRATIONS {
        let existing = applied
            .iter()
            .find(|(version, _, _)| *version == migration.version);
        if let Some((_, name, checksum)) = existing {
            verify_migration(migration, name, checksum)?;
            continue;
        }
        if migration.version > target_version {
            break;
        }
        if applied
            .iter()
            .any(|(version, _, _)| *version > migration.version)
        {
            return Err(StoreError::CorruptMetadata(format!(
                "missing migration {} before a later applied migration",
                migration.version
            )));
        }
        apply_migration(connection, migration)?;
    }
    Ok(())
}

fn apply_migration(connection: &mut Connection, migration: &Migration) -> StoreResult<()> {
    verify_published_sql(migration)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(migration.sql)?;
    transaction.execute(
        "INSERT INTO schema_migrations (version, name, checksum, applied_at_ms)
         VALUES (?1, ?2, ?3, CAST(unixepoch('subsec') * 1000 AS INTEGER))",
        params![migration.version, migration.name, migration.checksum()],
    )?;
    transaction.pragma_update(None, "user_version", migration.version)?;
    transaction.execute(
        "UPDATE store_meta
         SET schema_version = ?1,
             updated_at_ms = MAX(updated_at_ms, CAST(unixepoch('subsec') * 1000 AS INTEGER))
         WHERE singleton = 1",
        [migration.version],
    )?;
    fault::hit(FaultPoint::BeforeMetadataCommit)?;
    transaction.commit()?;
    fault::hit(FaultPoint::AfterMetadataCommit)?;
    Ok(())
}

fn load_applied_migrations(connection: &Connection) -> StoreResult<Vec<(u32, String, String)>> {
    let mut statement = connection
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version ASC")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let mut applied = Vec::new();
    for row in rows {
        applied.push(row?);
    }
    Ok(applied)
}

pub(crate) fn verify_applied_migrations(
    connection: &Connection,
    required_version: u32,
) -> StoreResult<()> {
    let table_exists: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if table_exists.is_none() {
        return Err(StoreError::CorruptMetadata(
            "schema_migrations is missing".to_string(),
        ));
    }

    let applied = load_applied_migrations(connection)?;
    let latest = applied.last().map_or(0, |(version, _, _)| *version);
    if latest > CURRENT_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedSchema {
            found: latest,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    if latest != required_version {
        return Err(StoreError::CorruptMetadata(format!(
            "schema version {latest} does not match required version {required_version}"
        )));
    }
    if applied.len() != required_version as usize {
        return Err(StoreError::CorruptMetadata(
            "schema migration history has gaps".to_string(),
        ));
    }
    for migration in MIGRATIONS.iter().take(required_version as usize) {
        let (_, name, checksum) = applied
            .iter()
            .find(|(version, _, _)| *version == migration.version)
            .ok_or_else(|| {
                StoreError::CorruptMetadata(format!("migration {} is missing", migration.version))
            })?;
        verify_migration(migration, name, checksum)?;
    }

    let user_version: u32 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let meta_version: u32 = connection.query_row(
        "SELECT schema_version FROM store_meta WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if user_version != latest || meta_version != latest {
        return Err(StoreError::CorruptMetadata(format!(
            "schema version disagreement: migrations={latest}, user_version={user_version}, store_meta={meta_version}"
        )));
    }
    Ok(())
}

fn verify_migration(migration: &Migration, name: &str, actual: &str) -> StoreResult<()> {
    if name != migration.name {
        return Err(StoreError::CorruptMetadata(format!(
            "migration {} name mismatch: expected {}, got {name}",
            migration.version, migration.name
        )));
    }
    verify_published_sql(migration)?;
    let expected = migration.checksum();
    if actual != expected {
        return Err(StoreError::MigrationChecksum {
            version: migration.version,
            expected,
            actual: actual.to_string(),
        });
    }
    Ok(())
}

fn verify_published_sql(migration: &Migration) -> StoreResult<()> {
    let actual = migration.sql_checksum();
    if actual != migration.published_checksum {
        return Err(StoreError::InvalidState(format!(
            "published migration {} SQL changed: expected {}, got {actual}",
            migration.version, migration.published_checksum
        )));
    }
    Ok(())
}

pub(crate) fn checkpoint_full(connection: &Connection) -> StoreResult<()> {
    let (busy, _log, _checkpointed): (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(FULL)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        return Err(StoreError::Conflict(
            "SQLite WAL checkpoint remained busy".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn migration_upgrade_applies_remaining_versions() {
        let root = unique_test_dir("upgrade");
        fs::create_dir_all(&root).expect("create root");
        let path = root.join("metadata.sqlite3");
        let options = test_options();

        let first = open_initialized_to(&path, options, 1).expect("open schema v1");
        let first_version: u32 = first
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read v1");
        assert_eq!(first_version, 1);
        drop(first);

        let upgraded = open_initialized(&path, options).expect("upgrade schema");
        let final_version: u32 = upgraded
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read current");
        assert_eq!(final_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            upgraded
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'weave_sessions'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .expect("query weave schema"),
            1
        );

        drop(upgraded);
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn published_v1_fixture_upgrades_to_current_schema() {
        let root = unique_test_dir("published-v1-fixture");
        fs::create_dir_all(&root).expect("create root");
        let path = root.join("metadata.sqlite3");
        let fixture = Connection::open(&path).expect("open fixture database");
        fixture
            .execute_batch(include_str!("../tests/fixtures/schema-v1.sql"))
            .expect("install published v1 fixture");
        drop(fixture);

        let upgraded = open_initialized(&path, test_options()).expect("upgrade v1 fixture");
        verify_applied_migrations(&upgraded, CURRENT_SCHEMA_VERSION)
            .expect("verify upgraded fixture");
        let versions: Vec<(u32, String)> = {
            let mut statement = upgraded
                .prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")
                .expect("prepare migration history");
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query migration history")
                .collect::<Result<_, _>>()
                .expect("read migration history")
        };
        assert_eq!(
            versions,
            vec![
                (
                    1,
                    "23794458e9e5342f83b254e6d6406b646d7d38fe6ebf0b3d2209346f4cd2a3dd".to_string(),
                ),
                (
                    2,
                    "18e03f2784a900574f552db161718dfa0ce8935db190435dced429371a6f37bc".to_string(),
                ),
            ]
        );

        drop(upgraded);
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn bundled_sqlite_runtime_meets_pinned_minimum() {
        let parsed = parse_version(rusqlite::version()).expect("parse SQLite runtime version");
        assert!(
            parsed >= MIN_SQLITE_VERSION,
            "SQLite runtime {} is older than 3.46.0",
            rusqlite::version()
        );
    }

    #[test]
    fn migration_fault_before_commit_rolls_back_and_retry_recovers() {
        let root = unique_test_dir("migration-fault");
        fs::create_dir_all(&root).expect("create root");
        let path = root.join("metadata.sqlite3");
        let options = test_options();
        crate::fault::fail_once(FaultPoint::BeforeMetadataCommit);

        open_initialized(&path, options).expect_err("injected migration fault");
        let raw = Connection::open(&path).expect("open partially initialized database");
        let store_meta_exists: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'store_meta'",
                [],
                |row| row.get(0),
            )
            .expect("query rolled back table");
        assert_eq!(store_meta_exists, 0);
        drop(raw);

        let recovered = open_initialized(&path, options).expect("retry migrations");
        verify_applied_migrations(&recovered, CURRENT_SCHEMA_VERSION)
            .expect("verify recovered schema");
        drop(recovered);
        fs::remove_dir_all(root).expect("remove root");
    }

    fn test_options() -> SqliteOptions {
        SqliteOptions {
            wal: true,
            busy_timeout: Duration::from_secs(1),
            wal_autocheckpoint_pages: 100,
        }
    }

    fn unique_test_dir(label: &str) -> std::path::PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "layrs-store-local-sqlite-{label}-{}-{timestamp}",
            std::process::id()
        ))
    }
}
