//! Real Step1 APIs; all fixtures/processes are explicitly owned and retained.
//! These receipts are test inputs, never an implementation of Store authority.
use super::{disposable_run, native_info};
use layrs_vfs::{
    ControlDirectoryFailure as Failure, ControlDirectoryGuard, ControlDirectoryIdentity,
    ControlDirectoryPublishFaults, ControlDirectoryPublishPoint as Point,
    ControlDirectoryStageIdentity, ControlRootPreflight, Error, NoControlDirectoryPublishFaults,
    Result, discover_control_directory, observe_control_directory_stage,
    publish_control_directory_recoverable, read_control_file,
};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    mem::{offset_of, size_of, size_of_val},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::{
    Win32::{
        Foundation::*,
        Security::*,
        Storage::{CloudFilters::*, FileSystem::*},
        System::{IO::DeviceIoControl, LibraryLoader::*},
    },
    core::GUID,
};
#[path = "../../src/windows/api.rs"]
mod api;

const BYTES: &[u8] = b"{\"testOnly\":\"Step1 opaque canonical FORMAT\"}\n";
const READ_CONTROL: u32 = 0x20000;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
type Id = (u32, u64);
fn id(file: &File) -> Id {
    let value = native_info(file);
    (
        value.dwVolumeSerialNumber,
        (u64::from(value.nFileIndexHigh) << 32) | u64::from(value.nFileIndexLow),
    )
}
fn fsid(value: Id) -> String {
    format!("fs:v1:{:016x}:{:016x}", value.0, value.1)
}
fn open(path: &Path, access: u32, sharing: u32) -> File {
    try_open(path, access, sharing).unwrap()
}
fn try_open(path: &Path, access: u32, sharing: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(sharing)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}
fn path_id(path: &Path) -> Id {
    id(&open(path, FILE_READ_ATTRIBUTES, SHARE_ALL))
}
fn wide(path: &Path) -> Vec<u16> {
    let value = path.to_str().unwrap().replace('/', "\\");
    assert!(path.is_absolute() && !value.starts_with("\\\\"));
    format!("\\\\?\\{value}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}
struct Library(HMODULE);
impl Library {
    fn load(name: &str) -> Self {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let module = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        assert!(
            !module.is_null(),
            "System32 DLL: {}",
            io::Error::last_os_error()
        );
        Self(module)
    }
    unsafe fn symbol<T: Copy>(&self, name: &'static [u8]) -> T {
        let symbol = unsafe { GetProcAddress(self.0, name.as_ptr()) }.unwrap();
        assert_eq!(size_of::<T>(), size_of_val(&symbol));
        unsafe { std::mem::transmute_copy(&symbol) }
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}
struct Allocation(*mut c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0) };
        }
    }
}
type Current = unsafe extern "system" fn() -> HANDLE;
type Token = unsafe extern "system" fn(HANDLE, u32, *mut HANDLE) -> i32;
type SidText = unsafe extern "system" fn(PSID, *mut *mut u16) -> i32;
type ParseSd =
    unsafe extern "system" fn(*const u16, u32, *mut PSECURITY_DESCRIPTOR, *mut u32) -> i32;
type SecurityInfo = unsafe extern "system" fn(
    HANDLE,
    i32,
    u32,
    *mut PSID,
    *mut PSID,
    *mut *mut ACL,
    *mut *mut ACL,
    *mut PSECURITY_DESCRIPTOR,
) -> u32;
struct Native {
    _advapi: Library,
    _kernel: Library,
    current: Current,
    token: Token,
    sid_text: SidText,
    parse_sd: ParseSd,
    security_info: SecurityInfo,
}
impl Native {
    fn load() -> Self {
        let advapi = Library::load("advapi32.dll");
        let kernel = Library::load("kernel32.dll");
        Self {
            current: unsafe { kernel.symbol(b"GetCurrentProcess\0") },
            token: unsafe { advapi.symbol(b"OpenProcessToken\0") },
            sid_text: unsafe { advapi.symbol(b"ConvertSidToStringSidW\0") },
            parse_sd: unsafe {
                advapi.symbol(b"ConvertStringSecurityDescriptorToSecurityDescriptorW\0")
            },
            security_info: unsafe { advapi.symbol(b"GetSecurityInfo\0") },
            _advapi: advapi,
            _kernel: kernel,
        }
    }
    fn sd(&self) -> Allocation {
        let mut token = std::ptr::null_mut();
        assert_ne!(
            unsafe { (self.token)((self.current)(), TOKEN_QUERY, &mut token) },
            0
        );
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut length = 0;
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut length,
            )
        };
        assert!((size_of::<TOKEN_USER>() as u32..=65536).contains(&length));
        let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        assert_ne!(
            unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    buffer.as_mut_ptr().cast(),
                    length,
                    &mut length,
                )
            },
            0
        );
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text = std::ptr::null_mut();
        assert_ne!(unsafe { (self.sid_text)(sid, &mut text) }, 0);
        let _text_allocation = Allocation(text.cast());
        let mut count = 0;
        while unsafe { *text.add(count) } != 0 {
            count += 1;
            assert!(count <= 256);
        }
        let owner = String::from_utf16(unsafe { std::slice::from_raw_parts(text, count) }).unwrap();
        let sddl: Vec<u16> = format!("O:{owner}D:P(A;OICI;FA;;;{owner})\0")
            .encode_utf16()
            .collect();
        let mut descriptor = std::ptr::null_mut();
        assert_ne!(
            unsafe { (self.parse_sd)(sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()) },
            0
        );
        Allocation(descriptor)
    }
    fn directory(&self, path: &Path) {
        let descriptor = self.sd();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        assert_ne!(
            unsafe { CreateDirectoryW(wide(path).as_ptr(), &attributes) },
            0,
            "create private dir: {}",
            io::Error::last_os_error()
        );
    }
    fn file(&self, path: &Path) -> File {
        let descriptor = self.sd();
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        let handle = unsafe {
            CreateFileW(
                wide(path).as_ptr(),
                FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL,
                SHARE_ALL,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            handle,
            INVALID_HANDLE_VALUE,
            "create private file: {}",
            io::Error::last_os_error()
        );
        unsafe { File::from_raw_handle(handle) }
    }
    fn write_new(&self, path: &Path, bytes: &[u8]) {
        let mut file = self.file(path);
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }
    fn protected(&self, path: &Path) -> bool {
        let file = open(path, READ_CONTROL | FILE_READ_ATTRIBUTES, SHARE_ALL);
        let mut descriptor = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                (self.security_info)(
                    file.as_raw_handle(),
                    1,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        let _allocation = Allocation(descriptor);
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        control & 0x1000 != 0
    }
}

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    parent: PathBuf,
    stage: PathBuf,
    metadata: PathBuf,
    allocated: ControlDirectoryStageIdentity,
}
impl Fixture {
    fn new(native: &Native, run: &Path, label: &str) -> Self {
        let base = run.join(label);
        std::fs::create_dir(&base).unwrap();
        let root = base.join("checkout");
        std::fs::create_dir(&root).unwrap();
        let parent = base.join("ordinary-inherited-stage-parent");
        std::fs::create_dir(&parent).unwrap();
        assert!(
            !native.protected(&parent),
            "ordinary parent arm must actually be inherited"
        );
        let stage = parent.join("fresh-unique-owned-stage");
        native.directory(&stage);
        let metadata = base.join("external-test-receipts");
        native.directory(&metadata);
        let preflight = ControlRootPreflight::open(&root).unwrap();
        let mut format = native.file(&stage.join("FORMAT"));
        let original_id = id(&format);
        let allocated = observe_control_directory_stage(&preflight, &stage).unwrap();
        assert_eq!(
            allocated.root_filesystem_identity(),
            preflight.filesystem_identity()
        );
        assert_eq!(
            allocated.stage_parent_filesystem_identity(),
            fsid(path_id(&parent))
        );
        assert_eq!(
            allocated.directory_filesystem_identity(),
            fsid(path_id(&stage))
        );
        assert_eq!(
            allocated.format_binding_identity(),
            format!(
                "{}:{}:{}",
                original_id.0,
                original_id.1 >> 32,
                original_id.1 as u32
            )
        );
        assert!(native.protected(&stage) && native.protected(&stage.join("FORMAT")));
        // Real durable test allocation before bytes; no production receipt parser.
        native.write_new(&metadata.join("allocation.bin"), &allocated.encode());
        format.write_all(BYTES).unwrap();
        format.sync_all().unwrap();
        drop(format);
        assert!(
            observe_control_directory_stage(&preflight, &stage).is_err(),
            "initial observation must refuse nonempty FORMAT"
        );
        drop(preflight);
        Self {
            base,
            root,
            parent,
            stage,
            metadata,
            allocated,
        }
    }
    fn prepared_path(&self) -> PathBuf {
        self.metadata.join("prepared.bin")
    }
    fn prepared(&self) -> ControlDirectoryIdentity {
        ControlDirectoryIdentity::decode(&std::fs::read(self.prepared_path()).unwrap()).unwrap()
    }
    fn publish(&self, native: &Native) -> ControlDirectoryGuard {
        let preflight = ControlRootPreflight::open(&self.root).unwrap();
        publish_control_directory_recoverable(
            &preflight,
            &self.stage,
            &self.allocated,
            BYTES,
            None,
            |expected| {
                native.write_new(&self.prepared_path(), &expected.encode());
                Ok(())
            },
            &NoControlDirectoryPublishFaults,
        )
        .unwrap()
    }
}
struct Fault<F: Fn(Point) -> io::Result<()>>(F);
impl<F: Fn(Point) -> io::Result<()>> ControlDirectoryPublishFaults for Fault<F> {
    fn at(&self, point: Point) -> io::Result<()> {
        (self.0)(point)
    }
}
fn cause<T>(result: Result<T>) -> Error {
    match result {
        Ok(_) => panic!("unexpected guard/success"),
        Err(error) => error,
    }
}
fn kind(error: &Error) -> Failure {
    match error {
        Error::ControlDirectory { kind, .. } => *kind,
        _ => panic!("unscoped error: {error:?}"),
    }
}
fn raw(error: &Error) -> Option<i32> {
    match error {
        Error::ControlDirectory { cause, .. } => raw(cause),
        Error::Io(error) => error.raw_os_error(),
        _ => None,
    }
}
fn sharing<T>(result: io::Result<T>, label: &str) {
    let error = match result {
        Ok(_) => panic!("{label} succeeded"),
        Err(error) => error,
    };
    eprintln!("{label} native={error:?}");
    assert_eq!(error.raw_os_error(), Some(32));
}

