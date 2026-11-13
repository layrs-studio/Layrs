//! The status view (council s20): what `layrs status` needs to decide
//! whether a Worktree's folder still equals its base, read without decoding
//! a checkout projection, the change index or any pack index.
//!
//! It is derived data, written beside the change index when the index is
//! bound to a projection (`ChangeIndex::bind`), in the same table under the
//! key `status:<worktree>`. It holds, per folder of the base projection, its
//! entries sorted by name: kind, length and executable bit, and for a file
//! the change index's proof (its listing key and when it was proven), with
//! whether the proven content is the base's or another. It holds no FileId.
//!
//! A view only decides clear cases (`Verdict`): every listed entry equal to
//! a trusted proof of the base content is clean; a deleted tracked path, a
//! tracked file of another length or proven different, or a new regular
//! file outside the ignore rules is a change. Anything else (a new folder, a
//! changed kind, an unproven file of the same length, ignore exceptions, a
//! name the scan would refuse) is `Unknown`: the caller takes the full path,
//! so the answer never differs from it. Every fact a view records stays
//! true for its base: a stale view only answers `Unknown` more often.
use super::change_index::StatKey;
use super::checkout::{CheckoutProjectionContent, CheckoutProjectionEntry, CheckoutReader};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

const MAGIC: &[u8; 8] = b"LYRSSV01";
/// The key prefix of a view in the change index table.
const KEY_PREFIX: &str = "status:";
/// The most bytes a stored view may hold (decompressed).
const MAX_VIEW_BYTES: u64 = 512 * 1024 * 1024;

/// The largest unproven file a view hashes; a larger one takes the full
/// path.
const MAX_HASHED_BYTES: u64 = 64 * 1024 * 1024;

const TAG_FILE: u8 = 0;
const TAG_FILE_EQUAL: u8 = 1;
const TAG_FILE_OTHER: u8 = 2;
const TAG_SYMLINK: u8 = 3;
const TAG_FOLDER: u8 = 4;

/// What the change index proved about a base file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Proof {
    /// Nothing usable: a listed file of the base length must be read.
    None,
    /// The file whose listing key is this held the base content, proven at
    /// `verified_at`.
    Base(StatKey, i64),
    /// The file whose listing key is this held other content.
    Other(StatKey, i64),
}

