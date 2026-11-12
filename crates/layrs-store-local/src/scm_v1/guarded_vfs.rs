//! A SQLite VFS that opens nothing but a database file (council s13 Q7).
//!
//! SQLite checks for sidecars (a hot rollback journal, a `-wal` and `-shm`)
//! when it locks, after any check made before it opens: a process able to
//! write beside the database could plant one in between, and SQLite would
//! replay a hot journal (deleting the super-journal it names) or write a
//! wal-index through a linked `-shm`. This VFS wraps the platform's default
//! one so that, for a database opened through it:
//! - `xOpen` opens only the main database file (and its `-wal`, for
//!   `MAIN_AND_WAL`); any journal or other named file is refused. Anonymous
//!   temporary files (no name: SQLite's VFS picks a random one in its
//!   temporary folder and deletes it on close) are opened as usual;
//! - `xAccess` reports every rollback journal and `-shm` (and a `-wal`, for
//!   `MAIN_ONLY`) absent, so no hot journal is ever seen, let alone
//!   replayed;
//! - `xDelete` deletes nothing but a `-wal` of `MAIN_AND_WAL`;
//! - a writable open must name a file of its own. Before the default VFS
//!   opens it, the name, not followed, must be absent or a regular file that
//!   is not a link (on Unix, with one link): SQLite's Unix VFS changes the
//!   mode of an empty `-wal` it opens, and Windows creates the missing target
//!   of a linked `-wal` (council s15 r1). After, on Windows, the file SQLite
//!   opened must be a regular file with a single link and no reparse point,
//!   and its name, opened without following links, must be that same file
//!   (its 128-bit file id): a name swapped for a link, or a hard link to a
//!   file outside the Store, is refused. On Unix the name is checked again
//!   after the open (a swap racing these checks stays possible; protected
//!   links on Linux mitigate it); callers open with `SQLITE_OPEN_NOFOLLOW`,
//!   so SQLite refuses a path whose resolution follows a link. A read-only
//!   open never writes.
//!
//! Every other method is the default VFS's, called with its own `sqlite3_vfs`
//! (the platform VFSes read their `pAppData`).
use rusqlite::ffi;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;

/// Opens the main database file only (`Connection::open_with_flags_and_vfs`).
pub(crate) const MAIN_ONLY: &str = "layrs-main-only";
/// Opens the main database file and its `-wal`, both checked the same way:
/// for a WAL database whose connection holds `locking_mode=EXCLUSIVE` from
/// before its first read, so that the WAL index lives in memory (SQLite opens
/// a `-shm` through the file's methods, not this VFS).
pub(crate) const MAIN_AND_WAL: &str = "layrs-main-wal";

/// The wrapped default VFS, and whether it opens a `-wal`.
struct Guard {
    inner: *mut ffi::sqlite3_vfs,
    wal: bool,
}

// SAFETY: the inner VFS is a process-lifetime SQLite object, safe to use
// from any thread (SQLite's VFS methods are thread-safe).
unsafe impl Send for Guard {}
unsafe impl Sync for Guard {}

/// Registers both VFSes once per process; whether they are available.
pub(crate) fn register() -> bool {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    *REGISTERED.get_or_init(|| {
        // SAFETY: `sqlite3_vfs_find(NULL)` returns the default VFS, which
        // lives as long as the process (registered before ours, which never
        // become the default).
        let inner = unsafe { ffi::sqlite3_vfs_find(std::ptr::null()) };
        !inner.is_null()
            && register_one(inner, MAIN_ONLY, false)
            && register_one(inner, MAIN_AND_WAL, true)
    })
}