fn clean_and_reopen(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "clean-clone-observe-reopen");
    let guard = fixture.publish(native);
    guard.revalidate_bootstrap_root().unwrap();
    let expected = guard.identity().clone();
    let observed = discover_control_directory(&fixture.root).unwrap();
    assert!(observed.identity().matches_expected(&expected));
    assert_eq!(observed.format_bytes(), BYTES);
    assert!(
        matches!(read_control_file(&fixture.root), Err(Error::Unsupported(_))),
        "held DELETE pin must not hide legacy type refusal behind sharing32"
    );
    let control = fixture.root.join(".layrs");
    let observer = open(
        &control,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        SHARE_ALL,
    );
    assert_eq!(fsid(id(&observer)), guard.directory_filesystem_identity());
    sharing(
        try_open(
            &control,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        ),
        "old incompatible shareRW observer",
    );
    let cloned = guard.try_clone().unwrap();
    drop(guard);
    drop(observer);
    sharing(
        try_open(&control, DELETE, SHARE_ALL),
        "clone excludes directory DELETE",
    );
    sharing(
        std::fs::rename(&control, fixture.root.join("moved")),
        "clone excludes directory rename",
    );
    sharing(
        try_open(&control.join("FORMAT"), FILE_WRITE_DATA, SHARE_ALL),
        "strong FORMAT excludes data writes",
    );
    cloned.revalidate().unwrap();
    drop(cloned);
    drop(observed);
    // ALL old native guards are gone. Even the historical stage parent pathname
    // is removed by an owned rename; final expected-open must not reopen it.
    std::fs::rename(
        &fixture.parent,
        fixture.base.join("retained-old-stage-parent"),
    )
    .unwrap();
    assert!(!fixture.parent.exists() && !fixture.stage.exists());
    let fresh = ControlDirectoryGuard::open(&fixture.root, &expected).unwrap();
    fresh.revalidate_bootstrap_root().unwrap();
    native.write_new(
        &fixture.root.join("user-work.txt"),
        b"real user bytes retained\n",
    );
    fresh.revalidate().unwrap();
    assert_eq!(
        kind(&cause(fresh.revalidate_bootstrap_root())),
        Failure::NeedsRepair
    );
    assert_eq!(
        std::fs::read(fixture.root.join("user-work.txt")).unwrap(),
        b"real user bytes retained\n"
    );
    eprintln!(
        "Step1 clean: ordinary inherited parent; protected stage/FORMAT; real32; clone; drop-all fresh open without obsolete stage path; populated layout succeeds, bootstrap refuses"
    );

    let legacy = run.join("legacy-control-file");
    std::fs::create_dir(&legacy).unwrap();
    let bytes = b"legacy opaque complete-Store file bytes\n";
    native.write_new(&legacy.join(".layrs"), bytes);
    assert!(matches!(
        discover_control_directory(&legacy),
        Err(Error::Unsupported(_))
    ));
    assert_eq!(read_control_file(&legacy).unwrap().0, bytes);
    assert_eq!(std::fs::read(legacy.join(".layrs")).unwrap(), bytes);
}