/// What a view decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    Changed,
    Unknown,
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_signed(out: &mut Vec<u8>, value: i64) {
    put_varint(out, ((value << 1) ^ (value >> 63)) as u64);
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

/// The sections of a view (without its header): for each folder of
/// `projection`, its entries. `proof` answers for each file path.
pub(crate) fn encode_sections(
    projection: &[CheckoutProjectionEntry],
    margin_nanos: i64,
    proof: &mut dyn FnMut(&str, u64) -> Proof,
) -> Vec<u8> {
    let _span = crate::perf::span("status_view.encode");
    // Folder path -> entries (name, kind and data), names in byte order.
    let mut folders: BTreeMap<&str, Vec<(&str, Option<&CheckoutProjectionEntry>)>> =
        BTreeMap::new();
    folders.insert("", Vec::new());
    let mut known = std::collections::HashSet::new();
    for entry in projection {
        let (folder, name) = entry.path.rsplit_once('/').unwrap_or(("", &entry.path));
        folders.entry(folder).or_default().push((name, Some(entry)));
        // Every ancestor folder is an entry of its parent (once).
        let mut child = folder;
        while !child.is_empty() && known.insert(child) {
            let (parent, name) = child.rsplit_once('/').unwrap_or(("", child));
            folders.entry(parent).or_default().push((name, None));
            folders.entry(child).or_default();
            child = parent;
        }
    }
    let mut out = Vec::with_capacity(projection.len() * 24 + 64);
    put_varint(&mut out, folders.len() as u64);
    for (folder, mut entries) in folders {
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        put_bytes(&mut out, folder.as_bytes());
        put_varint(&mut out, entries.len() as u64);
        let mut previous: &str = "";
        let (mut last_modified, mut last_verified) = (0_i64, 0_i64);
        for (name, entry) in entries {
            let shared = previous
                .bytes()
                .zip(name.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            put_varint(&mut out, shared as u64);
            put_bytes(&mut out, &name.as_bytes()[shared..]);
            previous = name;
            match entry.map(|entry| (&entry.path, &entry.content)) {
                None => out.push(TAG_FOLDER),
                Some((_, CheckoutProjectionContent::Symlink { target })) => {
                    out.push(TAG_SYMLINK);
                    put_bytes(&mut out, target.as_bytes());
                }
                Some((
                    path,
                    CheckoutProjectionContent::File {
                        file_id,
                        byte_length,
                        executable,
                    },
                )) => {
                    // A proof taken too soon after the file's last write
                    // never becomes trusted: the file is unproven.
                    let proof = match proof(path, *byte_length) {
                        Proof::Base(stat, verified_at) | Proof::Other(stat, verified_at)
                            if stat
                                .modified
                                .checked_add(margin_nanos)
                                .is_none_or(|limit| limit >= verified_at) =>
                        {
                            Proof::None
                        }
                        proof => proof,
                    };
                    out.push(match proof {
                        Proof::None => TAG_FILE,
                        Proof::Base(..) => TAG_FILE_EQUAL,
                        Proof::Other(..) => TAG_FILE_OTHER,
                    });
                    put_varint(&mut out, *byte_length);
                    out.push(u8::from(*executable));
                    // An unproven file (a few: those changed just before
                    // they were proven) keeps its FileId, to be hashed.
                    if proof == Proof::None {
                        out.extend_from_slice(&file_id.digest_bytes());
                    }
                    if let Proof::Base(stat, verified_at) | Proof::Other(stat, verified_at) = proof
                    {
                        put_stat(&mut out, &stat, &mut last_modified);
                        put_signed(&mut out, verified_at.wrapping_sub(last_verified));
                        last_verified = verified_at;
                    }
                }
            }
        }
    }
    out
}

fn put_stat(out: &mut Vec<u8>, stat: &StatKey, last_modified: &mut i64) {
    let flags = u8::from(stat.created.is_some())
        | (u8::from(stat.inode.is_some()) << 1)
        | (u8::from(stat.changed.is_some()) << 2)
        | (u8::from(stat.mode.is_some()) << 3);
    out.push(flags);
    put_varint(out, stat.length);
    put_signed(out, stat.modified.wrapping_sub(*last_modified));
    *last_modified = stat.modified;
    if let Some(created) = stat.created {
        put_signed(out, created.wrapping_sub(stat.modified));
    }
    if let Some(inode) = stat.inode {
        put_varint(out, inode);
    }
    if let Some(changed) = stat.changed {
        put_signed(out, changed.wrapping_sub(stat.modified));
    }
    if let Some(mode) = stat.mode {
        put_varint(out, u64::from(mode));
    }
}

/// A view's full bytes: its binding (the base root Tree, the base Step when
/// known), the trust margin, then `sections`.
pub(crate) fn encode(
    tree: &str,
    step: Option<&str>,
    margin_nanos: i64,
    sections: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(sections.len() + 160);
    out.extend_from_slice(MAGIC);
    put_bytes(&mut out, tree.as_bytes());
    put_bytes(&mut out, step.unwrap_or_default().as_bytes());
    put_signed(&mut out, margin_nanos);
    out.extend_from_slice(sections);
    out
}

/// The key of `worktree`'s view in the change index table.
pub(crate) fn key(worktree: &str) -> String {
    format!("{KEY_PREFIX}{worktree}")
}

pub(crate) fn remove(store_root: &Path, worktree: &str) {
    super::derived_cache::change_index_delete(store_root, &key(worktree));
}

/// Strict reader over a view's untrusted bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(length)?;
        let taken = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(taken)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
    fn signed(&mut self) -> Option<i64> {
        let raw = self.varint()?;
        Some(((raw >> 1) as i64) ^ -((raw & 1) as i64))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let length = usize::try_from(self.varint()?).ok()?;
        self.take(length)
    }
    fn str(&mut self) -> Option<&'a str> {
        std::str::from_utf8(self.bytes()?).ok()
    }
}

/// One folder of a view: where its entries start, and how many.
struct Folder {
    start: usize,
    count: usize,
}

/// A loaded status view.
pub struct StatusView {
    bytes: Vec<u8>,
    tree: (usize, usize),
    step: (usize, usize),
    margin_nanos: i64,
    folders: std::collections::HashMap<String, Folder>,
}

