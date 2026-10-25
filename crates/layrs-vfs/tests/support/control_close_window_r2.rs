//! R2 TEST ONLY. Every fixture is fresh and retained. No production guard,
//! receipt authentication, readiness, or atomic namespace guarantee is implied.
//! The r1 and both retained-handle failure recipes remain unchanged.
use super::{disposable_run, native_info};
use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    marker::PhantomData,
    mem::{offset_of, size_of, size_of_val},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    rc::Rc,
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
mod cfapi;

const BYTES: &[u8] = b"{\"kind\":\"layrs.control-close-r2-fixture.v1\"}\n";
const READ_SECURITY: u32 = 0x0002_0000;
const NOT_UNDER: i32 = 0x80070186u32 as i32;
const ALL_SHARING: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
type Id = (u32, u64);

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn open(path: &Path, access: u32, sharing: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(sharing)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}
fn id(file: &File) -> Id {
    let info = native_info(file);
    (
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}
fn length(file: &File) -> u64 {
    let info = native_info(file);
    (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow)
}
fn wide(path: &Path) -> Vec<u16> {
    let value = path.to_str().unwrap().replace('/', "\\");
    assert!(path.is_absolute() && !value.starts_with("\\\\"));
    format!("\\\\?\\{value}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}
fn new_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}
fn exact_bytes(mut file: &File, expected: &[u8]) -> io::Result<bool> {
    if length(file) != expected.len() as u64 {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![0; expected.len()];
    file.read_exact(&mut bytes)?;
    Ok(bytes == expected && file.read(&mut [0; 1])? == 0)
}
fn exact_layout(path: &Path, only: &str) -> io::Result<bool> {
    let mut entries = std::fs::read_dir(path)?;
    let Some(first) = entries.next().transpose()? else {
        return Ok(false);
    };
    Ok(first.file_name() == only && entries.next().transpose()?.is_none())
}

struct Dll(HMODULE);
impl Dll {
    fn load(name: &str) -> io::Result<Self> {
        let name: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
        let handle = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
    unsafe fn symbol<T: Copy>(&self, name: &'static [u8]) -> io::Result<T> {
        let value = unsafe { GetProcAddress(self.0, name.as_ptr()) }
            .ok_or_else(io::Error::last_os_error)?;
        assert_eq!(size_of::<T>(), size_of_val(&value));
        Ok(unsafe { std::mem::transmute_copy(&value) })
    }
}
impl Drop for Dll {
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

type SetMode = unsafe extern "system" fn(i8) -> i8;
type GetMode = unsafe extern "system" fn() -> i8;
type CurrentProcess = unsafe extern "system" fn() -> HANDLE;
type OpenToken = unsafe extern "system" fn(HANDLE, u32, *mut HANDLE) -> i32;
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
    _kernel: Dll,
    _security: Dll,
    _ntdll: Dll,
    current: CurrentProcess,
    token: OpenToken,
    sid_text: SidText,
    parse_sd: ParseSd,
    security_info: SecurityInfo,
    set_mode: SetMode,
    get_mode: GetMode,
    cf: cfapi::Api,
}
impl Native {
    fn load() -> Self {
        let kernel = Dll::load("kernel32.dll").unwrap();
        let security = Dll::load("advapi32.dll").unwrap();
        let ntdll = Dll::load("ntdll.dll").unwrap();
        Self {
            current: unsafe { kernel.symbol(b"GetCurrentProcess\0") }.unwrap(),
            token: unsafe { security.symbol(b"OpenProcessToken\0") }.unwrap(),
            sid_text: unsafe { security.symbol(b"ConvertSidToStringSidW\0") }.unwrap(),
            parse_sd: unsafe {
                security.symbol(b"ConvertStringSecurityDescriptorToSecurityDescriptorW\0")
            }
            .unwrap(),
            security_info: unsafe { security.symbol(b"GetSecurityInfo\0") }.unwrap(),
            set_mode: unsafe { ntdll.symbol(b"RtlSetThreadPlaceholderCompatibilityMode\0") }
                .unwrap(),
            get_mode: unsafe { ntdll.symbol(b"RtlQueryThreadPlaceholderCompatibilityMode\0") }
                .unwrap(),
            cf: cfapi::Api::load().unwrap(),
            _kernel: kernel,
            _security: security,
            _ntdll: ntdll,
        }
    }
    fn attrs(&self, file: &File) -> io::Result<FILE_ATTRIBUTE_TAG_INFO> {
        let mode = Mode::enter(self, 2)?;
        let mut value = FILE_ATTRIBUTE_TAG_INFO::default();
        let result = if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileAttributeTagInfo,
                (&mut value as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of_val(&value) as u32,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(value)
        };
        mode.restore()?;
        result
    }
    fn plain(&self, file: &File, directory: bool) -> io::Result<()> {
        let attrs = self.attrs(file)?;
        if attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || attrs.ReparseTag != 0
            || (attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        {
            return Err(invalid("ordinary expected object type required"));
        }
        Ok(())
    }
    fn duplicate(&self, file: &File) -> io::Result<File> {
        let mut duplicated = std::ptr::null_mut();
        let process = unsafe { (self.current)() };
        if unsafe {
            DuplicateHandle(
                process,
                file.as_raw_handle(),
                process,
                &mut duplicated,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_handle(duplicated) })
    }
    fn user(&self) -> io::Result<Vec<usize>> {
        let mut token = std::ptr::null_mut();
        if unsafe { (self.token)((self.current)(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
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
        if length < size_of::<TOKEN_USER>() as u32 || length > 65536 {
            return Err(invalid("bounded TOKEN_USER required"));
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let capacity = length;
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                capacity,
                &mut length,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if length > capacity || length < size_of::<TOKEN_USER>() as u32 {
            return Err(invalid("invalid TOKEN_USER length"));
        }
        Ok(buffer)
    }
    fn descriptor(&self, broad: bool) -> io::Result<Allocation> {
        let user = self.user()?;
        let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut value = std::ptr::null_mut();
        if unsafe { (self.sid_text)(sid, &mut value) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let allocation = Allocation(value.cast());
        let mut count = 0;
        while unsafe { *value.add(count) } != 0 {
            count += 1;
            if count > 256 {
                return Err(invalid("bounded SID string required"));
            }
        }
        let owner = String::from_utf16(unsafe { std::slice::from_raw_parts(value, count) })
            .map_err(|_| invalid("SID text"))?;
        drop(allocation);
        let trustee = if broad { "WD" } else { owner.as_str() };
        let text: Vec<u16> = format!("O:{owner}D:P(A;OICI;FA;;;{trustee})\0")
            .encode_utf16()
            .collect();
        let mut descriptor = std::ptr::null_mut();
        if unsafe { (self.parse_sd)(text.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Allocation(descriptor))
    }
    fn create_private(&self, path: &Path, broad: bool) -> io::Result<()> {
        let descriptor = self.descriptor(broad)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // CreateDirectoryW fails on an existing path; this never changes an ACL.
        if unsafe { CreateDirectoryW(wide(path).as_ptr(), &attributes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn private(&self, file: &File) -> io::Result<()> {
        let user = self.user()?;
        let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut owner = std::ptr::null_mut();
        let mut acl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        let error = unsafe {
            (self.security_info)(
                file.as_raw_handle(),
                1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let _allocation = Allocation(descriptor);
        let mut control = 0u16;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if owner.is_null()
            || acl.is_null()
            || control & 0x1000 == 0
            || unsafe { EqualSid(owner, sid) } == 0
            || unsafe { (*acl).AceCount } != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "protected current-owner-only DACL required",
            ));
        }
        let mut ace = std::ptr::null_mut();
        if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
        let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if header.AceType != 0 || (header.AceSize as usize) < sid_offset + 8 {
            return Err(invalid("bounded allow ACE required"));
        }
        let mask = unsafe {
            ace.cast::<u8>()
                .add(offset_of!(ACCESS_ALLOWED_ACE, Mask))
                .cast::<u32>()
                .read_unaligned()
        };
        let ace_sid = unsafe { ace.cast::<u8>().add(sid_offset).cast() };
        if mask != 0x001f01ff
            || unsafe { IsValidSid(ace_sid) } == 0
            || unsafe { GetLengthSid(ace_sid) } as usize > header.AceSize as usize - sid_offset
            || unsafe { EqualSid(ace_sid, sid) } == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "owner-only ACE required",
            ));
        }
        Ok(())
    }
}
struct Mode<'a> {
    native: &'a Native,
    previous: i8,
    active: bool,
    _same_thread: PhantomData<Rc<()>>,
}
impl<'a> Mode<'a> {
    fn enter(native: &'a Native, value: i8) -> io::Result<Self> {
        let previous = unsafe { (native.set_mode)(value) };
        if previous < 0 {
            return Err(io::Error::other(format!("compatibility enter={previous}")));
        }
        Ok(Self {
            native,
            previous,
            active: true,
            _same_thread: PhantomData,
        })
    }
    fn restore(mut self) -> io::Result<()> {
        let result = unsafe { (self.native.set_mode)(self.previous) };
        if result < 0 {
            return Err(io::Error::other(format!("compatibility restore={result}")));
        }
        self.active = false;
        assert_eq!(unsafe { (self.native.get_mode)() }, self.previous);
        Ok(())
    }
}
impl Drop for Mode<'_> {
    fn drop(&mut self) {
        if self.active {
            unsafe { (self.native.set_mode)(self.previous) };
        }
    }
}

#[derive(Debug)]
struct Pin {
    path: PathBuf,
    file: File,
    identity: Id,
}
fn pins(native: &Native, path: &Path) -> Vec<Pin> {
    let mut parts = path.components();
    assert!(
        matches!(parts.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
    );
    assert!(matches!(parts.next(), Some(Component::RootDir)));
    assert!(parts.all(|part| matches!(part, Component::Normal(_))));
    let value = path.to_str().unwrap().replace('/', "\\");
    assert!(
        !value[3..]
            .split('\\')
            .any(|p| matches!(p, "." | "..") || p.ends_with(['.', ' ']) || p.contains(':'))
    );
    let mut pins = Vec::new();
    for path in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let file = open(
            path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_SECURITY,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )
        .unwrap();
        native.plain(&file, true).unwrap();
        not_under(native, &file).unwrap();
        pins.push(Pin {
            path: path.to_owned(),
            identity: id(&file),
            file,
        });
    }
    pins
}

#[derive(Debug)]
enum Error {
    Io(io::Error),
    RootState {
        hresult: i32,
        attributes: u32,
        tag: u32,
    },
    Type,
    Identity,
    Bytes,
    Links,
    Layout,
    Streams,
    Receipt,
    Privacy,
    BusyRename {
        raw: io::Error,
    },
    Collision {
        raw: io::Error,
    },
}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
type Result<T> = std::result::Result<T, Error>;
fn not_under(native: &Native, file: &File) -> Result<()> {
    let attrs = native.attrs(file)?;
    let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
    let hr = unsafe {
        (native.cf.root_info)(
            file.as_raw_handle(),
            CF_SYNC_ROOT_INFO_BASIC,
            (&mut info as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
            size_of_val(&info) as u32,
            std::ptr::null_mut(),
        )
    };
    if hr != NOT_UNDER {
        return Err(Error::RootState {
            hresult: hr,
            attributes: attrs.FileAttributes,
            tag: attrs.ReparseTag,
        });
    }
    if attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || attrs.ReparseTag != 0
        || attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(Error::Type);
    }
    Ok(())
}
fn boundary(native: &Native, pins: &[Pin], name: &str) -> Result<()> {
    for pin in pins {
        not_under(native, &pin.file)?;
        let current = open(
            &pin.path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )?;
        not_under(native, &current)?;
        if id(&pin.file) != pin.identity || id(&current) != pin.identity {
            return Err(Error::Identity);
        }
    }
    eprintln!(
        "G-B {name}: {} held+reopened ancestors/root, EXPOSE attributes + exact NOT_UNDER=0x80070186",
        pins.len()
    );
    Ok(())
}

fn stream_names(file: &File) -> io::Result<Vec<String>> {
    let mut buffer = vec![u64::MAX; 8192]; // fixed 64KiB; no unbounded retry
    unsafe { SetLastError(0) };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStreamInfo,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
        )
    };
    let error = unsafe { GetLastError() };
    if error == ERROR_HANDLE_EOF {
        return Ok(Vec::new());
    }
    if ok == 0 {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8) };
    let mut offset = 0usize;
    let mut names = Vec::new();
    loop {
        let fixed = offset_of!(FILE_STREAM_INFO, StreamName);
        if names.len() >= 32
            || offset
                .checked_add(fixed)
                .is_none_or(|end| end > bytes.len())
        {
            return Err(invalid("stream inventory exceeds bounds"));
        }
        let u32_at = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        let next = u32_at(offset) as usize;
        let length = u32_at(offset + 4) as usize;
        if length == 0
            || length > 4096
            || length % 2 != 0
            || offset
                .checked_add(fixed + length)
                .is_none_or(|end| end > bytes.len())
        {
            return Err(invalid("invalid stream name bounds"));
        }
        let name = bytes[offset + fixed..offset + fixed + length]
            .chunks_exact(2)
            .map(|b| u16::from_ne_bytes([b[0], b[1]]))
            .collect::<Vec<_>>();
        names.push(String::from_utf16(&name).map_err(|_| invalid("invalid stream name"))?);
        if next == 0 {
            break;
        }
        if next % 8 != 0 || next < fixed + length {
            return Err(invalid("invalid stream next offset"));
        }
        offset = offset
            .checked_add(next)
            .ok_or_else(|| invalid("stream offset overflow"))?;
    }
    Ok(names)
}
fn streams(file: &File, directory: bool) -> Result<()> {
    let names = stream_names(file)?;
    let valid = if directory {
        names.iter().all(|name| name == "::$DATA")
    } else {
        names == ["::$DATA"]
    };
    if !valid {
        eprintln!("G-D rejected named streams {names:?}");
        return Err(Error::Streams);
    }
    Ok(())
}
fn rename(file: &File, path: &Path) -> io::Result<()> {
    let name = wide(path);
    let length = (offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2)
        .max(size_of::<FILE_RENAME_INFO>());
    let mut buffer = vec![0u64; length.div_ceil(8)];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer
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
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Expected {
    root: Id,
    directory: Id,
    format: Id,
    size: u64,
}

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    private: PathBuf,
    stage: PathBuf,
    roots: Vec<Pin>,
    private_pins: Vec<Pin>,
    directory: Option<File>,
    expected: Expected,
}
impl Fixture {
    fn new(native: &Native, run: &Path, label: &str) -> Self {
        let base = run.join(label);
        std::fs::create_dir(&base).unwrap();
        let root = base.join("checkout");
        std::fs::create_dir(&root).unwrap();
        let roots = pins(native, &root);
        let private = base.join("private");
        native.create_private(&private, false).unwrap();
        let private_pins = pins(native, &private);
        native.private(&private_pins.last().unwrap().file).unwrap();
        // A unique fresh directory per fixture, with no adoption by filename prefix.
        let stage = private.join("unique-stage-attempt-0001");
        native.create_private(&stage, false).unwrap();
        let directory = owner_pin(&stage).unwrap();
        native.private(&directory).unwrap();
        new_bytes(&stage.join("FORMAT"), BYTES).unwrap();
        new_bytes(&private.join("original-byte-witness"), BYTES).unwrap();
        let format = strong(&stage.join("FORMAT")).unwrap();
        let expected = Expected {
            root: roots.last().unwrap().identity,
            directory: id(&directory),
            format: id(&format),
            size: BYTES.len() as u64,
        };
        assert_eq!(
            expected.root.0, expected.directory.0,
            "same-volume stage required"
        );
        drop(format);
        Self {
            base,
            root,
            private,
            stage,
            roots,
            private_pins,
            directory: Some(directory),
            expected,
        }
    }
    fn owner(&self) -> &File {
        self.directory.as_ref().unwrap()
    }
    fn final_path(&self) -> PathBuf {
        self.root.join(".layrs")
    }
    fn receipt_path(&self) -> PathBuf {
        self.private.join("exact-external-intent")
    }
    fn receipt_bytes(&self) -> Vec<u8> {
        // Test receipt DATA, bound independently to this freshly created fixture.
        // A public production decode cannot authenticate the Store's durable receipt.
        format!(
            "test-only-r2-v1\nroot={}\nstage={}\nexpected={:?}\nformat={:?}\n",
            self.root.display(),
            self.stage.display(),
            self.expected,
            BYTES
        )
        .into_bytes()
    }
    fn expected_receipt(&self, native: &Native) -> Result<Expected> {
        boundary(native, &self.private_pins, "resume private parent")?;
        native
            .private(&self.private_pins.last().unwrap().file)
            .map_err(|_| Error::Privacy)?;
        let receipt = strong(&self.receipt_path())?;
        native.plain(&receipt, false).map_err(|_| Error::Type)?;
        if native_info(&receipt).nNumberOfLinks != 1
            || length(&receipt) > 4096
            || !exact_bytes(&receipt, &self.receipt_bytes())?
        {
            return Err(Error::Receipt);
        }
        Ok(self.expected)
    }
    fn reopen_stage(&mut self, native: &Native) {
        drop(self.directory.take());
        self.directory = Some(owner_pin(&self.stage).unwrap());
        assert_eq!(id(self.owner()), self.expected.directory);
        native.private(self.owner()).unwrap();
        eprintln!("G-E resume: new by-handle private stage pin, exact original native ID");
    }
}
fn owner_pin(path: &Path) -> io::Result<File> {
    open(
        path,
        DELETE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_SECURITY,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
    )
}
fn observer(path: &Path) -> io::Result<File> {
    open(
        path,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
        ALL_SHARING,
    )
}
fn strong(path: &Path) -> io::Result<File> {
    open(path, FILE_READ_DATA | FILE_READ_ATTRIBUTES, FILE_SHARE_READ)
}
fn directory_at(native: &Native, fixture: &Fixture, path: &Path) -> Result<File> {
    native
        .plain(fixture.owner(), true)
        .map_err(|_| Error::Type)?;
    native
        .private(fixture.owner())
        .map_err(|_| Error::Privacy)?;
    let current = observer(path)?; // Intentionally SHARE_DELETE: owner has DELETE access.
    native.plain(&current, true).map_err(|_| Error::Type)?;
    if id(fixture.owner()) != fixture.expected.directory
        || id(&current) != fixture.expected.directory
        || id(&fixture.roots.last().unwrap().file) != fixture.expected.root
    {
        return Err(Error::Identity);
    }
    Ok(current)
}
fn check_control(native: &Native, fixture: &Fixture, path: &Path, format: &File) -> Result<()> {
    let _current = directory_at(native, fixture, path)?;
    native.plain(format, false).map_err(|_| Error::Type)?;
    if id(format) != fixture.expected.format {
        return Err(Error::Identity);
    }
    let current_format = strong(&path.join("FORMAT"))?;
    native
        .plain(&current_format, false)
        .map_err(|_| Error::Type)?;
    if id(&current_format) != fixture.expected.format {
        return Err(Error::Identity);
    }
    if native_info(format).nNumberOfLinks != 1 {
        return Err(Error::Links);
    }
    if length(format) != fixture.expected.size || !exact_bytes(format, BYTES)? {
        return Err(Error::Bytes);
    }
    if !exact_layout(path, "FORMAT")? {
        return Err(Error::Layout);
    }
    streams(&fixture.roots.last().unwrap().file, true)?;
    streams(fixture.owner(), true)?;
    streams(format, false)?;
    Ok(())
}

// Recipe-only proof. It borrows the sole DELETE-capable directory pin rather than
// reopening an incompatible no-delete observer. It grants no production access.
struct FinalProof<'a> {
    native: &'a Native,
    fixture: &'a Fixture,
    format: File,
}
impl FinalProof<'_> {
    fn revalidate(&self) -> Result<()> {
        boundary(self.native, &self.fixture.roots, "final revalidate")?;
        check_control(
            self.native,
            self.fixture,
            &self.fixture.final_path(),
            &self.format,
        )
    }
}
#[derive(Default, Debug)]
struct Counts {
    persist: usize,
    rename: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    AfterPersist,
    PreClose,
    PostClose,
    PreRename,
    PostRename,
    PreStrongPin,
}
type Hook<'a> = dyn FnMut(Point, &Fixture) + 'a;

fn attempt<'a>(
    native: &'a Native,
    fixture: &'a Fixture,
    expected: Option<Expected>,
    counts: &mut Counts,
    hook: &mut Hook<'_>,
) -> Result<FinalProof<'a>> {
    boundary(native, &fixture.roots, "entry")?;
    boundary(native, &fixture.private_pins, "private stage ancestry")?;
    native
        .private(&fixture.private_pins.last().unwrap().file)
        .map_err(|_| Error::Privacy)?;
    if expected.is_some_and(|expected| expected != fixture.expected) {
        return Err(Error::Receipt);
    }
    let final_path = fixture.final_path();
    // Exact-final recovery is separately bound to the durable receipt. No fresh
    // registration or source authority follows from a matching layout.
    let final_exists = match std::fs::symlink_metadata(&final_path) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(Error::Io(error)),
    };
    if final_exists {
        if expected.is_none() {
            return Err(Error::Receipt);
        }
        let _final_directory = directory_at(native, fixture, &final_path)?;
        let format = strong(&final_path.join("FORMAT"))?;
        check_control(native, fixture, &final_path, &format)?;
        boundary(native, &fixture.roots, "exact final before proof")?;
        return Ok(FinalProof {
            native,
            fixture,
            format,
        });
    }
    if std::fs::read_dir(&fixture.root)?
        .next()
        .transpose()?
        .is_some()
    {
        return Err(Error::Layout);
    }
    let _stage_directory = directory_at(native, fixture, &fixture.stage)?;
    let format = strong(&fixture.stage.join("FORMAT"))?;
    check_control(native, fixture, &fixture.stage, &format)?;
    if expected.is_none() {
        new_bytes(&fixture.receipt_path(), &fixture.receipt_bytes())?;
        counts.persist += 1;
    }
    hook(Point::AfterPersist, fixture);
    boundary(native, &fixture.roots, "after persist")?;
    hook(Point::PreClose, fixture);
    boundary(native, &fixture.roots, "PreClose")?;
    check_control(native, fixture, &fixture.stage, &format)?;
    if std::fs::read_dir(&fixture.root)?
        .next()
        .transpose()?
        .is_some()
    {
        return Err(Error::Layout);
    }
    // ALL other FORMAT read handles must also be released. G-C deliberately
    // violates this precondition and proves ONE native rename per call.
    drop(format);
    hook(Point::PostClose, fixture);
    hook(Point::PreRename, fixture);
    counts.rename += 1;
    if let Err(raw) = rename(fixture.owner(), &final_path) {
        return Err(match raw.raw_os_error() {
            Some(5) => Error::BusyRename { raw },
            Some(80 | 183) => Error::Collision { raw },
            _ => Error::Io(raw),
        });
    }
    hook(Point::PostRename, fixture);
    hook(Point::PreStrongPin, fixture);
    boundary(native, &fixture.roots, "final before proof")?;
    let _final_directory = directory_at(native, fixture, &final_path)?;
    let format = strong(&final_path.join("FORMAT"))?;
    check_control(native, fixture, &final_path, &format)?;
    if !exact_layout(&fixture.root, ".layrs")? {
        return Err(Error::Layout);
    }
    Ok(FinalProof {
        native,
        fixture,
        format,
    })
}
fn clean<'a>(native: &'a Native, fixture: &'a Fixture) -> FinalProof<'a> {
    let mut counts = Counts::default();
    let proof = attempt(native, fixture, None, &mut counts, &mut |_, _| {}).unwrap();
    assert_eq!((counts.persist, counts.rename), (1, 1));
    proof.revalidate().unwrap();
    proof
}
fn error<T>(result: Result<T>) -> Error {
    match result {
        Ok(_) => panic!("unexpected final proof"),
        Err(error) => error,
    }
}
fn sharing<T>(result: io::Result<T>, operation: &str) {
    let error = match result {
        Ok(_) => panic!("{operation} unexpectedly succeeded"),
        Err(error) => error,
    };
    eprintln!("{operation}: raw Win32={:?}", error.raw_os_error());
    assert_eq!(error.raw_os_error(), Some(32), "{operation}");
}

