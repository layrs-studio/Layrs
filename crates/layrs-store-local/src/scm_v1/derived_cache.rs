//! The derived caches of a Store (change indexes, ADR 0025; projection
//! cache) in one SQLite database, `metadata/derived.sqlite3`.
//!
//! Each value keeps its own encoding, binding and blake3 checksum: the
//! database is only a container, and a value it returns is checked exactly
//! as a file's bytes were. It replaces one file per cache entry because a
//! command reads the caches a previous command just wrote, and a freshly
//! written data file costs several milliseconds more to open than a database
//! page on a machine with real-time antivirus scanning.
//!
//! It is a cache, like git's index: nothing in it is ever the only copy of
//! a fact.
//! - Commits are not synced (`synchronous=OFF`). A rollback journal
//!   (`journal_mode=TRUNCATE`) keeps it consistent when a process dies
//!   mid-write; a torn value fails its checksum and reads as absent.
//! - A database that is not a regular file is never opened. A writer checks
//!   the schema exactly and replaces a database that is not this cache; a
//!   reader only runs fixed queries (`trusted_schema=OFF`) and checks every
//!   value it gets, so it skips that check (a foreign database fails the
//!   queries or the values' checks).
//! - Every access is best effort: a busy or damaged database only makes the
//!   caller read the Store or the folder again.
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

const FILE: &str = "derived.sqlite3";
/// "LYRD": marks a database as this cache.
const APPLICATION_ID: i64 = 0x4C59_5244;
const SCHEMA_VERSION: i64 = 1;
const BUSY: Duration = Duration::from_secs(1);
const CHANGE_INDEX_TABLE: &str =
    "CREATE TABLE change_index (worktree TEXT PRIMARY KEY NOT NULL, bytes BLOB NOT NULL)";
const PROJECTION_TABLE: &str = "CREATE TABLE projection (root TEXT PRIMARY KEY NOT NULL, base TEXT, seq INTEGER NOT NULL, bytes BLOB NOT NULL)";

pub(crate) fn path(store_root: &Path) -> PathBuf {
    store_root.join("metadata").join(FILE)
}

/// How a caller uses the cache database.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Fixed queries whose values are checked: no schema check.
    Read,
    /// Writes to an existing database of this schema.
    Write,
    /// Writes, creating the database when missing and replacing one that is
    /// not this cache.
    Create,
}

/// The cache database of `store_root`, or `None` (missing, not a regular
/// file, or not usable for `access`).
pub(crate) fn open(store_root: &Path, access: Access) -> Option<Connection> {
    let _span = crate::perf::span("derived.open");
    let path = path(store_root);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => match open_existing(&path, access) {
            Ok(connection) => Some(connection),
            Err(_) if access == Access::Create => {
                discard(&path);
                initialize(&path).ok()
            }
            Err(_) => None,
        },
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound && access == Access::Create => {
            let _ = fs::create_dir_all(path.parent()?);
            initialize(&path).ok()
        }
        Err(_) => None,
    }
}

fn connect(path: &Path, create: bool) -> rusqlite::Result<Connection> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(BUSY)?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    Ok(connection)
}

