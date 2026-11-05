use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_core::scm::{StoreId, canonical_json};
use serde::{Deserialize, Serialize};

use super::layout::{NativeStoreAudit, NativeStoreExclusive, NativeStoreLayout, NativeStoreOpen};

/// The Store's shared lock, its file opened and its path validated once
/// (`open`), then held around each short read (`hold`) without the database
/// or identity checks of a full open. A reader of immutable, admitted CAS
/// objects thereby still waits for an exclusive writer like a shared open.
pub struct SharedReadLocks {
    lock: crate::lock::ReusableSharedLock,
}

/// One held shared read lock.
pub struct SharedReadLock<'a> {
    _hold: crate::lock::SharedHold<'a>,
}

impl SharedReadLocks {
    pub fn open(root: &Path) -> Result<Self, crate::StoreError> {
        let layout = NativeStoreLayout::new(root).map_err(crate::StoreError::Io)?;
        Ok(Self {
            lock: crate::lock::ReusableSharedLock::open(layout.lock_path())?,
        })
    }

    /// `LockBusy` while an exclusive writer holds the Store.
    pub fn hold(&self) -> Result<SharedReadLock<'_>, crate::StoreError> {
        Ok(SharedReadLock {
            _hold: self.lock.hold()?,
        })
    }
}
use super::sqlite::{self, ScmSchemaError};
use crate::StoreError;
use crate::layout::{
    create_dir_all_durable, create_new_file_no_follow, open_read_no_follow, sync_directory,
};
use crate::lock::{SessionMode, StoreLock};

pub const FORMAT_MAGIC: &str = "LAYRS_NATIVE_STORE";
pub const FORMAT_SCHEMA: &str = "layrs.store-format.v1";
/// The complete-Store format without packs (every object a loose file).
pub const FORMAT_VERSION: u32 = 2;
/// The complete-Store format whose CAS may hold packs of small objects (ADR
/// 0026). 3 and 4 belong to projection Stores of the same magic.
pub const PACKED_FORMAT_VERSION: u32 = 5;
/// The newest complete-Store format this reader understands.
pub const NEWEST_FORMAT_VERSION: u32 = PACKED_FORMAT_VERSION;
pub const MAX_FORMAT_BYTES: u64 = 64 * 1024;