fn interop(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "g-a-duplicate-observers");
    let proof = clean(native, &fixture);
    let path = fixture.final_path();
    let duplicate = native.duplicate(fixture.owner()).unwrap();
    assert_eq!(id(&duplicate), fixture.expected.directory);
    let downstream = observer(&path).unwrap();
    assert_eq!(id(&downstream), fixture.expected.directory);
    sharing(
        open(
            &path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        ),
        "G-A legacy shareRW observer conflicts with DELETE owner",
    );
    sharing(
        open(&path, DELETE, ALL_SHARING),
        "G-A third-party directory DELETE open",
    );
    sharing(
        std::fs::rename(&path, fixture.root.join("moved")),
        "G-A directory rename",
    );
    sharing(std::fs::remove_dir(&path), "G-A directory delete");
    proof.revalidate().unwrap();
    drop(proof);
    drop(downstream);
    // Prove protection is retained by the duplicate after the original is gone.
    let Fixture {
        directory,
        root,
        expected,
        ..
    } = fixture;
    drop(directory);
    assert_eq!(id(&duplicate), expected.directory);
    sharing(
        open(&path, DELETE, ALL_SHARING),
        "G-A duplicate alone excludes third-party DELETE",
    );
    let second = native.duplicate(&duplicate).unwrap();
    drop(duplicate);
    assert_eq!(id(&second), expected.directory);
    sharing(
        std::fs::rename(&path, root.join("still-held")),
        "G-A second duplicate retains protection",
    );
    let downstream = observer(&path).unwrap();
    assert_eq!(id(&downstream), expected.directory);
    eprintln!(
        "G-A PASS: DuplicateHandle SAME_ACCESS; SHARE_ALL observer same ID; shareRW incompatible; no authority/raw handle exported"
    );
}