fn publication_refusals(native: &Native, run: &Path) {
    for mutation in ["same-byte-replacement", "short-format"] {
        let fixture = Fixture::new(native, run, mutation);
        let original = path_id(&fixture.stage.join("FORMAT"));
        if mutation == "same-byte-replacement" {
            std::fs::rename(
                fixture.stage.join("FORMAT"),
                fixture.metadata.join("original-FORMAT-witness"),
            )
            .unwrap();
            native.write_new(&fixture.stage.join("FORMAT"), BYTES);
            assert_ne!(path_id(&fixture.stage.join("FORMAT")), original);
        } else {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(fixture.stage.join("FORMAT"))
                .unwrap();
            file.write_all(b"partial").unwrap();
            file.sync_all().unwrap();
            assert_eq!(path_id(&fixture.stage.join("FORMAT")), original);
        }
        let preflight = ControlRootPreflight::open(&fixture.root).unwrap();
        let calls = Cell::new(0);
        let error = cause(publish_control_directory_recoverable(
            &preflight,
            &fixture.stage,
            &fixture.allocated,
            BYTES,
            None,
            |_| {
                calls.set(calls.get() + 1);
                Ok(())
            },
            &NoControlDirectoryPublishFaults,
        ));
        assert_eq!(kind(&error), Failure::NeedsRepair);
        assert_eq!(calls.get(), 0);
        assert!(fixture.stage.exists() && !fixture.root.join(".layrs").exists());
        eprintln!("Step1 {mutation}: {error}; no adoption/repair/callback");
    }
    let fixture = Fixture::new(native, run, "ambiguous-persist-error");
    let preflight = ControlRootPreflight::open(&fixture.root).unwrap();
    let error = cause(publish_control_directory_recoverable(
        &preflight,
        &fixture.stage,
        &fixture.allocated,
        BYTES,
        None,
        |expected| {
            native.write_new(&fixture.prepared_path(), &expected.encode());
            Err(io::Error::from_raw_os_error(112).into())
        },
        &NoControlDirectoryPublishFaults,
    ));
    assert_eq!(kind(&error), Failure::ReceiptDurabilityUnknown);
    assert_eq!(raw(&error), Some(112));
    assert!(fixture.stage.exists() && !fixture.root.join(".layrs").exists());
    let prepared = fixture.prepared();
    let guard = publish_control_directory_recoverable(
        &preflight,
        &fixture.stage,
        &fixture.allocated,
        BYTES,
        Some(&prepared),
        |_| panic!("expectedSome must never repersist"),
        &NoControlDirectoryPublishFaults,
    )
    .unwrap();
    guard.revalidate_bootstrap_root().unwrap();
    drop(guard);
    // No final owner handle survives; the retained preflight alone is not the
    // final proof. Replay must acquire a fresh sole DELETE owner and FORMAT pin.
    let replay = publish_control_directory_recoverable(
        &preflight,
        fixture.allocated.stage(),
        &fixture.allocated,
        BYTES,
        Some(&prepared),
        |_| panic!("successful final replay must never repersist"),
        &NoControlDirectoryPublishFaults,
    )
    .unwrap();
    replay.revalidate_bootstrap_root().unwrap();
    assert_eq!(replay.identity(), &prepared);
    drop(replay);
    native.write_new(&fixture.root.join("additional-user-file"), b"keep\n");
    let error = cause(publish_control_directory_recoverable(
        &preflight,
        &fixture.stage,
        &fixture.allocated,
        BYTES,
        Some(&prepared),
        |_| panic!("replay must never repersist"),
        &NoControlDirectoryPublishFaults,
    ));
    assert_eq!(kind(&error), Failure::NeedsRepair);
    assert_eq!(
        std::fs::read(fixture.root.join("additional-user-file")).unwrap(),
        b"keep\n"
    );
    eprintln!(
        "Step1 persist error: durable receipt reread, explicit expectedSome recovery; publisher replay refuses extra user entry"
    );
}

fn leaked_reader_and_collision(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "one-rename-reader-window");
    let preflight = ControlRootPreflight::open(&fixture.root).unwrap();
    let leaked = RefCell::new(None);
    let renames = Cell::new(0);
    let faults = Fault(|point| {
        if point == Point::PostClose {
            *leaked.borrow_mut() = Some(open(
                &fixture.stage.join("FORMAT"),
                FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                SHARE_ALL,
            ));
        }
        if point == Point::PreRename {
            renames.set(renames.get() + 1);
        }
        Ok(())
    });
    let error = cause(publish_control_directory_recoverable(
        &preflight,
        fixture.allocated.stage(),
        &fixture.allocated,
        BYTES,
        None,
        |expected| {
            native.write_new(&fixture.prepared_path(), &expected.encode());
            Ok(())
        },
        &faults,
    ));
    assert_eq!(kind(&error), Failure::RenameRefused);
    assert_eq!(raw(&error), Some(5));
    assert!(
        matches!(&error, Error::ControlDirectory { point: Point::PreRename, operation, .. } if operation.contains("FileRenameInfo"))
    );
    assert_eq!(renames.get(), 1);
    assert!(fixture.stage.exists() && !fixture.root.join(".layrs").exists());
    eprintln!("Step1 single attempt {error}; raw5 retained, blocker not inferred");
    drop(leaked.borrow_mut().take());
    drop(preflight);
    let fresh = ControlRootPreflight::open(&fixture.root).unwrap();
    let allocated = ControlDirectoryStageIdentity::decode(
        &std::fs::read(fixture.metadata.join("allocation.bin")).unwrap(),
    )
    .unwrap();
    let prepared = fixture.prepared();
    let guard = publish_control_directory_recoverable(
        &fresh,
        allocated.stage(),
        &allocated,
        BYTES,
        Some(&prepared),
        |_| panic!("retry must not repersist"),
        &NoControlDirectoryPublishFaults,
    )
    .unwrap();
    guard.revalidate_bootstrap_root().unwrap();

    let fixture = Fixture::new(native, run, "same-byte-final-collision");
    let preflight = ControlRootPreflight::open(&fixture.root).unwrap();
    let foreign = RefCell::new(None);
    let faults = Fault(|point| {
        if point == Point::PreRename {
            native.directory(&fixture.root.join(".layrs"));
            native.write_new(&fixture.root.join(".layrs/FORMAT"), BYTES);
            *foreign.borrow_mut() = Some((
                path_id(&fixture.root.join(".layrs")),
                path_id(&fixture.root.join(".layrs/FORMAT")),
            ));
        }
        Ok(())
    });
    let stage_id = path_id(&fixture.stage);
    let format_id = path_id(&fixture.stage.join("FORMAT"));
    let error = cause(publish_control_directory_recoverable(
        &preflight,
        fixture.allocated.stage(),
        &fixture.allocated,
        BYTES,
        None,
        |expected| {
            native.write_new(&fixture.prepared_path(), &expected.encode());
            Ok(())
        },
        &faults,
    ));
    assert_eq!(kind(&error), Failure::NeedsRepair);
    assert!(matches!(raw(&error), Some(80 | 183)));
    assert_eq!(path_id(&fixture.stage), stage_id);
    assert_eq!(path_id(&fixture.stage.join("FORMAT")), format_id);
    let foreign = foreign.borrow().unwrap();
    assert_ne!(foreign.0, stage_id);
    assert_ne!(foreign.1, format_id);
    assert_eq!(path_id(&fixture.root.join(".layrs")), foreign.0);
    assert_eq!(std::fs::read(fixture.stage.join("FORMAT")).unwrap(), BYTES);
    assert_eq!(
        std::fs::read(fixture.root.join(".layrs/FORMAT")).unwrap(),
        BYTES
    );
    eprintln!("Step1 collision retained both exact IDs/bytes: {error}");
}