/// The format `init` creates: packed, unless `LAYRS_STORE_FORMAT=2`.
fn creation_format_version() -> u32 {
    match std::env::var("LAYRS_STORE_FORMAT").as_deref() {
        Ok("2") => FORMAT_VERSION,
        _ => PACKED_FORMAT_VERSION,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatState {
    Initializing,
    Ready,
}

/// Canonical root marker for the native Store baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormatMarker {
    schema_version: String,
    magic: String,
    format_version: u32,
    min_reader_version: u32,
    state: FormatState,
    store_id: StoreId,
}

impl FormatMarker {
    /// A marker in the format `init` creates today.
    pub fn new(store_id: StoreId, state: FormatState) -> Self {
        Self::with_version(store_id, state, creation_format_version())
    }

    /// A marker of an explicit supported version (2 or 5); readers of an
    /// older version refuse it.
    pub fn with_version(store_id: StoreId, state: FormatState, format_version: u32) -> Self {
        Self {
            schema_version: FORMAT_SCHEMA.to_string(),
            magic: FORMAT_MAGIC.to_string(),
            format_version,
            min_reader_version: format_version,
            state,
            store_id,
        }
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    pub fn min_reader_version(&self) -> u32 {
        self.min_reader_version
    }

    pub fn state(&self) -> FormatState {
        self.state
    }

    pub fn store_id(&self) -> &StoreId {
        &self.store_id
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, NativeStoreError> {
        canonical_json(self).map_err(|error| NativeStoreError::InvalidMarker(error.to_string()))
    }

    fn validate(&self) -> Result<(), NativeStoreError> {
        if self.schema_version != FORMAT_SCHEMA {
            return Err(NativeStoreError::InvalidMarker(format!(
                "unsupported marker schema {}",
                self.schema_version
            )));
        }
        if self.magic != FORMAT_MAGIC {
            return Err(NativeStoreError::InvalidMarker(
                "native Store magic does not match".into(),
            ));
        }
        if self.format_version != FORMAT_VERSION && self.format_version != PACKED_FORMAT_VERSION {
            return Err(NativeStoreError::InvalidMarker(format!(
                "native Store format {} is not supported by reader {}",
                self.format_version, NEWEST_FORMAT_VERSION
            )));
        }
        if self.min_reader_version > NEWEST_FORMAT_VERSION
            || self.min_reader_version > self.format_version
        {
            return Err(NativeStoreError::InvalidMarker(format!(
                "native Store requires reader {}",
                self.min_reader_version
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeStoreInspection {
    AbsentOrEmpty {
        layout: NativeStoreLayout,
    },
    NativeReady {
        layout: NativeStoreLayout,
        marker: FormatMarker,
    },
    NativeInitializing {
        layout: NativeStoreLayout,
        marker: FormatMarker,
    },
    LegacySqlite {
        layout: NativeStoreLayout,
    },
    LegacyClientDocuments {
        layout: NativeStoreLayout,
    },
    LegacyJson {
        layout: NativeStoreLayout,
    },
    FutureFormat {
        layout: NativeStoreLayout,
        found_version: u64,
        min_reader_version: Option<u64>,
    },
    InvalidNative {
        layout: NativeStoreLayout,
        reason: String,
    },
    UnknownNonEmpty {
        layout: NativeStoreLayout,
    },
}

impl NativeStoreInspection {
    pub fn layout(&self) -> &NativeStoreLayout {
        match self {
            Self::AbsentOrEmpty { layout }
            | Self::NativeReady { layout, .. }
            | Self::NativeInitializing { layout, .. }
            | Self::LegacySqlite { layout }
            | Self::LegacyClientDocuments { layout }
            | Self::LegacyJson { layout }
            | Self::FutureFormat { layout, .. }
            | Self::InvalidNative { layout, .. }
            | Self::UnknownNonEmpty { layout } => layout,
        }
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::AbsentOrEmpty { .. } => "absent_or_empty",
            Self::NativeReady { .. } => "native_ready",
            Self::NativeInitializing { .. } => "native_initializing",
            Self::LegacySqlite { .. } => "legacy_sqlite",
            Self::LegacyClientDocuments { .. } => "legacy_client_documents",
            Self::LegacyJson { .. } => "legacy_json",
            Self::FutureFormat { .. } => "future_format",
            Self::InvalidNative { .. } => "invalid_native",
            Self::UnknownNonEmpty { .. } => "unknown_nonempty",
        }
    }
}

#[derive(Debug)]
pub enum NativeStoreError {
    Io(io::Error),
    Lock(StoreError),
    Schema(ScmSchemaError),
    InvalidMarker(String),
    StoreIdMismatch {
        expected: StoreId,
        actual: StoreId,
    },
    UnsupportedFormat {
        inspection: Box<NativeStoreInspection>,
    },
}

impl fmt::Display for NativeStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "native Store I/O failed: {error}"),
            Self::Lock(error) => write!(formatter, "native Store lock failed: {error}"),
            Self::Schema(error) => write!(formatter, "native Store schema failed: {error}"),
            Self::InvalidMarker(reason) => {
                write!(formatter, "invalid native Store marker: {reason}")
            }
            Self::StoreIdMismatch { expected, actual } => write!(
                formatter,
                "native Store identity mismatch: expected {expected}, got {actual}"
            ),
            Self::UnsupportedFormat { inspection } => write!(
                formatter,
                "unsupported local Store format: {}",
                inspection.kind()
            ),
        }
    }
}

impl std::error::Error for NativeStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Schema(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for NativeStoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ScmSchemaError> for NativeStoreError {
    fn from(error: ScmSchemaError) -> Self {
        Self::Schema(error)
    }
}

impl From<StoreError> for NativeStoreError {
    fn from(error: StoreError) -> Self {
        Self::Lock(error)
    }
}

/// Inspects a Store root without creating files, directories, or SQLite sidecars.
pub fn inspect(root: impl AsRef<Path>) -> Result<NativeStoreInspection, NativeStoreError> {
    let layout = NativeStoreLayout::new(root)?;
    match fs::symlink_metadata(layout.root()) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(NativeStoreInspection::AbsentOrEmpty { layout });
        }
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native Store root is not a regular directory",
            )
            .into());
        }
        Ok(_) => {}
    }

    if layout.format_path().exists() {
        return inspect_marker(layout);
    }
    if directory_is_empty(layout.root())? {
        return Ok(NativeStoreInspection::AbsentOrEmpty { layout });
    }

    if layout.sqlite_path().exists() {
        let bytes = read_regular_file(layout.sqlite_path())?;
        if contains_ascii(&bytes, b"client_documents") {
            return Ok(NativeStoreInspection::LegacyClientDocuments { layout });
        }
        return Ok(NativeStoreInspection::LegacySqlite { layout });
    }
    if has_legacy_json(layout.root())? {
        return Ok(NativeStoreInspection::LegacyJson { layout });
    }
    Ok(NativeStoreInspection::UnknownNonEmpty { layout })
}

