//! One nonblocking capture lane per authenticated allocation receipt.
//!
//! The lock stays with the caller through checkpoint, hashing, installation,
//! accepted commit and acknowledgement. It creates no filesystem artifact and
//! never writes the receipt. It does not replace the usual family and private
//! validation at each durable boundary.
use super::{
    artifacts::Artifacts,
    records::Record,
    recovery::error::{ProviderBootstrapCause, invalid},
};
use crate::projection::{file, shared::provider_access::FamilyAccess};
use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::windows::io::AsRawHandle,
    sync::{Mutex, OnceLock},
};
use windows_sys::Win32::Storage::FileSystem::{LockFile, UnlockFile};

/// This lane is outside the receipt's bytes, at offset 2^32. LockFile does
/// not wait and Windows permits locking beyond EOF.
const LOCK_OFFSET_LOW: u32 = 0;
const LOCK_OFFSET_HIGH: u32 = 1;
const LOCK_LENGTH_LOW: u32 = 1;
const LOCK_LENGTH_HIGH: u32 = 0;

#[derive(Debug)]
pub(super) enum CaptureAttemptLockError {
    Contended,
    Cause(ProviderBootstrapCause),
}

impl From<ProviderBootstrapCause> for CaptureAttemptLockError {
    fn from(cause: ProviderBootstrapCause) -> Self {
        Self::Cause(cause)
    }
}

fn claimed() -> &'static Mutex<HashSet<String>> {
    static CLAIMED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CLAIMED.get_or_init(|| Mutex::new(HashSet::new()))
}

struct LocalClaim(String);

fn claim(identity: String) -> Option<LocalClaim> {
    // Poisoning records a previous panic, not a successful OS unlock. The
    // registry still contains every live claim, so preserve its exclusions.
    let mut claims = claimed().lock().unwrap_or_else(|e| e.into_inner());
    if claims.insert(identity.clone()) {
        Some(LocalClaim(identity))
    } else {
        None
    }
}

impl Drop for LocalClaim {
    fn drop(&mut self) {
        claimed()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// RAII lane guard. The native lock is on a clone of the exact retained A
/// handle, so replacement of the pathname cannot move this lock elsewhere.
pub(super) struct CaptureAttemptLock {
    file: File,
    identity: String,
    bytes: Vec<u8>,
    _local: LocalClaim,
}

impl CaptureAttemptLock {
    pub(super) fn acquire(
        artifacts: &Artifacts,
        family: &FamilyAccess,
    ) -> Result<Self, CaptureAttemptLockError> {
        artifacts.verify(family)?;
        let (file, identity, bytes) = artifacts.capture_lock_anchor()?;
        verify_handle(&file, &identity, &bytes)?;
        let local = claim(identity.clone()).ok_or(CaptureAttemptLockError::Contended)?;
        // SAFETY: file is a live readable Windows handle, the byte range is
        // fixed and nonempty, and LockFile synchronously reports contention.
        if unsafe {
            LockFile(
                file.as_raw_handle(),
                LOCK_OFFSET_LOW,
                LOCK_OFFSET_HIGH,
                LOCK_LENGTH_LOW,
                LOCK_LENGTH_HIGH,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            return Err(match error.raw_os_error() {
                Some(32 | 33) => CaptureAttemptLockError::Contended,
                _ => CaptureAttemptLockError::Cause(error.into()),
            });
        }
        let guard = Self {
            file,
            identity,
            bytes,
            _local: local,
        };
        // This check is after the native lock and before granting the caller
        // the lane. A failure drops the guard and releases both exclusions.
        guard.verify(artifacts, family)?;
        Ok(guard)
    }

    /// Call before and after each durable capture phase, while holding the
    /// relevant family/private gates. A changed A or family fails closed.
    pub(super) fn verify(
        &self,
        artifacts: &Artifacts,
        family: &FamilyAccess,
    ) -> Result<(), ProviderBootstrapCause> {
        artifacts.verify(family)?;
        verify_handle(&self.file, &self.identity, &self.bytes)?;
        if self.identity != artifacts.allocation.receipt_file_identity
            || self.bytes != artifacts.allocation.encode()?
        {
            return Err(invalid("capture lane allocation binding changed"));
        }
        Ok(())
    }
}

impl Drop for CaptureAttemptLock {
    fn drop(&mut self) {
        // UnlockFile is synchronous. If it fails, closing `file` releases the
        // region by OS rule; any delayed release remains protected by the next
        // caller's mandatory LockFile attempt. Fields then drop in declaration
        // order, closing the handle before clearing the process registry.
        // SAFETY: this is the same live handle and exact range locked above.
        let _ = unsafe {
            UnlockFile(
                self.file.as_raw_handle(),
                LOCK_OFFSET_LOW,
                LOCK_OFFSET_HIGH,
                LOCK_LENGTH_LOW,
                LOCK_LENGTH_HIGH,
            )
        };
    }
}

fn verify_handle(file: &File, identity: &str, bytes: &[u8]) -> Result<(), ProviderBootstrapCause> {
    if file::identity(file)? != identity || file.metadata()?.len() != bytes.len() as u64 {
        return Err(invalid(
            "capture lane allocation identity or length changed",
        ));
    }
    let mut view = file.try_clone()?;
    view.seek(SeekFrom::Start(0))?;
    let mut actual = Vec::new();
    view.take(bytes.len() as u64 + 1).read_to_end(&mut actual)?;
    if actual != bytes {
        return Err(invalid("capture lane allocation bytes changed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_registry_excludes_same_native_identity_and_releases_on_drop() {
        let identity = "capture-lock-registry-isolated-1".to_owned();
        let first = claim(identity.clone()).expect("first local lane");
        assert!(claim(identity.clone()).is_none());
        assert!(claim("capture-lock-registry-isolated-2".to_owned()).is_some());
        drop(first);
        assert!(claim(identity).is_some());
    }
}