fn leaked_readers(native: &Native, run: &Path) {
    for (label, leak_at) in [
        ("g-c-callback-reader", Point::AfterPersist),
        ("g-c-window-reader", Point::PostClose),
    ] {
        let mut fixture = Fixture::new(native, run, label);
        let mut counts = Counts::default();
        let mut leaked = None;
        let result = attempt(
            native,
            &fixture,
            None,
            &mut counts,
            &mut |point, fixture| {
                if point == leak_at {
                    leaked = Some(
                        open(
                            &fixture.stage.join("FORMAT"),
                            FILE_READ_DATA | FILE_READ_ATTRIBUTES,
                            ALL_SHARING,
                        )
                        .unwrap(),
                    );
                }
            },
        );
        let cause = error(result);
        match &cause {
            Error::BusyRename { raw } => assert_eq!(raw.raw_os_error(), Some(5)),
            other => panic!("G-C expected raw Win5/Busy after one attempt, got {other:?}"),
        }
        assert_eq!((counts.persist, counts.rename), (1, 1));
        assert_eq!(id(leaked.as_ref().unwrap()), fixture.expected.format);
        assert!(exact_bytes(leaked.as_ref().unwrap(), BYTES).unwrap());
        assert!(!fixture.final_path().exists());
        assert!(exact_layout(&fixture.stage, "FORMAT").unwrap());
        eprintln!("G-C {label}: Busy raw={cause:?}; exact stage retained; no internal retry");
        drop(leaked);
        fixture.reopen_stage(native);
        let expected = fixture.expected_receipt(native).unwrap();
        let proof = attempt(
            native,
            &fixture,
            Some(expected),
            &mut counts,
            &mut |_, _| {},
        )
        .unwrap();
        proof.revalidate().unwrap();
        assert_eq!((counts.persist, counts.rename), (1, 2));
        drop(proof);
        // A separate explicit call recovers the exact final layout. expectedSome
        // never persists and never chooses a different stage by prefix.
        let recovered = attempt(
            native,
            &fixture,
            Some(expected),
            &mut counts,
            &mut |_, _| {},
        )
        .unwrap();
        recovered.revalidate().unwrap();
        assert_eq!((counts.persist, counts.rename), (1, 2));
        eprintln!(
            "G-C {label}: explicit fully rebound retry PASS; expectedSome persist=0; exact-final recovery rename=0"
        );
    }
}

