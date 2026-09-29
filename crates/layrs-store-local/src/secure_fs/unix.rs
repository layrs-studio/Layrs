use std::ffi::{CString, OsStr, c_char, c_int, c_uint};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use super::{FileIdentity, PinnedFile};

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) const DIRECTORY_OPEN_FLAGS: c_int = 0o200_000 | 0o400_000 | 0o2_000_000;
#[cfg(target_vendor = "apple")]
pub(super) const DIRECTORY_OPEN_FLAGS: c_int = 0x0010_0000 | 0x0000_0100 | 0x0100_0000;
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
pub(super) const DIRECTORY_OPEN_FLAGS: c_int = 0x0002_0000 | 0x0000_0100;

const O_RDONLY: c_int = 0;
const O_RDWR: c_int = 2;
const O_CREAT: c_int = 0o100;
const O_EXCL: c_int = 0o200;
#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe extern "C" {
    fn renameat2(
        old_dir: c_int,
        old_name: *const c_char,
        new_dir: c_int,
        new_name: *const c_char,
        flags: c_uint,
    ) -> c_int;
}
#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    fn renameatx_np(
        old_dir: c_int,
        old_name: *const c_char,
        new_dir: c_int,
        new_name: *const c_char,
        flags: c_uint,
    ) -> c_int;
}
unsafe extern "C" {
    fn openat(directory: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn mkdirat(directory: c_int, path: *const c_char, mode: c_uint) -> c_int;
    fn linkat(
        old_directory: c_int,
        old_path: *const c_char,
        new_directory: c_int,
        new_path: *const c_char,
        flags: c_int,
    ) -> c_int;
    fn unlinkat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
}

pub(super) fn open_directory_at(
    parent: &File,
    name: &OsStr,
    create: bool,
    _writable: bool,
) -> io::Result<File> {
    let name = c_name(name)?;
    if create {
        // SAFETY: the parent fd is live and `name` is NUL-terminated.
        let result = unsafe { mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
    }
    open_at(
        parent.as_raw_fd(),
        &name,
        O_RDONLY | DIRECTORY_OPEN_FLAGS,
        0,
    )
}

pub(super) fn open_directory_for_move(parent: &File, name: &OsStr) -> io::Result<File> {
    open_directory_at(parent, name, false, false)
}

pub(super) fn create_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = c_name(name)?;
    open_at(
        parent.as_raw_fd(),
        &name,
        O_RDWR | O_CREAT | O_EXCL | no_follow_close_exec(),
        0o600,
    )
}

pub(super) fn open_private_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = c_name(name)?;
    // SAFETY: parent descriptor and the terminated basename remain live.
    if unsafe { mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    open_at(
        parent.as_raw_fd(),
        &name,
        O_RDONLY | DIRECTORY_OPEN_FLAGS,
        0,
    )
}

pub(super) fn open_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = c_name(name)?;
    open_at(
        parent.as_raw_fd(),
        &name,
        O_RDONLY | no_follow_close_exec(),
        0,
    )
}

pub(super) fn rename_at(source: &PinnedFile, destination: &File, name: &OsStr) -> io::Result<bool> {
    let existing = open_file_at(&source.parent, &source.name)?;
    if FileIdentity::from_metadata(&existing.metadata()?)? != source.identity {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "file changed before rename",
        ));
    }
    let old_name = c_name(&source.name)?;
    let new_name = c_name(name)?;
    #[cfg(test)]
    RENAME_TEST_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: both directory descriptors and NUL-terminated names are live.
    let result = unsafe {
        renameat2(
            source.parent.as_raw_fd(),
            old_name.as_ptr(),
            destination.as_raw_fd(),
            new_name.as_ptr(),
            1,
        )
    };
    #[cfg(target_vendor = "apple")]
    // SAFETY: RENAME_EXCL forbids replacing a destination; descriptors are live.
    let result = unsafe {
        renameatx_np(
            source.parent.as_raw_fd(),
            old_name.as_ptr(),
            destination.as_raw_fd(),
            new_name.as_ptr(),
            4,
        )
    };
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        let _ = (old_name, new_name, destination);
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic exclusive rename unavailable",
        ));
    }
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    if result == 0 {
        Ok(false)
    } else {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::AlreadyExists {
            Ok(true)
        } else {
            Err(error)
        }
    }
}

#[cfg(test)]
std::thread_local! {
    pub(super) static RENAME_TEST_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn hard_link_at(
    source: &PinnedFile,
    destination: &File,
    name: &OsStr,
) -> io::Result<bool> {
    let destination_name = c_name(name)?;
    let source_name = c_name(&source.name)?;
    // SAFETY: both descriptors and both NUL-terminated names remain live.
    let result = unsafe {
        linkat(
            source.parent.as_raw_fd(),
            source_name.as_ptr(),
            destination.as_raw_fd(),
            destination_name.as_ptr(),
            0,
        )
    };
    if result == 0 {
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::AlreadyExists {
        Ok(true)
    } else {
        Err(error)
    }
}

pub(super) fn unlink_file(source: &PinnedFile) -> io::Result<()> {
    let current = open_file_at(&source.parent, &source.name)?;
    if FileIdentity::from_metadata(&current.metadata()?)? != source.identity {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "pending source identity changed before unlink",
        ));
    }
    let name = c_name(&source.name)?;
    // SAFETY: the parent fd is live and `name` is NUL-terminated.
    if unsafe { unlinkat(source.parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn unlink_name_at(parent: &File, name: &OsStr) -> io::Result<()> {
    let name = c_name(name)?;
    // SAFETY: the parent fd is live and `name` is NUL-terminated.
    if unsafe { unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn remove_empty_directory_at(parent: &File, name: &OsStr) -> io::Result<()> {
    // AT_REMOVEDIR atomically refuses regular files and symlinks. No recursive
    // deletion is performed if another writer adds a child before this call.
    let name = c_name(name)?;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let flag = 0x200;
    #[cfg(target_vendor = "apple")]
    let flag = 0x80;
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory removal is unsupported on this platform",
    ));
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    if unsafe { unlinkat(parent.as_raw_fd(), name.as_ptr(), flag) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn open_at(directory: RawFd, name: &CString, flags: c_int, mode: c_uint) -> io::Result<File> {
    // SAFETY: `directory` is live and `name` is NUL-terminated.
    let fd = unsafe { openat(directory, name.as_ptr(), flags, mode) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: `openat` returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

fn c_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path component contains NUL"))
}

fn no_follow_close_exec() -> c_int {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        0o400_000 | 0o2_000_000
    }
    #[cfg(target_vendor = "apple")]
    {
        0x0000_0100 | 0x0100_0000
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    {
        0x0000_0100
    }
}
