//! Stable store paths, durable directory helpers, and path identity checks.

use std::fs::{self, File, OpenOptions};
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

    pub(crate) fn validated_layout(&self) -> io::Result<StoreLayout> {
        validate_component_name("CAS directory", &self.cas_dir_name)?;
        validate_component_name("metadata directory", &self.metadata_dir_name)?;
        validate_component_name("temporary directory", &self.tmp_dir_name)?;
        validate_component_name("SQLite file", &self.sqlite_file_name)?;

        let layout = self.layout();
        validate_layout_containment(&layout)?;
        validate_store_boundaries(&layout)?;
        Ok(layout)
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
    validate_layout_containment(layout)?;
    create_dir_all_durable(&layout.root)?;
    create_dir_all_durable(&layout.cas_dir)?;
    create_dir_all_durable(&layout.metadata_dir)?;
    create_dir_all_durable(&layout.tmp_dir)?;
    validate_store_boundaries(layout)?;

    let lock_path = layout.lock_path();
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) => validate_regular_non_reparse(&lock_path, &metadata)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let lock = create_new_file_no_follow(&lock_path);
            match lock {
                Ok(file) => file.sync_all()?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(&lock_path)?;
                    validate_regular_non_reparse(&lock_path, &metadata)?;
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    }
    sync_directory(&layout.metadata_dir)?;
    sync_directory(&layout.root)?;
    Ok(())
}

fn validate_component_name(label: &str, value: &str) -> io::Result<()> {
    let mut components = Path::new(value).components();
    let valid = matches!(components.next(), Some(Component::Normal(component)) if !component.is_empty())
        && components.next().is_none();
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} must be exactly one normal path component: {value:?}"),
        ));
    }
    Ok(())
}

fn validate_layout_containment(layout: &StoreLayout) -> io::Result<()> {
    if layout.root.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "store root must not be empty",
        ));
    }
    for (label, path) in [
        ("CAS directory", &layout.cas_dir),
        ("metadata directory", &layout.metadata_dir),
        ("temporary directory", &layout.tmp_dir),
    ] {
        if path.parent() != Some(layout.root.as_path()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{label} escapes the store root: {}", path.display()),
            ));
        }
    }
    if layout.sqlite_path.parent() != Some(layout.metadata_dir.as_path())
        || layout.sqlite_wal_path.parent() != Some(layout.metadata_dir.as_path())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SQLite paths escape the metadata directory",
        ));
    }
    Ok(())
}

pub(crate) fn validate_store_boundaries(layout: &StoreLayout) -> io::Result<()> {
    let lock_path = layout.lock_path();
    let sqlite_shm_path = sidecar_path(&layout.sqlite_path, "-shm");
    for path in [
        layout.root.as_path(),
        layout.cas_dir.as_path(),
        layout.metadata_dir.as_path(),
        layout.tmp_dir.as_path(),
        layout.sqlite_path.as_path(),
        layout.sqlite_wal_path.as_path(),
        sqlite_shm_path.as_path(),
        lock_path.as_path(),
    ] {
        reject_reparse_components(path)?;
    }

    if layout.root.exists() {
        let canonical_root = fs::canonicalize(&layout.root)?;
        for path in [&layout.cas_dir, &layout.metadata_dir, &layout.tmp_dir] {
            if path.exists() && !fs::canonicalize(path)?.starts_with(&canonical_root) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("store boundary resolves outside root: {}", path.display()),
                ));
            }
        }
    }
    Ok(())
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    reject_reparse_components(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        let result = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
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

pub(crate) fn create_dir_all_durable(path: &Path) -> io::Result<()> {
    let path = absolute_lexical(path)?;
    let mut missing = Vec::new();
    let mut cursor = path.as_path();
    loop {
        match fs::symlink_metadata(cursor) {
            Ok(metadata) => {
                validate_directory_non_reparse(cursor, &metadata)?;
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(cursor.to_path_buf());
                cursor = cursor.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "directory has no existing parent",
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }

    for directory in missing.iter().rev() {
        let parent = directory.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "directory has no parent")
        })?;
        let parent_metadata = fs::symlink_metadata(parent)?;
        validate_directory_non_reparse(parent, &parent_metadata)?;
        match fs::create_dir(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = fs::symlink_metadata(directory)?;
        validate_directory_non_reparse(directory, &metadata)?;
        sync_directory(directory)?;
        sync_directory(parent)?;
    }
    Ok(())
}

fn absolute_lexical(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("path escapes its filesystem root: {}", path.display()),
                    ));
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

pub(crate) fn open_read_no_follow(path: &Path) -> io::Result<File> {
    reject_reparse_components(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options.open(path)?;
    let metadata = fs::symlink_metadata(path)?;
    validate_regular_non_reparse(path, &metadata)?;
    Ok(file)
}

pub(crate) fn open_read_write_no_follow(path: &Path) -> io::Result<File> {
    reject_reparse_components(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    set_no_follow(&mut options);
    let file = options.open(path)?;
    let metadata = fs::symlink_metadata(path)?;
    validate_regular_non_reparse(path, &metadata)?;
    Ok(file)
}

pub(crate) fn create_new_file_no_follow(path: &Path) -> io::Result<File> {
    reject_reparse_components(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_no_follow(&mut options);
    options.open(path)
}

fn set_no_follow(options: &mut OpenOptions) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const O_NOFOLLOW: i32 = 0o400_000;
        options.custom_flags(O_NOFOLLOW);
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const O_NOFOLLOW: i32 = 0x100;
        options.custom_flags(O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
}

fn validate_directory_non_reparse(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    if metadata_has_reparse_point(metadata) {
        return reparse_error(path);
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("expected directory at {}", path.display()),
        ));
    }
    Ok(())
}

fn validate_regular_non_reparse(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    if metadata_has_reparse_point(metadata) {
        return reparse_error(path);
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected regular file at {}", path.display()),
        ));
    }
    Ok(())
}

fn reparse_error<T>(path: &Path) -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "store path crosses a symlink or reparse point: {}",
            path.display()
        ),
    ))
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
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata_has_reparse_point(&metadata) => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub(crate) fn reject_reparse_components(path: &Path) -> io::Result<()> {
    if contains_reparse_component(path)? {
        reparse_error(path)
    } else {
        Ok(())
    }
}

pub fn path_has_reparse_point(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;

    Ok(metadata_has_reparse_point(&metadata))
}

fn metadata_has_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