/// Initializes only an absent/empty root or resumes this format's own marker.
pub fn init(
    root: impl AsRef<Path>,
    store_id: StoreId,
) -> Result<NativeStoreOpen, NativeStoreError> {
    let inspection = inspect(root)?;
    let layout = inspection.layout().clone();
    match inspection {
        NativeStoreInspection::AbsentOrEmpty { .. } => {
            create_dir_all_durable(layout.root())?;
            publish_marker(
                &layout,
                &FormatMarker::new(store_id.clone(), FormatState::Initializing),
            )?;
        }
        NativeStoreInspection::NativeInitializing { marker, .. } => {
            if marker.store_id() != &store_id {
                return Err(NativeStoreError::StoreIdMismatch {
                    expected: marker.store_id().clone(),
                    actual: store_id,
                });
            }
        }
        other => {
            return Err(NativeStoreError::UnsupportedFormat {
                inspection: Box::new(other),
            });
        }
    }

    layout.ensure_lock_boundary()?;
    let store_lock = StoreLock::acquire_existing(layout.lock_path(), SessionMode::Exclusive)?;
    // Resuming an interrupted initialization keeps its marker's version: a
    // format-2 Store is never promoted implicitly.
    let format_version = match inspect(layout.root())? {
        NativeStoreInspection::NativeInitializing { marker, .. }
            if marker.store_id() == &store_id =>
        {
            marker.format_version()
        }
        NativeStoreInspection::NativeInitializing { marker, .. } => {
            return Err(NativeStoreError::StoreIdMismatch {
                expected: marker.store_id().clone(),
                actual: store_id,
            });
        }
        other => {
            return Err(NativeStoreError::UnsupportedFormat {
                inspection: Box::new(other),
            });
        }
    };
    layout.ensure_bootstrap_directories()?;
    let connection = if layout.sqlite_path().exists() {
        sqlite::open_existing(layout.sqlite_path(), Duration::from_secs(5))?
    } else {
        sqlite::create_fresh(layout.sqlite_path(), Duration::from_secs(5), &store_id)?
    };
    sqlite::validate_store_identity(&connection, &store_id)?;
    drop(connection);
    sync_directory(layout.metadata_dir())?;
    publish_marker(
        &layout,
        &FormatMarker::with_version(store_id.clone(), FormatState::Ready, format_version),
    )?;
    layout.validate_ready_boundaries()?;
    drop(store_lock);
    open(layout.root())
}

