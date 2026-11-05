//! Opt-in Stage1b feasibility only. No production capability is exercised.
//! Root owns execution; all test roots and native evidence are retained.
//! Native references: FILE_RENAME_INFO / CfRegisterSyncRoot / CfConnectSyncRoot /
//! CfDisconnectSyncRoot on learn.microsoft.com. A refused strong-pin rename is
//! recorded as a failed design gate, never permission to relax production pins.
use super::{disposable_run, native_info};
use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    marker::PhantomData,
    mem::{offset_of, size_of, size_of_val},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    rc::Rc,
    sync::atomic::{AtomicI32, AtomicU64, Ordering},
};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::*,
        Storage::{CloudFilters::*, FileSystem::*},
        System::LibraryLoader::*,
    },
};

// Reuse the dynamically loaded native table, without adding public production
// surface or statically importing CldApi. All functions remain test-private.
#[path = "../../src/windows/api.rs"]
mod api;

const MARKER: &[u8] = b"Stage1b FORMAT-only native feasibility fixture\n";
const READY: &[u8] = b"layrs-stage1b-feasibility-ready-v1";
const PREPARED: &[u8] = b"layrs-stage1b-feasibility-prepared-v1";
static BAD_CALLBACKS: AtomicU64 = AtomicU64::new(0);

