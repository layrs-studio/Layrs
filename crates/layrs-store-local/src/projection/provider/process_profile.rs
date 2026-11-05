//! Explicit process assertion plus observable drift checks; not a registry audit.
//!
//! The declaring host must have audited INITIAL SQLite configuration and its
//! entire native closure, and keep it immutable: no single-thread/custom mutex
//! configuration, shutdown/reinitialization, VFS/system-call replacement, or
//! unqualified automatic/dynamic extensions. FULLMUTEX and compile options do
//! not prove those properties. vfs_find itself may initialize SQLite. No product
//! host declares this profile implicitly.
use rusqlite::ffi;
use std::{ffi::CStr, fmt, sync::Mutex};

pub const PROVIDER_SQLITE_SOURCE_ID: &str =
    "2024-05-23 13:25:27 96c92aba00c8375bc32fafcdf12429c58bd8aabfcadab6683e35bbb9cdebf19e";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSqliteProcessDeclaration {
    /// An assertion by a separately audited executable, never an empty-registry check.
    AuditedLayrsHostV1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSqliteProfileError {
    Undeclared,
    UnsupportedBuild(String),
    UnsupportedVfs,
    Drift,
    Invalidated,
    Poisoned,
}
impl fmt::Display for ProviderSqliteProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "provider SQLite process profile: {self:?}")
    }
}
impl std::error::Error for ProviderSqliteProfileError {}

/// Observable candidate fields for qualification. These do not describe the
/// automatic-extension registry, mutex runtime configuration or final native link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSqliteBuildProfile {
    pub source_id: String,
    pub version_number: i32,
    pub compile_options: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Baseline {
    build: ProviderSqliteBuildProfile,
    pub(super) vfs_address: usize,
    vfs: Vec<usize>,
}
enum State {
    Undeclared,
    Active(Baseline),
    Invalid,
}
static PROFILE: Mutex<State> = Mutex::new(State::Undeclared);

/// A rejected declaration invalidates this process permanently. A repeated
/// declaration checks the original baseline; it cannot reset or replace it.
pub fn declare_provider_sqlite_process_profile(
    _: ProviderSqliteProcessDeclaration,
) -> crate::projection::Result<()> {
    declare().map_err(|e| super::recovery::profile_failure(e, "declare SQLite process profile"))
}

fn declare() -> Result<(), ProviderSqliteProfileError> {
    let mut state = PROFILE
        .lock()
        .map_err(|_| ProviderSqliteProfileError::Poisoned)?;
    if matches!(*state, State::Invalid) {
        return Err(ProviderSqliteProfileError::Invalidated);
    }
    let candidate = match observe() {
        Ok(v) => v,
        Err(e) => {
            *state = State::Invalid;
            return Err(e);
        }
    };
    match &*state {
        State::Undeclared => *state = State::Active(candidate),
        State::Active(old) if old == &candidate => {}
        _ => {
            *state = State::Invalid;
            return Err(ProviderSqliteProfileError::Drift);
        }
    }
    Ok(())
}

pub(super) fn gate() -> Result<Baseline, ProviderSqliteProfileError> {
    let mut state = PROFILE
        .lock()
        .map_err(|_| ProviderSqliteProfileError::Poisoned)?;
    let old = match &*state {
        State::Undeclared => return Err(ProviderSqliteProfileError::Undeclared),
        State::Invalid => return Err(ProviderSqliteProfileError::Invalidated),
        State::Active(old) => old.clone(),
    };
    match observe() {
        Ok(current) if current == old => Ok(old),
        _ => {
            *state = State::Invalid;
            Err(ProviderSqliteProfileError::Drift)
        }
    }
}

pub(super) fn invalidate() {
    if let Ok(mut state) = PROFILE.lock() {
        *state = State::Invalid;
    }
}

fn observe() -> Result<Baseline, ProviderSqliteProfileError> {
    // SAFETY: the declaration asserts the pinned process/native configuration.
    // SQLite owns these static strings and registry objects. No SQLite database
    // is opened, and no configuration/registry operation is performed here.
    let (source_id, version_number, compile_options) = unsafe {
        let source_id = CStr::from_ptr(ffi::sqlite3_sourceid())
            .to_string_lossy()
            .into_owned();
        let mut options = Vec::new();
        for index in 0..512 {
            let p = ffi::sqlite3_compileoption_get(index);
            if p.is_null() {
                break;
            }
            let value = CStr::from_ptr(p).to_str().map_err(|_| {
                ProviderSqliteProfileError::UnsupportedBuild("non-UTF8 option".into())
            })?;
            if value.len() > 256 {
                return Err(ProviderSqliteProfileError::UnsupportedBuild(
                    "option bound".into(),
                ));
            }
            options.push(value.to_owned());
        }
        if options.len() == 512 {
            return Err(ProviderSqliteProfileError::UnsupportedBuild(
                "option count".into(),
            ));
        }
        (source_id, ffi::sqlite3_libversion_number(), options)
    };
    let build = ProviderSqliteBuildProfile {
        source_id,
        version_number,
        compile_options,
    };
    validate_build(&build)?;
    // Literal name, never SQLite's replaceable default VFS or a caller string.
    let p = unsafe { ffi::sqlite3_vfs_find(c"win32".as_ptr()) };
    if p.is_null() {
        return Err(ProviderSqliteProfileError::UnsupportedVfs);
    }
    // SAFETY: the qualified host holds the registry immutable. Scalar/pointer
    // comparison detects observable drift, not same-pointer syscall replacement.
    let v = unsafe { &*p };
    if v.iVersion != 3
        || v.szOsFile <= 0
        || v.szOsFile > 65536
        || v.mxPathname <= 0
        || v.mxPathname > 32768
        || v.zName.is_null()
        || unsafe { CStr::from_ptr(v.zName) }.to_bytes() != b"win32"
        || v.xOpen.is_none()
        || v.xDelete.is_none()
        || v.xAccess.is_none()
        || v.xFullPathname.is_none()
        || v.xRandomness.is_none()
        || v.xSleep.is_none()
        || v.xCurrentTimeInt64.is_none()
    {
        return Err(ProviderSqliteProfileError::UnsupportedVfs);
    }
    macro_rules! pointer {
        ($x:expr) => {
            $x.map_or(0, |f| f as usize)
        };
    }
    let vfs = vec![
        v.iVersion as usize,
        v.szOsFile as usize,
        v.mxPathname as usize,
        v.pNext as usize,
        v.pAppData as usize,
        v.zName as usize,
        pointer!(v.xOpen),
        pointer!(v.xDelete),
        pointer!(v.xAccess),
        pointer!(v.xFullPathname),
        pointer!(v.xDlOpen),
        pointer!(v.xDlError),
        pointer!(v.xDlSym),
        pointer!(v.xDlClose),
        pointer!(v.xRandomness),
        pointer!(v.xSleep),
        pointer!(v.xCurrentTime),
        pointer!(v.xGetLastError),
        pointer!(v.xCurrentTimeInt64),
        pointer!(v.xSetSystemCall),
        pointer!(v.xGetSystemCall),
        pointer!(v.xNextSystemCall),
    ];
    Ok(Baseline {
        build,
        vfs_address: p as usize,
        vfs,
    })
}