fn register_one(inner: *mut ffi::sqlite3_vfs, name: &str, wal: bool) -> bool {
    {
        // SAFETY: `inner` is the default VFS, alive for the process; the
        // registered structure, its name and its `Guard` are leaked so they
        // live as long.
        unsafe {
            let name: &'static CStr = Box::leak(
                std::ffi::CString::new(name)
                    .expect("no NUL in the VFS name")
                    .into_boxed_c_str(),
            );
            let guard = Box::into_raw(Box::new(Guard { inner, wal }));
            let base = &*inner;
            let vfs = Box::new(ffi::sqlite3_vfs {
                iVersion: base.iVersion,
                szOsFile: base.szOsFile,
                mxPathname: base.mxPathname,
                pNext: std::ptr::null_mut(),
                zName: name.as_ptr(),
                pAppData: guard.cast(),
                xOpen: Some(x_open),
                xDelete: Some(x_delete),
                xAccess: Some(x_access),
                xFullPathname: base.xFullPathname.map(|_| x_full_pathname as _),
                xDlOpen: base.xDlOpen.map(|_| x_dl_open as _),
                xDlError: base.xDlError.map(|_| x_dl_error as _),
                xDlSym: base.xDlSym.map(|_| x_dl_sym as _),
                xDlClose: base.xDlClose.map(|_| x_dl_close as _),
                xRandomness: base.xRandomness.map(|_| x_randomness as _),
                xSleep: base.xSleep.map(|_| x_sleep as _),
                xCurrentTime: base.xCurrentTime.map(|_| x_current_time as _),
                xGetLastError: base.xGetLastError.map(|_| x_get_last_error as _),
                xCurrentTimeInt64: base.xCurrentTimeInt64.map(|_| x_current_time_int64 as _),
                xSetSystemCall: base.xSetSystemCall.map(|_| x_set_system_call as _),
                xGetSystemCall: base.xGetSystemCall.map(|_| x_get_system_call as _),
                xNextSystemCall: base.xNextSystemCall.map(|_| x_next_system_call as _),
            });
            ffi::sqlite3_vfs_register(Box::into_raw(vfs), 0) == ffi::SQLITE_OK
        }
    }
}

/// The guard behind `vfs` (ours).
///
/// # Safety
/// `vfs` is a structure `register_one` registered.
unsafe fn guard(vfs: *mut ffi::sqlite3_vfs) -> &'static Guard {
    unsafe { &*((*vfs).pAppData as *const Guard) }
}

/// The default VFS behind `vfs` (ours).
///
/// # Safety
/// As `guard`.
unsafe fn inner(vfs: *mut ffi::sqlite3_vfs) -> *mut ffi::sqlite3_vfs {
    unsafe { guard(vfs).inner }
}

/// Whether `name` (a path SQLite builds from the database's) is a sidecar
/// this VFS hides: a rollback journal or a `-shm`, and a `-wal` unless it
/// opens one. (A super-journal is only ever named in a rollback journal.)
fn hidden(name: *const c_char, wal: bool) -> bool {
    if name.is_null() {
        return false;
    }
    // SAFETY: SQLite passes NUL-terminated names.
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    name.ends_with(b"-journal") || name.ends_with(b"-shm") || (!wal && name.ends_with(b"-wal"))
}

unsafe extern "C" fn x_open(
    vfs: *mut ffi::sqlite3_vfs,
    name: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    // SAFETY: `file` is caller-owned storage of `szOsFile` bytes; on any
    // failure its `pMethods` must be null (SQLite's xOpen contract).
    unsafe {
        (*file).pMethods = std::ptr::null();
        let inner = inner(vfs);
        let open = (*inner).xOpen.expect("a VFS opens files");
        let temporary = ffi::SQLITE_OPEN_TEMP_DB
            | ffi::SQLITE_OPEN_TRANSIENT_DB
            | ffi::SQLITE_OPEN_TEMP_JOURNAL
            | ffi::SQLITE_OPEN_SUBJOURNAL;
        if name.is_null() {
            // An anonymous temporary file: no name anyone could plant.
            if flags & temporary == 0 {
                return ffi::SQLITE_CANTOPEN;
            }
            return open(inner, name, file, flags, out_flags);
        }
        let allowed = ffi::SQLITE_OPEN_MAIN_DB
            | if guard(vfs).wal {
                ffi::SQLITE_OPEN_WAL
            } else {
                0
            };
        if flags & allowed == 0 {
            return ffi::SQLITE_CANTOPEN;
        }
        let writable = flags & ffi::SQLITE_OPEN_READWRITE != 0;
        // Before the default VFS touches it (Unix sets an empty WAL's mode,
        // Windows creates a linked WAL's missing target).
        if writable && !plain_name(name) {
            return ffi::SQLITE_CANTOPEN;
        }
        let rc = open(inner, name, file, flags, out_flags);
        if rc != ffi::SQLITE_OK {
            return rc;
        }
        // A read-only open never writes: only a writable one must name a
        // file of its own.
        if writable && !opened_as_named(file, name) {
            if let Some(methods) = (*file).pMethods.as_ref() {
                if let Some(close) = methods.xClose {
                    close(file);
                }
            }
            (*file).pMethods = std::ptr::null();
            return ffi::SQLITE_CANTOPEN;
        }
        ffi::SQLITE_OK
    }
}