fn register(native: &Native, fixture: &Fixture) -> i32 {
    let blob = b"TEST ONLY R2 PostRename registration; no connect/source/readiness";
    new_bytes(&fixture.private.join("native-registration-intent"), blob).unwrap();
    let provider: Vec<u16> = "Layrs disposable R2 close-window\0"
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
    // CFAPI receives a normal absolute path, matching existing native recipes.
    let root: Vec<u16> = fixture
        .root
        .to_str()
        .unwrap()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        (native.cf.register)(
            root.as_ptr(),
            &registration,
            &policies,
            CF_REGISTER_FLAG_DISABLE_ON_DEMAND_POPULATION_ON_ROOT,
        )
    }
}
fn registered_in_window(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "g-b-postrename-registration");
    let mode = Mode::enter(native, 1).unwrap(); // Force DISGUISE; probes must still expose.
    let mut counts = Counts::default();
    let mut hr = None;
    let result = attempt(
        native,
        &fixture,
        None,
        &mut counts,
        &mut |point, fixture| {
            if point == Point::PostRename {
                hr = Some(register(native, fixture));
            }
        },
    );
    assert_eq!(hr, Some(0), "G-B native registration must actually succeed");
    let cause = error(result);
    match cause {
        Error::RootState {
            hresult,
            attributes,
            tag,
        } => {
            assert!(hresult >= 0, "expected positive syncroot discovery");
            eprintln!(
                "G-B final refusal under forced DISGUISE: root_info=0x{:08x}, EXPOSE attrs=0x{attributes:08x}, tag=0x{tag:08x}; NO FINAL PROOF",
                hresult as u32
            );
        }
        other => panic!("G-B wrong refusal: {other:?}"),
    }
    assert_eq!(unsafe { (native.get_mode)() }, 1);
    mode.restore().unwrap();
    assert_eq!(id(fixture.owner()), fixture.expected.directory);
    let format = strong(&fixture.final_path().join("FORMAT")).unwrap();
    assert_eq!(id(&format), fixture.expected.format);
    assert!(exact_bytes(&format, BYTES).unwrap());
    eprintln!(
        "G-B retained registration/root/stage intent at {}; never connected, hydrated, unregistered, or deleted",
        fixture.base.display()
    );
}

