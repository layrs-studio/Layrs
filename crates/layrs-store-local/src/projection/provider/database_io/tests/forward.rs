//! Test-child-only forwarding VFS. Real Win32 callbacks perform every operation.
//! The one interruption cut waits BEFORE the real delete; it does not fabricate
//! an I/O error and then rely on SQLite cleanup. Never linked in production.
#![allow(unsafe_op_in_unsafe_fn)]
use rusqlite::ffi;
use std::{
    ffi::{CStr, c_char, c_void},
    path::{Path, PathBuf},
    ptr,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

static ORIGINAL: AtomicPtr<ffi::sqlite3_vfs> = AtomicPtr::new(ptr::null_mut());
static ARMED: AtomicBool = AtomicBool::new(false);
static GEOMETRY_DRIFT: AtomicBool = AtomicBool::new(false);
static INITIAL_4096: AtomicBool = AtomicBool::new(false);
static SUPPRESSIBLE_CLOSE_ERROR: AtomicBool = AtomicBool::new(false);
static CUT: OnceLock<(String, PathBuf)> = OnceLock::new();
static WATCH: OnceLock<String> = OnceLock::new();
static EVENTS: Mutex<Vec<(String, String, i32)>> = Mutex::new(Vec::new());
static OVERFLOW: AtomicBool = AtomicBool::new(false);
pub(super) static RW_OPENS: AtomicUsize = AtomicUsize::new(0);
pub(super) static WATCHED_RW_SQL: AtomicUsize = AtomicUsize::new(0);
pub(super) static SUPPRESSED_CLOSES: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct Wrapped {
    base: ffi::sqlite3_file,
    real: *mut ffi::sqlite3_file,
    methods: ffi::sqlite3_io_methods,
    original: *const ffi::sqlite3_io_methods,
    writable: bool,
}
// SQLite allocates VFS file arenas with its native allocator alignment. Adding
// a multiple of16 preserves the original Win32 file arena's alignment; the
// real file remains inside the same SQLite-owned allocation through xClose.
const REAL_OFFSET: usize = (std::mem::size_of::<Wrapped>() + 15) & !15;

pub(super) fn install(watch: &Path) {
    assert!(
        ORIGINAL.load(Ordering::SeqCst).is_null(),
        "one forwarding registration per owned child"
    );
    WATCH.set(watch.to_str().unwrap().to_owned()).unwrap();
    unsafe {
        let original = ffi::sqlite3_vfs_find(c"win32".as_ptr());
        assert!(!original.is_null());
        assert_eq!((*original).iVersion, 3);
        let mut wrapper = Box::new(*original);
        wrapper.szOsFile = i32::try_from(REAL_OFFSET)
            .unwrap()
            .checked_add((*original).szOsFile)
            .unwrap();
        wrapper.pNext = ptr::null_mut();
        wrapper.xOpen = Some(open);
        wrapper.xDelete = Some(delete);
        wrapper.xAccess = Some(access);
        ORIGINAL.store(original, Ordering::SeqCst);
        let wrapper = Box::into_raw(wrapper);
        assert_eq!(ffi::sqlite3_vfs_register(wrapper, 1), ffi::SQLITE_OK);
        assert_eq!(ffi::sqlite3_vfs_find(ptr::null()), wrapper);
        assert_eq!(ffi::sqlite3_vfs_find(c"win32".as_ptr()), wrapper);
        // Known test-only observer: installs a statement trace but issues no SQL.
        assert_eq!(
            ffi::sqlite3_auto_extension(Some(trace_extension)),
            ffi::SQLITE_OK
        );
        // Real file open, not an in-memory DB: prove both default/named routing
        // before any negative relies on counters. This is setup before the
        // profile declaration; it performs no SQL and closes observably.
        let name = std::ffi::CString::new(watch.to_str().unwrap()).unwrap();
        for vfs_name in [ptr::null(), c"win32".as_ptr()] {
            let before = events().len();
            let mut db = ptr::null_mut();
            assert_eq!(
                ffi::sqlite3_open_v2(
                    name.as_ptr(),
                    &mut db,
                    ffi::SQLITE_OPEN_READONLY
                        | ffi::SQLITE_OPEN_PRIVATECACHE
                        | ffi::SQLITE_OPEN_FULLMUTEX,
                    vfs_name
                ),
                ffi::SQLITE_OK
            );
            assert!(!db.is_null());
            assert_eq!(ffi::sqlite3_close(db), ffi::SQLITE_OK);
            assert!(
                events()[before..]
                    .iter()
                    .any(|(op, _, flags)| op == "open" && flags & ffi::SQLITE_OPEN_MAIN_DB != 0),
                "real file open must pass through wrapper"
            );
        }
    }
}
pub(super) fn arm_delete(path_or_suffix: &str, ack: PathBuf) {
    CUT.set((path_or_suffix.into(), ack)).unwrap();
    ARMED.store(true, Ordering::SeqCst);
}
pub(super) fn drift_geometry() {
    GEOMETRY_DRIFT.store(true, Ordering::SeqCst);
}
pub(super) fn initial_sector_4096() {
    INITIAL_4096.store(true, Ordering::SeqCst);
}
pub(super) fn inject_reported_close_error() {
    SUPPRESSIBLE_CLOSE_ERROR.store(true, Ordering::SeqCst);
}
pub(super) fn events() -> Vec<(String, String, i32)> {
    assert!(!OVERFLOW.load(Ordering::SeqCst));
    EVENTS.lock().unwrap().clone()
}
fn event(op: &str, name: *const c_char, flags: i32) {
    let text = if name.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    if text.len() > 4096 {
        OVERFLOW.store(true, Ordering::SeqCst);
        return;
    }
    if let Ok(mut events) = EVENTS.lock() {
        if events.len() < 8192 {
            events.push((op.into(), text, flags));
        } else {
            OVERFLOW.store(true, Ordering::SeqCst);
        }
    }
}
unsafe extern "C" fn trace_extension(
    db: *mut ffi::sqlite3,
    _: *mut *mut c_char,
    _: *const ffi::sqlite3_api_routines,
) -> i32 {
    ffi::sqlite3_trace_v2(
        db,
        ffi::SQLITE_TRACE_STMT as u32,
        Some(trace),
        ptr::null_mut(),
    );
    ffi::SQLITE_OK
}
unsafe extern "C" fn trace(_: u32, _: *mut c_void, statement: *mut c_void, _: *mut c_void) -> i32 {
    let db = ffi::sqlite3_db_handle(statement.cast());
    let name = ffi::sqlite3_db_filename(db, c"main".as_ptr());
    if !name.is_null() && ffi::sqlite3_db_readonly(db, c"main".as_ptr()) == 0 {
        let name = CStr::from_ptr(name).to_string_lossy();
        if WATCH
            .get()
            .is_some_and(|watch| Path::new(name.as_ref()) == Path::new(watch))
        {
            WATCHED_RW_SQL.fetch_add(1, Ordering::SeqCst);
        }
    }
    0
}
unsafe extern "C" fn open(
    _: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    out: *mut ffi::sqlite3_file,
    flags: i32,
    out_flags: *mut i32,
) -> i32 {
    event("open", name, flags);
    if flags & ffi::SQLITE_OPEN_MAIN_DB != 0 && flags & ffi::SQLITE_OPEN_READWRITE != 0 {
        RW_OPENS.fetch_add(1, Ordering::SeqCst);
    }
    let real = out
        .cast::<u8>()
        .add(REAL_OFFSET)
        .cast::<ffi::sqlite3_file>();
    let original = ORIGINAL.load(Ordering::SeqCst);
    let rc = ((*original).xOpen.unwrap())(original, name, real, flags, out_flags);
    if (*real).pMethods.is_null() {
        (*out).pMethods = ptr::null();
        return rc;
    }
    let original_methods = (*real).pMethods;
    let mut methods = *original_methods;
    macro_rules! wrap {
        ($field:ident,$call:ident) => {
            if methods.$field.is_some() {
                methods.$field = Some($call);
            }
        };
    }
    wrap!(xClose, close);
    wrap!(xRead, read);
    wrap!(xWrite, write);
    wrap!(xTruncate, truncate);
    wrap!(xSync, sync);
    wrap!(xFileSize, size);
    wrap!(xLock, lock);
    wrap!(xUnlock, unlock);
    wrap!(xCheckReservedLock, reserved);
    wrap!(xFileControl, control);
    wrap!(xSectorSize, sector);
    wrap!(xDeviceCharacteristics, characteristics);
    wrap!(xShmMap, shm_map);
    wrap!(xShmLock, shm_lock);
    wrap!(xShmBarrier, shm_barrier);
    wrap!(xShmUnmap, shm_unmap);
    wrap!(xFetch, fetch);
    wrap!(xUnfetch, unfetch);
    let wrapped = out.cast::<Wrapped>();
    ptr::write(
        wrapped,
        Wrapped {
            base: ffi::sqlite3_file {
                pMethods: ptr::null(),
            },
            real,
            methods,
            original: original_methods,
            writable: flags & ffi::SQLITE_OPEN_READWRITE != 0
                && flags & ffi::SQLITE_OPEN_MAIN_DB != 0,
        },
    );
    (*wrapped).base.pMethods = &(*wrapped).methods;
    rc
}
unsafe extern "C" fn delete(_: *mut ffi::sqlite3_vfs, name: *const c_char, sync_dir: i32) -> i32 {
    event("delete", name, sync_dir);
    if ARMED.load(Ordering::SeqCst) && !name.is_null() {
        let text = CStr::from_ptr(name).to_string_lossy();
        if let Some((selector, ack)) = CUT.get() {
            let selected = if selector == "super-journal" {
                text.contains("-mj")
            } else {
                Path::new(text.as_ref()) == Path::new(selector)
            };
            if selected && ARMED.swap(false, Ordering::SeqCst) {
                use std::io::Write;
                // Complete bounded ACK before waiting. A failure is UNQUALIFIED,
                // never an alternative cut synthesized through SQLite cleanup.
                let result = (|| -> std::io::Result<()> {
                    let parent = crate::secure_fs::PinnedDirectory::open(
                        ack.parent()
                            .ok_or_else(|| std::io::Error::other("ACK parent absent"))?,
                    )?;
                    let final_name = ack
                        .file_name()
                        .ok_or_else(|| std::io::Error::other("ACK leaf absent"))?;
                    let mut temporary = final_name.to_os_string();
                    temporary.push(".pending");
                    let mut f = parent.create_new_private_file(&temporary)?;
                    f.file_mut().write_all(text.as_bytes())?;
                    f.file().sync_all()?;
                    if f.rename_to(&parent, final_name)? {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            "owned ACK already exists",
                        ));
                    }
                    parent.sync()
                })();
                if result.is_err() {
                    std::process::exit(151);
                }
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(240) {
                    std::thread::sleep(Duration::from_millis(25));
                }
                std::process::exit(152); // Parent must kill/reap this exact paused child.
            }
        }
    }
    let original = ORIGINAL.load(Ordering::SeqCst);
    ((*original).xDelete.unwrap())(original, name, sync_dir)
}
unsafe extern "C" fn access(
    _: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    flags: i32,
    result: *mut i32,
) -> i32 {
    event("access", name, flags);
    let v = ORIGINAL.load(Ordering::SeqCst);
    ((*v).xAccess.unwrap())(v, name, flags, result)
}
unsafe fn parts(
    file: *mut ffi::sqlite3_file,
) -> (
    *mut ffi::sqlite3_file,
    &'static ffi::sqlite3_io_methods,
    bool,
) {
    let w = &*file.cast::<Wrapped>();
    (w.real, &*w.original, w.writable)
}
unsafe extern "C" fn close(f: *mut ffi::sqlite3_file) -> i32 {
    let (r, m, _) = parts(f);
    let rc = (m.xClose.unwrap())(r);
    if rc == ffi::SQLITE_OK && SUPPRESSIBLE_CLOSE_ERROR.load(Ordering::SeqCst) {
        SUPPRESSED_CLOSES.fetch_add(1, Ordering::SeqCst);
        ffi::SQLITE_IOERR_CLOSE
    } else {
        rc
    }
}
unsafe extern "C" fn read(f: *mut ffi::sqlite3_file, p: *mut c_void, n: i32, o: i64) -> i32 {
    let (r, m, _) = parts(f);
    (m.xRead.unwrap())(r, p, n, o)
}
unsafe extern "C" fn write(f: *mut ffi::sqlite3_file, p: *const c_void, n: i32, o: i64) -> i32 {
    let (r, m, _) = parts(f);
    (m.xWrite.unwrap())(r, p, n, o)
}
unsafe extern "C" fn truncate(f: *mut ffi::sqlite3_file, n: i64) -> i32 {
    let (r, m, _) = parts(f);
    (m.xTruncate.unwrap())(r, n)
}
unsafe extern "C" fn sync(f: *mut ffi::sqlite3_file, flags: i32) -> i32 {
    let (r, m, _) = parts(f);
    (m.xSync.unwrap())(r, flags)
}
unsafe extern "C" fn size(f: *mut ffi::sqlite3_file, n: *mut i64) -> i32 {
    let (r, m, _) = parts(f);
    (m.xFileSize.unwrap())(r, n)
}
unsafe extern "C" fn lock(f: *mut ffi::sqlite3_file, n: i32) -> i32 {
    let (r, m, _) = parts(f);
    (m.xLock.unwrap())(r, n)
}
unsafe extern "C" fn unlock(f: *mut ffi::sqlite3_file, n: i32) -> i32 {
    let (r, m, _) = parts(f);
    (m.xUnlock.unwrap())(r, n)
}
unsafe extern "C" fn reserved(f: *mut ffi::sqlite3_file, n: *mut i32) -> i32 {
    let (r, m, _) = parts(f);
    (m.xCheckReservedLock.unwrap())(r, n)
}
unsafe extern "C" fn control(f: *mut ffi::sqlite3_file, n: i32, p: *mut c_void) -> i32 {
    let (r, m, _) = parts(f);
    (m.xFileControl.unwrap())(r, n, p)
}
unsafe extern "C" fn sector(f: *mut ffi::sqlite3_file) -> i32 {
    let (r, m, w) = parts(f);
    let actual = (m.xSectorSize.unwrap())(r);
    if INITIAL_4096.load(Ordering::SeqCst) {
        return 4096;
    }
    if w && GEOMETRY_DRIFT.load(Ordering::SeqCst) {
        if actual == 2048 { 4096 } else { 2048 }
    } else {
        actual
    }
}
unsafe extern "C" fn characteristics(f: *mut ffi::sqlite3_file) -> i32 {
    let (r, m, w) = parts(f);
    let actual = (m.xDeviceCharacteristics.unwrap())(r);
    if INITIAL_4096.load(Ordering::SeqCst) || (w && GEOMETRY_DRIFT.load(Ordering::SeqCst)) {
        actual & !ffi::SQLITE_IOCAP_POWERSAFE_OVERWRITE
    } else {
        actual
    }
}
unsafe extern "C" fn shm_map(
    f: *mut ffi::sqlite3_file,
    p: i32,
    n: i32,
    w: i32,
    out: *mut *mut c_void,
) -> i32 {
    let (r, m, _) = parts(f);
    (m.xShmMap.unwrap())(r, p, n, w, out)
}
unsafe extern "C" fn shm_lock(f: *mut ffi::sqlite3_file, o: i32, n: i32, flags: i32) -> i32 {
    let (r, m, _) = parts(f);
    (m.xShmLock.unwrap())(r, o, n, flags)
}
unsafe extern "C" fn shm_barrier(f: *mut ffi::sqlite3_file) {
    let (r, m, _) = parts(f);
    (m.xShmBarrier.unwrap())(r)
}
unsafe extern "C" fn shm_unmap(f: *mut ffi::sqlite3_file, d: i32) -> i32 {
    let (r, m, _) = parts(f);
    (m.xShmUnmap.unwrap())(r, d)
}
unsafe extern "C" fn fetch(f: *mut ffi::sqlite3_file, o: i64, n: i32, p: *mut *mut c_void) -> i32 {
    let (r, m, _) = parts(f);
    (m.xFetch.unwrap())(r, o, n, p)
}
unsafe extern "C" fn unfetch(f: *mut ffi::sqlite3_file, o: i64, p: *mut c_void) -> i32 {
    let (r, m, _) = parts(f);
    (m.xUnfetch.unwrap())(r, o, p)
}