/// The name SQLite passed, as UTF-8.
fn name_text<'n>(name: *const c_char) -> Option<&'n str> {
    // SAFETY: SQLite passes a NUL-terminated path that outlives the call.
    (unsafe { CStr::from_ptr(name) }).to_str().ok()
}

/// Whether the name, not followed, is absent (a creation) or a regular file
/// that is not a link (on Unix, with one link).
fn plain_name(name: *const c_char) -> bool {
    let Some(path) = name_text(name) else {
        return false;
    };
    match std::fs::symlink_metadata(path) {
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        Ok(metadata) => {
            #[cfg(windows)]
            let own = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes()
                    & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                    == 0
            };
            #[cfg(unix)]
            let own = {
                use std::os::unix::fs::MetadataExt;
                metadata.nlink() == 1
            };
            #[cfg(not(any(windows, unix)))]
            let own = false;
            metadata.is_file() && own
        }
    }
}

/// The links of the file `path` names, not followed; `None` when it is
/// absent. (A Store writer takes back a name with another link, `sqlite`.)
pub(crate) fn link_count(path: &std::path::Path) -> std::io::Result<Option<u64>> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            GetFileInformationByHandle,
        };
        let file = match std::fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is open for the call; `info` is writable.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Some(u64::from(info.nNumberOfLinks)))
    }
    #[cfg(not(windows))]
    {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    Ok(Some(metadata.nlink()))
                }
                #[cfg(not(unix))]
                {
                    let _ = metadata;
                    Ok(Some(u64::MAX))
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

/// Windows: the handle SQLite opened names a regular file with one link and
/// no reparse point, and the name, opened without following links, is that
/// same file: same volume and 128-bit file id (ReFS ids do not fit the
/// 64-bit index).
#[cfg(windows)]
unsafe fn opened_as_named(file: *mut ffi::sqlite3_file, name: *const c_char) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };
    let info = |handle: *mut c_void| -> Option<(BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO)> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: `handle` is open for the calls; both outputs are writable
        // and `id` is exactly `size_of::<FILE_ID_INFO>()` bytes.
        let read = unsafe {
            GetFileInformationByHandle(handle, &mut info) != 0
                && GetFileInformationByHandleEx(
                    handle,
                    FileIdInfo,
                    (&mut id as *mut FILE_ID_INFO).cast(),
                    std::mem::size_of::<FILE_ID_INFO>() as u32,
                ) != 0
        };
        read.then_some((info, id))
    };
    // SAFETY: the file was just opened by the default VFS, whose methods
    // answer SQLITE_FCNTL_WIN32_GET_HANDLE with its HANDLE.
    let opened = unsafe {
        let Some(control) = (*file).pMethods.as_ref().and_then(|m| m.xFileControl) else {
            return false;
        };
        let mut handle: *mut c_void = std::ptr::null_mut();
        if control(
            file,
            ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
            (&mut handle as *mut *mut c_void).cast(),
        ) != ffi::SQLITE_OK
            || handle.is_null()
        {
            return false;
        }
        info(handle)
    };
    let Some((opened, opened_id)) = opened else {
        return false;
    };
    let Some(path) = name_text(name) else {
        return false;
    };
    let Ok(named) = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
    else {
        return false;
    };
    let Some((named_info, named_id)) = info(named.as_raw_handle()) else {
        return false;
    };
    opened.nNumberOfLinks == 1
        && opened.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
        && named_info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
        && named_id.VolumeSerialNumber == opened_id.VolumeSerialNumber
        && named_id.FileId.Identifier == opened_id.FileId.Identifier
}

/// Elsewhere: the name, not followed, is still a regular file with one link.
#[cfg(not(windows))]
unsafe fn opened_as_named(_file: *mut ffi::sqlite3_file, name: *const c_char) -> bool {
    plain_name(name) && name_text(name).is_some_and(|path| std::path::Path::new(path).exists())
}

unsafe extern "C" fn x_delete(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    sync_dir: c_int,
) -> c_int {
    // SAFETY: the inner VFS gets its own structure and SQLite's name.
    unsafe {
        // Only a `-wal` this VFS opens is ever deleted (SQLite deletes one it
        // finds beside an empty database): nothing else is ever made, and a
        // name read from a planted file (a super-journal) is never deleted.
        if !guard(vfs).wal || name.is_null() || !CStr::from_ptr(name).to_bytes().ends_with(b"-wal")
        {
            return ffi::SQLITE_OK;
        }
        let inner = inner(vfs);
        (*inner).xDelete.expect("a VFS deletes files")(inner, name, sync_dir)
    }
}

