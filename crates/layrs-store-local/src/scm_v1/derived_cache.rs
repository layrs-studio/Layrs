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
//! - Commits are not synced (`synchronous=OFF`) and the rollback journal is
//!   kept in memory (`journal_mode=MEMORY`): no file besides the database is
//!   ever written, so no other name in the Store folder (a link to a file
//!   outside it) receives writes (council s12 r1). A process killed
//!   mid-write may leave the database damaged: a torn value fails its
//!   checksum or binding and reads as absent, and a writer that meets a
//!   damaged database replaces it.
//! - Readers open the database read-only: SQLite then never replays a hot
//!   journal (`SQLITE_READONLY_ROLLBACK`), never deletes a super-journal and
//!   never writes a sidecar, while still taking the locks that keep their
//!   reads consistent with concurrent writers. A database that is not a
//!   regular file, is not in rollback mode (its header), or has a `-wal` or
//!   `-shm` (which would switch the connection to WAL mode, even empty), or
//!   a non-empty `-journal`, is never opened by a reader. A writer removes
//!   the sidecar names first (removing a name never writes to the file it
//!   names). Every access then checks the schema exactly (marker, version,
//!   the two tables and their keys, nothing else: no view or trigger) and
//!   runs fixed queries (`trusted_schema=OFF`, defensive mode); a value is
//!   read in the statement that checks its type and length.
//! - Residual risk, shared with the Store's own database: SQLite's checks
//!   happen when it locks, after ours. A process racing to plant a hot
//!   journal between a writer's check and its lock can make SQLite replay it
//!   (writing into this database) and delete the super-journal it names;
//!   and SQLite's Windows VFS does not honour `NOFOLLOW`, so a name swapped
//!   for a link after the check is followed. Closing these needs a VFS that
//!   opens nothing but the main database, for both databases (council s12
//!   r2, recorded for later).
//! - A writer never writes a database file with more than one link (a
//!   hard link planted to a file outside the Store).
//! - A writer replaces only a database proven foreign or damaged. An empty
//!   database (another process creating it) is initialized in place; one of
//!   a newer version of this cache, or busy, is a miss and is kept.
//! - Every access is best effort: a busy or damaged database only makes the
//!   caller read the Store or the folder again.
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, TransactionBehavior, params};
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
const PROJECTION_TABLE: &str = "CREATE TABLE projection (root TEXT PRIMARY KEY NOT NULL, base TEXT, positions INTEGER NOT NULL, seq INTEGER NOT NULL, bytes BLOB NOT NULL)";
/// Names SQLite may give the files beside a database.
const SIDECARS: [&str; 3] = ["-journal", "-wal", "-shm"];

pub(crate) fn path(store_root: &Path) -> PathBuf {
    store_root.join("metadata").join(FILE)
}

/// How a caller uses the cache database.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Fixed queries whose values are checked.
    Read,
    /// Writes to an existing database of this schema.
    Write,
    /// Writes, creating the database when missing and replacing one that is
    /// not this cache.
    Create,
}

/// The cache database of `store_root`, or `None` (missing, not a regular
/// file, busy, of a newer version, or not usable for `access`).
pub(crate) fn open(store_root: &Path, access: Access) -> Option<Connection> {
    let _span = crate::perf::span("derived.open");
    let path = path(store_root);
    if !without_sidecars(&path, access) {
        return None;
    }
    let writer = access == Access::Create;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            let replace = || {
                discard(&path);
                initialize(&path).ok()
            };
            match inspect(&path) {
                (Header::Rollback, single) if single || access == Access::Read => {}
                (Header::Foreign, true) if writer => return replace(),
                _ => return None,
            }
            let connection = match connect(
                &path,
                if access == Access::Read {
                    Connect::ReadOnly
                } else {
                    Connect::ReadWrite
                },
            ) {
                Ok(connection) => connection,
                Err(error) if writer && invalid(&error) => return replace(),
                Err(_) => return None,
            };
            match check_schema(&connection) {
                Ok(Schema::Ours) => Some(connection),
                Ok(Schema::Empty) if writer => {
                    drop(connection);
                    initialize(&path).ok()
                }
                Ok(Schema::Foreign) if writer => {
                    drop(connection);
                    replace()
                }
                Err(error) if writer && invalid(&error) => {
                    drop(connection);
                    replace()
                }
                _ => None,
            }
        }
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound && writer => {
            let _ = fs::create_dir_all(path.parent()?);
            initialize(&path).ok()
        }
        Err(_) => None,
    }
}

