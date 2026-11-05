//! Read-only current-owner privacy checks. No permission changes or allocation.
use super::{api::win32_error, fs};
use crate::{Error, Result};
use std::{
    ffi::c_void,
    fs::File,
    mem::{offset_of, size_of},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{Foundation::*, Security::*, System::LibraryLoader::*};

pub(super) const READ_CONTROL: u32 = 0x0002_0000;
type Current = unsafe extern "system" fn() -> HANDLE;
type Token = unsafe extern "system" fn(HANDLE, u32, *mut HANDLE) -> i32;
type Info = unsafe extern "system" fn(
    HANDLE,
    i32,
    u32,
    *mut PSID,
    *mut PSID,
    *mut *mut ACL,
    *mut *mut ACL,
    *mut PSECURITY_DESCRIPTOR,
) -> u32;

struct Library(HMODULE);
impl Library {
    fn load(name: &str) -> Result<Self> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let handle = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if handle.is_null() {
            return Err(win32_error("LoadLibraryExW(control privacy System32)"));
        }
        Ok(Self(handle))
    }
    unsafe fn symbol<T: Copy>(&self, name: &'static [u8]) -> Result<T> {
        let pointer = unsafe { GetProcAddress(self.0, name.as_ptr()) }
            .ok_or_else(|| win32_error("GetProcAddress(control privacy)"))?;
        assert_eq!(
            size_of::<T>(),
            size_of::<unsafe extern "system" fn() -> isize>()
        );
        Ok(unsafe { std::mem::transmute_copy(&pointer) })
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}
struct Allocation(PSECURITY_DESCRIPTOR);
impl Drop for Allocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0) };
        }
    }
}

/// The small function table is loaded only by explicitly requested control APIs.
pub(super) struct Privacy {
    _security: Library,
    _kernel: Library,
    current: Current,
    token: Token,
    info: Info,
}
// Immutable System32 function pointers and module references. Methods keep all
// per-call token/descriptor state local; the last owner alone unloads the DLLs.
unsafe impl Send for Privacy {}
unsafe impl Sync for Privacy {}
impl Privacy {
    pub fn load() -> Result<Self> {
        let security = Library::load("advapi32.dll")?;
        let kernel = Library::load("kernel32.dll")?;
        Ok(Self {
            current: unsafe { kernel.symbol(b"GetCurrentProcess\0") }?,
            token: unsafe { security.symbol(b"OpenProcessToken\0") }?,
            info: unsafe { security.symbol(b"GetSecurityInfo\0") }?,
            _security: security,
            _kernel: kernel,
        })
    }
    pub fn duplicate(&self, file: &File) -> Result<File> {
        let mut handle = std::ptr::null_mut();
        let process = unsafe { (self.current)() };
        if unsafe {
            DuplicateHandle(
                process,
                fs::handle(file),
                process,
                &mut handle,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(win32_error("DuplicateHandle(control pin)"));
        }
        Ok(unsafe { File::from_raw_handle(handle) })
    }
    fn user(&self) -> Result<(Vec<usize>, usize)> {
        let mut token = std::ptr::null_mut();
        if unsafe { (self.token)((self.current)(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(win32_error("OpenProcessToken(control privacy)"));
        }
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut required = 0;
        let ok = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut required,
            )
        };
        let error = unsafe { GetLastError() };
        if ok == 0 && error != ERROR_INSUFFICIENT_BUFFER {
            return Err(std::io::Error::from_raw_os_error(error as i32).into());
        }
        if ok != 0 || required < size_of::<TOKEN_USER>() as u32 || required > 65536 {
            return Err(Error::Unsupported("bounded current user SID unavailable"));
        }
        let capacity = required;
        let mut bytes = vec![0usize; (capacity as usize).div_ceil(size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                bytes.as_mut_ptr().cast(),
                capacity,
                &mut required,
            )
        } == 0
        {
            return Err(win32_error("GetTokenInformation(control privacy)"));
        }
        if required < size_of::<TOKEN_USER>() as u32 || required > capacity {
            return Err(Error::IdentityMismatch);
        }
        Ok((bytes, required as usize))
    }
    pub fn verify(&self, file: &File) -> Result<()> {
        let (user, length) = self.user()?;
        let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let start = user.as_ptr() as usize;
        let offset = (sid as usize)
            .checked_sub(start)
            .ok_or(Error::IdentityMismatch)?;
        if offset > length.saturating_sub(8)
            || unsafe { IsValidSid(sid) } == 0
            || unsafe { GetLengthSid(sid) } as usize > length - offset
        {
            return Err(Error::IdentityMismatch);
        }
        let mut owner = std::ptr::null_mut();
        let mut acl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        let error = unsafe {
            (self.info)(
                fs::handle(file),
                1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        let _allocation = Allocation(descriptor);
        if error != 0 {
            return Err(std::io::Error::from_raw_os_error(error as i32).into());
        }
        let mut flags = 0u16;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut flags, &mut revision) } == 0 {
            return Err(win32_error("GetSecurityDescriptorControl(control privacy)"));
        }
        if owner.is_null()
            || acl.is_null()
            || flags & 0x1000 == 0
            || unsafe { IsValidSid(owner) } == 0
            || unsafe { EqualSid(owner, sid) } == 0
            || unsafe { IsValidAcl(acl) } == 0
            || unsafe { (*acl).AceCount } != 1
        {
            return Err(Error::Unsupported(
                "control requires protected current-owner-only DACL",
            ));
        }
        let mut ace: *mut c_void = std::ptr::null_mut();
        if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
            return Err(win32_error("GetAce(control privacy)"));
        }
        let header = unsafe { ace.cast::<ACE_HEADER>().read_unaligned() };
        let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if header.AceType != 0
            || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0
            || (header.AceSize as usize) < sid_offset + 8
        {
            return Err(Error::Unsupported(
                "control requires one effective owner allow ACE",
            ));
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
            return Err(Error::Unsupported(
                "control requires current owner full-access ACE only",
            ));
        }
        Ok(())
    }
}
