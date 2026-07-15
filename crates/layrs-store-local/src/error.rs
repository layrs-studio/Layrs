//! Error boundary shared by metadata, locking, recovery, and fsck.

use std::fmt;
use std::io;
use std::path::PathBuf;

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    InvalidConfig(String),
    LockBusy(PathBuf),
    ReentrantLock(PathBuf),
    ReadOnlySession,
    MigrationChecksum {
        version: u32,
        expected: String,
        actual: String,
    },
    UnsupportedSchema {
        found: u32,
        supported: u32,
    },
    PragmaMismatch {
        name: &'static str,
        expected: String,
        actual: String,
    },
    GenerationConflict {
        entity: String,
        expected: u64,
        actual: Option<u64>,
    },
    NotFound(String),
    Conflict(String),
    InvalidState(String),
    CorruptMetadata(String),
}

impl StoreError {
    pub(crate) fn into_io_error(self) -> io::Error {
        match self {
            Self::Io(error) => error,
            other => io::Error::other(other),
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::InvalidConfig(message) => write!(formatter, "invalid store config: {message}"),
            Self::LockBusy(path) => write!(formatter, "store lock is busy: {}", path.display()),
            Self::ReentrantLock(path) => {
                write!(
                    formatter,
                    "incompatible process-local lock: {}",
                    path.display()
                )
            }
            Self::ReadOnlySession => write!(formatter, "metadata session is read-only"),
            Self::MigrationChecksum {
                version,
                expected,
                actual,
            } => write!(
                formatter,
                "migration {version} checksum mismatch: expected {expected}, got {actual}"
            ),
            Self::UnsupportedSchema { found, supported } => write!(
                formatter,
                "metadata schema version {found} is newer than supported version {supported}"
            ),
            Self::PragmaMismatch {
                name,
                expected,
                actual,
            } => write!(
                formatter,
                "SQLite PRAGMA {name} mismatch: expected {expected}, got {actual}"
            ),
            Self::GenerationConflict {
                entity,
                expected,
                actual,
            } => write!(
                formatter,
                "generation conflict for {entity}: expected {expected}, got {}",
                actual.map_or_else(|| "missing".to_string(), |value| value.to_string())
            ),
            Self::NotFound(message) => write!(formatter, "not found: {message}"),
            Self::Conflict(message) => write!(formatter, "conflict: {message}"),
            Self::InvalidState(message) => write!(formatter, "invalid state: {message}"),
            Self::CorruptMetadata(message) => write!(formatter, "corrupt metadata: {message}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}
