//! Test-only owned allocation. No production receipt authentication or cleanup.
use super::{fs, io};
use crate::*;
use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io::Write,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::*, Security::*, Storage::FileSystem::*, System::LibraryLoader::*,
};

pub(super) const FORMAT: &[u8] = b"{\"fixture\":\"allocated control observation\"}\n";
struct Library(HMODULE);
impl Library {
    fn load() -> Self {
        let name: Vec<u16> = "advapi32.dll\0".encode_utf16().collect();
        let module = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        assert!(!module.is_null(), "{}", std::io::Error::last_os_error());
        Self(module)
    }
    unsafe fn symbol<T: Copy>(&self, name: &'static [u8]) -> T {
        let pointer = unsafe { GetProcAddress(self.0, name.as_ptr()) }.unwrap();
        assert_eq!(size_of::<T>(), size_of_val(&pointer));
        unsafe { std::mem::transmute_copy(&pointer) }
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
        unsafe { LocalFree(self.0) };
    }
}
fn descriptor(library: &Library) -> Allocation {
    type Token = unsafe extern "system" fn(HANDLE, u32, *mut HANDLE) -> i32;
    type SidText = unsafe extern "system" fn(PSID, *mut *mut u16) -> i32;
    type Parse =
        unsafe extern "system" fn(*const u16, u32, *mut PSECURITY_DESCRIPTOR, *mut u32) -> i32;
    let token_fn: Token = unsafe { library.symbol(b"OpenProcessToken\0") };
    let sid_text: SidText = unsafe { library.symbol(b"ConvertSidToStringSidW\0") };
    let parse: Parse =
        unsafe { library.symbol(b"ConvertStringSecurityDescriptorToSecurityDescriptorW\0") };
    let mut handle = std::ptr::null_mut();
    // GetCurrentProcess's documented pseudo-handle avoids new dependency features.
    assert_ne!(
        unsafe { token_fn(-1isize as HANDLE, TOKEN_QUERY, &mut handle) },
        0
    );
    let token = unsafe { OwnedHandle::from_raw_handle(handle) };
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
    assert_ne!(unsafe { sid_text(sid, &mut text) }, 0);
    let _text = Allocation(text.cast());
    let mut count = 0;
    while unsafe { *text.add(count) } != 0 {
        count += 1;
        assert!(count <= 256);
    }
    let owner = String::from_utf16(unsafe { std::slice::from_raw_parts(text, count) }).unwrap();
    let sddl: Vec<_> = format!("O:{owner}D:P(A;OICI;FA;;;{owner})\0")
        .encode_utf16()
        .collect();
    let mut sd = std::ptr::null_mut();
    assert_ne!(
        unsafe { parse(sddl.as_ptr(), 1, &mut sd, std::ptr::null_mut()) },
        0
    );
    Allocation(sd)
}
pub(super) fn private_directory(path: &Path) {
    let library = Library::load();
    let sd = descriptor(&library);
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    assert_ne!(
        unsafe { CreateDirectoryW(fs::native_path(path).unwrap().as_ptr(), &attrs) },
        0,
        "create private directory: {}",
        std::io::Error::last_os_error()
    );
}
pub(super) fn private_file(path: &Path) -> File {
    let library = Library::load();
    let sd = descriptor(&library);
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateFileW(
            fs::native_path(path).unwrap().as_ptr(),
            FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | 0x20000,
            io::ALL_SHARING,
            &attrs,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        handle,
        INVALID_HANDLE_VALUE,
        "create private FORMAT: {}",
        std::io::Error::last_os_error()
    );
    unsafe { File::from_raw_handle(handle) }
}