/// Opens only a complete, ready native Store and never repairs or creates it.
pub fn open(root: impl AsRef<Path>) -> Result<NativeStoreOpen, NativeStoreError> {
    let _span = crate::perf::span("store.open_shared");
    let inspected = {
        let _span = crate::perf::span("open.inspect");
        inspect(root)?
    };
    match inspected {
        NativeStoreInspection::NativeReady { layout, marker } => {
            {
                let _span = crate::perf::span("open.wal");
                recover_orphaned_wal(&layout, marker.store_id())?;
            }
            let lock_span = crate::perf::span("open.lock");
            let store_lock = StoreLock::acquire_existing(layout.lock_path(), SessionMode::Shared)?;
            drop(lock_span);
            let ready_span = crate::perf::span("open.ready_under_lock");
            let (layout, marker) = ready_under_lock(&layout, marker.store_id())?;
            drop(ready_span);
            let sqlite_span = crate::perf::span("open.sqlite");
            let connection = sqlite::open_read_only(layout.sqlite_path(), Duration::from_secs(5))?;
            sqlite::validate_store_identity(&connection, marker.store_id())?;
            drop(sqlite_span);
            Ok(NativeStoreOpen::new(
                layout,
                marker.store_id().clone(),
                connection,
                store_lock,
            )
            .with_format_version(marker.format_version()))
        }
        other => Err(NativeStoreError::UnsupportedFormat {
            inspection: Box::new(other),
        }),
    }
}

/// Acquires a Ready Store for non-mutating audit without opening SQLite.
pub fn open_audit(root: impl AsRef<Path>) -> Result<NativeStoreAudit, NativeStoreError> {
    match inspect(root)? {
        NativeStoreInspection::NativeReady { layout, marker } => {
            let store_lock = StoreLock::acquire_existing(layout.lock_path(), SessionMode::Shared)?;
            let (layout, marker) = ready_under_lock(&layout, marker.store_id())?;
            Ok(NativeStoreAudit::new(
                layout,
                marker.store_id().clone(),
                store_lock,
            )
            .with_format_version(marker.format_version()))
        }
        other => Err(NativeStoreError::UnsupportedFormat {
            inspection: Box::new(other),
        }),
    }
}

/// Acquires the exclusive writer capability without changing logical state.
pub fn open_exclusive(root: impl AsRef<Path>) -> Result<NativeStoreExclusive, NativeStoreError> {
    let _span = crate::perf::span("store.open_exclusive");
    match inspect(root)? {
        NativeStoreInspection::NativeReady { layout, marker } => {
            let store_lock =
                StoreLock::acquire_existing(layout.lock_path(), SessionMode::Exclusive)?;
            let (layout, marker) = ready_under_lock(&layout, marker.store_id())?;
            if sqlite::has_uncheckpointed_wal(layout.sqlite_path())? {
                sqlite::checkpoint_orphaned_wal(layout.sqlite_path(), Duration::from_secs(5))?;
            }
            let connection = sqlite::open_read_only(layout.sqlite_path(), Duration::from_secs(5))?;
            sqlite::validate_store_identity(&connection, marker.store_id())?;
            drop(connection);
            Ok(NativeStoreExclusive::new(
                layout,
                marker.store_id().clone(),
                store_lock,
            )
            .with_format_version(marker.format_version()))
        }
        other => Err(NativeStoreError::UnsupportedFormat {
            inspection: Box::new(other),
        }),
    }
}

