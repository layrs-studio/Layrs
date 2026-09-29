//! Stable store paths, durable directory helpers, and path identity checks.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};

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

pub fn contains_reparse_component(path: &Path) -> io::Result<bool> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        // A Windows prefix (notably \\?\C:) is not a directory by itself.
        // Inspect the drive/UNC root after RootDir, then every descendant.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
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
