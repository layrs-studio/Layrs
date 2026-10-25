//! Test-only owned allocation. No production receipt authentication or cleanup.
use super::{api, control_directory_io as io, fs};
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

pub(super) const FORMAT: &[u8] = b"{\"fixture\":\"opaque Stage2 immutable FORMAT\"}\n";
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
fn private_directory(path: &Path) {
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
fn private_file(path: &Path) -> File {
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
    let path = parent
        .root
        .join(format!("stage2-{label}-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    eprintln!("RETAINED Stage2 fixture: {}", path.display());
    path
}
pub(super) struct Fixture {
    pub base: PathBuf,
    pub root: PathBuf,
    pub root_id: [u8; 16],
}
impl Fixture {
    pub fn create(run: &Path, label: &str) -> (Self, ControlDirectoryGuard) {
        Self::create_with_root_spelling(run, label, "checkout", "checkout")
    }
    pub fn create_case_alias(run: &Path, label: &str) -> (Self, ControlDirectoryGuard, PathBuf) {
        let (fixture, control) =
            Self::create_with_root_spelling(run, label, "CheckOutMiXeD", "cHECKoUTmIxEd");
        let requested = fixture.base.join("cHECKoUTmIxEd");
        (fixture, control, requested)
    }
    fn create_with_root_spelling(
        run: &Path,
        label: &str,
        actual_leaf: &str,
        requested_leaf: &str,
    ) -> (Self, ControlDirectoryGuard) {
        let base = run.join(label);
        std::fs::create_dir(&base).unwrap();
        let root = base.join(actual_leaf);
        std::fs::create_dir(&root).unwrap();
        let stage = base.join("unique-private-stage");
        private_directory(&stage);
        let requested = base.join(requested_leaf);
        let preflight = ControlRootPreflight::open(&requested).unwrap();
        if actual_leaf != requested_leaf {
            assert_ne!(root, requested, "case arm must supply a different spelling");
            assert_ne!(preflight.root(), requested);
            assert_eq!(preflight.root().file_name(), root.file_name());
            let (path, held, identity) = preflight.root.held_paths().last().unwrap();
            assert_eq!(path, requested);
            assert_eq!(io::id(held).unwrap(), identity);
            assert_eq!(identity, preflight.root.identity());
            let actual = io::directory_observer(&root).unwrap();
            assert_eq!(io::id(&actual).unwrap(), identity);
            eprintln!(
                "CASE preflight accepted requested={}, canonical={}, actual_native_id={identity:?}",
                requested.display(),
                preflight.root().display()
            );
        }
        let mut format = private_file(&stage.join("FORMAT"));
        let allocated = observe_control_directory_stage(&preflight, &stage).unwrap();
        assert_eq!(
            allocated.root_filesystem_identity(),
            preflight.filesystem_identity()
        );
        assert_eq!(
            allocated.directory_filesystem_identity(),
            crate::control_directory::filesystem_identity(
                io::id(&io::directory_observer(&stage).unwrap()).unwrap().0,
                io::id(&io::directory_observer(&stage).unwrap()).unwrap().1
            )
        );
        let id = io::id(&format).unwrap();
        assert_eq!(
            allocated.format_binding_identity(),
            format!("{}:{}:{}", id.0, id.1 >> 32, id.1 as u32)
        );
        write_new(&base.join("allocated.bin"), &allocated.encode());
        format.write_all(FORMAT).unwrap();
        format.sync_all().unwrap();
        drop(format);
        let guard = publish_control_directory_recoverable(
            &preflight,
            allocated.stage(),
            &allocated,
            FORMAT,
            None,
            |expected| {
                write_new(&base.join("control.bin"), &expected.encode());
                Ok(())
            },
            &NoControlDirectoryPublishFaults,
        )
        .unwrap();
        let root = guard.root().to_path_buf();
        let root_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_le_bytes();
        write_new(&base.join("root-id.bin"), &root_id);
        (
            Self {
                base,
                root,
                root_id,
            },
            guard,
        )
    }
    pub fn reopen(base: PathBuf) -> (Self, ControlDirectoryGuard) {
        let expected =
            ControlDirectoryIdentity::decode(&std::fs::read(base.join("control.bin")).unwrap())
                .unwrap();
        let root = expected.stage_identity().root().to_path_buf();
        let root_id = std::fs::read(base.join("root-id.bin"))
            .unwrap()
            .try_into()
            .unwrap();
        let control = ControlDirectoryGuard::open(&root, &expected).unwrap();
        (
            Self {
                base,
                root,
                root_id,
            },
            control,
        )
    }
    pub fn ticket(&self) -> Registration {
        Registration::decode_control_directory(
            &std::fs::read(self.base.join("ticket.bin")).unwrap(),
        )
        .unwrap()
    }
    pub fn persist(&self, ticket: &Registration) -> Result<()> {
        write_new(&self.base.join("ticket.bin"), &ticket.encode());
        Ok(())
    }
}
pub(super) fn raw32(error: Error) {
    match error {
        Error::Io(error) => assert_eq!(error.raw_os_error(), Some(32), "{error:?}"),
        Error::Windows { hresult, .. } => assert_eq!(hresult as u32, 0x80070020),
        other => panic!("expected sharing violation32: {other:?}"),
    }
}
pub(super) fn denied_delete(path: &Path) {
    raw32(fs::open(path, io::ALL_SHARING, DELETE).unwrap_err());
}
pub(super) fn not_under(file: &File) {
    io::not_under(&api::Api::load().unwrap(), file).unwrap();
}