/// One entry of a folder, as decoded.
#[derive(Debug)]
enum Entry<'a> {
    Folder,
    Symlink(&'a str),
    File {
        length: u64,
        executable: bool,
        proof: Proof,
        /// The base FileId of an unproven file.
        file: Option<[u8; 32]>,
    },
}

impl StatusView {
    /// Forgets the view of `worktree` (the next `status` takes the full
    /// comparison).
    pub fn discard(store_root: &Path, worktree: &str) {
        remove(store_root, worktree);
    }

    /// The view of `worktree`, if one is stored and well formed.
    pub fn load(store_root: &Path, worktree: &str) -> Option<Self> {
        let _span = crate::perf::span("status_view.load");
        let super::derived_cache::Stored::Present(bytes) =
            super::derived_cache::change_index_get(store_root, &key(worktree), MAX_VIEW_BYTES)
        else {
            return None;
        };
        Self::parse(bytes)
    }

    fn parse(bytes: Vec<u8>) -> Option<Self> {
        let _span = crate::perf::span("status_view.parse");
        if bytes.get(..MAGIC.len())? != MAGIC {
            return None;
        }
        let mut reader = Reader {
            bytes: &bytes,
            at: MAGIC.len(),
        };
        let tree = reader.str()?.len();
        let tree = (reader.at - tree, tree);
        let step = reader.str()?.len();
        let step = (reader.at - step, step);
        let margin_nanos = reader.signed()?;
        let count = usize::try_from(reader.varint()?).ok()?;
        let mut folders = std::collections::HashMap::with_capacity(count.min(1 << 20));
        for _ in 0..count {
            let path = reader.str()?.to_owned();
            let entries = usize::try_from(reader.varint()?).ok()?;
            let start = reader.at;
            // Walked once here: bounds and sizes are checked before use.
            let mut cursor = EntryCursor::new(&bytes[start..], entries);
            for _ in 0..entries {
                cursor.next()?;
            }
            reader.at = start + cursor.reader.at;
            if folders
                .insert(
                    path,
                    Folder {
                        start,
                        count: entries,
                    },
                )
                .is_some()
            {
                return None;
            }
        }
        if reader.at != bytes.len() {
            return None;
        }
        Some(Self {
            bytes,
            tree,
            step,
            margin_nanos,
            folders,
        })
    }

    fn slice(&self, range: (usize, usize)) -> &str {
        std::str::from_utf8(&self.bytes[range.0..range.0 + range.1]).unwrap_or_default()
    }

    /// The base root Tree this view is bound to.
    pub fn tree(&self) -> &str {
        self.slice(self.tree)
    }

    /// The base Step this view is bound to, when it was known.
    pub fn step(&self) -> Option<&str> {
        Some(self.slice(self.step)).filter(|step| !step.is_empty())
    }

    fn folder(&self, path: &str) -> Option<EntryCursor<'_>> {
        let folder = self.folders.get(path)?;
        Some(EntryCursor::new(&self.bytes[folder.start..], folder.count))
    }

    /// Whether the folder of `checkout` still equals the base this view is
    /// bound to (the caller checked the binding): read-only, one listing of
    /// each folder, in parallel; never reads a file's content.
    pub fn verdict(&self, checkout: &CheckoutReader) -> io::Result<Verdict> {
        let _span = crate::perf::span("status_view.verdict");
        let ignored = super::checkout_ignore::IgnoreRules::load(checkout.pinned())?;
        let walk = Walk {
            view: self,
            root: checkout.root(),
            ignored: &ignored,
            queue: Mutex::new(vec![String::new()]),
            pending: AtomicUsize::new(1),
            verdict: AtomicU8::new(CLEAN),
        };
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, 16);
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| walk.work());
            }
        });
        Ok(match walk.verdict.load(Ordering::Acquire) {
            CLEAN => Verdict::Clean,
            CHANGED => Verdict::Changed,
            _ => Verdict::Unknown,
        })
    }
}

/// The entries of one folder, decoded in order.
struct EntryCursor<'a> {
    reader: Reader<'a>,
    left: usize,
    name: Vec<u8>,
    last_modified: i64,
    last_verified: i64,
}