pub(super) fn write_new(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
pub(super) fn run(label: &str) -> PathBuf {
    let parent = std::env::var_os("LAYRS_VFS_TEST_PARENT")
        .expect("explicit disposable LAYRS_VFS_TEST_PARENT required");
    let parent = fs::RootGuard::open(Path::new(&parent), false).unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = parent.root.join(format!(
        "control-observation-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&path).unwrap();
    eprintln!("RETAINED observation fixture: {}", path.display());
    path
}
pub(super) struct Fixture {
    pub base: PathBuf,
    pub root: PathBuf,
    pub stage: PathBuf,
    pub preflight: ControlRootPreflight,
    pub allocated: ControlDirectoryStageIdentity,
}
impl Fixture {
    pub fn new(run: &Path, label: &str, bytes: &[u8]) -> Self {
        let base = run.join(label);
        std::fs::create_dir(&base).unwrap();
        let root = base.join("checkout");
        std::fs::create_dir(&root).unwrap();
        let parent = base.join("allocation-parent");
        std::fs::create_dir(&parent).unwrap(); // deliberately ordinary inherited parent
        let stage = parent.join("unique-stage");
        private_directory(&stage);
        let preflight = ControlRootPreflight::open(&root).unwrap();
        let mut file = private_file(&stage.join("FORMAT"));
        let allocated = observe_control_directory_stage(&preflight, &stage).unwrap();
        assert_eq!(
            allocated.root_filesystem_identity(),
            preflight.filesystem_identity()
        );
        let actual = io::id(&file).unwrap();
        assert_eq!(
            allocated.format_binding_identity(),
            format!("{}:{}:{}", actual.0, actual.1 >> 32, actual.1 as u32)
        );
        write_new(&base.join("allocation.bin"), &allocated.encode());
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        drop(file);
        Self {
            base,
            root: preflight.root().to_path_buf(),
            stage: allocated.stage().to_path_buf(),
            preflight,
            allocated,
        }
    }
    pub fn persist(&self, prepared: &ControlDirectoryIdentity) -> Result<()> {
        write_new(&self.base.join("prepared.bin"), &prepared.encode());
        Ok(())
    }
    pub fn prepared(&self) -> ControlDirectoryIdentity {
        ControlDirectoryIdentity::decode(&std::fs::read(self.base.join("prepared.bin")).unwrap())
            .unwrap()
    }
    pub fn publish(&self) -> ControlDirectoryGuard {
        publish_control_directory_recoverable(
            &self.preflight,
            self.allocated.stage(),
            &self.allocated,
            FORMAT,
            None,
            |prepared| self.persist(prepared),
            &NoControlDirectoryPublishFaults,
        )
        .unwrap()
    }
}
pub(super) fn error<T>(value: Result<T>) -> Error {
    match value {
        Err(error) => error,
        Ok(_) => panic!("unexpected success"),
    }
}
pub(super) fn raw(error: &Error) -> Option<i32> {
    match error {
        Error::Io(error) => error.raw_os_error(),
        Error::Windows { hresult, .. } if (*hresult as u32 & 0xffff0000) == 0x80070000 => {
            Some((*hresult as u32 & 0xffff) as i32)
        }
        Error::ControlDirectory { cause, .. } => raw(cause),
        _ => None,
    }
}
pub(super) fn sharing<T>(value: Result<T>) {
    let error = error(value);
    assert_eq!(raw(&error), Some(32), "{error:?}");
}
pub(super) fn unchanged_stage(f: &Fixture, bytes: &[u8]) {
    assert_eq!(
        io::id(&io::directory_observer(&f.stage).unwrap()).unwrap(),
        (f.allocated.volume, f.allocated.directory_index)
    );
    let file = fs::open(
        &f.stage.join("FORMAT"),
        io::ALL_SHARING,
        FILE_READ_ATTRIBUTES,
    )
    .unwrap();
    assert_eq!(
        io::id(&file).unwrap(),
        (f.allocated.volume, f.allocated.format_index)
    );
    assert_eq!(std::fs::read(f.stage.join("FORMAT")).unwrap(), bytes);
    assert!(std::fs::read_dir(&f.root).unwrap().next().is_none());
    assert!(
        matches!(fs::open(&f.root.join(".layrs"), io::ALL_SHARING, FILE_READ_ATTRIBUTES), Err(Error::Io(e)) if e.raw_os_error() == Some(2))
    );
}
pub(super) struct Fault<F: Fn(ControlDirectoryPublishPoint) -> std::io::Result<()>>(pub F);
impl<F: Fn(ControlDirectoryPublishPoint) -> std::io::Result<()>> ControlDirectoryPublishFaults
    for Fault<F>
{
    fn at(&self, point: ControlDirectoryPublishPoint) -> std::io::Result<()> {
        (self.0)(point)
    }
}
pub(super) fn unprotect(path: &Path) {
    type Get = unsafe extern "system" fn(
        HANDLE,
        i32,
        u32,
        *mut PSID,
        *mut PSID,
        *mut *mut ACL,
        *mut *mut ACL,
        *mut PSECURITY_DESCRIPTOR,
    ) -> u32;
    type Set =
        unsafe extern "system" fn(HANDLE, i32, u32, PSID, PSID, *const ACL, *const ACL) -> u32;
    let library = Library::load();
    let get: Get = unsafe { library.symbol(b"GetSecurityInfo\0") };
    let set: Set = unsafe { library.symbol(b"SetSecurityInfo\0") };
    let file = fs::open(
        path,
        io::ALL_SHARING,
        0x40000 | 0x20000 | FILE_READ_ATTRIBUTES,
    )
    .unwrap();
    let mut acl = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            get(
                fs::handle(&file),
                1,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        },
        0
    );
    let _descriptor = Allocation(descriptor);
    assert_eq!(
        unsafe {
            set(
                fs::handle(&file),
                1,
                DACL_SECURITY_INFORMATION | UNPROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null(),
            )
        },
        0
    );
}
pub(super) fn guid_reparse(path: &Path) {
    let file = fs::open(
        path,
        io::ALL_SHARING,
        FILE_WRITE_ATTRIBUTES | FILE_READ_ATTRIBUTES,
    )
    .unwrap();
    let before = io::id(&file).unwrap();
    let mut buffer = [0u8; 32];
    buffer[..4].copy_from_slice(&0x42u32.to_le_bytes());
    buffer[4..6].copy_from_slice(&8u16.to_le_bytes());
    buffer[8..24].copy_from_slice(&[
        0x62, 0x2e, 0x40, 0x7a, 0x1a, 0x03, 0x4f, 0x48, 0x92, 0x2d, 0x18, 0x61, 0x0a, 0xa3, 0x47,
        0x52,
    ]);
    buffer[24..].copy_from_slice(b"OBSERVRP");
    let mut returned = 0;
    let ok = unsafe {
        windows_sys::Win32::System::IO::DeviceIoControl(
            fs::handle(&file),
            0x900a4,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        ok,
        0,
        "UNQUALIFIED GUID reparse mutation: {:?}",
        std::io::Error::last_os_error()
    );
    assert_eq!(io::id(&file).unwrap(), before);
    assert_eq!(fs::attributes(&file).unwrap().ReparseTag, 0x42);
}