/// No sidecar beside the database (this cache never makes one): a writer
/// removes its name first; a reader refuses any but an empty regular file
/// (which SQLite never replays, and a reader never writes).
fn without_sidecars(path: &Path, access: Access) -> bool {
    SIDECARS.iter().all(|suffix| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let sidecar = PathBuf::from(name);
        let Ok(metadata) = fs::symlink_metadata(&sidecar) else {
            return true;
        };
        if access == Access::Read {
            // An empty journal is never hot; any WAL file switches the
            // connection to WAL mode (council s12 r2).
            return *suffix == "-journal" && metadata.is_file() && metadata.len() == 0;
        }
        fs::remove_file(&sidecar).is_ok() && fs::symlink_metadata(&sidecar).is_err()
    })
}

enum Header {
    /// Empty, or a SQLite database in rollback-journal mode.
    Rollback,
    /// Not a SQLite database, or one in WAL mode (never this cache's).
    Foreign,
    Unreadable,
}

/// The database file's header, and whether the file has a single link (a
/// writer never writes one with more: council s12 r2).
fn inspect(path: &Path) -> (Header, bool) {
    use std::io::Read;
    let Ok(file) = fs::File::open(path) else {
        return (Header::Unreadable, false);
    };
    let single = single_link(&file);
    let mut header = Vec::with_capacity(100);
    let header = match (&file).take(100).read_to_end(&mut header) {
        Err(_) => Header::Unreadable,
        Ok(0) => Header::Rollback,
        Ok(_)
            if header.len() == 100
                && header.starts_with(b"SQLite format 3\0")
                && header[18] == 1
                && header[19] == 1 =>
        {
            Header::Rollback
        }
        Ok(_) => Header::Foreign,
    };
    (header, single)
}

#[cfg(windows)]
fn single_link(file: &fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `file` stays open and the whole output structure is writable.
    (unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) }) != 0
        && info.nNumberOfLinks == 1
}

#[cfg(unix)]
fn single_link(file: &fs::File) -> bool {
    use std::os::unix::fs::MetadataExt;
    file.metadata().is_ok_and(|metadata| metadata.nlink() == 1)
}

#[cfg(not(any(windows, unix)))]
fn single_link(_file: &fs::File) -> bool {
    false
}

/// Raised when a database is proven not to be this cache.
fn not_this_cache() -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
        Some("not the derived cache database".into()),
    )
}