impl<'a> EntryCursor<'a> {
    fn new(bytes: &'a [u8], count: usize) -> Self {
        Self {
            reader: Reader { bytes, at: 0 },
            left: count,
            name: Vec::with_capacity(64),
            last_modified: 0,
            last_verified: 0,
        }
    }

    /// The next entry: its name and data.
    fn next(&mut self) -> Option<(String, Entry<'a>)> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let shared = usize::try_from(self.reader.varint()?).ok()?;
        let suffix = self.reader.bytes()?;
        if shared > self.name.len() {
            return None;
        }
        self.name.truncate(shared);
        self.name.extend_from_slice(suffix);
        let entry = match self.reader.u8()? {
            TAG_FOLDER => Entry::Folder,
            TAG_SYMLINK => Entry::Symlink(self.reader.str()?),
            tag @ (TAG_FILE | TAG_FILE_EQUAL | TAG_FILE_OTHER) => {
                let length = self.reader.varint()?;
                let executable = match self.reader.u8()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                let mut file = None;
                let proof = if tag == TAG_FILE {
                    let mut digest = [0_u8; 32];
                    digest.copy_from_slice(self.reader.take(32)?);
                    file = Some(digest);
                    Proof::None
                } else {
                    let stat = self.stat()?;
                    let verified_at = self.last_verified.wrapping_add(self.reader.signed()?);
                    self.last_verified = verified_at;
                    if tag == TAG_FILE_EQUAL {
                        Proof::Base(stat, verified_at)
                    } else {
                        Proof::Other(stat, verified_at)
                    }
                };
                Entry::File {
                    length,
                    executable,
                    proof,
                    file,
                }
            }
            _ => return None,
        };
        let name = std::str::from_utf8(&self.name).ok()?.to_owned();
        Some((name, entry))
    }

    fn stat(&mut self) -> Option<StatKey> {
        let flags = self.reader.u8()?;
        if flags & !0b1111 != 0 {
            return None;
        }
        let length = self.reader.varint()?;
        let modified = self.last_modified.wrapping_add(self.reader.signed()?);
        self.last_modified = modified;
        let created = (flags & 1 != 0)
            .then(|| Some(modified.wrapping_add(self.reader.signed()?)))
            .flatten();
        if flags & 1 != 0 && created.is_none() {
            return None;
        }
        let inode = if flags & 2 != 0 {
            Some(self.reader.varint()?)
        } else {
            None
        };
        let changed = if flags & 4 != 0 {
            Some(modified.wrapping_add(self.reader.signed()?))
        } else {
            None
        };
        let mode = if flags & 8 != 0 {
            Some(u32::try_from(self.reader.varint()?).ok()?)
        } else {
            None
        };
        Some(StatKey {
            length,
            modified,
            created,
            inode,
            changed,
            mode,
        })
    }
}

const CLEAN: u8 = 0;
const CHANGED: u8 = 1;
const UNKNOWN: u8 = 2;

/// One parallel comparison of a checkout with a view.
struct Walk<'a> {
    view: &'a StatusView,
    root: &'a Path,
    ignored: &'a super::checkout_ignore::IgnoreRules,
    queue: Mutex<Vec<String>>,
    /// Folders queued or being compared.
    pending: AtomicUsize,
    verdict: AtomicU8,
}

