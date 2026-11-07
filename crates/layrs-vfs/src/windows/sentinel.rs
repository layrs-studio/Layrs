//! A change sentinel on a Room root: one directory-change request kept
//! pending on the whole tree. NTFS completes it during the first reported
//! change below the root (a name created, deleted or renamed; a size, write
//! time, creation time or attribute changed), or when its buffer overflows.
//! While it is still pending, no such change was reported since it was armed:
//! what git's fsmonitor relies on. Any completion, for any reason, only means
//! "look again".
//!
//! The answer is read by `settle`, never by peeking at the pending request:
//! it cancels the request and takes its completion packet from the
//! sentinel's own completion port. NTFS completes a pending request while it
//! reports the change, so a change reported before `settle` leaves it
//! completed with data (or an overflow); only a request that saw nothing ends
//! cancelled. Bound to a completion port, the request belongs to no thread (a
//! thread exiting does not cancel it), so that cancellation can only be
//! `settle`'s own.
//!
//! Limit, as for git's fsmonitor: NTFS reports a file's new size and write
//! time when the writer's handle is closed (or the change flushed), so a
//! writer still holding a modified file open is seen at its close; the
//! directory listing a full capture reads has the same lag.

use super::Session;
use crate::{Error, Result};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::{ERROR_OPERATION_ABORTED, GetLastError, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
    },
    System::IO::{CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED},
};

const FILTER: u32 = FILE_NOTIFY_CHANGE_FILE_NAME
    | FILE_NOTIFY_CHANGE_DIR_NAME
    | FILE_NOTIFY_CHANGE_ATTRIBUTES
    | FILE_NOTIFY_CHANGE_SIZE
    | FILE_NOTIFY_CHANGE_LAST_WRITE
    | FILE_NOTIFY_CHANGE_CREATION;

pub struct ChangeSentinel {
    directory: OwnedHandle,
    /// This sentinel's own completion port: it receives exactly the one
    /// request's completion.
    port: OwnedHandle,
    // Boxed: the kernel writes both while the request is pending; they are
    // freed only after it completed or was cancelled (`Drop`).
    overlapped: Box<OVERLAPPED>,
    buffer: Box<[u32; 1024]>,
    /// The request already finished (`settle`).
    settled: bool,
}

// The request is only read by `settle` and cancelled in `Drop`, both of which
// take its completion before the buffers can be freed; it is bound to no
// thread.
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
        // SAFETY: binds the new handle to a new port of its own.
        let port = unsafe {
            CreateIoCompletionPort(directory.as_raw_handle(), std::ptr::null_mut(), 0, 1)
        };
        if port.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: `port` is the one successful CreateIoCompletionPort result.
        let port = unsafe { OwnedHandle::from_raw_handle(port) };
        // SAFETY: an all-zero OVERLAPPED (completion goes to the port) is
        // valid.
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
            port,
            overlapped,
            buffer,
            settled: false,
        })
    }

    /// Cancels the request (if still pending) and takes its completion:
    /// whether it ended cancelled, having seen nothing.
    fn finish(&mut self) -> bool {
        let mut bytes = 0;
        let mut key = 0;
        let mut completed: *mut OVERLAPPED = std::ptr::null_mut();
        // SAFETY: cancels this handle's one request, then blocks until the
        // port hands over its completion (exactly one, this request's).
        let (done, error) = unsafe {
            CancelIoEx(self.directory.as_raw_handle(), &*self.overlapped);
            let done = GetQueuedCompletionStatus(
                self.port.as_raw_handle(),
                &mut bytes,
                &mut key,
                &mut completed,
                u32::MAX,
            );
            (done, GetLastError())
        };
        self.settled = true;
        let ours = std::ptr::eq(completed, &*self.overlapped);
        ours && done == 0 && error == ERROR_OPERATION_ABORTED
    }

    /// Ends the sentinel: whether no change was reported below the root
    /// between its arming and this call. Arm the next sentinel first, so that
    /// together they leave no gap.
    pub fn settle(mut self) -> bool {
        self.finish()
    }
}

impl Drop for ChangeSentinel {
    fn drop(&mut self) {
        if !self.settled {
            // The kernel no longer writes the buffer or the OVERLAPPED once
            // the completion was taken.
            self.finish();
        }
        let _ = &self.buffer;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sentinel_settles_quiet_only_when_nothing_below_changed() {
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
        // Nothing, then a read only: quiet.
        assert!(ChangeSentinel::arm(&root).unwrap().settle());
        let sentinel = ChangeSentinel::arm(&root).unwrap();
        assert_eq!(std::fs::read(root.join("a/b/f.txt")).unwrap(), b"one");
        assert!(sentinel.settle());
        for change in [
            |root: &std::path::Path| std::fs::write(root.join("a/b/f.txt"), b"two").unwrap(),
            |root: &std::path::Path| std::fs::write(root.join("a/new.txt"), b"x").unwrap(),
            |root: &std::path::Path| std::fs::remove_file(root.join("a/b/f.txt")).unwrap(),
            |root: &std::path::Path| std::fs::create_dir(root.join("a/c")).unwrap(),
        ] {
            let sentinel = ChangeSentinel::arm(&root).unwrap();
            change(&root);
            assert!(!sentinel.settle());
        }
        // A sentinel armed by a thread that exited still watches.
        let armed = std::thread::spawn({
            let root = root.clone();
            move || ChangeSentinel::arm(&root).unwrap()
        })
        .join()
        .unwrap();
        std::fs::write(root.join("a/late.txt"), b"x").unwrap();
        assert!(!armed.settle());
        let armed = std::thread::spawn({
            let root = root.clone();
            move || ChangeSentinel::arm(&root).unwrap()
        })
        .join()
        .unwrap();
        assert!(armed.settle());
        let _ = std::fs::remove_dir_all(&root);
    }
}