/// A writer's settings: unsynced commits under a rollback journal.
fn configure_writer(connection: &Connection) -> rusqlite::Result<()> {
    let mode: String =
        connection.query_row("PRAGMA journal_mode = TRUNCATE", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("truncate") {
        return Err(rusqlite::Error::InvalidQuery);
    }
    connection.pragma_update(None, "synchronous", "OFF")
}

fn open_existing(path: &Path, access: Access) -> rusqlite::Result<Connection> {
    let connection = connect(path, false)?;
    if access != Access::Read {
        configure_writer(&connection)?;
        check_schema(&connection)?;
    }
    Ok(connection)
}

/// Creates the tables of an empty database (another process may be doing
/// the same: the first transaction creates them, the other finds them).
fn initialize(path: &Path) -> rusqlite::Result<Connection> {
    let mut connection = connect(path, true)?;
    configure_writer(&connection)?;
    // Only effective before the first table: a database another process
    // initialized keeps its own (identical) settings.
    connection.pragma_update(None, "page_size", 4096)?;
    connection.pragma_update(None, "auto_vacuum", "FULL")?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 0 {
        transaction.execute_batch(&format!("{CHANGE_INDEX_TABLE}; {PROJECTION_TABLE};"))?;
        transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
        transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    transaction.commit()?;
    check_schema(&connection)?;
    Ok(connection)
}

/// Exactly this cache's schema: its marker, version, two tables and their
/// primary key indexes, and nothing else (no view, trigger or index).
fn check_schema(connection: &Connection) -> rusqlite::Result<()> {
    let application: i64 = connection.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let mut statement =
        connection.prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY name")?;
    let objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let expected = [
        (
            "table",
            "change_index",
            "change_index",
            Some(CHANGE_INDEX_TABLE),
        ),
        ("table", "projection", "projection", Some(PROJECTION_TABLE)),
        (
            "index",
            "sqlite_autoindex_change_index_1",
            "change_index",
            None,
        ),
        ("index", "sqlite_autoindex_projection_1", "projection", None),
    ];
    let same = objects.len() == expected.len()
        && objects.iter().zip(expected).all(|(found, wanted)| {
            found.0 == wanted.0
                && found.1 == wanted.1
                && found.2 == wanted.2
                && found.3.as_deref() == wanted.3
        });
    if application == APPLICATION_ID && version == SCHEMA_VERSION && same {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

/// Removes a database that is not this cache (best effort: one in use stays,
/// and is then neither read nor written).
fn discard(path: &Path) {
    let _ = fs::remove_file(path);
    let mut journal = path.as_os_str().to_owned();
    journal.push("-journal");
    let _ = fs::remove_file(PathBuf::from(journal));
}

/// The value stored under `key` in `table`, at most `max_bytes` long.
/// `Ok(None)`: no such value; `Err`: unreadable or larger than allowed.
fn get(
    connection: &Connection,
    table: Table,
    key: &str,
    max_bytes: u64,
) -> rusqlite::Result<Option<Vec<u8>>> {
    let (length, value) = match table {
        Table::ChangeIndex => (
            "SELECT length(bytes) FROM change_index WHERE worktree = ?1",
            "SELECT bytes FROM change_index WHERE worktree = ?1",
        ),
        Table::Projection => (
            "SELECT length(bytes) FROM projection WHERE root = ?1",
            "SELECT bytes FROM projection WHERE root = ?1",
        ),
    };
    let Some(length) = connection
        .query_row(length, [key], |row| row.get::<_, i64>(0))
        .optional()?
    else {
        return Ok(None);
    };
    if u64::try_from(length).map_or(true, |length| length > max_bytes) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    connection
        .query_row(value, [key], |row| row.get::<_, Vec<u8>>(0))
        .optional()
}

#[derive(Clone, Copy)]
enum Table {
    ChangeIndex,
    Projection,
}

/// A change index's stored bytes.
pub(crate) enum Stored {
    Missing,
    Present(Vec<u8>),
    /// Present but unreadable (busy, damaged or too large).
    Unreadable,
}

pub(crate) fn change_index_get(store_root: &Path, worktree: &str, max_bytes: u64) -> Stored {
    let Some(connection) = open(store_root, Access::Read) else {
        return if fs::symlink_metadata(path(store_root)).is_ok() {
            Stored::Unreadable
        } else {
            Stored::Missing
        };
    };
    match get(&connection, Table::ChangeIndex, worktree, max_bytes) {
        Ok(Some(bytes)) => Stored::Present(bytes),
        Ok(None) => Stored::Missing,
        Err(_) => Stored::Unreadable,
    }
}

pub(crate) enum Presence {
    Missing,
    Present,
    /// The database exists but cannot be read.
    Unknown,
}

/// Whether a change index is stored for `worktree`, without reading it.
pub(crate) fn change_index_presence(store_root: &Path, worktree: &str) -> Presence {
    let Some(connection) = open(store_root, Access::Read) else {
        return if fs::symlink_metadata(path(store_root)).is_ok() {
            Presence::Unknown
        } else {
            Presence::Missing
        };
    };
    match connection
        .query_row(
            "SELECT 1 FROM change_index WHERE worktree = ?1",
            [worktree],
            |_| Ok(()),
        )
        .optional()
    {
        Ok(Some(())) => Presence::Present,
        Ok(None) => Presence::Missing,
        Err(_) => Presence::Unknown,
    }
}

#[cfg(test)]
fn change_index_exists(store_root: &Path, worktree: &str) -> bool {
    matches!(
        change_index_presence(store_root, worktree),
        Presence::Present
    )
}

pub(crate) fn change_index_put(store_root: &Path, worktree: &str, bytes: &[u8]) -> io::Result<()> {
    let connection = open(store_root, Access::Create)
        .ok_or_else(|| io::Error::other("the derived cache database cannot be opened"))?;
    connection
        .execute(
            "INSERT INTO change_index (worktree, bytes) VALUES (?1, ?2) \
             ON CONFLICT (worktree) DO UPDATE SET bytes = excluded.bytes",
            params![worktree, bytes],
        )
        .map(drop)
        .map_err(io::Error::other)
}

pub(crate) fn change_index_delete(store_root: &Path, worktree: &str) -> io::Result<()> {
    let Some(connection) = open(store_root, Access::Write) else {
        return if fs::symlink_metadata(path(store_root)).is_ok() {
            Err(io::Error::other(
                "the derived cache database cannot be opened",
            ))
        } else {
            Ok(())
        };
    };
    connection
        .execute("DELETE FROM change_index WHERE worktree = ?1", [worktree])
        .map(drop)
        .map_err(io::Error::other)
}

/// The projection cache's access to the database, one connection per use.
pub(crate) struct Projections(Connection);

/// A cached projection's row: its root, and the root of the complete
/// projection it is a delta to.
pub(crate) struct ProjectionRow {
    pub root: String,
    pub base: Option<String>,
}

impl Projections {
    pub(crate) fn open(store_root: &Path, access: Access) -> Option<Self> {
        open(store_root, access).map(Self)
    }

    pub(crate) fn get(&self, root: &str, max_bytes: u64) -> Option<Vec<u8>> {
        get(&self.0, Table::Projection, root, max_bytes)
            .ok()
            .flatten()
    }

    /// Marks `root` most recent; whether it was recorded.
    pub(crate) fn touch(&self, root: &str) -> bool {
        self.0
            .execute(
                "UPDATE projection SET seq = (SELECT coalesce(max(seq), 0) + 1 FROM projection) \
                 WHERE root = ?1",
                [root],
            )
            .is_ok_and(|changed| changed > 0)
    }

    /// The most recently recorded complete projection: its root and bytes.
    pub(crate) fn newest_complete(&self, max_bytes: u64) -> Option<(String, Vec<u8>)> {
        let root: String = self
            .0
            .query_row(
                "SELECT root FROM projection WHERE base IS NULL ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        let bytes = self.get(&root, max_bytes)?;
        Some((root, bytes))
    }

    pub(crate) fn insert(&self, root: &str, base: Option<&str>, bytes: &[u8]) -> io::Result<()> {
        self.0
            .execute(
                "INSERT INTO projection (root, base, seq, bytes) \
                 VALUES (?1, ?2, (SELECT coalesce(max(seq), 0) + 1 FROM projection), ?3) \
                 ON CONFLICT (root) DO UPDATE SET base = excluded.base, seq = excluded.seq, \
                 bytes = excluded.bytes",
                params![root, base, bytes],
            )
            .map(drop)
            .map_err(io::Error::other)
    }

    /// Every recorded projection, most recent first.
    pub(crate) fn rows(&self) -> Vec<ProjectionRow> {
        let Ok(mut statement) = self
            .0
            .prepare("SELECT root, base FROM projection ORDER BY seq DESC")
        else {
            return Vec::new();
        };
        statement
            .query_map([], |row| {
                Ok(ProjectionRow {
                    root: row.get(0)?,
                    base: row.get(1)?,
                })
            })
            .and_then(|rows| rows.collect())
            .unwrap_or_default()
    }

    pub(crate) fn count(&self) -> usize {
        self.0
            .query_row("SELECT count(*) FROM projection", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_or(0, |count| usize::try_from(count).unwrap_or(usize::MAX))
    }

    pub(crate) fn remove(&self, root: &str) {
        let _ = self
            .0
            .execute("DELETE FROM projection WHERE root = ?1", [root]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "layrs-derived-{}-{}-{}",
            std::process::id(),
            super::super::change_index::now(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("metadata")).unwrap();
        root
    }

    #[test]
    fn values_round_trip_and_a_foreign_database_is_replaced() {
        let store = temp();
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Missing
        ));
        change_index_put(&store, "w", b"index").unwrap();
        assert!(change_index_exists(&store, "w"));
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Present(bytes) if bytes == b"index"
        ));
        // Larger than allowed: unreadable, never truncated.
        assert!(matches!(
            change_index_get(&store, "w", 4),
            Stored::Unreadable
        ));
        let projections = Projections::open(&store, Access::Create).unwrap();
        projections.insert("a", None, b"complete a").unwrap();
        projections.insert("b", Some("a"), b"delta b").unwrap();
        assert!(projections.touch("a"));
        assert!(!projections.touch("missing"));
        assert_eq!(
            projections
                .rows()
                .iter()
                .map(|row| (row.root.as_str(), row.base.as_deref()))
                .collect::<Vec<_>>(),
            [("a", None), ("b", Some("a"))]
        );
        assert_eq!(
            projections.newest_complete(1 << 20),
            Some(("a".into(), b"complete a".to_vec()))
        );
        projections.remove("a");
        assert_eq!(projections.count(), 1);
        drop(projections);
        change_index_delete(&store, "w").unwrap();
        assert!(!change_index_exists(&store, "w"));
        // A database with another schema fails a reader's queries, and a
        // writer replaces it (one table altered is enough).
        {
            let foreign = Connection::open(path(&store)).unwrap();
            foreign
                .execute_batch("DROP TABLE projection; CREATE TABLE projection (x)")
                .unwrap();
        }
        assert!(Projections::open(&store, Access::Write).is_none());
        {
            let foreign = Connection::open(path(&store)).unwrap();
            foreign
                .execute_batch("DROP TABLE change_index; CREATE TABLE change_index (x)")
                .unwrap();
        }
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Unreadable
        ));
        change_index_put(&store, "w", b"again").unwrap();
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Present(bytes) if bytes == b"again"
        ));
        // Garbage instead of a database: the same.
        fs::write(path(&store), b"not a database at all, not even close").unwrap();
        assert!(!change_index_exists(&store, "w"));
        change_index_put(&store, "w", b"third").unwrap();
        assert!(change_index_exists(&store, "w"));
        let _ = fs::remove_dir_all(store);
    }
}