fn wide(path: &Path) -> Vec<u16> {
    // Every path comes from the freshly created, validated disposable tree.
    assert!(path.is_absolute());
    let value = path.to_str().unwrap().replace('/', "\\");
    assert!(!value.starts_with("\\\\"));
    format!("\\\\?\\{value}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

fn cf_path(path: &Path) -> Vec<u16> {
    // Match the existing provider's normal absolute CFAPI path spelling. The
    // extended native spelling above is only for SetFileInformationByHandle.
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn open(path: &Path, access: u32, sharing: u32) -> File {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(sharing)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap_or_else(|error| {
            panic!(
                "open {} access={access:x} sharing={sharing:x}: {error}",
                path.display()
            )
        })
}

fn id(file: &File) -> (u32, u64) {
    let info = native_info(file);
    (
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}

fn attrs(file: &File) -> FILE_ATTRIBUTE_TAG_INFO {
    let mut value = FILE_ATTRIBUTE_TAG_INFO::default();
    assert_ne!(
        unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileAttributeTagInfo,
                (&mut value as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of_val(&value) as u32,
            )
        },
        0,
        "attribute query: {}",
        std::io::Error::last_os_error()
    );
    value
}

fn plain(file: &File, directory: bool) {
    let mode = Mode::enter(2);
    let value = attrs(file);
    assert_eq!(value.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
    assert_eq!(value.ReparseTag, 0);
    assert_eq!(
        value.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        directory
    );
    if !directory {
        assert_eq!(native_info(file).nNumberOfLinks, 1);
    }
    mode.restore();
}

fn pins(path: &Path) -> Vec<File> {
    let mut held = Vec::new();
    for path in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let file = open(
            path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        );
        plain(&file, true);
        held.push(file);
    }
    held
}

fn format_pin(path: &Path) -> File {
    let file = open(path, FILE_READ_DATA | FILE_READ_ATTRIBUTES, FILE_SHARE_READ);
    plain(&file, false);
    file
}

fn exact_bytes(mut file: &File) {
    assert_eq!(file.metadata().unwrap().len(), MARKER.len() as u64);
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = vec![0; MARKER.len()];
    file.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, MARKER);
    assert_eq!(file.read(&mut [0u8; 1]).unwrap(), 0);
}

fn write_new(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    // No Store-style conflicting write or DELETE handle survives this function.
}

fn not_registered(api: &api::Api, file: &File) {
    let mut value = CF_SYNC_ROOT_BASIC_INFO::default();
    let hr = unsafe {
        (api.root_info)(
            file.as_raw_handle(),
            CF_SYNC_ROOT_INFO_BASIC,
            (&mut value as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
            size_of_val(&value) as u32,
            std::ptr::null_mut(),
        )
    };
    eprintln!("FRESH-ROOT PREFLIGHT: CfGetSyncRootInfoByHandle HRESULT=0x{:08x}; only exact ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT accepted", hr as u32);
    assert_eq!(
        hr,
        (0x80070000u32 | ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT) as i32,
        "fresh test target must be outside every sync root; HRESULT=0x{:08x}",
        hr as u32
    );
}

fn rename_no_replace(file: &File, destination: &Path) -> std::io::Result<()> {
    let name = wide(destination);
    let length = (offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2)
        .max(size_of::<FILE_RENAME_INFO>());
    let mut storage = vec![0u64; length.div_ceil(8)];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            storage
                .as_mut_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            length as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn write_delete_refused(path: &Path) {
    for access in [FILE_WRITE_DATA, DELETE] {
        let error = OpenOptions::new()
            .access_mode(access | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(32));
    }
}

fn rename_arm(run: &Path, api: &api::Api, strong: bool) -> bool {
    let label = if strong {
        "strong-format-pin"
    } else {
        "directory-only-control"
    };
    let arm = run.join(label);
    let root = arm.join("root");
    let stage = arm.join("unique-staged-control");
    std::fs::create_dir(&arm).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&stage).unwrap();
    write_new(&stage.join("FORMAT"), MARKER);
    let roots = pins(&root);
    let stage_parents = pins(&arm);
    not_registered(api, roots.last().unwrap());
    not_registered(api, stage_parents.last().unwrap());
    let directory = open(
        &stage,
        DELETE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    );
    plain(&directory, true);
    let before = id(&directory);
    let format = format_pin(&stage.join("FORMAT"));
    let format_before = id(&format);
    exact_bytes(&format);
    let format = if strong {
        Some(format)
    } else {
        drop(format);
        None
    };
    if strong {
        write_delete_refused(&stage.join("FORMAT"));
    }
    let result = rename_no_replace(&directory, &root.join(".layrs"));
    match &result {
        Ok(()) => eprintln!("G1 {label}: SAME-HANDLE NO-REPLACE RENAME SUCCESS"),
        Err(error) => eprintln!(
            "G1 {label}: REFUSED Win32={:?} HRESULT=0x{:08x} detail={error}; no retry/relaxation",
            error.raw_os_error(),
            0x80070000u32 | error.raw_os_error().unwrap_or(0) as u32
        ),
    }
    assert_eq!(id(&directory), before);
    let actual = if result.is_ok() {
        assert!(!stage.exists());
        root.join(".layrs")
    } else {
        assert!(!root.join(".layrs").exists());
        stage.clone()
    };
    // The original directory handle owns DELETE, so this observer must share
    // DELETE. It is only a proof read, never the proposed retained capability.
    let observed = open(
        &actual,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    );
    assert_eq!(id(&observed), before);
    let reopened_format = format_pin(&actual.join("FORMAT"));
    assert_eq!(id(&reopened_format), format_before);
    if let Some(format) = &format {
        assert_eq!(id(format), format_before);
        exact_bytes(format);
    }
    exact_bytes(&reopened_format);
    eprintln!("G2 {label}: directory={before:?}, FORMAT={format_before:?}, exact same native IDs/bytes at {}; fixtures retained", actual.display());
    result.is_ok()
}

fn collision_arm(run: &Path) {
    let arm = run.join("no-replace-collision");
    let stage = arm.join("unique-staged-control");
    let root = arm.join("root");
    std::fs::create_dir(&arm).unwrap();
    std::fs::create_dir(&stage).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join(".layrs")).unwrap();
    write_new(&stage.join("FORMAT"), MARKER);
    write_new(&root.join(".layrs/FORMAT"), b"foreign collision bytes\n");
    let _parents = pins(&root);
    let target = open(
        &root.join(".layrs"),
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    );
    let target_id = id(&target);
    drop(target);
    let stage_file = open(
        &stage,
        DELETE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    );
    let stage_id = id(&stage_file);
    let error = rename_no_replace(&stage_file, &root.join(".layrs")).unwrap_err();
    let target = open(
        &root.join(".layrs"),
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    );
    assert_eq!(id(&target), target_id);
    assert_eq!(id(&stage_file), stage_id);
    assert_eq!(
        std::fs::read(root.join(".layrs/FORMAT")).unwrap(),
        b"foreign collision bytes\n"
    );
    assert_eq!(std::fs::read(stage.join("FORMAT")).unwrap(), MARKER);
    eprintln!(
        "G1 NO-REPLACE COLLISION: refused Win32={:?}; both identities/bytes retained",
        error.raw_os_error()
    );
}

type SetMode = unsafe extern "system" fn(i8) -> i8;
type QueryMode = unsafe extern "system" fn() -> i8;
struct Mode {
    module: HMODULE,
    set: SetMode,
    query: QueryMode,
    process: QueryMode,
    previous: i8,
    process_before: i8,
    active: bool,
    _same_thread: PhantomData<Rc<()>>,
}
impl Mode {
    fn enter(value: i8) -> Self {
        let name: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        let module = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        assert!(!module.is_null());
        let set = unsafe {
            GetProcAddress(
                module,
                c"RtlSetThreadPlaceholderCompatibilityMode".as_ptr().cast(),
            )
        }
        .unwrap();
        let query = unsafe {
            GetProcAddress(
                module,
                c"RtlQueryThreadPlaceholderCompatibilityMode"
                    .as_ptr()
                    .cast(),
            )
        }
        .unwrap();
        let process = unsafe {
            GetProcAddress(
                module,
                c"RtlQueryProcessPlaceholderCompatibilityMode"
                    .as_ptr()
                    .cast(),
            )
        }
        .unwrap();
        let set =
            unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, SetMode>(set) };
        let query = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryMode>(query)
        };
        let process = unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, QueryMode>(process)
        };
        let process_before = unsafe { process() };
        let previous = unsafe { set(value) };
        assert!(previous >= 0);
        assert_eq!(unsafe { query() }, value);
        Self {
            module,
            set,
            query,
            process,
            previous,
            process_before,
            active: true,
            _same_thread: PhantomData,
        }
    }
    fn restore(mut self) {
        assert!(unsafe { (self.set)(self.previous) } >= 0);
        self.active = false;
        assert_eq!(unsafe { (self.query)() }, self.previous);
        assert_eq!(unsafe { (self.process)() }, self.process_before);
    }
}
impl Drop for Mode {
    fn drop(&mut self) {
        if self.active {
            unsafe { (self.set)(self.previous) };
        }
        unsafe { FreeLibrary(self.module) };
    }
}

