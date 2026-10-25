//! Filesystem capabilities for an explicitly reviewed Git bridge operation.
//! Review retains identities; execution anchors every write and Git subprocess.
use crate::secure_fs::PinnedDirectory;
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};
mod checkpoint;
mod import_source;
pub use checkpoint::{ScanCheckpointDestination, ScanCheckpointWriter};
pub use import_source::GitImportSource;

fn stale() -> io::Error {
    io::Error::other("Git bridge reviewed filesystem state changed")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileSnapshot {
    length: u64,
    hash: [u8; 32],
}

fn read_optional(
    directory: &PinnedDirectory,
    name: &OsStr,
    limit: usize,
) -> io::Result<Option<FileSnapshot>> {
    directory.verify_path_identity()?;
    let mut file = match directory.open_file(name) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            directory.verify_path_identity()?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let mut hasher = blake3::Hasher::new();
    let mut length = 0usize;
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.file_mut().read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length = length.checked_add(count).ok_or_else(stale)?;
        if length > limit {
            return Err(io::Error::other(
                "Git bridge review file exceeds byte limit",
            ));
        }
        hasher.update(&buffer[..count]);
    }
    if !file.same_file(&directory.open_file(name)?) {
        return Err(stale());
    }
    directory.verify_path_identity()?;
    Ok(Some(FileSnapshot {
        length: length as u64,
        hash: *hasher.finalize().as_bytes(),
    }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LocatorSnapshot {
    Directory(String),
    File(FileSnapshot),
}

/// Binds native authority/receipt bytes (including absence), checkout identity
/// and locator without opening a Store lock inside a guarded writer callback.
#[derive(Clone, Debug)]
pub struct NativeBridgeFiles {
    checkout: Arc<PinnedDirectory>,
    store: Arc<PinnedDirectory>,
    locator: LocatorSnapshot,
    authority: Option<FileSnapshot>,
}
impl NativeBridgeFiles {
    pub fn capture(
        checkout: &Path,
        checkout_identity: &str,
        store: &Path,
        store_identity: &str,
    ) -> io::Result<Self> {
        let checkout = Arc::new(PinnedDirectory::open(checkout)?);
        let store = Arc::new(PinnedDirectory::open(store)?);
        if checkout.identity_token() != checkout_identity
            || store.identity_token() != store_identity
        {
            return Err(stale());
        }
        let locator = locator(&checkout)?;
        let authority = read_optional(&store, OsStr::new("native-remote.json"), 64 * 1024 * 1024)?;
        let result = Self {
            checkout,
            store,
            locator,
            authority,
        };
        result.verify()?;
        Ok(result)
    }
    pub fn verify(&self) -> io::Result<()> {
        self.checkout.verify_path_identity()?;
        self.store.verify_path_identity()?;
        if locator(&self.checkout)? != self.locator
            || read_optional(
                &self.store,
                OsStr::new("native-remote.json"),
                64 * 1024 * 1024,
            )? != self.authority
        {
            return Err(stale());
        }
        self.checkout.verify_path_identity()?;
        self.store.verify_path_identity()
    }
}
fn locator(checkout: &PinnedDirectory) -> io::Result<LocatorSnapshot> {
    match checkout.open_child_for_traversal(OsStr::new(".layrs")) {
        Ok(directory) => Ok(LocatorSnapshot::Directory(directory.identity_token())),
        Err(_) => read_optional(checkout, OsStr::new(".layrs"), 16384)?
            .map(LocatorSnapshot::File)
            .ok_or_else(stale),
    }
}

#[derive(Clone, Debug)]
pub struct ExportDestination {
    path: PathBuf,
    parent: Arc<PinnedDirectory>,
    existing: Option<Arc<PinnedDirectory>>,
    snapshot: Option<[u8; 32]>,
    max_entries: usize,
    max_bytes: usize,
}
impl ExportDestination {
    pub fn capture(
        path: impl AsRef<Path>,
        max_entries: usize,
        max_bytes: usize,
    ) -> io::Result<Self> {
        let path = std::path::absolute(path)?;
        if path
            .to_str()
            .is_none_or(|value| value.contains(['\n', '\r']))
        {
            return Err(io::Error::other(
                "Git host path must be UTF-8 without newlines",
            ));
        }
        let parent = Arc::new(PinnedDirectory::open(path.parent().ok_or_else(stale)?)?);
        let name = path.file_name().ok_or_else(stale)?;
        let existing = match parent.open_child_for_traversal(name) {
            Ok(directory) => Some(Arc::new(directory)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let snapshot = existing
            .as_ref()
            .map(|directory| {
                tree(directory, &path, max_entries, Some(max_bytes)).map(|tree| tree.0)
            })
            .transpose()?;
        let result = Self {
            path,
            parent,
            existing,
            snapshot,
            max_entries,
            max_bytes,
        };
        result.verify_review()?;
        Ok(result)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn read_incomplete_checkpoint(&self) -> io::Result<Vec<u8>> {
        self.verify_review()?;
        let directory = self.existing.as_ref().ok_or_else(stale)?;
        let mut file = directory.open_file(OsStr::new("layrs-export.incomplete"))?;
        let mut bytes = Vec::new();
        file.file_mut().take(16385).read_to_end(&mut bytes)?;
        if bytes.len() > 16384 {
            return Err(io::Error::other("export checkpoint exceeds limit"));
        }
        self.verify_review()?;
        Ok(bytes)
    }
    pub fn verify_review(&self) -> io::Result<()> {
        self.parent.verify_path_identity()?;
        match &self.existing {
            Some(directory) => {
                directory.verify_path_identity()?;
                if Some(
                    tree(
                        directory,
                        &self.path,
                        self.max_entries,
                        Some(self.max_bytes),
                    )?
                    .0,
                ) != self.snapshot
                {
                    return Err(stale());
                }
            }
            None => match std::fs::symlink_metadata(&self.path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                Ok(_) => return Err(stale()),
            },
        }
        self.parent.verify_path_identity()
    }
    pub fn begin(&self, resume: bool) -> io::Result<Arc<ExportLease>> {
        // Windows prevents path-chain renames throughout creation. Unix creates
        // relative to the retained parent, so replacement cannot redirect it.
        let mut paths = vec![self.path.parent().ok_or_else(stale)?.to_path_buf()];
        if self.existing.is_some() {
            paths.push(self.path.clone());
        }
        let locks = lock_paths(&paths)?;
        self.verify_review()?;
        let root = match (&self.existing, resume) {
            (Some(directory), true) => Arc::clone(directory),
            (None, false) => Arc::new(
                self.parent
                    .create_new_child(self.path.file_name().ok_or_else(stale)?)?,
            ),
            _ => {
                return Err(io::Error::other(
                    "Git export destination existence differs from explicit export/resume action",
                ));
            }
        };
        let result = Arc::new(ExportLease {
            path: self.path.clone(),
            parent: Arc::clone(&self.parent),
            root,
            max_entries: self.max_entries,
            max_bytes: self.max_bytes,
            _locks: locks,
        });
        result.verify()?;
        Ok(result)
    }
}

/// Kept alive until every child process and direct report/checkpoint write ends.
pub struct ExportLease {
    path: PathBuf,
    parent: Arc<PinnedDirectory>,
    root: Arc<PinnedDirectory>,
    max_entries: usize,
    max_bytes: usize,
    _locks: Vec<File>,
}
pub struct ExportCommandGuard {
    _locks: Vec<File>,
}
impl ExportLease {
    pub fn verify(&self) -> io::Result<()> {
        self.parent.verify_path_identity()?;
        self.root.verify_path_identity()
    }
    pub fn prepare_command(&self, command: &mut Command) -> io::Result<ExportCommandGuard> {
        self.verify()?;
        let (_, directories) = tree(&self.root, &self.path, self.max_entries, None)?;
        let locks = lock_paths(&directories)?;
        self.verify()?;
        anchor_command(&self.root, &self.path, command)?;
        Ok(ExportCommandGuard { _locks: locks })
    }
    pub fn write_file(&self, name: &str, bytes: &[u8], replace: bool) -> io::Result<()> {
        if bytes.len() > self.max_bytes {
            return Err(io::Error::other("export file exceeds byte limit"));
        }
        self.verify()?;
        let name = OsStr::new(name);
        let mut file = match self.root.create_file(name) {
            Ok(file) => file,
            Err(error) if replace && error.kind() == io::ErrorKind::AlreadyExists => {
                self.root.open_file(name)?.unlink()?;
                self.root.create_file(name)?
            }
            Err(error) => return Err(error),
        };
        file.file_mut().set_len(0)?;
        file.file_mut().write_all(bytes)?;
        file.file().sync_all()?;
        file.sync_parent()?;
        self.verify()
    }
    pub fn remove_file(&self, name: &str) -> io::Result<()> {
        self.verify()?;
        // Unlink the pinned file, never a newly substituted path entry.
        self.root.open_file(OsStr::new(name))?.unlink()?;
        self.root.sync()?;
        self.verify()
    }
}

fn anchor_command(
    directory: &PinnedDirectory,
    path: &Path,
    command: &mut Command,
) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::{fd::AsRawFd, unix::process::CommandExt};
        let directory = directory.duplicate_handle()?;
        unsafe extern "C" {
            fn fchdir(fd: std::ffi::c_int) -> std::ffi::c_int;
        }
        // SAFETY: fchdir is async-signal-safe; the captured owned descriptor
        // stays open through pre_exec. Git receives --git-dir=. afterward.
        unsafe {
            command.pre_exec(move || {
                if fchdir(directory.as_raw_fd()) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        command.current_dir(path);
    }
    #[cfg(not(any(unix, windows)))]
    {
        return Err(io::Error::other(
            "anchored Git execution unsupported on this platform",
        ));
    }
    let _ = (directory, path);
    Ok(())
}

fn tree(
    root: &PinnedDirectory,
    path: &Path,
    max_entries: usize,
    max_bytes: Option<usize>,
) -> io::Result<([u8; 32], Vec<PathBuf>)> {
    fn visit(
        directory: &PinnedDirectory,
        path: &Path,
        relative: &Path,
        budget: &mut (usize, usize),
        max_bytes: Option<usize>,
        hash: &mut blake3::Hasher,
        directories: &mut Vec<PathBuf>,
        depth: usize,
    ) -> io::Result<()> {
        if depth > 128 {
            return Err(io::Error::other("export review depth limit"));
        }
        directory.verify_path_identity()?;
        directories.push(path.to_path_buf());
        hash.update(directory.identity_token().as_bytes());
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            if budget.0 == 0 {
                return Err(io::Error::other("export review entry limit"));
            }
            budget.0 -= 1;
            entries.push(entry?.file_name());
        }
        entries.sort();
        for name in entries {
            let child_path = path.join(&name);
            let relative = relative.join(&name);
            let text = relative.to_str().ok_or_else(stale)?;
            hash.update(&(text.len() as u64).to_le_bytes());
            hash.update(text.as_bytes());
            match directory.open_child_for_traversal(&name) {
                Ok(child) => {
                    hash.update(b"directory");
                    visit(
                        &child,
                        &child_path,
                        &relative,
                        budget,
                        max_bytes,
                        hash,
                        directories,
                        depth + 1,
                    )?;
                }
                Err(_) => {
                    hash.update(b"file");
                    if max_bytes.is_some() {
                        let file = read_optional(directory, &name, budget.1)?.ok_or_else(stale)?;
                        budget.1 = budget
                            .1
                            .checked_sub(file.length as usize)
                            .ok_or_else(stale)?;
                        hash.update(&file.length.to_le_bytes());
                        hash.update(&file.hash);
                    } else {
                        directory.open_file(&name)?;
                    }
                }
            }
        }
        directory.verify_path_identity()
    }
    let mut hash = blake3::Hasher::new();
    let mut directories = vec![];
    visit(
        root,
        path,
        Path::new(""),
        &mut (max_entries, max_bytes.unwrap_or(0)),
        max_bytes,
        &mut hash,
        &mut directories,
        0,
    )?;
    Ok((*hash.finalize().as_bytes(), directories))
}

#[cfg(not(windows))]
fn lock_paths(_: &[PathBuf]) -> io::Result<Vec<File>> {
    Ok(vec![])
}
#[cfg(windows)]
fn lock_paths(paths: &[PathBuf]) -> io::Result<Vec<File>> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    let mut all = std::collections::BTreeSet::new();
    for path in paths {
        all.extend(path.ancestors().map(Path::to_path_buf));
    }
    let mut locks = Vec::new();
    for path in all {
        // FILE_LIST_DIRECTORY participates in data sharing: FILE_TRAVERSE alone
        // does not establish an effective deny-DELETE reservation on Windows.
        let file = std::fs::OpenOptions::new()
            .access_mode(0x1 | 0x80 | 0x0010_0000)
            .share_mode(0x1 | 0x2)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err(stale());
        }
        locks.push(file);
    }
    Ok(locks)
}

#[cfg(test)]
mod tests;
