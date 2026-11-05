//! Private, bounded transfer cache. It is not a Store and never contains history
//! authorities. Immutable records become visible only after data and name sync.
use crate::secure_fs::{PinnedDirectory, PinnedFile};
use fs2::FileExt;
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime},
};

const MAGIC: &[u8] = b"layrs.download-cache.v1\n";
const MAX_ENTRIES: usize = 128;
pub const MAX_CACHE_RECORDS: usize = 128;
pub const MAX_CACHE_RECORD_BYTES: usize = 1024 * 1024 + 4096;
pub const DEFAULT_DOWNLOAD_CACHE_BYTES: u64 = 256 * 1024 * 1024;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// A lease covers one authority/object binding while network I/O runs. The
/// shared cache accounting lock is held only during bounded local filesystem I/O.
pub struct DownloadCacheLease {
    root: PinnedDirectory,
    directory: PinnedDirectory,
    root_path: PathBuf,
    name: String,
    _lease: PinnedFile,
    quota: u64,
}
impl DownloadCacheLease {
    pub fn open(root_path: &Path, binding: &str, quota: u64) -> io::Result<Self> {
        if binding.len() != 64
            || !binding
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || !(1024..=DEFAULT_DOWNLOAD_CACHE_BYTES).contains(&quota)
        {
            return Err(invalid("invalid download cache bounds or binding"));
        }
        let parent = root_path
            .parent()
            .ok_or_else(|| invalid("cache requires a parent directory"))?;
        let leaf = root_path
            .file_name()
            .ok_or_else(|| invalid("cache requires a directory name"))?;
        let root = PinnedDirectory::open(parent)?.ensure_private_child(leaf)?;
        let global = open_or_create(&root, "cache.lock")?;
        FileExt::lock_exclusive(global.file())?;
        match root.open_file(OsStr::new("format")) {
            Ok(file) => {
                if read_small(file.file(), 64)? != MAGIC {
                    return Err(invalid("unrecognized download cache format"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if fs::read_dir(root_path)?.any(|entry| {
                    entry
                        .map(|e| e.file_name() != OsStr::new("cache.lock"))
                        .unwrap_or(true)
                }) {
                    return Err(invalid("refusing nonempty unrecognized cache directory"));
                }
                write_new(&root, "format", MAGIC)?;
            }
            Err(error) => return Err(error),
        }
        let name = format!("entry-{binding}");
        let mut entries = inventory(&root, root_path)?;
        loop {
            let needed = if entries.iter().any(|entry| entry.name == name) {
                0
            } else {
                64
            };
            let used = entries.iter().try_fold(MAGIC.len() as u64, |sum, entry| {
                sum.checked_add(entry.bytes)
                    .ok_or_else(|| invalid("cache size overflow"))
            })?;
            if used.checked_add(needed).is_some_and(|bytes| bytes <= quota) {
                break;
            }
            if !evict_one(&root, root_path, &name, &entries)? {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "download cache byte quota exhausted",
                ));
            }
            entries = inventory(&root, root_path)?;
        }
        if !entries.iter().any(|entry| entry.name == name) && entries.len() >= MAX_ENTRIES {
            if !evict_one(&root, root_path, &name, &entries)? {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "download cache has too many active entries",
                ));
            }
            entries = inventory(&root, root_path)?;
            if entries.len() >= MAX_ENTRIES {
                return Err(invalid("download cache entry bound exceeded"));
            }
        }
        let directory = root.ensure_private_child(OsStr::new(&name))?;
        match directory.open_file(OsStr::new("binding")) {
            Ok(file) => {
                if read_small(file.file(), 128)? != binding.as_bytes() {
                    return Err(invalid("download cache binding mismatch"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if fs::read_dir(root_path.join(&name))?.next().is_some() {
                    return Err(invalid("unrecognized cache entry"));
                }
                write_new(&directory, "binding", binding.as_bytes())?;
            }
            Err(error) => return Err(error),
        }
        let lease = open_or_create(&directory, "lease")?;
        drop(global);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match FileExt::try_lock_exclusive(lease.file()) {
                Ok(()) => break,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(error) => return Err(error),
            }
        }
        root.verify_path_identity()?;
        directory.verify_path_identity()?;
        Ok(Self {
            root,
            directory,
            root_path: root_path.into(),
            name,
            _lease: lease,
            quota,
        })
    }
    pub fn record(&self, index: usize) -> io::Result<Option<File>> {
        let name = record_name(index)?;
        self.directory.verify_path_identity()?;
        match self.directory.open_file(OsStr::new(&name)) {
            Ok(file) => {
                if file.file().metadata()?.len() > MAX_CACHE_RECORD_BYTES as u64 {
                    return Err(invalid("download cache record exceeds bounds"));
                }
                Ok(Some(file.file().try_clone()?))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn append(&self, index: usize, bytes: &[u8]) -> io::Result<()> {
        let name = record_name(index)?;
        if bytes.len() > MAX_CACHE_RECORD_BYTES {
            return Err(invalid("download cache record exceeds bounds"));
        }
        let global = open_or_create(&self.root, "cache.lock")?;
        FileExt::lock_exclusive(global.file())?;
        self.root.verify_path_identity()?;
        self.directory.verify_path_identity()?;
        loop {
            let entries = inventory(&self.root, &self.root_path)?;
            let used = entries.iter().try_fold(MAGIC.len() as u64, |sum, entry| {
                sum.checked_add(entry.bytes)
                    .ok_or_else(|| invalid("cache size overflow"))
            })?;
            if used
                .checked_add(bytes.len() as u64)
                .is_some_and(|size| size <= self.quota)
            {
                break;
            }
            if !evict_one(&self.root, &self.root_path, &self.name, &entries)? {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "download cache byte quota exhausted",
                ));
            }
        }
        let temporary = format!(
            "tmp-{}-{}",
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = self.directory.create_file(OsStr::new(&temporary))?;
        file.file_mut().write_all(bytes)?;
        file.file().sync_all()?;
        if file.rename_to(&self.directory, OsStr::new(&name))? {
            file.unlink()?;
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "download cache checkpoint already exists",
            ));
        }
        self.directory.sync()?;
        Ok(())
    }
    /// Only our bounded record/temp namespace is removed. Binding/lease remain
    /// until a later bounded eviction; no application history is traversed.
    pub fn clear(&self) -> io::Result<()> {
        let global = open_or_create(&self.root, "cache.lock")?;
        FileExt::lock_exclusive(global.file())?;
        self.root.verify_path_identity()?;
        self.directory.verify_path_identity()?;
        clear_records(&self.directory, &self.root_path.join(&self.name))
    }
    /// Remove a finished cache entry under the accounting lock. Waiting readers
    /// must revalidate pinned directory identity before accessing any records.
    pub fn discard(self) -> io::Result<()> {
        let global = open_or_create(&self.root, "cache.lock")?;
        FileExt::lock_exclusive(global.file())?;
        self.root.verify_path_identity()?;
        self.directory.verify_path_identity()?;
        clear_records(&self.directory, &self.root_path.join(&self.name))?;
        self.directory.open_file(OsStr::new("binding"))?.unlink()?;
        self._lease.unlink()?;
        self.directory.sync()?;
        let Self {
            root,
            directory,
            name,
            _lease,
            ..
        } = self;
        drop(_lease);
        drop(directory);
        root.remove_empty_child(OsStr::new(&name))?;
        root.sync()
    }
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn record_name(index: usize) -> io::Result<String> {
    if index >= MAX_CACHE_RECORDS {
        Err(invalid("too many download cache records"))
    } else {
        Ok(format!("part-{index:04}"))
    }
}
fn owned_record(name: &str) -> bool {
    (name.len() == 9
        && name.starts_with("part-")
        && name[5..]
            .parse::<usize>()
            .is_ok_and(|n| n < MAX_CACHE_RECORDS))
        || (name.starts_with("tmp-")
            && name.len() <= 64
            && name[4..].bytes().all(|b| b.is_ascii_digit() || b == b'-'))
}
fn open_or_create(directory: &PinnedDirectory, name: &str) -> io::Result<PinnedFile> {
    match directory.create_file(OsStr::new(name)) {
        Ok(file) => {
            file.file().sync_all()?;
            directory.sync()?;
            Ok(file)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            directory.open_file(OsStr::new(name))
        }
        Err(error) => Err(error),
    }
}
fn write_new(directory: &PinnedDirectory, name: &str, bytes: &[u8]) -> io::Result<()> {
    let mut file = directory.create_file(OsStr::new(name))?;
    file.file_mut().write_all(bytes)?;
    file.file().sync_all()?;
    directory.sync()
}
fn read_small(file: &File, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("cache metadata exceeds bounds"));
    }
    Ok(bytes)
}
struct Entry {
    name: String,
    bytes: u64,
    modified: SystemTime,
}
fn inventory(root: &PinnedDirectory, path: &Path) -> io::Result<Vec<Entry>> {
    root.verify_path_identity()?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("non-UTF8 cache name"))?;
        if name == "format" || name == "cache.lock" {
            continue;
        }
        if name.len() != 70
            || !name.starts_with("entry-")
            || !name[6..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || entries.len() >= MAX_ENTRIES
        {
            return Err(invalid("unrecognized or excessive download cache entries"));
        }
        let directory = root.ensure_private_child(OsStr::new(&name))?;
        let marker = directory.open_file(OsStr::new("binding"))?;
        if read_small(marker.file(), 128)?.as_slice() != &name.as_bytes()[6..] {
            return Err(invalid("cache entry binding mismatch"));
        }
        let mut bytes = 0u64;
        let mut count = 0;
        for file in fs::read_dir(entry.path())? {
            count += 1;
            if count > MAX_CACHE_RECORDS * 2 + 2 {
                return Err(invalid("too many cache files"));
            }
            let file = file?;
            let leaf = file
                .file_name()
                .into_string()
                .map_err(|_| invalid("invalid cache filename"))?;
            if leaf != "binding" && leaf != "lease" && !owned_record(&leaf) {
                return Err(invalid("unrecognized cache file"));
            }
            let pinned = directory.open_file(OsStr::new(&leaf))?;
            let size = pinned.file().metadata()?.len();
            if size > MAX_CACHE_RECORD_BYTES as u64 {
                return Err(invalid("oversize cache file"));
            }
            bytes = bytes
                .checked_add(size)
                .ok_or_else(|| invalid("cache bytes overflow"))?;
        }
        directory.verify_path_identity()?;
        entries.push(Entry {
            name,
            bytes,
            modified: entry.metadata()?.modified()?,
        });
    }
    root.verify_path_identity()?;
    entries.sort_by_key(|entry| entry.modified);
    Ok(entries)
}
fn clear_records(directory: &PinnedDirectory, path: &Path) -> io::Result<()> {
    let mut count = 0;
    for entry in fs::read_dir(path)? {
        count += 1;
        if count > MAX_CACHE_RECORDS * 2 + 2 {
            return Err(invalid("too many cache files"));
        }
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("invalid cache filename"))?;
        if name == "binding" || name == "lease" {
            continue;
        }
        if !owned_record(&name) {
            return Err(invalid("refusing non-cache file deletion"));
        }
        directory.unlink_file(OsStr::new(&name))?;
    }
    directory.sync()
}
fn evict_one(
    root: &PinnedDirectory,
    path: &Path,
    current: &str,
    entries: &[Entry],
) -> io::Result<bool> {
    for entry in entries.iter().filter(|entry| entry.name != current) {
        let directory = root.ensure_private_child(OsStr::new(&entry.name))?;
        let lease = open_or_create(&directory, "lease")?;
        match FileExt::try_lock_exclusive(lease.file()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
        clear_records(&directory, &path.join(&entry.name))?;
        directory.unlink_file(OsStr::new("binding"))?;
        directory.unlink_file(OsStr::new("lease"))?;
        drop(lease);
        drop(directory);
        root.remove_empty_child(OsStr::new(&entry.name))?;
        return Ok(true);
    }
    Ok(false)
}