fn collision_and_privacy(native: &Native, run: &Path) {
    for (label, foreign_bytes) in [
        ("g-e-foreign-final", b"foreign collision bytes\n".as_slice()),
        ("g-e-same-byte-foreign-final", BYTES),
    ] {
        let fixture = Fixture::new(native, run, label);
        let mut counts = Counts::default();
        let mut foreign = None;
        let result = attempt(
            native,
            &fixture,
            None,
            &mut counts,
            &mut |point, fixture| {
                if point == Point::PreRename {
                    std::fs::create_dir(fixture.final_path()).unwrap();
                    new_bytes(&fixture.final_path().join("FORMAT"), foreign_bytes).unwrap();
                    let directory = observer(&fixture.final_path()).unwrap();
                    let file = strong(&fixture.final_path().join("FORMAT")).unwrap();
                    foreign = Some((id(&directory), id(&file)));
                }
            },
        );
        let cause = error(result);
        match &cause {
            Error::Collision { raw } => assert!(matches!(raw.raw_os_error(), Some(80 | 183))),
            other => panic!("G-E collision expected: {other:?}"),
        }
        let foreign = foreign.unwrap();
        assert_ne!(foreign.0, fixture.expected.directory);
        assert_ne!(foreign.1, fixture.expected.format);
        assert_eq!(id(&observer(&fixture.final_path()).unwrap()), foreign.0);
        let foreign_file = strong(&fixture.final_path().join("FORMAT")).unwrap();
        assert_eq!(id(&foreign_file), foreign.1);
        assert!(exact_bytes(&foreign_file, foreign_bytes).unwrap());
        let staged = strong(&fixture.stage.join("FORMAT")).unwrap();
        assert_eq!(id(&staged), fixture.expected.format);
        assert!(exact_bytes(&staged, BYTES).unwrap());
        native.private(fixture.owner()).unwrap();
        let expected = fixture.expected_receipt(native).unwrap();
        let refusal = error(attempt(
            native,
            &fixture,
            Some(expected),
            &mut counts,
            &mut |_, _| {},
        ));
        assert!(matches!(refusal, Error::Identity));
        assert_eq!((counts.persist, counts.rename), (1, 1));
        eprintln!(
            "G-E NeedsRepair {cause:?}; foreign={foreign:?}, stage={:?}; both bytes/IDs preserved, no adoption",
            fixture.expected
        );
    }

    let broad = run.join("g-e-new-broad-acl-negative");
    native.create_private(&broad, true).unwrap();
    let broad_pin = owner_pin(&broad).unwrap();
    assert_eq!(
        native.private(&broad_pin).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    eprintln!(
        "G-E owner-only protected DACL checked by handle at create/resume/final; fresh broad-ACL negative rejected; no ACL changes on existing paths"
    );
}

fn typed_receipts(native: &Native, run: &Path) {
    let mut fixture = Fixture::new(native, run, "g-f-directory-vs-legacy");
    let proof = clean(native, &fixture);
    proof.revalidate().unwrap();
    drop(proof);
    drop(fixture.directory.take());
    // Release the incompatible directory DELETE owner so this checks the FILE
    // API's TYPE refusal, rather than accidentally passing on sharing error 32.
    assert!(matches!(
        layrs_vfs::read_control_file(&fixture.root),
        Err(layrs_vfs::Error::Unsupported(_))
    ));
    let legacy = Fixture::new(native, run, "g-f-legacy-vs-directory");
    new_bytes(&legacy.final_path(), b"legacy regular control file\n").unwrap();
    let legacy_file = strong(&legacy.final_path()).unwrap();
    let kind = native.plain(&legacy_file, true).map_err(|_| Error::Type);
    assert!(matches!(kind, Err(Error::Type)));
    assert!(exact_bytes(&legacy_file, b"legacy regular control file\n").unwrap());
    eprintln!("G-F mutual FILE/DIRECTORY type refusal; no generic reserved-path bypass");

    let fixture = Fixture::new(native, run, "g-f-partial-receipt");
    new_bytes(&fixture.receipt_path(), b"test-only-r2").unwrap();
    let before = id(fixture.owner());
    let refusal = fixture.expected_receipt(native).unwrap_err();
    assert!(matches!(refusal, Error::Receipt));
    assert_eq!(
        std::fs::read(fixture.receipt_path()).unwrap(),
        b"test-only-r2"
    );
    assert_eq!(id(fixture.owner()), before);
    assert!(exact_layout(&fixture.stage, "FORMAT").unwrap());
    assert!(!fixture.final_path().exists());
    // A similarly named stage, even with identical bytes, is never considered.
    let unrelated = fixture
        .private
        .join("unique-stage-attempt-0001-oldstage_prefix");
    native.create_private(&unrelated, false).unwrap();
    new_bytes(&unrelated.join("FORMAT"), BYTES).unwrap();
    assert!(matches!(
        fixture.expected_receipt(native),
        Err(Error::Receipt)
    ));
    assert!(exact_layout(&unrelated, "FORMAT").unwrap());
    eprintln!(
        "G-F partial receipt -> NeedsRepair before mutation; exact receipt only; no overwrite or stage-prefix search; expectedSome never re-persists (G-C)"
    );

    let fixture = Fixture::new(native, run, "g-f-wrong-expected-id");
    let mut different = fixture.expected;
    different.directory.1 ^= 1;
    let mut counts = Counts::default();
    assert!(matches!(
        attempt(
            native,
            &fixture,
            Some(different),
            &mut counts,
            &mut |_, _| {}
        ),
        Err(Error::Receipt)
    ));
    assert_eq!((counts.persist, counts.rename), (0, 0));
    assert!(!fixture.receipt_path().exists());
    assert!(exact_layout(&fixture.stage, "FORMAT").unwrap());
    eprintln!(
        "G-F wrong expected native identity refused; no receipt manufacture or stage mutation"
    );
}

fn named_stream(path: &Path) -> io::Result<()> {
    let stream = PathBuf::from(format!("{}:r2-private-test-stream", path.display()));
    new_bytes(&stream, b"retained named-stream witness\n")
}
fn stream_mutations(native: &Native, run: &Path) {
    for final_phase in [false, true] {
        for target in ["root", "control", "format"] {
            let label = format!(
                "g-d-stream-{}-{target}",
                if final_phase {
                    "revalidate"
                } else {
                    "preclose"
                }
            );
            let fixture = Fixture::new(native, run, &label);
            let path = |published: bool| {
                let control = if published {
                    fixture.final_path()
                } else {
                    fixture.stage.clone()
                };
                match target {
                    "root" => fixture.root.clone(),
                    "control" => control,
                    _ => control.join("FORMAT"),
                }
            };
            if final_phase {
                let proof = clean(native, &fixture);
                let mutation = named_stream(&path(true));
                eprintln!("G-D {label} native stream mutation: {mutation:?}");
                mutation.expect(
                    "named-stream mutation must actually exist to qualify this revalidation arm",
                );
                assert!(matches!(proof.revalidate(), Err(Error::Streams)));
            } else {
                let mut counts = Counts::default();
                let cause = error(attempt(
                    native,
                    &fixture,
                    None,
                    &mut counts,
                    &mut |point, _| {
                        if point == Point::PreClose {
                            let mutation = named_stream(&path(false));
                            eprintln!("G-D {label} native stream mutation: {mutation:?}");
                            mutation.expect("named-stream mutation must actually exist to qualify this PreClose arm");
                        }
                    },
                ));
                assert!(matches!(cause, Error::Streams), "{label}: {cause:?}");
                assert_eq!((counts.persist, counts.rename), (1, 0));
            }
            eprintln!(
                "G-D {label}: bounded by-HANDLE stream inventory refused mutation; not a share-lock guarantee"
            );
        }
    }
}

fn link_mutations(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "g-d-hardlink-before-pin");
    let witness = fixture.private.join("extra-hardlink-before");
    std::fs::hard_link(fixture.stage.join("FORMAT"), &witness).unwrap();
    let mut counts = Counts::default();
    let cause = error(attempt(native, &fixture, None, &mut counts, &mut |_, _| {}));
    assert!(matches!(cause, Error::Links));
    assert_eq!((counts.persist, counts.rename), (0, 0));
    assert_eq!(id(&strong(&witness).unwrap()), fixture.expected.format);
    eprintln!(
        "G-D real hardlink before strong pin: refused before persist/rename, witness retained"
    );

    let fixture = Fixture::new(native, run, "g-d-hardlink-after-pin");
    let proof = clean(native, &fixture);
    let witness = fixture.private.join("extra-hardlink-after");
    let mutation = std::fs::hard_link(fixture.final_path().join("FORMAT"), &witness);
    eprintln!("G-D hardlink after final strong pin native result: {mutation:?}");
    mutation.expect("after-pin link revalidation requires a genuinely established native hardlink");
    assert_eq!(id(&strong(&witness).unwrap()), fixture.expected.format);
    assert!(native_info(&proof.format).nNumberOfLinks > 1);
    assert!(matches!(proof.revalidate(), Err(Error::Links)));
    eprintln!(
        "G-D actual after-pin hardlink refused on revalidation; original bytes/links retained; not a share-lock claim"
    );
}

