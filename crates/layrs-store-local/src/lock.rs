//! Process-local coordination layered over real cross-process file locks.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::error::{StoreError, StoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, Copy)]
enum LocalLockState {
    Shared(usize),
    Exclusive,
}

fn local_locks() -> &'static Mutex<HashMap<PathBuf, LocalLockState>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, LocalLockState>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug)]
pub(crate) struct StoreLock {
    file: File,
    key: PathBuf,
    mode: SessionMode,
}

impl StoreLock {
    pub fn acquire_existing(path: &Path, mode: SessionMode) -> StoreResult<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let key = canonical_lock_key(path)?;
        claim_local(&key, mode)?;

        let lock_result = match mode {
            SessionMode::Shared => fs2::FileExt::try_lock_shared(&file),
            SessionMode::Exclusive => fs2::FileExt::try_lock_exclusive(&file),
        };
        if let Err(error) = lock_result {
            release_local(&key, mode);
            return if lock_is_contended(&error) {
                Err(StoreError::LockBusy(path.to_path_buf()))
            } else {
                Err(StoreError::Io(error))
            };
        }

        Ok(Self { file, key, mode })
    }
}

fn lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        // LockFileEx can surface raw Win32 lock conflicts through fs2.
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
        release_local(&self.key, self.mode);
    }
}

fn canonical_lock_key(path: &Path) -> io::Result<PathBuf> {
    let canonical = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        Ok(PathBuf::from(
            canonical
                .to_string_lossy()
                .replace('\\', "/")
                .to_lowercase(),
        ))
    }
    #[cfg(not(windows))]
    {
        Ok(canonical)
    }
}

fn claim_local(key: &Path, mode: SessionMode) -> StoreResult<()> {
    let mut locks = local_locks()
        .lock()
        .map_err(|_| StoreError::InvalidState("process-local lock registry poisoned".into()))?;

    match (locks.get_mut(key), mode) {
        (None, SessionMode::Shared) => {
            locks.insert(key.to_path_buf(), LocalLockState::Shared(1));
            Ok(())
        }
        (Some(LocalLockState::Shared(count)), SessionMode::Shared) => {
            *count = count.checked_add(1).ok_or_else(|| {
                StoreError::InvalidState("process-local shared lock count overflow".into())
            })?;
            Ok(())
        }
        (None, SessionMode::Exclusive) => {
            locks.insert(key.to_path_buf(), LocalLockState::Exclusive);
            Ok(())
        }
        (Some(_), _) => Err(StoreError::ReentrantLock(key.to_path_buf())),
    }
}

fn release_local(key: &Path, mode: SessionMode) {
    let Ok(mut locks) = local_locks().lock() else {
        return;
    };
    match (locks.get_mut(key), mode) {
        (Some(LocalLockState::Shared(count)), SessionMode::Shared) if *count > 1 => {
            *count -= 1;
        }
        (Some(LocalLockState::Shared(_)), SessionMode::Shared)
        | (Some(LocalLockState::Exclusive), SessionMode::Exclusive) => {
            locks.remove(key);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn shared_locks_coexist_but_block_local_exclusive_lock() {
        let root = unique_test_dir();
        fs::create_dir_all(&root).expect("create root");
        let path = root.join("layrs.lock");
        File::create(&path).expect("create lock file");

        let first = StoreLock::acquire_existing(&path, SessionMode::Shared).expect("first lock");
        let second = StoreLock::acquire_existing(&path, SessionMode::Shared).expect("second lock");
        assert!(matches!(
            StoreLock::acquire_existing(&path, SessionMode::Exclusive),
            Err(StoreError::ReentrantLock(_))
        ));

        drop(first);
        drop(second);
        StoreLock::acquire_existing(&path, SessionMode::Exclusive).expect("exclusive after drop");
        fs::remove_dir_all(root).expect("remove root");
    }

    fn unique_test_dir() -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "layrs-store-local-lock-{}-{timestamp}",
            std::process::id()
        ))
    }
}
