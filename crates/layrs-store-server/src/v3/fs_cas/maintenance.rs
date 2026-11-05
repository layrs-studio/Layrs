//! Cooperative process-lifetime exclusion for filesystem CAS users.
//! The lock file is permanent: never unlink/replace it during maintenance.
use crate::v3::{V3StoreError, V3StoreResult};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub const LOCK_FILE: &str = ".layrs-cas-maintenance.lock";

/// Exclusive fence against every FsServerCas opened by this implementation.
/// This does not prove that older binaries or external filesystem writers have
/// stopped, and does not by itself authorize deletion or retention changes.
pub struct FsCasMaintenance {
    _file: File,
}
impl FsCasMaintenance {
    /// Refuses immediately if a reader/writer or another maintainer is active.
    /// Requires an existing root; acquisition never creates a CAS directory.
    pub fn try_acquire(root: &Path) -> V3StoreResult<Self> {
        let file = open(root)?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
            refused("CAS maintenance unavailable: active CAS user or unsupported locking")
        })?;
        Ok(Self { _file: file })
    }
}

pub(super) fn shared(root: &Path) -> V3StoreResult<File> {
    let file = open(root)?;
    fs2::FileExt::try_lock_shared(&file)
        .map_err(|_| refused("CAS unavailable: exclusive maintenance or unsupported locking"))?;
    Ok(file)
}

fn refused(message: &str) -> V3StoreError {
    V3StoreError::Backend(message.into())
}

fn open(root: &Path) -> V3StoreResult<File> {
    // CAS paths are operator-owned. Refuse lexical aliases and links/reparse
    // components before opening the stable lock inode; arbitrary external
    // rename/unlink of these private directories remains outside this fence.
    let root = std::path::absolute(root)?;
    let mut ancestor = std::path::PathBuf::new();
    for component in root.components() {
        match component {
            std::path::Component::ParentDir | std::path::Component::CurDir => {
                return Err(refused("CAS maintenance root must be lexical"));
            }
            _ => ancestor.push(component),
        }
        let metadata = std::fs::symlink_metadata(&ancestor)?;
        reject_links(&metadata)?;
        if !metadata.is_dir() {
            return Err(refused("CAS maintenance ancestor is not a directory"));
        }
    }
    let path = root.join(LOCK_FILE);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    return Err(refused(
        "CAS maintenance locking supports Linux and Windows only",
    ));
    let file = options.open(&path)?;
    let metadata = file.metadata()?;
    reject_links(&metadata)?;
    if !metadata.is_file() || metadata.len() != 0 {
        return Err(refused(
            "CAS maintenance lock must be an empty regular file",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let path_metadata = std::fs::symlink_metadata(path)?;
        if path_metadata.dev() != metadata.dev()
            || path_metadata.ino() != metadata.ino()
            || metadata.nlink() != 1
        {
            return Err(refused(
                "CAS maintenance lock identity changed or is hardlinked",
            ));
        }
    }
    Ok(file)
}

fn reject_links(metadata: &std::fs::Metadata) -> V3StoreResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(refused("CAS maintenance reparse point refused"));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(refused("CAS maintenance symbolic link refused"));
    }
    Ok(())
}