struct Observer {
    api: api::Api,
    directory: u64,
    format: u64,
    callbacks: AtomicU64,
    control_callbacks: AtomicU64,
    fetches: AtomicU64,
    malformed: AtomicU64,
    last_execute: AtomicI32,
}

unsafe fn callback_info(
    info: *const CF_CALLBACK_INFO,
) -> Option<(CF_CALLBACK_INFO, &'static Observer)> {
    if info.is_null() {
        BAD_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    // Read the guaranteed prefix first. Do not make a full Rust reference to a
    // native structure before validating its size.
    let size = unsafe { info.cast::<u32>().read_unaligned() };
    if !(size_of::<CF_CALLBACK_INFO>() as u32..=65_536).contains(&size) {
        BAD_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    let info = unsafe { info.read_unaligned() };
    if info.CallbackContext.is_null() {
        BAD_CALLBACKS.fetch_add(1, Ordering::SeqCst);
        return None;
    }
    // The test deliberately retains this one Box and DLL until process exit,
    // even on panic/disconnect failure; no callback can outlive its context.
    let observer = unsafe { &*info.CallbackContext.cast::<Observer>() };
    observer.callbacks.fetch_add(1, Ordering::SeqCst);
    if info.FileId as u64 == observer.directory || info.FileId as u64 == observer.format {
        observer.control_callbacks.fetch_add(1, Ordering::SeqCst);
    }
    Some((info, observer))
}

unsafe extern "system" fn notified(
    info: *const CF_CALLBACK_INFO,
    _: *const CF_CALLBACK_PARAMETERS,
) {
    let _ = unsafe { callback_info(info) };
}

unsafe fn refuse(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
    placeholders: bool,
) {
    let Some((info, observer)) = (unsafe { callback_info(info) }) else {
        return;
    };
    observer.fetches.fetch_add(1, Ordering::SeqCst);
    let mut operation = CF_OPERATION_INFO {
        StructSize: size_of::<CF_OPERATION_INFO>() as u32,
        Type: CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS,
        ConnectionKey: info.ConnectionKey,
        TransferKey: info.TransferKey,
        RequestKey: info.RequestKey,
        ..Default::default()
    };
    let mut parameters = CF_OPERATION_PARAMETERS::default();
    if placeholders {
        parameters.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_4>()) as u32;
        parameters.Anonymous.TransferPlaceholders = CF_OPERATION_PARAMETERS_0_4 {
            CompletionStatus: STATUS_CLOUD_FILE_NOT_SUPPORTED,
            ..Default::default()
        };
    } else {
        // FetchData's required fields are copied only after the native size
        // covers both, without reading the optional tail.
        let end = offset_of!(CF_CALLBACK_PARAMETERS, Anonymous)
            + offset_of!(CF_CALLBACK_PARAMETERS_0_1, RequiredLength)
            + size_of::<i64>();
        if params.is_null() || unsafe { params.cast::<u32>().read_unaligned() } < end as u32 {
            observer.malformed.fetch_add(1, Ordering::SeqCst);
            return;
        }
        let offset = unsafe {
            std::ptr::addr_of!((*params).Anonymous.FetchData.RequiredFileOffset).read_unaligned()
        };
        let length = unsafe {
            std::ptr::addr_of!((*params).Anonymous.FetchData.RequiredLength).read_unaligned()
        };
        operation.Type = CF_OPERATION_TYPE_TRANSFER_DATA;
        parameters.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32;
        parameters.Anonymous.TransferData = CF_OPERATION_PARAMETERS_0_0 {
            CompletionStatus: STATUS_CLOUD_FILE_NOT_SUPPORTED,
            Offset: offset,
            Length: length,
            ..Default::default()
        };
    }
    observer.last_execute.store(
        unsafe { (observer.api.execute)(&operation, &mut parameters) },
        Ordering::SeqCst,
    );
}

unsafe extern "system" fn fetch(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    unsafe { refuse(info, params, false) };
}
unsafe extern "system" fn populate(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    unsafe { refuse(info, params, true) };
}

fn register(api: &api::Api, root: &Path, blob: &[u8], update: bool) -> i32 {
    let provider: Vec<u16> = "Layrs disposable Stage1b feasibility\0"
        .encode_utf16()
        .collect();
    let version = [b'1' as u16, 0];
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
    let flags = CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT
        | if update {
            CF_REGISTER_FLAG_UPDATE
        } else {
            CF_REGISTER_FLAG_NONE
        };
    unsafe { (api.register)(cf_path(root).as_ptr(), &registration, &policies, flags) }
}

fn root_blob(api: &api::Api, root: &File, expected: &[u8]) {
    let mut storage = [0u64; 1024];
    let mut returned = 0;
    let hr = unsafe {
        (api.root_info)(
            root.as_raw_handle(),
            CF_SYNC_ROOT_INFO_STANDARD,
            storage.as_mut_ptr().cast(),
            size_of_val(&storage) as u32,
            &mut returned,
        )
    };
    assert!(hr >= 0, "root info HRESULT=0x{:08x}", hr as u32);
    assert!(
        returned as usize >= size_of::<CF_SYNC_ROOT_STANDARD_INFO>()
            && returned as usize <= size_of_val(&storage)
    );
    let info = unsafe { storage.as_ptr().cast::<CF_SYNC_ROOT_STANDARD_INFO>().read() };
    let start = offset_of!(CF_SYNC_ROOT_STANDARD_INFO, SyncRootIdentity);
    let length = info.SyncRootIdentityLength as usize;
    assert!(length <= 4096 && start + length <= returned as usize);
    let blob =
        unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>().add(start), length) };
    assert_eq!(blob, expected);
    assert_eq!(info.SyncRootFileId as u64, id(root).1);
    assert_eq!(
        info.HydrationPolicy.Primary,
        CF_HYDRATION_POLICY_PROGRESSIVE
    );
    assert_eq!(
        info.PopulationPolicy.Primary,
        CF_POPULATION_POLICY_ALWAYS_FULL
    );
    assert_eq!(info.HardLinkPolicy, CF_HARDLINK_POLICY_NONE);
}

