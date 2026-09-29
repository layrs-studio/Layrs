//! Checkout leaves. A Linux symlink is opened itself, never its target.
use super::*;

pub(crate) enum PinnedLeaf {
    File(PinnedFile),
    #[cfg(target_os = "linux")]
    Symlink(PinnedSymlink),
}

impl PinnedDirectory {
    pub(crate) fn open_leaf(&self, name: &OsStr) -> io::Result<PinnedLeaf> {
        validate_name(name)?;
        #[cfg(target_os = "linux")]
        {
            let file = linux::open_link(&self.file, name)?;
            let metadata = file.metadata()?;
            if metadata.file_type().is_symlink() {
                return Ok(PinnedLeaf::Symlink(PinnedSymlink {
                    identity: file_identity(&file)?,
                    file,
                    parent: self.file.try_clone()?,
                    name: name.to_owned(),
                }));
            }
            if !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::IsADirectory,
                    "checkout leaf is not a regular file or symlink",
                ));
            }
            let regular = self.open_file(name)?;
            if regular.identity != file_identity(&file)? {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "checkout leaf changed while opening",
                ));
            }
            Ok(PinnedLeaf::File(regular))
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.open_file(name).map(PinnedLeaf::File)
        }
    }

    pub(crate) fn create_symlink(&self, name: &OsStr, target: &str) -> io::Result<PinnedLeaf> {
        validate_name(name)?;
        #[cfg(target_os = "linux")]
        {
            linux::create_link(&self.file, name, target)?;
            self.sync()?;
            let leaf = self.open_leaf(name)?;
            if leaf.link_target()?.as_deref() != Some(target) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "staged symlink changed",
                ));
            }
            Ok(leaf)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = target;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "symlink materialization requires Linux pinned-link primitives",
            ))
        }
    }
}
impl PinnedLeaf {
    pub(crate) fn link_target(&self) -> io::Result<Option<String>> {
        match self {
            Self::File(_) => Ok(None),
            #[cfg(target_os = "linux")]
            Self::Symlink(link) => linux::target(&link.file).map(Some),
        }
    }
    pub(crate) fn regular_file(&self) -> Option<&PinnedFile> {
        match self {
            Self::File(file) => Some(file),
            #[cfg(target_os = "linux")]
            Self::Symlink(_) => None,
        }
    }
    pub(crate) fn rename_to(
        &self,
        destination: &PinnedDirectory,
        name: &OsStr,
    ) -> io::Result<bool> {
        validate_name(name)?;
        match self {
            Self::File(file) => file.rename_to(destination, name),
            #[cfg(target_os = "linux")]
            Self::Symlink(link) => linux::rename(link, &destination.file, name),
        }
    }
    pub(crate) fn sync_parent(&self) -> io::Result<()> {
        match self {
            Self::File(file) => file.sync_parent(),
            #[cfg(target_os = "linux")]
            Self::Symlink(link) => sync_directory_handle(&link.parent),
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct PinnedSymlink {
    file: File,
    parent: File,
    name: std::ffi::OsString,
    identity: FileIdentity,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        ffi::{CString, c_char, c_int},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    unsafe extern "C" {
        fn openat(dir: c_int, name: *const c_char, flags: c_int, ...) -> c_int;
        fn readlinkat(dir: c_int, name: *const c_char, bytes: *mut c_char, size: usize) -> isize;
        fn symlinkat(target: *const c_char, dir: c_int, name: *const c_char) -> c_int;
        fn renameat2(
            old_dir: c_int,
            old_name: *const c_char,
            new_dir: c_int,
            new_name: *const c_char,
            flags: u32,
        ) -> c_int;
    }
    fn cstring(bytes: &[u8]) -> io::Result<CString> {
        CString::new(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "symlink contains NUL"))
    }
    pub(super) fn open_link(parent: &File, name: &OsStr) -> io::Result<File> {
        let name = cstring(name.as_bytes())?;
        // O_PATH | O_NOFOLLOW | O_CLOEXEC pins the directory entry itself.
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                0o10_000_000 | 0o400_000 | 0o2_000_000,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
    pub(super) fn target(file: &File) -> io::Result<String> {
        let mut bytes = vec![0u8; 4096];
        // Linux AT_EMPTY_PATH behavior reads the link pinned by its O_PATH fd.
        let n = unsafe {
            readlinkat(
                file.as_raw_fd(),
                c"".as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n as usize >= bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "symlink target exceeds 4095 bytes",
            ));
        }
        bytes.truncate(n as usize);
        String::from_utf8(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "symlink target is not UTF-8"))
    }
    pub(super) fn create_link(parent: &File, name: &OsStr, target: &str) -> io::Result<()> {
        let name = cstring(name.as_bytes())?;
        let target = cstring(target.as_bytes())?;
        if unsafe { symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr()) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    pub(super) fn rename(
        link: &PinnedSymlink,
        destination: &File,
        name: &OsStr,
    ) -> io::Result<bool> {
        let current = open_link(&link.parent, &link.name)?;
        if file_identity(&current)? != link.identity {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "symlink changed before rename",
            ));
        }
        let old = cstring(link.name.as_bytes())?;
        let new = cstring(name.as_bytes())?;
        if unsafe {
            renameat2(
                link.parent.as_raw_fd(),
                old.as_ptr(),
                destination.as_raw_fd(),
                new.as_ptr(),
                1,
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::AlreadyExists {
                Ok(true)
            } else {
                Err(error)
            };
        }
        let moved = open_link(destination, name)?;
        if file_identity(&moved)? != link.identity {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "symlink changed during rename; moved entry retained",
            ));
        }
        Ok(false)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn links_are_pinned_and_exclusive_rename_never_replaces_a_collision() {
        let root = std::env::temp_dir().join(format!(
            "layrs-link-primitive-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let directory = PinnedDirectory::open(&root).unwrap();
        let link = directory
            .create_symlink(OsStr::new("source"), "/outside/never-opened")
            .unwrap();
        fs::write(root.join("occupied"), b"untouched").unwrap();
        assert!(link.rename_to(&directory, OsStr::new("occupied")).unwrap());
        assert_eq!(fs::read(root.join("occupied")).unwrap(), b"untouched");
        fs::remove_file(root.join("source")).unwrap();
        std::os::unix::fs::symlink("replacement", root.join("source")).unwrap();
        assert!(
            link.rename_to(&directory, OsStr::new("destination"))
                .is_err()
        );
        assert_eq!(
            link.link_target().unwrap().as_deref(),
            Some("/outside/never-opened")
        );
        assert_eq!(
            fs::read_link(root.join("source")).unwrap(),
            PathBuf::from("replacement")
        );
        assert!(!root.join("destination").exists());
        assert!(
            directory
                .open_child_for_traversal(OsStr::new("source"))
                .is_err()
        );
        fs::remove_file(root.join("source")).unwrap();
        fs::remove_file(root.join("occupied")).unwrap();
        fs::remove_dir(&root).unwrap();
    }
}