unsafe extern "C" fn x_access(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    // SAFETY: `out` is writable; the inner VFS gets its own structure.
    unsafe {
        if hidden(name, guard(vfs).wal) {
            *out = 0;
            return ffi::SQLITE_OK;
        }
        let inner = inner(vfs);
        (*inner).xAccess.expect("a VFS answers access")(inner, name, flags, out)
    }
}

// The remaining methods are the default VFS's, called with its structure.

unsafe extern "C" fn x_full_pathname(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    n: c_int,
    out: *mut c_char,
) -> c_int {
    unsafe {
        let inner = inner(vfs);
        (*inner).xFullPathname.expect("checked at registration")(inner, name, n, out)
    }
}

unsafe extern "C" fn x_dl_open(vfs: *mut ffi::sqlite3_vfs, name: *const c_char) -> *mut c_void {
    unsafe {
        let inner = inner(vfs);
        (*inner).xDlOpen.expect("checked at registration")(inner, name)
    }
}

unsafe extern "C" fn x_dl_error(vfs: *mut ffi::sqlite3_vfs, n: c_int, out: *mut c_char) {
    unsafe {
        let inner = inner(vfs);
        (*inner).xDlError.expect("checked at registration")(inner, n, out)
    }
}

unsafe extern "C" fn x_dl_sym(
    vfs: *mut ffi::sqlite3_vfs,
    handle: *mut c_void,
    symbol: *const c_char,
) -> Option<unsafe extern "C" fn(*mut ffi::sqlite3_vfs, *mut c_void, *const c_char)> {
    unsafe {
        let inner = inner(vfs);
        (*inner).xDlSym.expect("checked at registration")(inner, handle, symbol)
    }
}

unsafe extern "C" fn x_dl_close(vfs: *mut ffi::sqlite3_vfs, handle: *mut c_void) {
    unsafe {
        let inner = inner(vfs);
        (*inner).xDlClose.expect("checked at registration")(inner, handle)
    }
}

unsafe extern "C" fn x_randomness(vfs: *mut ffi::sqlite3_vfs, n: c_int, out: *mut c_char) -> c_int {
    unsafe {
        let inner = inner(vfs);
        (*inner).xRandomness.expect("checked at registration")(inner, n, out)
    }
}

unsafe extern "C" fn x_sleep(vfs: *mut ffi::sqlite3_vfs, microseconds: c_int) -> c_int {
    unsafe {
        let inner = inner(vfs);
        (*inner).xSleep.expect("checked at registration")(inner, microseconds)
    }
}

unsafe extern "C" fn x_current_time(vfs: *mut ffi::sqlite3_vfs, out: *mut f64) -> c_int {
    unsafe {
        let inner = inner(vfs);
        (*inner).xCurrentTime.expect("checked at registration")(inner, out)
    }
}

unsafe extern "C" fn x_get_last_error(
    vfs: *mut ffi::sqlite3_vfs,
    n: c_int,
    out: *mut c_char,
) -> c_int {
    unsafe {
        let inner = inner(vfs);
        (*inner).xGetLastError.expect("checked at registration")(inner, n, out)
    }
}

unsafe extern "C" fn x_current_time_int64(
    vfs: *mut ffi::sqlite3_vfs,
    out: *mut ffi::sqlite3_int64,
) -> c_int {
    unsafe {
        let inner = inner(vfs);
        (*inner).xCurrentTimeInt64.expect("checked at registration")(inner, out)
    }
}

unsafe extern "C" fn x_set_system_call(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    call: ffi::sqlite3_syscall_ptr,
) -> c_int {
    unsafe {
        let inner = inner(vfs);
        (*inner).xSetSystemCall.expect("checked at registration")(inner, name, call)
    }
}

unsafe extern "C" fn x_get_system_call(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
) -> ffi::sqlite3_syscall_ptr {
    unsafe {
        let inner = inner(vfs);
        (*inner).xGetSystemCall.expect("checked at registration")(inner, name)
    }
}

