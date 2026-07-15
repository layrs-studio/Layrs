//! Stable store paths, durable directory helpers, and path identity checks.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::sqlite::SqliteOptions;

pub const DEFAULT_STORE_DIR: &str = ".layrs";
pub const DEFAULT_CAS_DIR: &str = "objects";
pub const DEFAULT_METADATA_DIR: &str = "metadata";
pub const DEFAULT_TMP_DIR: &str = "tmp";
pub const DEFAULT_SQLITE_FILE: &str = "layrs.sqlite3";
pub const DEFAULT_LOCK_FILE: &str = "layrs.lock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStoreConfig {
    pub root: PathBuf,
    pub cas_dir_name: String,
    pub metadata_dir_name: String,
    pub tmp_dir_name: String,
    pub sqlite_file_name: String,
    pub sqlite_wal: bool,
}

impl LocalStoreConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cas_dir_name: DEFAULT_CAS_DIR.to_string(),
            metadata_dir_name: DEFAULT_METADATA_DIR.to_string(),
            tmp_dir_name: DEFAULT_TMP_DIR.to_string(),
            sqlite_file_name: DEFAULT_SQLITE_FILE.to_string(),
            sqlite_wal: true,
        }
    }

    pub fn layout(&self) -> StoreLayout {
        let metadata_dir = self.root.join(&self.metadata_dir_name);
        StoreLayout {
            root: self.root.clone(),
            cas_dir: self.root.join(&self.cas_dir_name),
            metadata_dir: metadata_dir.clone(),
            tmp_dir: self.root.join(&self.tmp_dir_name),
            sqlite_path: metadata_dir.join(&self.sqlite_file_name),
            sqlite_wal_path: metadata_dir.join(format!("{}-wal", self.sqlite_file_name)),
        }
    }

    pub(crate) fn sqlite_options(&self) -> SqliteOptions {
        SqliteOptions {
            wal: self.sqlite_wal,
            busy_timeout: Duration::from_secs(5),
            wal_autocheckpoint_pages: 1_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreLayout {
    pub root: PathBuf,
    pub cas_dir: PathBuf,
    pub metadata_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub sqlite_path: PathBuf,
    pub sqlite_wal_path: PathBuf,
}

impl StoreLayout {
    pub fn lock_path(&self) -> PathBuf {
        self.metadata_dir.join(DEFAULT_LOCK_FILE)
    }

    pub fn repair_backup_dir(&self) -> PathBuf {
        self.metadata_dir.join("repair-backups")
    }
}

pub(crate) fn ensure_layout(layout: &StoreLayout) -> io::Result<()> {
    fs::create_dir_all(&layout.root)?;
    fs::create_dir_all(&layout.cas_dir)?;
    fs::create_dir_all(&layout.metadata_dir)?;
    fs::create_dir_all(&layout.tmp_dir)?;

    let lock_path = layout.lock_path();
    if !lock_path.exists() {
        let lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path);
        match lock {
            Ok(file) => file.sync_all()?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    sync_directory(&layout.metadata_dir)?;
    sync_directory(&layout.root)?;
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let result = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?
            .sync_all();
        match result {
            Ok(()) => Ok(()),
            // FlushFileBuffers does not support directory handles on Windows.
            // Durable object publication uses MoveFileExW WRITE_THROUGH below.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidInput
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(not(windows))]
    {
        std::fs::File::open(path)?.sync_all()
    }
}

pub(crate) fn durable_rename(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(
                existing_file_name: *const u16,
                new_file_name: *const u16,
                flags: u32,
            ) -> i32;
        }

        let source: Vec<u16> = source
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: both pointers refer to NUL-terminated UTF-16 buffers that
        // remain alive for the duration of the Win32 call.
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(not(windows))]
    {
        fs::rename(source, destination)
    }
}

pub(crate) fn create_dir_all_durable(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }

    let mut missing = Vec::new();
    let mut cursor = path;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory has no existing parent",
            )
        })?;
    }

    fs::create_dir_all(path)?;
    for directory in missing.iter().rev() {
        sync_directory(directory)?;
        if let Some(parent) = directory.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathIdentity {
    pub canonical_path: PathBuf,
    pub comparison_key: String,
    pub contains_reparse_point: bool,
}

impl PathIdentity {
    pub fn inspect(path: &Path, case_sensitive: bool) -> io::Result<Self> {
        let canonical_path = fs::canonicalize(path)?;
        Ok(Self {
            comparison_key: normalize_path_key(&canonical_path, case_sensitive),
            contains_reparse_point: contains_reparse_component(path)?,
            canonical_path,
        })
    }
}

pub fn normalize_path_key(path: &Path, case_sensitive: bool) -> String {
    let mut prefix = String::new();
    let mut rooted = false;
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Prefix(value) => {
                prefix = value.as_os_str().to_string_lossy().into_owned();
            }
            Component::RootDir => rooted = true,
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
        }
    }

    let mut key = prefix.replace('\\', "/");
    if rooted {
        key.push('/');
    }
    if !parts.is_empty() {
        if !key.is_empty() && !key.ends_with('/') {
            key.push('/');
        }
        key.push_str(&parts.join("/"));
    }
    if !case_sensitive {
        key = key.to_lowercase();
    }
    key
}

pub fn contains_reparse_component(path: &Path) -> io::Result<bool> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if current.exists() && path_has_reparse_point(&current)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn path_has_reparse_point(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        Ok(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }

    #[cfg(not(windows))]
    {
        Ok(metadata.file_type().is_symlink())
    }
}