fn validate_build(build: &ProviderSqliteBuildProfile) -> Result<(), ProviderSqliteProfileError> {
    if build.source_id != PROVIDER_SQLITE_SOURCE_ID || build.version_number != 3_046_000 {
        return Err(ProviderSqliteProfileError::UnsupportedBuild(
            "bundled source/version differs".into(),
        ));
    }
    // Source-derived defaults: libsqlite3-sys0.30.1 build.rs and SQLite3.46.0
    // sqlite3azCompileOpt/default limit definitions. No arbitrary -D option is
    // accepted. COMPILER identifies the compiler only; it is not a link audit.
    const OPTIONS: &[&str] = &[
        "ATOMIC_INTRINSICS=0",
        "DEFAULT_AUTOVACUUM",
        "DEFAULT_CACHE_SIZE=-2000",
        "DEFAULT_FILE_FORMAT=4",
        "DEFAULT_FOREIGN_KEYS",
        "DEFAULT_JOURNAL_SIZE_LIMIT=-1",
        "DEFAULT_MEMSTATUS=0",
        "DEFAULT_MMAP_SIZE=0",
        "DEFAULT_PAGE_SIZE=4096",
        "DEFAULT_PCACHE_INITSZ=20",
        "DEFAULT_RECURSIVE_TRIGGERS",
        "DEFAULT_SECTOR_SIZE=4096",
        "DEFAULT_SYNCHRONOUS=2",
        "DEFAULT_WAL_AUTOCHECKPOINT=1000",
        "DEFAULT_WAL_SYNCHRONOUS=2",
        "DEFAULT_WORKER_THREADS=0",
        "DIRECT_OVERFLOW_READ",
        "ENABLE_API_ARMOR",
        "ENABLE_COLUMN_METADATA",
        "ENABLE_DBSTAT_VTAB",
        "ENABLE_FTS3",
        "ENABLE_FTS3_PARENTHESIS",
        "ENABLE_FTS5",
        "ENABLE_LOAD_EXTENSION",
        "ENABLE_MEMORY_MANAGEMENT",
        "ENABLE_RTREE",
        "ENABLE_STAT4",
        "ENABLE_UNLOCK_NOTIFY",
        "HAVE_ISNAN",
        "MALLOC_SOFT_LIMIT=1024",
        "MAX_ATTACHED=10",
        "MAX_COLUMN=2000",
        "MAX_COMPOUND_SELECT=500",
        "MAX_DEFAULT_PAGE_SIZE=8192",
        "MAX_EXPR_DEPTH=1000",
        "MAX_FUNCTION_ARG=127",
        "MAX_LENGTH=1000000000",
        "MAX_LIKE_PATTERN_LENGTH=50000",
        "MAX_MMAP_SIZE=0x7fff0000",
        "MAX_PAGE_COUNT=0xfffffffe",
        "MAX_PAGE_SIZE=65536",
        "MAX_SQL_LENGTH=1000000000",
        "MAX_TRIGGER_DEPTH=1000",
        "MAX_VARIABLE_NUMBER=32766",
        "MAX_VDBE_OP=250000000",
        "MAX_WORKER_THREADS=8",
        "MUTEX_W32",
        "SOUNDEX",
        "SYSTEM_MALLOC",
        "TEMP_STORE=1",
        "THREADSAFE=1",
        "USE_URI",
    ];
    let mut seen = std::collections::BTreeSet::new();
    for option in &build.compile_options {
        if !seen.insert(option.as_str())
            || (!OPTIONS.contains(&option.as_str())
                && !(option.starts_with("COMPILER=msvc-")
                    && option.len() > 14
                    && option[14..].bytes().all(|b| b.is_ascii_digit())))
        {
            return Err(ProviderSqliteProfileError::UnsupportedBuild(format!(
                "unqualified option: {option}"
            )));
        }
    }
    for required in [
        "THREADSAFE=1",
        "MUTEX_W32",
        "MAX_PAGE_SIZE=65536",
        "DEFAULT_SECTOR_SIZE=4096",
        "USE_URI",
    ] {
        if !seen.contains(required) {
            return Err(ProviderSqliteProfileError::UnsupportedBuild(format!(
                "required option absent: {required}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests;
