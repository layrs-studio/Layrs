//! Linux operations anchored to pinned directory descriptors; no shell paths.
use super::types::*;
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};

pub struct Directory(File);
fn name(value: &str) -> GcResult<CString> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > 255
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err("invalid GC path component".into());
    }
    Ok(CString::new(value)?)
}
fn opened(fd: i32) -> GcResult<File> {
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fresh descriptor returned by open/openat, owned exactly once.
    Ok(unsafe { File::from_raw_fd(fd) })
}
impl Directory {
    pub fn root(path: &Path) -> GcResult<Self> {
        if !path.is_absolute() {
            return Err("GC root must be absolute".into());
        }
        let anchor = CString::new("/")?;
        // SAFETY: static NUL-terminated root, no pointer retained.
        let file = opened(unsafe {
            libc::open(
                anchor.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        })?;
        let mut root = Self(file);
        let mut depth = 0;
        for component in path.components() {
            match component {
                Component::RootDir => (),
                Component::Normal(value) => {
                    depth += 1;
                    if depth > 64 {
                        return Err("GC root depth exceeded".into());
                    }
                    root = root.child_inner(
                        value.to_str().ok_or("GC path must be UTF-8")?,
                        false,
                        false,
                    )?;
                }
                _ => return Err("GC root must be lexical".into()),
            }
        }
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: valid descriptor and writable statfs buffer; checked return.
        if unsafe { libc::fstatfs(root.0.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let kind = unsafe { filesystem.assume_init() }.f_type as u64;
        if !matches!(
            kind,
            0xef53 | 0x58465342 | 0x9123683e | 0x01021994 | 0x794c7630
        ) {
            return Err("GC supports local ext4/XFS/Btrfs/tmpfs/overlay filesystems only".into());
        }
        Ok(root)
    }
    pub fn identity(&self) -> GcResult<(u64, u64)> {
        let metadata = self.0.metadata()?;
        Ok((metadata.dev(), metadata.ino()))
    }
    pub fn child(&self, value: &str, create: bool) -> GcResult<Self> {
        self.child_inner(value, create, true)
    }
    fn child_inner(&self, value: &str, create: bool, same_device: bool) -> GcResult<Self> {
        let component = name(value)?;
        if create {
            // SAFETY: live parent descriptor and a single validated component.
            let result = unsafe { libc::mkdirat(self.0.as_raw_fd(), component.as_ptr(), 0o700) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(std::io::Error::last_os_error().into());
            }
            self.0.sync_all()?;
        }
        // SAFETY: single component, O_NOFOLLOW prohibits symlink traversal.
        let file = opened(unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                component.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
        let child = Self(file);
        if same_device && child.identity()?.0 != self.identity()?.0 {
            return Err("GC refuses nested filesystem mounts".into());
        }
        Ok(child)
    }
    fn parent(&self, path: &str, create: bool) -> GcResult<(Self, String)> {
        if path.len() > 1024 {
            return Err("GC relative path too long".into());
        }
        let components: Vec<_> = path.split('/').collect();
        if components.len() > 8 {
            return Err("GC relative depth exceeded".into());
        }
        let mut current = Self(self.0.try_clone()?);
        for value in &components[..components.len() - 1] {
            current = current.child(value, create)?;
        }
        let leaf = components.last().ok_or("missing GC filename")?;
        name(leaf)?;
        Ok((current, (*leaf).into()))
    }
    pub fn read(&self, path: &str, limit: usize) -> GcResult<Option<Vec<u8>>> {
        let Some(mut file) = self.file(path)? else {
            return Ok(None);
        };
        if file.metadata()?.len() > limit as u64 {
            return Err("GC record size exceeded".into());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err("GC record grew while reading".into());
        }
        Ok(Some(bytes))
    }
    fn file(&self, path: &str) -> GcResult<Option<File>> {
        let (parent, leaf) = self.parent(path, false)?;
        let leaf = name(&leaf)?;
        // O_NONBLOCK avoids blocking on a substituted FIFO before metadata check.
        let fd = unsafe {
            libc::openat(
                parent.0.as_raw_fd(),
                leaf.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        let file = opened(fd)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err("GC requires a regular non-hardlinked file".into());
        }
        Ok(Some(file))
    }
    pub fn fingerprint(&self, path: &str) -> GcResult<Option<(Identity, String)>> {
        let Some(mut file) = self.file(path)? else {
            return Ok(None);
        };
        let before = identity(&file)?;
        if before.bytes > MAX_FILE_BYTES {
            return Err("GC file byte limit exceeded".into());
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut bytes = 0u64;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes += count as u64;
            if bytes > MAX_FILE_BYTES {
                return Err("GC file grew while hashing".into());
            }
            hash.update(&buffer[..count]);
        }
        if identity(&file)? != before || bytes != before.bytes {
            return Err("GC file changed while hashing".into());
        }
        Ok(Some((before, hex::encode(hash.finalize()))))
    }
    pub fn write_new(&self, path: &str, bytes: &[u8]) -> GcResult<()> {
        if bytes.len() > MAX_RECORD_BYTES {
            return Err("GC record size exceeded".into());
        }
        let (parent, leaf) = self.parent(path, true)?;
        if let Some(existing) = self.read(path, MAX_RECORD_BYTES)? {
            if existing == bytes {
                return Ok(());
            }
            return Err("GC immutable journal collision".into());
        }
        let temp = format!(".prepare-{}", uuid::Uuid::new_v4());
        let temp_c = name(&temp)?;
        let mut file = opened(unsafe {
            libc::openat(
                parent.0.as_raw_fd(),
                temp_c.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        })?;
        file.write_all(bytes)?;
        file.sync_all()?;
        parent.rename(&temp, &parent, &leaf)?;
        Ok(())
    }
    fn rename(&self, from: &str, to: &Self, destination: &str) -> GcResult<()> {
        let from = name(from)?;
        let destination = name(destination)?;
        // Atomic same-filesystem move, never overwrite any existing destination.
        let result = unsafe {
            libc::renameat2(
                self.0.as_raw_fd(),
                from.as_ptr(),
                to.0.as_raw_fd(),
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        self.0.sync_all()?;
        to.0.sync_all()?;
        Ok(())
    }
    pub fn move_checked(&self, from: &str, to: &str, expected: &Candidate) -> GcResult<()> {
        self.check(from, expected)?;
        let (source, leaf) = self.parent(from, false)?;
        let (target, dest) = self.parent(to, true)?;
        source.rename(&leaf, &target, &dest)?;
        self.check(to, expected)
    }
    pub fn check(&self, path: &str, expected: &Candidate) -> GcResult<()> {
        if self.fingerprint(path)? != Some((expected.identity.clone(), expected.raw_sha256.clone()))
        {
            return Err("GC candidate identity/content changed".into());
        }
        Ok(())
    }
    pub fn remove_record(&self, path: &str, expected: &[u8]) -> GcResult<()> {
        if self.read(path, MAX_RECORD_BYTES)?.as_deref() != Some(expected) {
            return Err("GC journal identity changed".into());
        }
        self.unlink(path)
    }
    pub fn unlink_checked(&self, path: &str, candidate: &Candidate) -> GcResult<()> {
        self.check(path, candidate)?;
        self.unlink(path)
    }
    fn unlink(&self, path: &str) -> GcResult<()> {
        let (parent, leaf) = self.parent(path, false)?;
        let leaf = name(&leaf)?;
        let result = unsafe { libc::unlinkat(parent.0.as_raw_fd(), leaf.as_ptr(), 0) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        parent.0.sync_all()?;
        Ok(())
    }
}
fn identity(file: &File) -> GcResult<Identity> {
    let m = file.metadata()?;
    Ok(Identity {
        device: m.dev(),
        inode: m.ino(),
        bytes: m.len(),
        modified_seconds: m.mtime(),
        modified_nanos: m.mtime_nsec(),
    })
}
