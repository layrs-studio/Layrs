//! Pinned traversal. Files are never read, created, renamed or removed.
use super::{AuditError, AuditLimits, Result};
#[cfg(windows)]
mod windows;
use std::{
    ffi::OsStr,
    fs::{File, Metadata},
    path::{Component, Path, PathBuf},
};
pub(super) struct ObservedFile {
    pub relative_path: String,
    pub bytes: u64,
}

pub(super) fn scan(root: &Path, limits: AuditLimits) -> Result<Vec<ObservedFile>> {
    let directory = Directory::root(root)?;
    let mut entries = 0;
    let mut files = Vec::new();
    visit(
        &directory,
        Path::new(""),
        0,
        limits,
        &mut entries,
        &mut files,
    )?;
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(files)
}
fn visit(
    directory: &Directory,
    relative: &Path,
    depth: usize,
    limits: AuditLimits,
    entries: &mut usize,
    files: &mut Vec<ObservedFile>,
) -> Result<()> {
    if depth > limits.depth {
        return Err(AuditError("retention directory depth limit exceeded"));
    }
    for name in directory.names(limits.filesystem_entries.saturating_sub(*entries))? {
        *entries += 1;
        if *entries > limits.filesystem_entries {
            return Err(AuditError("retention filesystem entry limit exceeded"));
        }
        let name_text = name
            .to_str()
            .ok_or(AuditError("retention filename is not UTF-8"))?;
        if name_text.len() > 255
            || name_text.is_empty()
            || name_text.contains('\\')
            || name_text.chars().any(|c| c.is_control())
        {
            return Err(AuditError("retention filename is unsupported"));
        }
        let path = relative.join(&name);
        let text = path
            .to_str()
            .ok_or(AuditError("retention path is not UTF-8"))?
            .replace('\\', "/");
        if text.len() > 1024 {
            return Err(AuditError("retention relative path limit exceeded"));
        }
        let file = directory.child(&name)?;
        let metadata = file.metadata().map_err(filesystem)?;
        reject_link(&metadata)?;
        if metadata.is_dir() {
            let child = directory.wrap(file, &name);
            visit(&child, &path, depth + 1, limits, entries, files)?;
        } else if metadata.is_file() {
            if metadata.len() > limits.file_bytes {
                return Err(AuditError("retention file size limit exceeded"));
            }
            files.push(ObservedFile {
                relative_path: text,
                bytes: metadata.len(),
            });
        } else {
            return Err(AuditError("retention special filesystem entry refused"));
        }
    }
    Ok(())
}
fn reject_link(metadata: &Metadata) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(AuditError("retention reparse point refused"));
        }
    }
    if metadata.file_type().is_symlink() {
        return Err(AuditError("retention symbolic link refused"));
    }
    Ok(())
}
fn filesystem(_: std::io::Error) -> AuditError {
    AuditError("retention filesystem unavailable or changed during observation")
}
struct Directory {
    file: File,
    path: PathBuf,
}
impl Directory {
    fn root(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(AuditError("retention CAS path must be absolute"));
        }
        let mut anchor = PathBuf::new();
        let mut names = Vec::new();
        for part in path.components() {
            match part {
                Component::Prefix(prefix) => anchor.push(prefix.as_os_str()),
                Component::RootDir => anchor.push(std::path::MAIN_SEPARATOR.to_string()),
                Component::Normal(name) => names.push(name),
                _ => return Err(AuditError("retention CAS path must be lexical")),
            }
        }
        if names.len() > 64 {
            return Err(AuditError("retention CAS ancestor limit exceeded"));
        }
        let file = open_anchor(&anchor)?;
        let mut directory = Self { file, path: anchor };
        for name in names {
            let file = directory.child(name)?;
            let metadata = file.metadata().map_err(filesystem)?;
            reject_link(&metadata)?;
            if !metadata.is_dir() {
                return Err(AuditError("retention CAS ancestor is not a directory"));
            }
            directory.file = file;
            directory.path.push(name);
        }
        Ok(directory)
    }
    fn wrap(&self, file: File, name: &OsStr) -> Self {
        Self {
            file,
            path: self.path.join(name),
        }
    }
    #[cfg(target_os = "linux")]
    fn list_path(&self) -> PathBuf {
        use std::os::fd::AsRawFd;
        // This trusted kernel link resolves the pinned directory, not its old
        // pathname. All user-controlled entries are opened with O_NOFOLLOW.
        PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
    }
    #[cfg(target_os = "linux")]
    fn names(&self, limit: usize) -> Result<Vec<std::ffi::OsString>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(self.list_path()).map_err(filesystem)? {
            if names.len() >= limit {
                return Err(AuditError("retention filesystem entry limit exceeded"));
            }
            names.push(entry.map_err(filesystem)?.file_name());
        }
        Ok(names)
    }
    #[cfg(windows)]
    fn names(&self, limit: usize) -> Result<Vec<std::ffi::OsString>> {
        windows::names(&self.file, limit)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    fn names(&self, _: usize) -> Result<Vec<std::ffi::OsString>> {
        Err(AuditError(
            "retention audit supports Linux and Windows only",
        ))
    }
    #[cfg(target_os = "linux")]
    fn child(&self, name: &OsStr) -> Result<File> {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        };
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| AuditError("invalid retention path component"))?;
        // SAFETY: NUL-terminated single component, live parent fd; returned fd
        // is owned exactly once. O_PATH never opens special-file I/O streams.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(filesystem(std::io::Error::last_os_error()));
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(windows)]
    fn child(&self, name: &OsStr) -> Result<File> {
        windows::child(&self.file, name)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    fn child(&self, _: &OsStr) -> Result<File> {
        Err(AuditError(
            "retention audit supports Linux and Windows only",
        ))
    }
}
#[cfg(target_os = "linux")]
fn open_anchor(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(filesystem)
}
#[cfg(windows)]
fn open_windows(path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // Only the volume anchor uses an absolute path. Every later open and
    // enumeration is relative to a pinned kernel handle, never this pathname.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(7)
        .custom_flags(0x02000000 | 0x00200000)
        .open(path)
        .map_err(filesystem)
}
#[cfg(windows)]
fn open_anchor(path: &Path) -> Result<File> {
    open_windows(path)
}
#[cfg(not(any(target_os = "linux", windows)))]
fn open_anchor(_: &Path) -> Result<File> {
    Err(AuditError(
        "retention audit supports Linux and Windows only",
    ))
}

#[cfg(test)]
mod tests;