fn publication_window_mutations(native: &Native, run: &Path) {
    for mutation in [
        "same-byte-new-object",
        "changed-bytes",
        "changed-length",
        "extra-child",
        "missing-FORMAT",
        "hardlink",
    ] {
        let fixture = Fixture::new(native, run, &format!("window-{mutation}"));
        let preflight = ControlRootPreflight::open(&fixture.root).unwrap();
        let original = path_id(&fixture.stage.join("FORMAT"));
        let directory = path_id(&fixture.stage);
        native.write_new(&fixture.metadata.join("original-byte-witness"), BYTES);
        let entered = Cell::new(false);
        let persisted = Cell::new(0);
        let faults = Fault(|point| {
            if point != Point::PostClose {
                return Ok(());
            }
            assert!(!entered.replace(true));
            let format = fixture.stage.join("FORMAT");
            match mutation {
                "same-byte-new-object" => {
                    std::fs::rename(&format, fixture.metadata.join("original-object-witness"))?;
                    native.write_new(&format, BYTES);
                    assert_ne!(path_id(&format), original);
                }
                "changed-bytes" => {
                    let mut bytes = BYTES.to_vec();
                    bytes[0] ^= 1;
                    let mut file = OpenOptions::new().write(true).open(&format)?;
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    assert_eq!(id(&file), original);
                }
                "changed-length" => {
                    let file = OpenOptions::new().write(true).open(&format)?;
                    file.set_len(BYTES.len() as u64 - 1)?;
                    file.sync_all()?;
                    assert_eq!(id(&file), original);
                }
                "extra-child" => native.write_new(
                    &fixture.stage.join("foreign-extra"),
                    b"retain foreign extra\n",
                ),
                "missing-FORMAT" => {
                    std::fs::rename(&format, fixture.metadata.join("original-object-witness"))?
                }
                "hardlink" => {
                    std::fs::hard_link(&format, fixture.metadata.join("retained-link-witness"))?
                }
                _ => unreachable!(),
            }
            Ok(())
        });
        let error = cause(publish_control_directory_recoverable(
            &preflight,
            fixture.allocated.stage(),
            &fixture.allocated,
            BYTES,
            None,
            |expected| {
                persisted.set(persisted.get() + 1);
                native.write_new(&fixture.prepared_path(), &expected.encode());
                Ok(())
            },
            &faults,
        ));
        assert!(entered.get());
        assert_eq!(persisted.get(), 1);
        assert_eq!(kind(&error), Failure::NeedsRepair);
        assert_eq!(
            std::fs::read(fixture.metadata.join("original-byte-witness")).unwrap(),
            BYTES
        );
        let retained = if fixture.stage.exists() {
            fixture.stage.clone()
        } else {
            fixture.root.join(".layrs")
        };
        assert_eq!(path_id(&retained), directory);
        match mutation {
            "same-byte-new-object" => {
                assert_ne!(path_id(&retained.join("FORMAT")), original);
                assert_eq!(std::fs::read(retained.join("FORMAT")).unwrap(), BYTES);
                assert_eq!(
                    path_id(&fixture.metadata.join("original-object-witness")),
                    original
                );
            }
            "changed-bytes" => {
                let bytes = std::fs::read(retained.join("FORMAT")).unwrap();
                assert_eq!(bytes.len(), BYTES.len());
                assert_ne!(bytes, BYTES);
            }
            "changed-length" => assert_eq!(
                std::fs::metadata(retained.join("FORMAT")).unwrap().len(),
                BYTES.len() as u64 - 1
            ),
            "extra-child" => assert_eq!(
                std::fs::read(retained.join("foreign-extra")).unwrap(),
                b"retain foreign extra\n"
            ),
            "missing-FORMAT" => {
                assert!(!retained.join("FORMAT").exists());
                assert_eq!(
                    std::fs::read(fixture.metadata.join("original-object-witness")).unwrap(),
                    BYTES
                );
            }
            "hardlink" => assert!(
                native_info(&open(
                    &retained.join("FORMAT"),
                    FILE_READ_ATTRIBUTES,
                    SHARE_ALL
                ))
                .nNumberOfLinks
                    > 1
            ),
            _ => unreachable!(),
        }
        eprintln!(
            "Step1 production close-window {mutation}: {error}; native identity and all witnesses retained, no final proof/repair"
        );
    }
}

