//! Pinned, directory-relative filesystem operations used by CAS publication.

use std::ffi::OsStr;
use std::fs::{self, File, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};

mod directory_move;
mod leaf;
mod privacy;
pub(crate) use leaf::PinnedLeaf;
pub use privacy::private_spool_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    file: u64,
}

impl FileIdentity {
    #[cfg(unix)]
    fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            volume: metadata.dev(),
            file: metadata.ino(),
        })
    }
}

#[derive(Debug)]
pub(crate) struct PinnedDirectory {
    file: File,
    identity: FileIdentity,
    display_path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct PinnedFile {
    file: File,
    parent: File,
    parent_identity: FileIdentity,
    name: std::ffi::OsString,
    identity: FileIdentity,
}

impl PinnedDirectory {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let pinned = Self::open_components(path)?;
        pinned.verify_path_identity()?;
        Ok(pinned)
    }

    // Used by both initial opening and revalidation: opening only the last
    // component would follow an intermediate Windows junction before testing it.
    fn open_components(path: &Path) -> io::Result<Self> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        let mut components = absolute.components();
        let mut anchor = PathBuf::new();
        if let Some(Component::Prefix(prefix)) = components.clone().next() {
            anchor.push(prefix.as_os_str());
            components.next();
        }
        if matches!(components.clone().next(), Some(Component::RootDir)) {
            anchor.push(std::path::MAIN_SEPARATOR.to_string());
            components.next();
        }
        if anchor.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "pinned directory path has no filesystem root: {}",
                    path.display()
                ),
            ));
        }

        let file = open_directory_path(&anchor)?;
        let metadata = file.metadata()?;
        validate_directory_metadata(&anchor, &metadata)?;
        let mut pinned = Self {
            identity: file_identity(&file)?,
            file,
            display_path: anchor,
        };
        for component in components {
            match component {
                Component::CurDir => {}
                Component::Normal(name) => pinned = pinned.open_child_for_traversal(name)?,
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("pinned directory path is not lexical: {}", path.display()),
                    ));
                }
            }
        }
        Ok(pinned)
    }

    pub(crate) fn open_child(&self, name: &OsStr) -> io::Result<Self> {
        self.open_child_with_access(name, true)
    }

    pub(crate) fn open_child_for_traversal(&self, name: &OsStr) -> io::Result<Self> {
        self.open_child_with_access(name, false)
    }

    fn open_child_with_access(&self, name: &OsStr, writable: bool) -> io::Result<Self> {
        validate_name(name)?;
        let display_path = self.display_path.join(name);
        let file =
            platform::open_directory_at(&self.file, name, false, writable).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("open pinned directory {}: {error}", display_path.display()),
                )
            })?;
        let metadata = file.metadata()?;
        validate_directory_metadata(&display_path, &metadata)?;
        Ok(Self {
            identity: file_identity(&file)?,
            file,
            display_path,
        })
    }

    pub(crate) fn ensure_child(&self, name: &OsStr) -> io::Result<Self> {
        validate_name(name)?;
        let file = platform::open_directory_at(&self.file, name, true, true)?;
        let metadata = file.metadata()?;
        let display_path = self.display_path.join(name);
        validate_directory_metadata(&display_path, &metadata)?;
        self.sync()?;
        let child = Self {
            identity: file_identity(&file)?,
            file,
            display_path,
        };
        child.sync()?;
        Ok(child)
    }

    pub(crate) fn create_file(&self, name: &OsStr) -> io::Result<PinnedFile> {
        validate_name(name)?;
        let file = platform::create_file_at(&self.file, name)?;
        self.wrap_file(name, file)
    }

    pub(crate) fn open_file(&self, name: &OsStr) -> io::Result<PinnedFile> {
        validate_name(name)?;
        let file = platform::open_file_at(&self.file, name)?;
        self.wrap_file(name, file)
    }

    pub(crate) fn unlink_file(&self, name: &OsStr) -> io::Result<()> {
        validate_name(name)?;
        platform::unlink_name_at(&self.file, name)
    }

    /// Remove one empty directory through its pinned parent. Never recursive,
    /// never follows a symlink/reparse point and never deletes a regular file.
    pub(crate) fn remove_empty_child(&self, name: &OsStr) -> io::Result<()> {
        validate_name(name)?;
        self.verify_path_identity()?;
        platform::remove_empty_directory_at(&self.file, name)?;
        self.sync()
    }

    pub(crate) fn existing_parent(
        &self,
        relative: &Path,
    ) -> io::Result<(Self, std::ffi::OsString)> {
        let mut components = relative.components().peekable();
        let mut directory = self.try_clone()?;
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "noncanonical relative path",
                ));
            };
            if components.peek().is_none() {
                directory.verify_path_identity()?;
                return Ok((directory, name.to_os_string()));
            }
            directory = directory.open_child_for_traversal(name)?;
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path has no leaf",
        ))
    }

    pub(crate) fn open_relative_file(&self, relative: &Path) -> io::Result<PinnedFile> {
        let mut components = relative.components().peekable();
        let mut directory = self.try_clone()?;
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "file path contains a non-normal component",
                ));
            };
            if components.peek().is_none() {
                return directory.open_file(name);
            }
            directory = directory.open_child_for_traversal(name)?;
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file path has no file name",
        ))
    }

    fn wrap_file(&self, name: &OsStr, file: File) -> io::Result<PinnedFile> {
        let metadata = file.metadata()?;
        validate_file_metadata(&self.display_path.join(name), &metadata)?;
        Ok(PinnedFile {
            identity: file_identity(&file)?,
            file,
            parent: self.file.try_clone()?,
            parent_identity: self.identity,
            name: name.to_os_string(),
        })
    }

    pub(crate) fn sync(&self) -> io::Result<()> {
        sync_directory_handle(&self.file)
    }

    pub(crate) fn identity_token(&self) -> String {
        format!(
            "fs:v1:{:016x}:{:016x}",
            self.identity.volume, self.identity.file
        )
    }

    pub(crate) fn verify_path_identity(&self) -> io::Result<()> {
        let reopened = Self::open_components(&self.display_path)?;
        if reopened.identity != self.identity {
            return Err(identity_changed(&self.display_path));
        }
        Ok(())
    }

    pub(crate) fn components_to_parent(
        &self,
        relative: &Path,
    ) -> io::Result<(Self, std::ffi::OsString)> {
        let mut components = relative.components().peekable();
        let mut directory = self.try_clone()?;
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "CAS path contains a non-normal component",
                ));
            };
            if components.peek().is_none() {
                return Ok((directory, name.to_os_string()));
            }
            directory = directory.ensure_child(name)?;
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CAS target has no file name",
        ))
    }

    fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            identity: self.identity,
            display_path: self.display_path.clone(),
        })
    }
}