/// A writer killed after commit leaves its frames in the WAL, which the
/// read-only open refuses. Fold them back under a short exclusive lock; if a
/// live session holds the Store, leave the WAL to it.
fn recover_orphaned_wal(
    layout: &NativeStoreLayout,
    store_id: &StoreId,
) -> Result<(), NativeStoreError> {
    if !sqlite::has_uncheckpointed_wal(layout.sqlite_path())? {
        return Ok(());
    }
    let _store_lock = match StoreLock::acquire_existing(layout.lock_path(), SessionMode::Exclusive)
    {
        Ok(lock) => lock,
        Err(StoreError::LockBusy(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let (layout, _) = ready_under_lock(layout, store_id)?;
    if sqlite::has_uncheckpointed_wal(layout.sqlite_path())? {
        sqlite::checkpoint_orphaned_wal(layout.sqlite_path(), Duration::from_secs(5))?;
    }
    Ok(())
}

fn ready_under_lock(
    expected_layout: &NativeStoreLayout,
    expected_store_id: &StoreId,
) -> Result<(NativeStoreLayout, FormatMarker), NativeStoreError> {
    super::compaction::refuse_active(expected_layout)?;
    match inspect(expected_layout.root())? {
        NativeStoreInspection::NativeReady { layout, marker }
            if marker.store_id() == expected_store_id =>
        {
            Ok((layout, marker))
        }
        NativeStoreInspection::NativeReady { marker, .. } => {
            Err(NativeStoreError::StoreIdMismatch {
                expected: expected_store_id.clone(),
                actual: marker.store_id().clone(),
            })
        }
        other => Err(NativeStoreError::UnsupportedFormat {
            inspection: Box::new(other),
        }),
    }
}

/// The full boundary validation (every boundary a regular directory or file,
/// no reparse point on any path component) once per process and Store root.
/// Later opens still re-read and validate the marker, pin the root and check
/// its identity; a command opening its Store many times no longer walks every
/// ancestor of every boundary each time (git makes no such check at all).
fn validate_ready_boundaries_once(layout: &NativeStoreLayout) -> io::Result<()> {
    static VALIDATED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    let validated = VALIDATED.get_or_init(Default::default);
    if validated
        .lock()
        .is_ok_and(|roots| roots.contains(layout.root()))
    {
        return Ok(());
    }
    layout.validate_ready_boundaries()?;
    if let Ok(mut roots) = validated.lock() {
        roots.insert(layout.root().to_path_buf());
    }
    Ok(())
}

/// A Ready marker already parsed and validated by this process, keyed by
/// the FORMAT file's metadata (it is only ever replaced by a rename, which
/// changes them): a command opening its Store many times reads it once.
fn ready_marker_memo() -> &'static std::sync::Mutex<
    std::collections::HashMap<PathBuf, (u64, Option<SystemTime>, FormatMarker)>,
> {
    static MEMO: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, (u64, Option<SystemTime>, FormatMarker)>>,
    > = std::sync::OnceLock::new();
    MEMO.get_or_init(Default::default)
}

fn inspect_marker(layout: NativeStoreLayout) -> Result<NativeStoreInspection, NativeStoreError> {
    let observed = fs::symlink_metadata(layout.format_path())
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| (metadata.len(), metadata.modified().ok()));
    if let Some((length, modified)) = observed {
        let remembered = ready_marker_memo().lock().ok().and_then(|memo| {
            memo.get(layout.format_path())
                .filter(|(l, m, _)| *l == length && *m == modified)
                .map(|(_, _, marker)| marker.clone())
        });
        if let Some(marker) = remembered {
            return match validate_ready_boundaries_once(&layout) {
                Ok(()) => Ok(NativeStoreInspection::NativeReady { layout, marker }),
                Err(error) => Ok(NativeStoreInspection::InvalidNative {
                    layout,
                    reason: error.to_string(),
                }),
            };
        }
    }
    let inspected = inspect_marker_uncached(layout)?;
    if let (NativeStoreInspection::NativeReady { layout, marker }, Some((length, modified))) =
        (&inspected, observed)
    {
        if let Ok(mut memo) = ready_marker_memo().lock() {
            memo.insert(layout.format_path().to_path_buf(), (length, modified, marker.clone()));
        }
    }
    Ok(inspected)
}

fn inspect_marker_uncached(layout: NativeStoreLayout) -> Result<NativeStoreInspection, NativeStoreError> {
    let bytes = match read_regular_file_bounded(layout.format_path(), MAX_FORMAT_BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return Ok(NativeStoreInspection::InvalidNative {
                layout,
                reason: error.to_string(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    let probe: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            return Ok(NativeStoreInspection::InvalidNative {
                layout,
                reason: format!("marker is not JSON: {error}"),
            });
        }
    };
    let magic = probe.get("magic").and_then(serde_json::Value::as_str);
    let version = probe
        .get("formatVersion")
        .and_then(serde_json::Value::as_u64);
    // Any version this complete-Store reader does not read is "future" to
    // it: 3 and 4 (projection Stores, routed elsewhere) as well as 6+.
    if magic == Some(FORMAT_MAGIC)
        && version.is_some_and(|value| {
            value > u64::from(FORMAT_VERSION) && value != u64::from(PACKED_FORMAT_VERSION)
        })
    {
        return Ok(NativeStoreInspection::FutureFormat {
            layout,
            found_version: version.unwrap_or_default(),
            min_reader_version: probe
                .get("minReaderVersion")
                .and_then(serde_json::Value::as_u64),
        });
    }

    let marker: FormatMarker = match serde_json::from_value(probe) {
        Ok(marker) => marker,
        Err(error) => {
            return Ok(NativeStoreInspection::InvalidNative {
                layout,
                reason: error.to_string(),
            });
        }
    };
    if let Err(error) = marker.validate() {
        return Ok(NativeStoreInspection::InvalidNative {
            layout,
            reason: error.to_string(),
        });
    }
    if marker.canonical_bytes()? != bytes {
        return Ok(NativeStoreInspection::InvalidNative {
            layout,
            reason: "FORMAT is not canonically encoded".into(),
        });
    }
    match marker.state() {
        FormatState::Initializing => {
            Ok(NativeStoreInspection::NativeInitializing { layout, marker })
        }
        FormatState::Ready => match validate_ready_boundaries_once(&layout) {
            Ok(()) => Ok(NativeStoreInspection::NativeReady { layout, marker }),
            Err(error) => Ok(NativeStoreInspection::InvalidNative {
                layout,
                reason: error.to_string(),
            }),
        },
    }
}

fn publish_marker(
    layout: &NativeStoreLayout,
    marker: &FormatMarker,
) -> Result<(), NativeStoreError> {
    let bytes = marker.canonical_bytes()?;
    let temporary_path = unique_marker_temp(layout.root());
    let result = (|| -> io::Result<()> {
        let mut temporary = create_new_file_no_follow(&temporary_path)?;
        temporary.write_all(&bytes)?;
        temporary.sync_all()?;
        sync_directory(layout.root())?;
        atomic_replace(&temporary_path, layout.format_path())?;
        sync_directory(layout.root())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        let _ = sync_directory(layout.root());
    }
    result.map_err(NativeStoreError::Io)
}

fn unique_marker_temp(root: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    root.join(format!(".FORMAT-{}-{timestamp}.tmp", std::process::id()))
}

#[cfg(windows)]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain([0]).collect();
    let destination: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: both path buffers are NUL-terminated and live for the native call.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn directory_is_empty(path: &Path) -> io::Result<bool> {
    Ok(fs::read_dir(path)?.next().transpose()?.is_none())
}

fn read_regular_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_read_no_follow(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_regular_file_bounded(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let mut file = open_read_no_follow(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a regular file", path.display()),
        ));
    }
    let expected_length = metadata.len();
    if expected_length > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} exceeds the {max_bytes}-byte FORMAT limit",
                path.display()
            ),
        ));
    }
    let mut bytes = Vec::with_capacity(expected_length as usize);
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected_length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} changed size during bounded read", path.display()),
        ));
    }
    Ok(bytes)
}

fn contains_ascii(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn has_legacy_json(root: &Path) -> io::Result<bool> {
    let mut directories = vec![root.to_path_buf()];
    let metadata = root.join("metadata");
    if metadata.is_dir() {
        directories.push(metadata);
    }
    for directory in directories {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "legacy inspection refused a link: {}",
                        entry.path().display()
                    ),
                ));
            }
            if metadata.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