fn set_attributes(path: &Path, attributes: u32) -> io::Result<()> {
    let file = try_open(
        path,
        FILE_WRITE_ATTRIBUTES | FILE_READ_ATTRIBUTES,
        SHARE_ALL,
    )?;
    let info = FILE_BASIC_INFO {
        FileAttributes: attributes,
        ..Default::default()
    };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileBasicInfo,
            (&info as *const FILE_BASIC_INFO).cast(),
            size_of_val(&info) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn attrs(path: &Path) -> u32 {
    native_info(&open(path, FILE_READ_ATTRIBUTES, SHARE_ALL)).dwFileAttributes
}
fn metadata_mutations(native: &Native, run: &Path) {
    for target in ["FORMAT", "directory"] {
        let fixture = Fixture::new(native, run, &format!("attribute-after-pin-{target}"));
        let guard = fixture.publish(native);
        let path = if target == "FORMAT" {
            fixture.root.join(".layrs/FORMAT")
        } else {
            fixture.root.join(".layrs")
        };
        let before = attrs(&path);
        // Metadata writes may succeed while data writes/deletion are excluded.
        let mutation = set_attributes(&path, before | FILE_ATTRIBUTE_OFFLINE);
        eprintln!("Step1 {target} OFFLINE native mutation: {mutation:?}");
        mutation.expect("attribute detection requires an actually established mutation");
        assert_ne!(attrs(&path) & FILE_ATTRIBUTE_OFFLINE, 0);
        assert_eq!(kind(&cause(guard.revalidate())), Failure::NeedsRepair);
    }
    for target in ["root", "directory", "FORMAT"] {
        let fixture = Fixture::new(native, run, &format!("stream-after-pin-{target}"));
        let guard = fixture.publish(native);
        let path = match target {
            "root" => fixture.root.clone(),
            "directory" => fixture.root.join(".layrs"),
            _ => fixture.root.join(".layrs/FORMAT"),
        };
        let stream = PathBuf::from(format!("{}:step1-retained-witness", path.display()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stream)
            .unwrap();
        file.write_all(b"real named stream\n").unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert_eq!(kind(&cause(guard.revalidate())), Failure::NeedsRepair);
    }
    let fixture = Fixture::new(native, run, "hardlink-after-pin");
    let guard = fixture.publish(native);
    let witness = fixture.metadata.join("retained-hardlink");
    std::fs::hard_link(fixture.root.join(".layrs/FORMAT"), &witness).unwrap();
    assert!(native_info(&open(&witness, FILE_READ_ATTRIBUTES, SHARE_ALL)).nNumberOfLinks > 1);
    assert_eq!(kind(&cause(guard.revalidate())), Failure::NeedsRepair);
    eprintln!(
        "Step1 actual attrs/streams/links rejected by revalidation, never claimed frozen by share locks"
    );

    let base = run.join("inherited-FORMAT-privacy-negative");
    std::fs::create_dir(&base).unwrap();
    let root = base.join("checkout");
    std::fs::create_dir(&root).unwrap();
    let stage = base.join("protected-stage");
    native.directory(&stage);
    let format = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stage.join("FORMAT"))
        .unwrap();
    drop(format);
    assert!(
        !native.protected(&stage.join("FORMAT")),
        "negative must actually lack protected FORMAT DACL"
    );
    let preflight = ControlRootPreflight::open(&root).unwrap();
    assert!(observe_control_directory_stage(&preflight, &stage).is_err());
    eprintln!("Step1 inherited FORMAT DACL refused; no ACL repair performed");
}

fn non_microsoft_reparse(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "non-microsoft-GUID-reparse");
    let guard = fixture.publish(native);
    native.write_new(&fixture.metadata.join("original-byte-witness"), BYTES);
    let path = fixture.root.join(".layrs/FORMAT");
    let writer = try_open(
        &path,
        FILE_WRITE_ATTRIBUTES | FILE_READ_ATTRIBUTES,
        SHARE_ALL,
    );
    let result = match writer {
        Err(error) => Err(("open attributes", error)),
        Ok(writer) => {
            // REPARSE_GUID_DATA_BUFFER, valid non-Microsoft/non-surrogate tag.
            // Header8 + GUID16 + bounded private payload; no privilege adjustment.
            let mut data = [0u8; 32];
            data[..4].copy_from_slice(&0x00000042u32.to_le_bytes());
            data[4..6].copy_from_slice(&8u16.to_le_bytes());
            data[8..24].copy_from_slice(&[
                0x61, 0x2e, 0x40, 0x7a, 0x1a, 0x03, 0x4f, 0x48, 0x92, 0x2d, 0x18, 0x61, 0x0a, 0xa3,
                0x47, 0x52,
            ]);
            data[24..].copy_from_slice(b"STEP1-RP");
            let mut returned = 0;
            if unsafe {
                DeviceIoControl(
                    writer.as_raw_handle(),
                    0x900a4,
                    data.as_ptr().cast(),
                    data.len() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                Err((
                    "FSCTL_SET_REPARSE_POINT non-Microsoft/GUID",
                    io::Error::last_os_error(),
                ))
            } else {
                Ok(())
            }
        }
    };
    match result {
        Ok(()) => {
            assert_eq!(kind(&cause(guard.revalidate())), Failure::NeedsRepair);
            eprintln!(
                "Step1 non-Microsoft/GUID mutation QUALIFIED: native success, actual guard refuses"
            );
        }
        Err((operation, error)) => {
            eprintln!(
                "Step1 non-Microsoft/GUID mutation UNQUALIFIED: {operation}, exact native {error:?}; denied mutation is not a detection or sharing-lock proof"
            );
            guard.revalidate().unwrap();
        }
    }
    assert_eq!(
        std::fs::read(fixture.metadata.join("original-byte-witness")).unwrap(),
        BYTES
    );
}

fn register(root: &Path, label: &str) -> i32 {
    let api = api::Api::load().unwrap();
    let provider: Vec<u16> = "Layrs disposable Step1 API qualification\0"
        .encode_utf16()
        .collect();
    let version = [b'1' as u16, 0];
    let blob = label.as_bytes();
    let registration = CF_SYNC_REGISTRATION {
        StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: provider.as_ptr(),
        ProviderVersion: version.as_ptr(),
        SyncRootIdentity: blob.as_ptr().cast(),
        SyncRootIdentityLength: blob.len() as u32,
        FileIdentity: blob.as_ptr().cast(),
        FileIdentityLength: blob.len() as u32,
        ProviderId: GUID::from_u128(0xc34d5080_5f37_4981_b726_614bec4cb46c),
    };
    let policies = CF_SYNC_POLICIES {
        StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY {
            Primary: CF_HYDRATION_POLICY_PROGRESSIVE,
            Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
        },
        Population: CF_POPULATION_POLICY {
            Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
            Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
        },
        InSync: CF_INSYNC_POLICY_TRACK_ALL,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    let root: Vec<u16> = root
        .to_str()
        .unwrap()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        (api.register)(
            root.as_ptr(),
            &registration,
            &policies,
            CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT,
        )
    }
}
fn registration_negatives(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "stage-registration-before-observation");
    let file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(fixture.stage.join("FORMAT"))
        .unwrap();
    file.sync_all().unwrap();
    drop(file);
    let preflight = ControlRootPreflight::open(&fixture.root).unwrap();
    native.write_new(
        &fixture.metadata.join("registration-intent"),
        b"test-only stage registration, retained\n",
    );
    let hr = register(&fixture.stage, "Step1 registered stage negative");
    eprintln!("Step1 actual stage register HRESULT=0x{:08x}", hr as u32);
    assert_eq!(hr, 0);
    let error = cause(observe_control_directory_stage(&preflight, &fixture.stage));
    assert!(
        matches!(error, Error::Windows { hresult: 0, .. }),
        "exact NOT_UNDER refusal required: {error:?}"
    );
    let calls = Cell::new(0);
    let error = cause(publish_control_directory_recoverable(
        &preflight,
        fixture.allocated.stage(),
        &fixture.allocated,
        BYTES,
        None,
        |_| {
            calls.set(calls.get() + 1);
            Ok(())
        },
        &NoControlDirectoryPublishFaults,
    ));
    assert_eq!(kind(&error), Failure::NeedsRepair);
    assert_eq!(calls.get(), 0);
    fn registered_cause(error: &Error) -> bool {
        match error {
            Error::ControlDirectory { cause, .. } => registered_cause(cause),
            Error::Windows {
                operation,
                hresult: 0,
            } => operation.contains("NOT_UNDER"),
            _ => false,
        }
    }
    assert!(
        registered_cause(&error),
        "must be registration refusal before short-byte checks: {error}"
    );

    for (name, point, final_control) in [
        ("stage-register-after-persist", Point::AfterPersist, false),
        (
            "final-control-register-before-proof",
            Point::BeforeProof,
            true,
        ),
    ] {
        let fixture = Fixture::new(native, run, name);
        let preflight = ControlRootPreflight::open(&fixture.root).unwrap();
        let injected = Cell::new(false);
        let faults = Fault(|current| {
            if current == point {
                assert!(!injected.replace(true));
                native.write_new(
                    &fixture.metadata.join("registration-intent"),
                    b"test-only nested registration with provider pins held\n",
                );
                let target = if final_control {
                    fixture.root.join(".layrs")
                } else {
                    fixture.stage.clone()
                };
                let hr = register(&target, name);
                eprintln!(
                    "Step1 {name} actual register with pins HRESULT=0x{:08x}",
                    hr as u32
                );
                assert_eq!(
                    hr, 0,
                    "requires established registration, never infer negative from a failed mutation"
                );
            }
            Ok(())
        });
        let error = cause(publish_control_directory_recoverable(
            &preflight,
            fixture.allocated.stage(),
            &fixture.allocated,
            BYTES,
            None,
            |expected| {
                native.write_new(&fixture.prepared_path(), &expected.encode());
                Ok(())
            },
            &faults,
        ));
        assert!(injected.get());
        assert_eq!(kind(&error), Failure::NeedsRepair);
        assert!(
            registered_cause(&error),
            "exact nested registration boundary required: {error}"
        );
        assert!(fixture.prepared_path().is_file());
        eprintln!(
            "Step1 {name}: {error}; registration/fixture retained, no guard/connect/hydration"
        );
    }

    let fixture = Fixture::new(native, run, "final-control-registration");
    let guard = fixture.publish(native);
    let expected = guard.identity().clone();
    drop(guard);
    native.write_new(
        &fixture.metadata.join("registration-intent"),
        b"test-only final control registration, retained\n",
    );
    let hr = register(
        &fixture.root.join(".layrs"),
        "Step1 registered final control negative",
    );
    eprintln!(
        "Step1 actual final .layrs register HRESULT=0x{:08x}",
        hr as u32
    );
    assert_eq!(hr, 0);
    match ControlDirectoryGuard::open(&fixture.root, &expected) {
        Err(error) => assert_eq!(kind(&error), Failure::NeedsRepair),
        Ok(guard) => assert_eq!(
            kind(&cause(guard.revalidate_bootstrap_root())),
            Failure::NeedsRepair
        ),
    }

    let fixture = Fixture::new(native, run, "root-register-before-final-proof");
    let preflight = ControlRootPreflight::open(&fixture.root).unwrap();
    let registered = Cell::new(false);
    let faults = Fault(|point| {
        if point == Point::BeforeProof {
            native.write_new(
                &fixture.metadata.join("registration-intent"),
                b"test-only last-boundary registration\n",
            );
            assert_eq!(register(&fixture.root, "Step1 before final proof"), 0);
            registered.set(true);
        }
        Ok(())
    });
    let error = cause(publish_control_directory_recoverable(
        &preflight,
        fixture.allocated.stage(),
        &fixture.allocated,
        BYTES,
        None,
        |expected| {
            native.write_new(&fixture.prepared_path(), &expected.encode());
            Ok(())
        },
        &faults,
    ));
    assert!(registered.get());
    assert_eq!(kind(&error), Failure::NeedsRepair);
    drop(preflight);
    let fresh = ControlDirectoryGuard::open(&fixture.root, &fixture.prepared()).unwrap();
    fresh.revalidate().unwrap();
    assert_eq!(
        kind(&cause(fresh.revalidate_bootstrap_root())),
        Failure::NeedsRepair
    );
    eprintln!(
        "Step1 nested stage/final registration refusals + final root registration boundary; registrations retained, no connect or hydration in these arms"
    );
}