impl PinnedFile {
    pub(crate) fn file(&self) -> &File {
        &self.file
    }

    pub(crate) fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub(crate) fn same_file(&self, other: &Self) -> bool {
        self.identity == other.identity
    }

    pub(crate) fn verify_parent(&self) -> io::Result<()> {
        if file_identity(&self.parent)? != self.parent_identity {
            return Err(io::Error::other("pinned parent directory identity changed"));
        }
        Ok(())
    }

    #[cfg(unix)]
    pub(crate) fn hard_link_to(
        &self,
        destination: &PinnedDirectory,
        name: &OsStr,
    ) -> io::Result<bool> {
        validate_name(name)?;
        platform::hard_link_at(self, &destination.file, name)
    }

    #[cfg(any(windows, unix))]
    pub(crate) fn rename_to(
        &self,
        destination: &PinnedDirectory,
        name: &OsStr,
    ) -> io::Result<bool> {
        validate_name(name)?;
        platform::rename_at(self, &destination.file, name)
    }

    pub(crate) fn unlink(&self) -> io::Result<()> {
        platform::unlink_file(self)
    }

    pub(crate) fn remove_replacement(&self) -> io::Result<()> {
        let replacement = match platform::open_file_at(&self.parent, &self.name) {
            Ok(file) => PinnedFile {
                identity: file_identity(&file)?,
                file,
                parent: self.parent.try_clone()?,
                parent_identity: self.parent_identity,
                name: self.name.clone(),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if replacement.identity != self.identity {
            replacement.unlink()?;
            replacement.sync_parent()?;
        }
        Ok(())
    }

    pub(crate) fn sync_parent(&self) -> io::Result<()> {
        sync_directory_handle(&self.parent)
    }
}

fn validate_name(name: &OsStr) -> io::Result<()> {
    let mut components = Path::new(name).components();
    if matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("expected one normal path component, got {name:?}"),
        ))
    }
}

fn validate_directory_metadata(path: &Path, metadata: &Metadata) -> io::Result<()> {
    if metadata.file_type().is_symlink() || metadata_has_reparse_point(metadata) {
        return Err(reparse_error(path));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected directory at {}", path.display()),
        ));
    }
    Ok(())
}

fn validate_file_metadata(path: &Path, metadata: &Metadata) -> io::Result<()> {
    if metadata.file_type().is_symlink() || metadata_has_reparse_point(metadata) {
        return Err(reparse_error(path));
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected regular file at {}", path.display()),
        ));
    }
    Ok(())
}

fn metadata_has_reparse_point(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x0000_0400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

fn reparse_error(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "pinned path is a symlink or reparse point: {}",
            path.display()
        ),
    )
}

fn identity_changed(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("pinned path identity changed: {}", path.display()),
    )
}

fn file_identity(file: &File) -> io::Result<FileIdentity> {
    #[cfg(unix)]
    {
        FileIdentity::from_metadata(&file.metadata()?)
    }
    #[cfg(windows)]
    {
        platform::file_identity(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pinned filesystem identities are unsupported on this platform",
        ))
    }
}

fn open_directory_path(path: &Path) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.read(true);
        options.custom_flags(platform::DIRECTORY_OPEN_FLAGS);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(0x0000_0020 | 0x0000_0080 | 0x0010_0000)
            .share_mode(0x0000_0007)
            .custom_flags(0x0200_0000 | 0x0020_0000);
    }
    #[cfg(not(any(unix, windows)))]
    {
        options.read(true);
    }
    options.open(path)
}

fn sync_directory_handle(file: &File) -> io::Result<()> {
    match file.sync_all() {
        Ok(()) => Ok(()),
        #[cfg(windows)]
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

#[cfg(unix)]
#[path = "secure_fs/unix.rs"]
mod platform;

#[cfg(windows)]
#[path = "secure_fs/windows.rs"]
mod platform;