fn registration_arm(run: &Path) -> bool {
    let root = run.join("pinned-registration-root");
    std::fs::create_dir(&root).unwrap();
    let roots = pins(&root);
    let api = api::Api::load().unwrap();
    not_registered(&api, roots.last().unwrap());
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    std::fs::create_dir(root.join(".layrs")).unwrap();
    write_new(&root.join(".layrs/FORMAT"), MARKER);
    let control = open(
        &root.join(".layrs"),
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    );
    let format = format_pin(&root.join(".layrs/FORMAT"));
    plain(&control, true);
    let ids = (id(roots.last().unwrap()), id(&control), id(&format));
    write_new(&run.join("native-registration-intent.txt"),
        format!("TEST ONLY no product ticket\nroot={}\nids={ids:?}\nprepared={PREPARED:?}\nready={READY:?}\nPROGRESSIVE ALWAYS_FULL no hardlinks\n", root.display()).as_bytes());
    for (update, blob) in [(false, PREPARED), (true, READY)] {
        let hr = register(&api, &root, blob, update);
        eprintln!("G3 REGISTER update={update}: HRESULT=0x{:08x}, root/control/strong FORMAT pins all retained", hr as u32);
        if hr < 0 {
            return false;
        }
        root_blob(&api, roots.last().unwrap(), blob);
        assert_eq!((id(roots.last().unwrap()), id(&control), id(&format)), ids);
        plain(&control, true);
        plain(&format, false);
        exact_bytes(&format);
    }
    write_delete_refused(&root.join(".layrs/FORMAT"));
    let observer = Box::leak(Box::new(Observer {
        api,
        directory: ids.1 .1,
        format: ids.2 .1,
        callbacks: AtomicU64::new(0),
        control_callbacks: AtomicU64::new(0),
        fetches: AtomicU64::new(0),
        malformed: AtomicU64::new(0),
        last_execute: AtomicI32::new(0),
    }));
    let callbacks = [
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_DATA,
            Callback: Some(fetch),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS,
            Callback: Some(populate),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_OPEN_COMPLETION,
            Callback: Some(notified),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_CLOSE_COMPLETION,
            Callback: Some(notified),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NONE,
            Callback: None,
        },
    ];
    let mut key = 0;
    let hr = unsafe {
        (observer.api.connect)(
            cf_path(&root).as_ptr(),
            callbacks.as_ptr(),
            (observer as *const Observer).cast::<c_void>(),
            CF_CONNECT_FLAG_BLOCK_SELF_IMPLICIT_HYDRATION,
            &mut key,
        )
    };
    eprintln!("G4 CONNECT: HRESULT=0x{:08x}", hr as u32);
    if hr < 0 {
        return false;
    }
    // Ordinary/default/disguise/exposed handle observations. These reproduce
    // native Store-style no-follow access; they are NOT actual Store API tests.
    for mode in [0, 1, 2] {
        let scope = Mode::enter(mode);
        let names = std::fs::read_dir(root.join(".layrs"))
            .unwrap()
            .map(|item| item.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, [std::ffi::OsString::from("FORMAT")]);
        let directory = open(
            &root.join(".layrs"),
            FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        );
        let file = open(
            &root.join(".layrs/FORMAT"),
            FILE_READ_DATA | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ,
        );
        let d = attrs(&directory);
        let f = attrs(&file);
        let r = attrs(roots.last().unwrap());
        assert_eq!(d.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
        assert_eq!(f.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
        assert_eq!((d.ReparseTag, f.ReparseTag), (0, 0));
        if mode == 2 {
            assert_ne!(r.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
            assert_eq!(r.ReparseTag & 0xffff0fff, 0x9000001a);
        }
        assert_eq!((id(&directory), id(&file)), (ids.1, ids.2));
        exact_bytes(&file);
        eprintln!("G5 mode={mode}: root attrs={:08x}/tag={:08x}; control={:08x}/{:08x}, FORMAT={:08x}/{:08x}; exact IDs/bytes; ordinary control has no placeholder tag",
            r.FileAttributes, r.ReparseTag, d.FileAttributes, d.ReparseTag, f.FileAttributes, f.ReparseTag);
        scope.restore();
    }
    let hr = unsafe { (observer.api.disconnect)(key) };
    eprintln!("G4 DISCONNECT/DRAIN: HRESULT=0x{:08x}", hr as u32);
    assert!(
        hr >= 0,
        "native context intentionally retained even on failure"
    );
    // CfDisconnectSyncRoot guarantees no further callbacks on successful return.
    let counts = (
        observer.callbacks.load(Ordering::SeqCst),
        observer.control_callbacks.load(Ordering::SeqCst),
        observer.fetches.load(Ordering::SeqCst),
        observer.malformed.load(Ordering::SeqCst),
    );
    eprintln!("G4 callbacks total/control/fetch/malformed={counts:?}; bad_headers={}; last_refusal_execute_HRESULT=0x{:08x}; source reads/transfers=0 (no source adapter installed)", BAD_CALLBACKS.load(Ordering::SeqCst), observer.last_execute.load(Ordering::SeqCst) as u32);
    assert_eq!(
        counts.1, 0,
        "ordinary control unexpectedly produced callbacks"
    );
    assert_eq!(
        counts.2, 0,
        "ordinary controls unexpectedly requested hydration/population"
    );
    assert_eq!(counts.3, 0);
    assert_eq!(BAD_CALLBACKS.load(Ordering::SeqCst), 0);
    assert_eq!((id(roots.last().unwrap()), id(&control), id(&format)), ids);
    plain(&control, true);
    plain(&format, false);
    exact_bytes(&format);
    eprintln!("G3/G4/G5: native registration/update and ordinary control evidence verified. Actual Store API admission remains a separate unqualified integration gate.");
    true
}

pub(super) fn run() {
    let (run, _) = disposable_run("control-directory-feasibility");
    let api = api::Api::load().unwrap();
    let strong = rename_arm(&run, &api, true);
    let directory_only = rename_arm(&run, &api, false);
    collision_arm(&run);
    let registration = registration_arm(&run);
    eprintln!("STAGE1B FEASIBILITY strong_format_pin_rename={strong} directory_only_control={directory_only} pinned_registration={registration}; all fixtures retained at {}; no production guarantee inferred", run.display());
    assert!(strong && directory_only && registration,
        "Stage1b native prerequisite failed; retain exact evidence and review handoff design before production relaxation");
}