struct Counters {
    api: api::Api,
    fetch_data: AtomicU64,
    fetch_names: AtomicU64,
    notifications: AtomicU64,
    malformed: AtomicU64,
    last_execute: AtomicI32,
}
static BAD_CALLBACK: AtomicU64 = AtomicU64::new(0);
unsafe fn callback(info: *const CF_CALLBACK_INFO) -> Option<(CF_CALLBACK_INFO, &'static Counters)> {
    if info.is_null() {
        BAD_CALLBACK.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    let size = unsafe { info.cast::<u32>().read_unaligned() };
    if !(size_of::<CF_CALLBACK_INFO>() as u32..=65536).contains(&size) {
        BAD_CALLBACK.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    let info = unsafe { info.read_unaligned() };
    if info.CallbackContext.is_null() {
        BAD_CALLBACK.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    // Leaked context/DLL deliberately outlive disconnect even on a failed test.
    Some((info, unsafe { &*info.CallbackContext.cast::<Counters>() }))
}
unsafe extern "system" fn notification(
    info: *const CF_CALLBACK_INFO,
    _: *const CF_CALLBACK_PARAMETERS,
) {
    if let Some((_, counters)) = unsafe { callback(info) } {
        counters.notifications.fetch_add(1, Ordering::SeqCst);
    }
}
unsafe fn refuse_fetch(
    info: *const CF_CALLBACK_INFO,
    parameters: *const CF_CALLBACK_PARAMETERS,
    names: bool,
) {
    let Some((info, counters)) = (unsafe { callback(info) }) else {
        return;
    };
    let mut operation = CF_OPERATION_INFO {
        StructSize: size_of::<CF_OPERATION_INFO>() as u32,
        Type: CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS,
        ConnectionKey: info.ConnectionKey,
        TransferKey: info.TransferKey,
        RequestKey: info.RequestKey,
        ..Default::default()
    };
    let mut output = CF_OPERATION_PARAMETERS::default();
    if names {
        counters.fetch_names.fetch_add(1, Ordering::SeqCst);
        output.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_4>()) as u32;
        output.Anonymous.TransferPlaceholders = CF_OPERATION_PARAMETERS_0_4 {
            CompletionStatus: STATUS_CLOUD_FILE_NOT_SUPPORTED,
            ..Default::default()
        };
    } else {
        counters.fetch_data.fetch_add(1, Ordering::SeqCst);
        let end = offset_of!(CF_CALLBACK_PARAMETERS, Anonymous)
            + offset_of!(CF_CALLBACK_PARAMETERS_0_1, RequiredLength)
            + size_of::<i64>();
        if parameters.is_null() || unsafe { parameters.cast::<u32>().read_unaligned() } < end as u32
        {
            counters.malformed.fetch_add(1, Ordering::SeqCst);
            return;
        }
        let offset = unsafe {
            std::ptr::addr_of!((*parameters).Anonymous.FetchData.RequiredFileOffset)
                .read_unaligned()
        };
        let length = unsafe {
            std::ptr::addr_of!((*parameters).Anonymous.FetchData.RequiredLength).read_unaligned()
        };
        operation.Type = CF_OPERATION_TYPE_TRANSFER_DATA;
        output.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32;
        output.Anonymous.TransferData = CF_OPERATION_PARAMETERS_0_0 {
            CompletionStatus: STATUS_CLOUD_FILE_NOT_SUPPORTED,
            Offset: offset,
            Length: length,
            ..Default::default()
        };
    }
    counters.last_execute.store(
        unsafe { (counters.api.execute)(&operation, &mut output) },
        Ordering::SeqCst,
    );
}
unsafe extern "system" fn fetch_data(
    info: *const CF_CALLBACK_INFO,
    parameters: *const CF_CALLBACK_PARAMETERS,
) {
    unsafe { refuse_fetch(info, parameters, false) };
}
unsafe extern "system" fn fetch_names(
    info: *const CF_CALLBACK_INFO,
    parameters: *const CF_CALLBACK_PARAMETERS,
) {
    unsafe { refuse_fetch(info, parameters, true) };
}

fn connected_metadata(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "connected-cloud-metadata-APIs");
    let guard = fixture.publish(native);
    let expected = guard.identity().clone();
    drop(guard);
    native.write_new(
        &fixture.metadata.join("registration-intent"),
        b"test-only connected metadata counters\n",
    );
    assert_eq!(
        register(&fixture.root, "Step1 metadata API callback counters"),
        0
    );
    let counters = Box::leak(Box::new(Counters {
        api: api::Api::load().unwrap(),
        fetch_data: AtomicU64::new(0),
        fetch_names: AtomicU64::new(0),
        notifications: AtomicU64::new(0),
        malformed: AtomicU64::new(0),
        last_execute: AtomicI32::new(0),
    }));
    let callbacks = [
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_DATA,
            Callback: Some(fetch_data),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS,
            Callback: Some(fetch_names),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_OPEN_COMPLETION,
            Callback: Some(notification),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_CLOSE_COMPLETION,
            Callback: Some(notification),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NONE,
            Callback: None,
        },
    ];
    let path: Vec<u16> = fixture
        .root
        .to_str()
        .unwrap()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut key = 0;
    let hr = unsafe {
        (counters.api.connect)(
            path.as_ptr(),
            callbacks.as_ptr(),
            (counters as *const Counters).cast(),
            CF_CONNECT_FLAG_BLOCK_SELF_IMPLICIT_HYDRATION,
            &mut key,
        )
    };
    assert_eq!(
        hr, 0,
        "connect metadata counter HRESULT=0x{:08x}",
        hr as u32
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let observed = discover_control_directory(&fixture.root).unwrap();
        assert!(observed.identity().matches_expected(&expected));
        let guard = ControlDirectoryGuard::open(&fixture.root, &expected).unwrap();
        assert_eq!(guard.format_bytes(), BYTES);
        guard.revalidate().unwrap();
        let clone = guard.try_clone().unwrap();
        drop(guard);
        clone.revalidate().unwrap();
        drop(clone);
        let fresh = ControlDirectoryGuard::open(&fixture.root, &expected).unwrap();
        fresh.revalidate().unwrap();
        assert_eq!(
            kind(&cause(fresh.revalidate_bootstrap_root())),
            Failure::NeedsRepair
        );
    }));
    let disconnect = unsafe { (counters.api.disconnect)(key) };
    let data = counters.fetch_data.load(Ordering::SeqCst);
    let names = counters.fetch_names.load(Ordering::SeqCst);
    eprintln!(
        "Step1 ACTUAL production metadata APIs on connected cloud root: FETCH_DATA={data}, FETCH_PLACEHOLDERS={names}, notifications={}, malformed={}, last_execute=0x{:08x}, disconnect=0x{:08x}; no user content source exists",
        counters.notifications.load(Ordering::SeqCst),
        counters.malformed.load(Ordering::SeqCst),
        counters.last_execute.load(Ordering::SeqCst) as u32,
        disconnect as u32
    );
    assert_eq!(disconnect, 0);
    assert_eq!(BAD_CALLBACK.load(Ordering::SeqCst), 0);
    assert_eq!(counters.malformed.load(Ordering::SeqCst), 0);
    assert_eq!((data, names), (0, 0));
    assert!(
        result.is_ok(),
        "metadata API assertion failed; native counters/fixture retained"
    );
}