unsafe extern "C" fn x_next_system_call(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
) -> *const c_char {
    unsafe {
        let inner = inner(vfs);
        (*inner).xNextSystemCall.expect("checked at registration")(inner, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, OpenFlags};
    use std::fs;

    fn temp() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "layrs-guarded-vfs-{}-{}-{}",
            std::process::id(),
            super::super::change_index::now(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn connect(path: &std::path::Path, flags: OpenFlags) -> rusqlite::Result<Connection> {
        assert!(register());
        let connection = Connection::open_with_flags_and_vfs(path, flags, MAIN_ONLY)?;
        connection.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(connection)
    }

    #[test]
    fn planted_sidecars_are_never_seen_replayed_or_written() {
        let root = temp();
        let path = root.join("cache.sqlite3");
        {
            let connection = connect(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            )
            .unwrap();
            connection
                .query_row("PRAGMA journal_mode = MEMORY", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap();
            connection
                .execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1), (2);")
                .unwrap();
        }
        // Sidecars planted beside it: a non-empty journal, a WAL and a
        // shared-memory file linked to a file outside.
        let outside = root.join("outside.txt");
        fs::write(&outside, b"keep me").unwrap();
        let journal = root.join("cache.sqlite3-journal");
        fs::write(&journal, vec![0xd9_u8; 4096]).unwrap();
        fs::write(root.join("cache.sqlite3-wal"), vec![7_u8; 4096]).unwrap();
        fs::hard_link(&outside, root.join("cache.sqlite3-shm")).unwrap();
        for flags in [
            OpenFlags::SQLITE_OPEN_READ_ONLY,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        ] {
            let connection = connect(&path, flags).unwrap();
            let count: i64 = connection
                .query_row("SELECT count(*) FROM t", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 2);
            if flags == OpenFlags::SQLITE_OPEN_READ_WRITE {
                connection
                    .query_row("PRAGMA journal_mode = MEMORY", [], |row| {
                        row.get::<_, String>(0)
                    })
                    .unwrap();
                connection.execute("INSERT INTO t VALUES (3)", []).unwrap();
            }
        }
        assert_eq!(fs::read(&outside).unwrap(), b"keep me");
        assert_eq!(fs::read(&journal).unwrap(), vec![0xd9_u8; 4096]);
        // No temporary file either: temporary data stays in memory.
        let connection = connect(&path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
        connection
            .execute_batch("CREATE TEMP TABLE scratch (x); INSERT INTO scratch VALUES (1);")
            .unwrap();
        drop(connection);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_real_hot_journal_is_never_replayed() {
        let root = temp();
        let path = root.join("cache.sqlite3");
        let journal = root.join("cache.sqlite3-journal");
        // A genuine hot journal: the original page, taken while a
        // transaction that then committed was open.
        let planted = {
            let plain = Connection::open(&path).unwrap();
            plain.pragma_update(None, "synchronous", "OFF").unwrap();
            plain
                .execute_batch("CREATE TABLE t (x); INSERT INTO t VALUES (1);")
                .unwrap();
            plain.execute_batch("BEGIN; UPDATE t SET x = 2;").unwrap();
            let planted = fs::read(&journal).unwrap();
            plain.execute_batch("COMMIT").unwrap();
            planted
        };
        assert!(!journal.exists());
        fs::write(&journal, &planted).unwrap();
        let value = |connection: &Connection| -> i64 {
            connection
                .query_row("SELECT x FROM t", [], |row| row.get(0))
                .unwrap()
        };
        for flags in [
            OpenFlags::SQLITE_OPEN_READ_ONLY,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        ] {
            let connection = connect(&path, flags).unwrap();
            assert_eq!(value(&connection), 2);
        }
        assert_eq!(fs::read(&journal).unwrap(), planted);
        // SQLite's own VFS would replay it: the attack is real.
        let plain = Connection::open(&path).unwrap();
        assert_eq!(value(&plain), 1);
        drop(plain);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn a_database_with_another_link_is_refused() {
        let root = temp();
        let path = root.join("cache.sqlite3");
        {
            let connection = connect(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            )
            .unwrap();
            // A rollback journal on disk is refused like any sidecar.
            assert!(connection.execute_batch("CREATE TABLE t (x);").is_err());
            connection
                .query_row("PRAGMA journal_mode = MEMORY", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap();
            connection.execute_batch("CREATE TABLE t (x);").unwrap();
        }
        let elsewhere = root.join("elsewhere.sqlite3");
        fs::hard_link(&path, &elsewhere).unwrap();
        assert!(connect(&path, OpenFlags::SQLITE_OPEN_READ_WRITE).is_err());
        // A read-only open never writes: allowed.
        let reader = connect(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let count: i64 = reader
            .query_row("SELECT count(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        drop(reader);
        fs::remove_file(&elsewhere).unwrap();
        assert!(connect(&path, OpenFlags::SQLITE_OPEN_READ_WRITE).is_ok());
        let _ = fs::remove_dir_all(root);
    }
}