fn reparse_attempt(native: &Native, run: &Path) {
    let fixture = Fixture::new(native, run, "g-d-reparse-native-outcome");
    let proof = clean(native, &fixture);
    let format_path = fixture.final_path().join("FORMAT");
    // Relative symlink target is another owned ordinary file. The target is not
    // created inside .layrs, whose exact closed layout is independently checked.
    let target = fixture.root.join("retained-reparse-target");
    new_bytes(&target, b"native reparse target witness\n").unwrap();
    let writer = open(
        &format_path,
        FILE_WRITE_ATTRIBUTES | FILE_READ_ATTRIBUTES,
        ALL_SHARING,
    );
    let outcome = match writer {
        Err(error) => Err(("open FILE_WRITE_ATTRIBUTES", error)),
        Ok(writer) => {
            // SYMLINK_REPARSE_BUFFER: header8 + offsets8 + Flags4 + UTF16 payload.
            // SDK19041 FSCTL_SET_REPARSE_POINT; no privilege enable or OS changes.
            let name: Vec<u16> = "..\\retained-reparse-target".encode_utf16().collect();
            let mut buffer = vec![0u8; 20 + name.len() * 4];
            let data_length = (buffer.len() - 8) as u16;
            buffer[0..4].copy_from_slice(&0xa000000cu32.to_ne_bytes());
            buffer[4..6].copy_from_slice(&data_length.to_ne_bytes());
            let bytes = (name.len() * 2) as u16;
            buffer[10..12].copy_from_slice(&bytes.to_ne_bytes());
            buffer[12..14].copy_from_slice(&bytes.to_ne_bytes());
            buffer[14..16].copy_from_slice(&bytes.to_ne_bytes());
            buffer[16..20].copy_from_slice(&1u32.to_ne_bytes()); // SYMLINK_FLAG_RELATIVE
            for (at, value) in name.iter().chain(name.iter()).enumerate() {
                buffer[20 + at * 2..22 + at * 2].copy_from_slice(&value.to_ne_bytes());
            }
            let mut returned = 0;
            if unsafe {
                DeviceIoControl(
                    writer.as_raw_handle(),
                    0x000900a4,
                    buffer.as_ptr().cast(),
                    buffer.len() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                Err(("FSCTL_SET_REPARSE_POINT", io::Error::last_os_error()))
            } else {
                Ok(())
            }
        }
    };
    let exposed = native.attrs(&proof.format).unwrap();
    match outcome {
        Ok(()) => {
            eprintln!(
                "G-D reparse mutation actually succeeded: EXPOSE attrs=0x{:08x}, tag=0x{:08x}",
                exposed.FileAttributes, exposed.ReparseTag
            );
            assert_ne!(exposed.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
            assert!(matches!(proof.revalidate(), Err(Error::Type)));
        }
        Err((operation, error)) => {
            eprintln!(
                "G-D reparse mutation NOT ESTABLISHED: {operation} raw Win32={:?}; attrs=0x{:08x}, tag=0x{:08x}; no claim sharing prevents metadata mutation",
                error.raw_os_error(),
                exposed.FileAttributes,
                exposed.ReparseTag
            );
            assert_eq!(exposed.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT, 0);
            proof.revalidate().unwrap();
        }
    }
    assert_eq!(id(&proof.format), fixture.expected.format);
    assert_eq!(
        std::fs::read(fixture.private.join("original-byte-witness")).unwrap(),
        BYTES
    );
}

pub(super) fn run() {
    let (run, _) = disposable_run("control-close-window-r2");
    let native = Native::load();
    let _parents = pins(&native, &run);
    eprintln!(
        "R2 TEST ONLY retained fixtures {}; ONE attempt/call, no source or production guard, no callbacks/connect/hydration",
        run.display()
    );
    let gates: [(&str, fn(&Native, &Path)); 8] = [
        ("G-A", interop),
        ("G-C", leaked_readers),
        ("G-E", collision_and_privacy),
        ("G-F", typed_receipts),
        ("G-B", registered_in_window),
        ("G-D streams", stream_mutations),
        ("G-D links", link_mutations),
        ("G-D reparse", reparse_attempt),
    ];
    let mut outcomes = Vec::new();
    for (name, gate) in gates {
        let passed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gate(&native, &run))).is_ok();
        eprintln!(
            "R2 gate {name}: {passed}; failed gates remain terminal, subsequent independent fixtures still run"
        );
        outcomes.push(passed);
    }
    assert!(
        outcomes.iter().all(|passed| *passed),
        "R2 gates={outcomes:?}; fixtures retained; no production grant"
    );
    eprintln!(
        "R2 recipe PASS: G-A observer/duplicate, G-B exact registration boundaries, G-C explicit retry, G-D inventory/link checks and exact reparse outcome, G-E privacy/collision, G-F typed/receipt refusal. No production authority or namespace atomicity claimed."
    );
}