pub(super) fn run_native() {
    let (run, _) = disposable_run("control-step1-native");
    let native = Native::load();
    let gates: [(&str, fn(&Native, &Path)); 8] = [
        ("clean/reopen/legacy", clean_and_reopen),
        ("refusals/replay", publication_refusals),
        ("single-attempt/collision", leaked_reader_and_collision),
        (
            "production close-window mutations",
            publication_window_mutations,
        ),
        ("metadata/privacy", metadata_mutations),
        (
            "non-Microsoft reparse actual outcome",
            non_microsoft_reparse,
        ),
        ("nested registration", registration_negatives),
        ("production cloud metadata counters", connected_metadata),
    ];
    let mut results = Vec::new();
    for (name, gate) in gates {
        let passed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gate(&native, &run))).is_ok();
        eprintln!("Step1 native gate {name}: {passed}");
        results.push(passed);
    }
    assert!(
        results.iter().all(|passed| *passed),
        "Step1 native gates={results:?}, fixtures retained at {}",
        run.display()
    );
    eprintln!(
        "Step1 native recipe passed; reparse mutation qualification is its separate logged actual outcome; no Store/source/readiness or power-loss claim"
    );
}

const CHILD_BASE: &str = "LAYRS_VFS_STEP1_CHILD_BASE";
const CHILD_MODE: &str = "LAYRS_VFS_STEP1_CHILD_MODE";
const CHILD_POINT: &str = "LAYRS_VFS_STEP1_CHILD_POINT";
const CHILD_EXIT: i32 = 117;
const PROCESS_POINTS: [(Point, &str); 8] = [
    (Point::StageValidated, "StageValidated"),
    (Point::AfterPersist, "AfterPersist"),
    (Point::PreClose, "PreClose"),
    (Point::PostClose, "PostClose"),
    (Point::PreRename, "PreRename"),
    (Point::PostRename, "PostRename"),
    (Point::PreStrongPin, "PreStrongPin"),
    (Point::BeforeProof, "BeforeProof"),
];

fn bounded_record(path: &Path, maximum: usize) -> Vec<u8> {
    let file = File::open(path).unwrap();
    let length = file.metadata().unwrap().len();
    assert!((1..=maximum as u64).contains(&length));
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes.len() as u64, length, "test receipt changed length");
    bytes
}

fn canonical_test_path(path: &Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap();
    let value = path.to_str().unwrap();
    let value = value.strip_prefix("\\\\?\\").unwrap_or(value);
    assert!(
        value.as_bytes().get(1) == Some(&b':'),
        "local test fixture required"
    );
    PathBuf::from(value)
}