/// Whether `error` proves the database is not this cache or is damaged
/// (what a writer may replace); busy, locked or I/O errors do not.
pub(crate) fn invalid(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(failure.code, ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Connect {
    ReadOnly,
    ReadWrite,
    Create,
}

fn connect(path: &Path, mode: Connect) -> rusqlite::Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW
        | match mode {
            Connect::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY,
            Connect::ReadWrite => OpenFlags::SQLITE_OPEN_READ_WRITE,
            Connect::Create => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        };
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(BUSY)?;
    connection.pragma_update(None, "trusted_schema", "OFF")?;
    connection.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    if mode == Connect::ReadOnly {
        return Ok(connection);
    }
    // Before anything is written: no journal file, ever.
    let mode: String =
        connection.query_row("PRAGMA journal_mode = MEMORY", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("memory") {
        return Err(not_this_cache());
    }
    connection.pragma_update(None, "synchronous", "OFF")?;
    Ok(connection)
}

/// Creates the tables of an empty database (another process may be doing
/// the same: the first transaction creates them, the other finds them).
fn initialize(path: &Path) -> rusqlite::Result<Connection> {
    let mut connection = connect(path, Connect::Create)?;
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
    match check_schema(&connection)? {
        Schema::Ours => Ok(connection),
        _ => Err(not_this_cache()),
    }
}

/// What a database holds, for `open`.
enum Schema {
    /// Exactly this cache's schema.
    Ours,
    /// Nothing yet (another process may be creating it).
    Empty,
    /// This cache, of a version this binary does not know: kept.
    Newer,
    Foreign,
}

/// Exactly this cache's schema: its marker, version, two tables and their
/// primary key indexes, and nothing else (no view, trigger or index).
fn check_schema(connection: &Connection) -> rusqlite::Result<Schema> {
    // One read transaction: the marker, version and objects of one snapshot
    // (another process may be initializing it: council s12 r2).
    let snapshot = connection.unchecked_transaction()?;
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
    drop(statement);
    snapshot.commit()?;
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
    Ok(
        if application == APPLICATION_ID && version == SCHEMA_VERSION && same {
            Schema::Ours
        } else if application == 0 && version == 0 && objects.is_empty() {
            Schema::Empty
        } else if application == APPLICATION_ID && version > SCHEMA_VERSION {
            Schema::Newer
        } else {
            Schema::Foreign
        },
    )
}

/// Removes a database proven not to be this cache or damaged (best effort:
/// one in use stays, and is then neither read nor written). Removing a name
/// never writes to the file it names.
fn discard(path: &Path) {
    let _ = fs::remove_file(path);
}

/// The value stored under `key` in `table`, at most `max_bytes` long.
/// `Ok(None)`: no such value; `Err`: unreadable, not a blob, or larger than
/// allowed. One statement checks the value and reads it (one snapshot): a
/// value larger than allowed is never materialized.
/// Why a value could not be read.
pub(crate) enum Unread {
    /// The row is there but is not a blob within the bound: a damaged
    /// value, which a writer may replace.
    Malformed,
    /// The database could not answer (busy, damaged, I/O).
    Unavailable,
}

fn get(
    connection: &Connection,
    table: Table,
    key: &str,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, Unread> {
    let query = match table {
        Table::ChangeIndex => {
            "SELECT CASE WHEN typeof(bytes) = 'blob' AND length(bytes) <= ?2 THEN bytes END \
             FROM change_index WHERE worktree = ?1"
        }
        Table::Projection => {
            "SELECT CASE WHEN typeof(bytes) = 'blob' AND length(bytes) <= ?2 THEN bytes END \
             FROM projection WHERE root = ?1"
        }
    };
    let limit = i64::try_from(max_bytes).unwrap_or(i64::MAX);
    match connection
        .query_row(query, params![key, limit], |row| {
            row.get::<_, Option<Vec<u8>>>(0)
        })
        .optional()
    {
        Err(_) => Err(Unread::Unavailable),
        Ok(None) => Ok(None),
        Ok(Some(Some(bytes))) => Ok(Some(bytes)),
        Ok(Some(None)) => Err(Unread::Malformed),
    }
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
    /// A row that is not a blob within the bound: a damaged value.
    Malformed,
    /// The database could not answer (busy, damaged, foreign, newer).
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
        Err(Unread::Malformed) => Stored::Malformed,
        Err(Unread::Unavailable) => Stored::Unreadable,
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
    let put = || -> Result<(), Option<rusqlite::Error>> {
        let connection = open(store_root, Access::Create).ok_or(None)?;
        connection
            .execute(
                "INSERT INTO change_index (worktree, bytes) VALUES (?1, ?2) \
                 ON CONFLICT (worktree) DO UPDATE SET bytes = excluded.bytes",
                params![worktree, bytes],
            )
            .map(drop)
            .map_err(Some)
    };
    match put() {
        // A database damaged by a process killed mid-write: replaced, once.
        Err(Some(error)) if invalid(&error) => {
            discard(&path(store_root));
            put()
        }
        other => other,
    }
    .map_err(|error| match error {
        Some(error) => io::Error::other(error),
        None => io::Error::other("the derived cache database cannot be opened"),
    })
}

/// Best effort: a value left behind (busy or damaged database) names a
/// retired Worktree, which nothing reads again (council s12 r1).
pub(crate) fn change_index_delete(store_root: &Path, worktree: &str) {
    if let Some(connection) = open(store_root, Access::Write) {
        let _ = connection.execute("DELETE FROM change_index WHERE worktree = ?1", [worktree]);
    }
}

/// The projection cache's access to the database, one connection per use.
/// A write that finds the database damaged replaces it once the connection
/// is closed.
pub(crate) struct Projections {
    connection: Option<Connection>,
    path: PathBuf,
    damaged: std::cell::Cell<bool>,
}

impl Drop for Projections {
    fn drop(&mut self) {
        drop(self.connection.take());
        if self.damaged.get() {
            discard(&self.path);
        }
    }
}

/// A cached projection's row: its root, and the root of the complete
/// projection it is a delta to.
pub(crate) struct ProjectionRow {
    pub root: String,
    pub base: Option<String>,
}

impl Projections {
    pub(crate) fn open(store_root: &Path, access: Access) -> Option<Self> {
        open(store_root, access).map(|connection| Self {
            connection: Some(connection),
            path: path(store_root),
            damaged: std::cell::Cell::new(false),
        })
    }

    fn connection(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("a Projections connection is only taken when dropped")
    }

    /// Records a damaged database for replacement on drop (only writers
    /// write: a reader's connection is read-only).
    fn noted<T>(&self, result: rusqlite::Result<T>) -> rusqlite::Result<T> {
        if let Err(error) = &result {
            if invalid(error) {
                self.damaged.set(true);
            }
        }
        result
    }

    /// The value of `root`: `Ok(None)` when there is none, `Err` when one
    /// is there but cannot be read (so a load removes it).
    pub(crate) fn fetch(&self, root: &str, max_bytes: u64) -> Result<Option<Vec<u8>>, ()> {
        get(self.connection(), Table::Projection, root, max_bytes).map_err(drop)
    }

    pub(crate) fn get(&self, root: &str, max_bytes: u64) -> Option<Vec<u8>> {
        self.fetch(root, max_bytes).ok().flatten()
    }

    /// Marks `root` most recent; whether it was recorded.
    pub(crate) fn touch(&self, root: &str) -> bool {
        self.noted(self.connection().execute(
            "UPDATE projection SET seq = (SELECT coalesce(max(seq), 0) + 1 FROM projection) \
                 WHERE root = ?1",
            [root],
        ))
        .is_ok_and(|changed| changed > 0)
    }

    /// The most recently recorded complete projection a caller can read
    /// (one naming files by position only with `positions`): its root and
    /// bytes.
    pub(crate) fn newest_complete(
        &self,
        max_bytes: u64,
        positions: bool,
    ) -> Option<(String, Vec<u8>)> {
        let root: String = self
            .connection()
            .query_row(
                "SELECT root FROM projection WHERE base IS NULL AND (positions = 0 OR ?1) \
                 ORDER BY seq DESC LIMIT 1",
                [positions],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        let bytes = self.get(&root, max_bytes)?;
        Some((root, bytes))
    }

    pub(crate) fn insert(
        &self,
        root: &str,
        base: Option<&str>,
        positions: bool,
        bytes: &[u8],
    ) -> io::Result<()> {
        self.noted(self.connection().execute(
            "INSERT INTO projection (root, base, positions, seq, bytes) \
                 VALUES (?1, ?2, ?3, (SELECT coalesce(max(seq), 0) + 1 FROM projection), ?4) \
                 ON CONFLICT (root) DO UPDATE SET base = excluded.base, \
                 positions = excluded.positions, seq = excluded.seq, bytes = excluded.bytes",
            params![root, base, positions, bytes],
        ))
        .map(drop)
        .map_err(io::Error::other)
    }

    /// Every recorded projection, most recent first.
    pub(crate) fn rows(&self) -> Vec<ProjectionRow> {
        let Ok(mut statement) = self
            .connection()
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
        self.connection()
            .query_row("SELECT count(*) FROM projection", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_or(0, |count| usize::try_from(count).unwrap_or(usize::MAX))
    }

    /// Whether the value is gone (removed, or was never there).
    pub(crate) fn remove(&self, root: &str) -> bool {
        self.noted(
            self.connection()
                .execute("DELETE FROM projection WHERE root = ?1", [root]),
        )
        .is_ok()
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
        // Larger than allowed: malformed, never truncated.
        assert!(matches!(
            change_index_get(&store, "w", 4),
            Stored::Malformed
        ));
        let projections = Projections::open(&store, Access::Create).unwrap();
        projections.insert("a", None, false, b"complete a").unwrap();
        projections
            .insert("b", Some("a"), false, b"delta b")
            .unwrap();
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
            projections.newest_complete(1 << 20, false),
            Some(("a".into(), b"complete a".to_vec()))
        );
        // A complete projection naming files by position is no base for a
        // caller without positions.
        projections.insert("p", None, true, b"complete p").unwrap();
        assert_eq!(
            projections.newest_complete(1 << 20, false),
            Some(("a".into(), b"complete a".to_vec()))
        );
        assert_eq!(
            projections.newest_complete(1 << 20, true),
            Some(("p".into(), b"complete p".to_vec()))
        );
        projections.remove("p");
        projections.remove("a");
        assert_eq!(projections.count(), 1);
        drop(projections);
        change_index_delete(&store, "w");
        assert!(!change_index_exists(&store, "w"));
        // A database with another schema (here a view that would never
        // finish) is never queried, and a writer replaces it.
        {
            let foreign = Connection::open(path(&store)).unwrap();
            foreign
                .execute_batch(
                    "DROP TABLE change_index; CREATE VIEW change_index (worktree, bytes) AS \
                     WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) \
                     SELECT 'w', x'00' FROM n",
                )
                .unwrap();
        }
        assert!(Projections::open(&store, Access::Write).is_none());
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Unreadable
        ));
        change_index_put(&store, "w", b"again").unwrap();
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Present(bytes) if bytes == b"again"
        ));
        // A value that is not a blob, or larger than allowed, is refused by
        // the statement that reads it.
        {
            let foreign = Connection::open(path(&store)).unwrap();
            foreign
                .execute(
                    "UPDATE change_index SET bytes = 'text' WHERE worktree = 'w'",
                    [],
                )
                .unwrap();
        }
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Malformed
        ));
        // Garbage instead of a database: the same.
        fs::write(path(&store), b"not a database at all, not even close").unwrap();
        assert!(!change_index_exists(&store, "w"));
        change_index_put(&store, "w", b"third").unwrap();
        assert!(change_index_exists(&store, "w"));
        // No file besides the database is ever written.
        assert!(
            !store
                .join("metadata")
                .join("derived.sqlite3-journal")
                .exists()
        );
        // A sidecar beside it (planted, or another program's) is never
        // opened: a reader misses, a writer removes its name.
        let sidecar = store.join("metadata").join("derived.sqlite3-journal");
        let outside = store.join("outside.txt");
        fs::write(&outside, b"keep me").unwrap();
        fs::hard_link(&outside, &sidecar).unwrap();
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Unreadable
        ));
        change_index_put(&store, "w", b"fourth").unwrap();
        assert!(!sidecar.exists());
        assert_eq!(fs::read(&outside).unwrap(), b"keep me");
        // A database file with another link is never written.
        let elsewhere = store.join("elsewhere.sqlite3");
        fs::rename(path(&store), &elsewhere).unwrap();
        fs::hard_link(&elsewhere, path(&store)).unwrap();
        assert!(change_index_put(&store, "w", b"linked").is_err());
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Present(bytes) if bytes == b"fourth"
        ));
        fs::remove_file(&elsewhere).unwrap();
        // An empty WAL beside it (which would switch SQLite to WAL mode)
        // with a shared-memory file linked outside: a reader never opens the
        // database; a writer removes both names, never writing through them.
        let wal = store.join("metadata").join("derived.sqlite3-wal");
        let shm = store.join("metadata").join("derived.sqlite3-shm");
        fs::write(&wal, b"").unwrap();
        fs::write(&outside, b"").unwrap();
        fs::hard_link(&outside, &shm).unwrap();
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Unreadable
        ));
        assert!(!change_index_exists(&store, "w"));
        change_index_put(&store, "w", b"fourth").unwrap();
        assert!(!wal.exists() && !shm.exists());
        assert_eq!(fs::read(&outside).unwrap(), b"");
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Present(bytes) if bytes == b"fourth"
        ));
        // An empty database (another process creating it) is initialized in
        // place; one of a newer version of this cache is kept, unread.
        fs::write(path(&store), b"").unwrap();
        change_index_put(&store, "w", b"fifth").unwrap();
        {
            let newer = Connection::open(path(&store)).unwrap();
            newer
                .pragma_update(None, "user_version", SCHEMA_VERSION + 1)
                .unwrap();
        }
        assert!(change_index_put(&store, "w", b"sixth").is_err());
        assert!(matches!(
            change_index_get(&store, "w", 1 << 20),
            Stored::Unreadable
        ));
        let _ = fs::remove_dir_all(store);
    }
}