impl Walk<'_> {
    fn decide(&self, verdict: u8) {
        // Changed wins over Unknown: a change is certain, an unknown is not.
        let _ = self
            .verdict
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current == CLEAN || (current == UNKNOWN && verdict == CHANGED)).then_some(verdict)
            });
    }

    fn work(&self) {
        loop {
            if self.verdict.load(Ordering::Acquire) == CHANGED {
                return;
            }
            let next = self.queue.lock().ok().and_then(|mut queue| queue.pop());
            let Some(folder) = next else {
                if self.pending.load(Ordering::Acquire) == 0 {
                    return;
                }
                std::thread::yield_now();
                continue;
            };
            match self.compare(&folder) {
                Ok(children) => {
                    self.pending.fetch_add(children.len(), Ordering::AcqRel);
                    if let Ok(mut queue) = self.queue.lock() {
                        queue.extend(children);
                    }
                }
                // The full path reports the error itself.
                Err(_) => self.decide(UNKNOWN),
            }
            self.pending.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Compares one folder: its verdict is recorded; returns the tracked
    /// subfolders to compare next.
    fn compare(&self, folder: &str) -> io::Result<Vec<String>> {
        let disk = if folder.is_empty() {
            self.root.to_path_buf()
        } else {
            self.root.join(folder)
        };
        let mut listed: Vec<(String, fs::Metadata)> = Vec::new();
        for entry in fs::read_dir(&disk)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                self.decide(UNKNOWN);
                return Ok(Vec::new());
            };
            listed.push((name, entry.metadata()?));
        }
        listed.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let Some(mut base) = self.view.folder(folder) else {
            self.decide(UNKNOWN);
            return Ok(Vec::new());
        };
        let mut children = Vec::new();
        let mut listed = listed.into_iter().peekable();
        let mut next_base = base.next();
        loop {
            if self.verdict.load(Ordering::Relaxed) == CHANGED {
                return Ok(Vec::new());
            }
            let order = match (listed.peek(), &next_base) {
                (None, None) => break,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some((name, _)), Some((base_name, _))) => {
                    name.as_bytes().cmp(base_name.as_bytes())
                }
            };
            match order {
                // On disk only.
                std::cmp::Ordering::Less => {
                    let (name, metadata) = listed.next().expect("peeked");
                    self.untracked(folder, &name, &metadata);
                }
                // In the base only: a tracked path is gone.
                std::cmp::Ordering::Greater => {
                    self.decide(CHANGED);
                    return Ok(Vec::new());
                }
                std::cmp::Ordering::Equal => {
                    let (name, metadata) = listed.next().expect("peeked");
                    let (_, entry) = next_base.take().expect("compared");
                    if let Some(child) = self.tracked(folder, &name, &metadata, entry)? {
                        children.push(child);
                    }
                    next_base = base.next();
                }
            }
        }
        if base.left != 0 {
            // A malformed folder: decided by the full path.
            self.decide(UNKNOWN);
        }
        Ok(children)
    }

    fn path(folder: &str, name: &str) -> String {
        if folder.is_empty() {
            name.to_owned()
        } else {
            format!("{folder}/{name}")
        }
    }

    /// An entry the base does not have.
    fn untracked(&self, folder: &str, name: &str, metadata: &fs::Metadata) {
        if folder.is_empty()
            && (name.eq_ignore_ascii_case(".layrs") || name.eq_ignore_ascii_case(".git"))
        {
            return;
        }
        let path = Self::path(folder, name);
        if self.ignored.ignores(&path, metadata.is_dir()) {
            if metadata.is_dir() && self.ignored.has_exceptions {
                self.decide(UNKNOWN);
            }
            return;
        }
        // A new regular file with a name the scan accepts is a change;
        // anything else is left to the full path.
        let canonical =
            layrs_core::scm::RepoPath::parse(&path).is_ok_and(|parsed| parsed.as_str() == path);
        if metadata.is_file() && !reparse(metadata) && canonical {
            self.decide(CHANGED);
        } else {
            self.decide(UNKNOWN);
        }
    }

    /// An entry of the base, listed: the subfolder to compare next, if any.
    fn tracked(
        &self,
        folder: &str,
        name: &str,
        metadata: &fs::Metadata,
        entry: Entry<'_>,
    ) -> io::Result<Option<String>> {
        match entry {
            Entry::Folder => {
                if metadata.is_dir() && !reparse(metadata) {
                    return Ok(Some(Self::path(folder, name)));
                }
                self.decide(UNKNOWN);
            }
            Entry::Symlink(target) => {
                if !metadata.file_type().is_symlink() {
                    self.decide(UNKNOWN);
                    return Ok(None);
                }
                let disk = self.root.join(Self::path(folder, name));
                match fs::read_link(&disk)?.to_str() {
                    Some(listed) if listed == target => {}
                    Some(_) => self.decide(CHANGED),
                    None => self.decide(UNKNOWN),
                }
            }
            Entry::File {
                length,
                executable: base_executable,
                proof,
                file,
            } => {
                if !metadata.is_file() || reparse(metadata) {
                    self.decide(UNKNOWN);
                    return Ok(None);
                }
                if executable(metadata) != base_executable {
                    self.decide(CHANGED);
                    return Ok(None);
                }
                let Some(stat) = StatKey::of(metadata) else {
                    self.decide(UNKNOWN);
                    return Ok(None);
                };
                let trusted = |proven: &StatKey, verified_at: i64| {
                    *proven == stat
                        && proven.length == stat.length
                        && stat
                            .modified
                            .checked_add(self.view.margin_nanos)
                            .is_some_and(|limit| limit < verified_at)
                };
                match proof {
                    Proof::Base(proven, verified_at) if trusted(&proven, verified_at) => {}
                    Proof::Other(proven, verified_at) if trusted(&proven, verified_at) => {
                        self.decide(CHANGED);
                    }
                    // Unproven (or no longer): another length is a change;
                    // an unproven file of the same length is hashed, as git
                    // hashes its racily clean entries; else the full path.
                    _ => {
                        if stat.length != length {
                            self.decide(CHANGED);
                        } else if let Some(base) = file.filter(|_| length <= MAX_HASHED_BYTES) {
                            let disk = self.root.join(Self::path(folder, name));
                            let bytes = fs::read(&disk)?;
                            crate::perf::count("status_view.hashed", 1);
                            if bytes.len() as u64 != length
                                || layrs_core::scm::FileId::from_bytes(&bytes).digest_bytes()
                                    != base
                            {
                                self.decide(CHANGED);
                            }
                        } else {
                            self.decide(UNKNOWN);
                        }
                    }
                }
            }
        }
        Ok(None)
    }
}