/// Invoked only by the explicit parent recipe, never by normal test discovery.
/// The parent owns these disposable allocations and compares them with records;
/// this is not a production Store receipt authentication implementation.
pub(super) fn run_child() {
    let native = Native::load();
    let parent = canonical_test_path(&PathBuf::from(
        std::env::var_os("LAYRS_VFS_TEST_PARENT").expect("explicit parent"),
    ));
    let base = canonical_test_path(&PathBuf::from(
        std::env::var_os(CHILD_BASE).expect("parent-created fixture"),
    ));
    assert!(base != parent && base.starts_with(&parent));
    let metadata = base.join("external-test-receipts");
    let allocated = ControlDirectoryStageIdentity::decode(&bounded_record(
        &metadata.join("allocation.bin"),
        layrs_vfs::CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES,
    ))
    .unwrap();
    assert_eq!(allocated.root(), base.join("checkout"));
    assert_eq!(
        allocated.stage_parent(),
        base.join("ordinary-inherited-stage-parent")
    );
    assert_eq!(
        allocated.stage(),
        base.join("ordinary-inherited-stage-parent")
            .join("fresh-unique-owned-stage")
    );
    let prepared_path = metadata.join("prepared.bin");
    match std::env::var(CHILD_MODE).unwrap().as_str() {
        "publish" => {
            assert!(!prepared_path.exists());
            let point_name = std::env::var(CHILD_POINT).unwrap();
            let point = PROCESS_POINTS
                .iter()
                .find(|(_, name)| *name == point_name)
                .unwrap()
                .0;
            let preflight = ControlRootPreflight::open(allocated.root()).unwrap();
            let _guard = publish_control_directory_recoverable(
                &preflight,
                allocated.stage(),
                &allocated,
                BYTES,
                None,
                |expected| {
                    native.write_new(&prepared_path, &expected.encode());
                    Ok(())
                },
                &Fault(|current| {
                    if current == point {
                        eprintln!(
                            "Step1 process exit at {point:?}; fixture={}",
                            base.display()
                        );
                        std::io::stderr().flush().unwrap();
                        // A process exit releases ALL native pins. This proves
                        // process recovery, not a power-loss/directory-flush guarantee.
                        std::process::exit(CHILD_EXIT);
                    }
                    Ok(())
                }),
            )
            .unwrap();
            panic!("requested process exit point was not reached");
        }
        "resume" => {
            let prepared = match std::fs::symlink_metadata(&prepared_path) {
                Ok(_) => Some(
                    ControlDirectoryIdentity::decode(&bounded_record(
                        &prepared_path,
                        layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES,
                    ))
                    .unwrap(),
                ),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => panic!("test receipt observation failed: {error}"),
            };
            if let Some(expected) = &prepared {
                assert_eq!(expected.stage_identity(), &allocated);
            }
            let final_exists = match std::fs::symlink_metadata(allocated.root().join(".layrs")) {
                Ok(_) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => panic!("final path observation failed: {error}"),
            };
            let guard = if final_exists {
                // Deliberately no empty-root preflight and no historical stage
                // lookup: this is a fresh-process expected-open after rename.
                ControlDirectoryGuard::open(
                    allocated.root(),
                    prepared.as_ref().expect("final requires prepared record"),
                )
                .unwrap()
            } else {
                let preflight = ControlRootPreflight::open(allocated.root()).unwrap();
                publish_control_directory_recoverable(
                    &preflight,
                    allocated.stage(),
                    &allocated,
                    BYTES,
                    prepared.as_ref(),
                    |expected| {
                        assert!(prepared.is_none(), "expectedSome must not repersist");
                        native.write_new(&prepared_path, &expected.encode());
                        Ok(())
                    },
                    &NoControlDirectoryPublishFaults,
                )
                .unwrap()
            };
            guard.revalidate_bootstrap_root().unwrap();
            assert_eq!(guard.format_bytes(), BYTES);
            native.write_new(
                &metadata.join("resume-success.bin"),
                &guard.identity().encode(),
            );
            eprintln!(
                "Step1 FRESH process resume success; root={}, control={}, FORMAT={}; final_was_present={final_exists}",
                guard.root_filesystem_identity(),
                guard.directory_filesystem_identity(),
                guard.format_binding_identity()
            );
        }
        value => panic!("unrecognized test child mode {value}"),
    }
}

fn execute_child(native: &Native, fixture: &Fixture, mode: &str, point: &str, expected_exit: i32) {
    let stdout_path = fixture.metadata.join(format!("{mode}-stdout.log"));
    let stderr_path = fixture.metadata.join(format!("{mode}-stderr.log"));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "control_directory_step1_child",
            "--nocapture",
        ])
        .env(CHILD_BASE, &fixture.base)
        .env(CHILD_MODE, mode)
        .env(CHILD_POINT, point)
        .stdin(Stdio::null())
        .stdout(Stdio::from(native.file(&stdout_path)))
        .stderr(Stdio::from(native.file(&stderr_path)))
        .creation_flags(0x0800_0000)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            // This is only our explicitly spawned child, never another process.
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "Step1 child timeout; all fixture/logs retained at {}",
                fixture.base.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    eprintln!(
        "Step1 child {point}/{mode}: {status}; stdout={}, stderr={}",
        stdout_path.display(),
        stderr_path.display()
    );
    assert_eq!(
        status.code(),
        Some(expected_exit),
        "child failure; inspect retained logs"
    );
}

pub(super) fn run_process() {
    let (run, _) = disposable_run("control-step1-process");
    let native = Native::load();
    for (point, name) in PROCESS_POINTS {
        let fixture = Fixture::new(&native, &run, name);
        // Fixture::new returned only paths/data. Neither this process nor the
        // previous child retains a root/control/FORMAT handle during resume.
        execute_child(&native, &fixture, "publish", name, CHILD_EXIT);
        assert_eq!(
            fixture.prepared_path().exists(),
            point != Point::StageValidated
        );
        let prepared_before = fixture.prepared_path().exists().then(|| {
            bounded_record(
                &fixture.prepared_path(),
                layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES,
            )
        });
        execute_child(&native, &fixture, "resume", name, 0);
        if let Some(before) = prepared_before {
            assert_eq!(
                bounded_record(
                    &fixture.prepared_path(),
                    layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES
                ),
                before,
                "expectedSome receipt bytes must remain unchanged"
            );
        }
        let expected = fixture.prepared();
        let recovered = ControlDirectoryIdentity::decode(&bounded_record(
            &fixture.metadata.join("resume-success.bin"),
            layrs_vfs::CONTROL_DIRECTORY_MAX_ENCODED_BYTES,
        ))
        .unwrap();
        assert_eq!(recovered, expected);
        assert_eq!(expected.stage_identity(), &fixture.allocated);
        let guard = ControlDirectoryGuard::open(&fixture.root, &expected).unwrap();
        guard.revalidate_bootstrap_root().unwrap();
        assert_eq!(guard.format_bytes(), BYTES);
        assert_eq!(
            guard.directory_filesystem_identity(),
            fixture.allocated.directory_filesystem_identity()
        );
        assert_eq!(
            guard.format_binding_identity(),
            fixture.allocated.format_binding_identity()
        );
        assert!(!fixture.stage.exists());
        eprintln!(
            "Step1 process boundary {point:?} passed with all-old-pins-gone identity/bytes/bootstrap verification"
        );
    }
    eprintln!(
        "Step1 process recovery: 8 predetermined exits + fresh-process resumes; retained {}; no power-loss or Store readiness claim",
        run.display()
    );
}
