//! Owner-only directories for resumable transfer data; never weaken an existing
//! directory's ACL, follow a symlink, or expose bytes before applying privacy.
use super::*;
impl PinnedDirectory {
    pub(crate) fn ensure_private_child(&self, name: &OsStr) -> io::Result<Self> {
        validate_name(name)?;
        #[cfg(unix)]
        let file = platform::open_private_directory_at(&self.file, name)?;
        #[cfg(windows)]
        let file = {
            let descriptor = windows::private_descriptor()?;
            platform::open_private_directory_at(&self.file, name, descriptor.0)?
        };
        #[cfg(not(any(unix, windows)))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "private transfer cache unsupported",
        ));
        #[cfg(any(unix, windows))]
        {
            let metadata = file.metadata()?;
            let display_path = self.display_path.join(name);
            validate_directory_metadata(&display_path, &metadata)?;
            verify_owner_only(&file)?;
            let result = Self {
                identity: file_identity(&file)?,
                file,
                display_path,
            };
            self.sync()?;
            result.sync()?;
            result.verify_path_identity()?;
            Ok(result)
        }
    }
}

#[cfg(unix)]
fn verify_owner_only(file: &File) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no arguments or mutable output.
    if metadata.uid() != unsafe { geteuid() } || metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "transfer cache directory must be owned by current user and mode0700",
        ));
    }
    Ok(())
}
#[cfg(windows)]
fn verify_owner_only(file: &File) -> io::Result<()> {
    windows::verify(file)
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        ffi::c_void,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{Authorization::*, *},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    pub(super) struct Allocation(pub(super) *mut c_void);
    impl Drop for Allocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }
    fn user() -> io::Result<Vec<usize>> {
        let mut token = ptr::null_mut();
        // SAFETY: writable handle slot and process pseudo-handle are valid.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut length = 0;
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                ptr::null_mut(),
                0,
                &mut length,
            );
        }
        if length == 0 || length > 64 * 1024 {
            return Err(io::Error::other("invalid current user token size"));
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(buffer)
    }
    pub(super) fn private_descriptor() -> io::Result<Allocation> {
        let user = user()?;
        let sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut value = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let allocation = Allocation(value.cast());
        let mut length = 0;
        while unsafe { *value.add(length) } != 0 {
            length += 1;
            if length > 256 {
                return Err(io::Error::other("invalid user SID length"));
            }
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })
            .map_err(|_| io::Error::other("invalid user SID"))?;
        drop(allocation);
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;OICI;FA;;;{sid})\0")
            .encode_utf16()
            .collect();
        let mut descriptor = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Allocation(descriptor))
    }
    pub(super) fn verify(file: &File) -> io::Result<()> {
        let user = user()?;
        let user_sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut owner = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let error = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        let _allocation = Allocation(descriptor);
        let rejected = || {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "transfer cache requires current owner and an owner-only ACL",
            )
        };
        if owner.is_null()
            || acl.is_null()
            || unsafe { EqualSid(owner, user_sid) } == 0
            || unsafe { (*acl).AceCount } != 1
        {
            return Err(rejected());
        }
        let mut ace = ptr::null_mut();
        if unsafe { GetAce(acl, 0, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        if allowed.Header.AceType != 0
            || allowed.Mask != 0x001f_01ff
            || unsafe { EqualSid(ptr::addr_of!(allowed.SidStart).cast_mut().cast(), user_sid) } == 0
        {
            return Err(rejected());
        }
        Ok(())
    }
}