fn executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::FileId;

    fn file(path: &str, length: u64) -> CheckoutProjectionEntry {
        CheckoutProjectionEntry {
            path: path.into(),
            content: CheckoutProjectionContent::File {
                file_id: FileId::from_bytes(path.as_bytes()),
                byte_length: length,
                executable: false,
            },
        }
    }

    fn stat(length: u64, modified: i64) -> StatKey {
        StatKey {
            length,
            modified,
            created: Some(modified - 5),
            inode: None,
            changed: None,
            mode: Some(0),
        }
    }

    #[test]
    fn sections_round_trip_with_folders_and_proofs() {
        let projection = vec![
            file("a-b.txt", 3),
            file("a/b/c.txt", 4),
            file("a/b/d.txt", 5),
            file("a/x.txt", 6),
            CheckoutProjectionEntry {
                path: "link".into(),
                content: CheckoutProjectionContent::Symlink {
                    target: "a/x.txt".into(),
                },
            },
            file("z.txt", 7),
        ];
        let sections = encode_sections(&projection, 100, &mut |path, _| match path {
            "a/b/c.txt" => Proof::Base(stat(4, 1_000), 9_000),
            "a/x.txt" => Proof::Other(stat(6, 2_000), 9_500),
            _ => Proof::None,
        });
        let bytes = encode("blake3:tree", Some("step:one"), 100, &sections);
        let view = StatusView::parse(bytes).unwrap();
        assert_eq!(view.tree(), "blake3:tree");
        assert_eq!(view.step(), Some("step:one"));
        let names = |folder: &str| {
            let mut cursor = view.folder(folder).unwrap();
            let mut out = Vec::new();
            while let Some((name, entry)) = cursor.next() {
                out.push(format!("{name}:{entry:?}"));
            }
            out
        };
        assert_eq!(
            names(""),
            vec![
                "a:Folder".to_owned(),
                format!(
                    "a-b.txt:File {{ length: 3, executable: false, proof: None, file: Some({:?}) }}",
                    FileId::from_bytes(b"a-b.txt").digest_bytes()
                ),
                "link:Symlink(\"a/x.txt\")".to_owned(),
                format!(
                    "z.txt:File {{ length: 7, executable: false, proof: None, file: Some({:?}) }}",
                    FileId::from_bytes(b"z.txt").digest_bytes()
                ),
            ]
        );
        assert_eq!(names("a")[0], "b:Folder");
        assert!(names("a/b")[0].contains("Base(StatKey { length: 4, modified: 1000"));
        assert!(names("a")[1].contains("Other("));
        // Truncated or extended bytes are refused.
        let full = encode("t", None, 0, &sections);
        assert!(StatusView::parse(full[..full.len() - 1].to_vec()).is_none());
        let mut longer = full.clone();
        longer.push(0);
        assert!(StatusView::parse(longer).is_none());
        assert!(StatusView::parse(full).unwrap().step().is_none());
    }
}
