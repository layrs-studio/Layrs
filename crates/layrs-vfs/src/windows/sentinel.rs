//! A change sentinel on a Room root: one directory-change request kept
//! pending on the whole tree. NTFS completes it during the first reported
//! change below the root (a name created, deleted or renamed; a size, write
//! time, creation time or attribute changed), or when its buffer overflows.
//! While it is still pending, no such change was reported since it was armed:
//! what git's fsmonitor relies on. Any completion, for any reason, only means
//! "look again".

use super::Session;
use crate::{Error, Result};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::{ERROR_IO_INCOMPLETE, GetLastError, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
    },
    System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
};

const FILTER: u32 = FILE_NOTIFY_CHANGE_FILE_NAME
    | FILE_NOTIFY_CHANGE_DIR_NAME
    | FILE_NOTIFY_CHANGE_ATTRIBUTES
    | FILE_NOTIFY_CHANGE_SIZE
    | FILE_NOTIFY_CHANGE_LAST_WRITE
    | FILE_NOTIFY_CHANGE_CREATION;

pub struct ChangeSentinel {
    directory: OwnedHandle,
    // Boxed: the kernel writes both while the request is pending; they are
    // freed only after it completed or was cancelled (`Drop`).
    overlapped: Box<OVERLAPPED>,
    buffer: Box<[u32; 1024]>,
}

// The request is only observed through `quiet` and cancelled in `Drop`; the
// kernel's completion is not tied to the thread that issued it.
unsafe impl Send for ChangeSentinel {}
unsafe impl Sync for ChangeSentinel {}

impl Session {
    /// Arms a sentinel on this session's root. The session keeps the root
    /// and its ancestors open without delete sharing, so the path names the
    /// very directory the session serves for as long as it lives.
    pub fn arm_change_sentinel(&self) -> Result<ChangeSentinel> {
        ChangeSentinel::arm(&self.root.root)
    }
}

impl ChangeSentinel {
    fn arm(root: &std::path::Path) -> Result<Self> {
        let name = super::fs::native_path(root)?;
        // SAFETY: `name` is a NUL-terminated wide path; no security attributes
        // or template handle.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: `handle` is the one successful CreateFileW result above.
        let directory = unsafe { OwnedHandle::from_raw_handle(handle) };
        // SAFETY: an all-zero OVERLAPPED (no event: completion signals the
        // handle, which serves no other request) is valid.
        let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
        let mut buffer = Box::new([0_u32; 1024]);
        // SAFETY: the buffer (DWORD-aligned) and the OVERLAPPED are boxed and
        // outlive the request (`Drop` cancels it and waits for completion).
        let issued = unsafe {
            ReadDirectoryChangesW(
                directory.as_raw_handle(),
                buffer.as_mut_ptr().cast(),
                std::mem::size_of_val(&*buffer) as u32,
                1,
                FILTER,
                std::ptr::null_mut(),
                &mut *overlapped,
                None,
            )
        };
        if issued == 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        Ok(Self {
            directory,
            overlapped,
            buffer,
        })
    }

    /// No change has been reported below the root since this was armed.
    pub fn quiet(&self) -> bool {
        let mut bytes = 0;
        // SAFETY: non-blocking query of this handle's one pending request.
        let done = unsafe {
            GetOverlappedResult(
                self.directory.as_raw_handle(),
                &*self.overlapped,
                &mut bytes,
                0,
            )
        };
        // SAFETY: reads the calling thread's last error.
        done == 0 && unsafe { GetLastError() } == ERROR_IO_INCOMPLETE
    }
}

impl Drop for ChangeSentinel {
    fn drop(&mut self) {
        let handle = self.directory.as_raw_handle();
        let mut bytes = 0;
        // SAFETY: cancels then waits for this handle's request, so the kernel
        // no longer writes the buffer or the OVERLAPPED when they are freed.
        unsafe {
            CancelIoEx(handle, &*self.overlapped);
            GetOverlappedResult(handle, &*self.overlapped, &mut bytes, 1);
        }
        let _ = &self.buffer;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sentinel_stays_quiet_until_something_below_changes() {
        let root = std::env::temp_dir().join(format!(
            "layrs-sentinel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.txt"), b"one").unwrap();
        let sentinel = ChangeSentinel::arm(&root).unwrap();
        assert!(sentinel.quiet());
        // Reading changes nothing that is watched.
        assert_eq!(std::fs::read(root.join("a/b/f.txt")).unwrap(), b"one");
        assert!(sentinel.quiet());
        std::fs::write(root.join("a/b/f.txt"), b"two").unwrap();
        assert!(!sentinel.quiet());
        drop(sentinel);
        for change in [
            |root: &std::path::Path| std::fs::write(root.join("a/new.txt"), b"x").unwrap(),
            |root: &std::path::Path| std::fs::remove_file(root.join("a/b/f.txt")).unwrap(),
            |root: &std::path::Path| std::fs::create_dir(root.join("a/c")).unwrap(),
        ] {
            let sentinel = ChangeSentinel::arm(&root).unwrap();
            assert!(sentinel.quiet());
            change(&root);
            assert!(!sentinel.quiet());
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
