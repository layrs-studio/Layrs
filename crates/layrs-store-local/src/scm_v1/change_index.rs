//! Per-Worktree change index (ADR 0025, "Détection des changements"): a cache
//! that lets a capture recognise an unchanged regular file from its directory
//! listing metadata without reopening it, like git's index.
//!
//! An entry only ever records content that was proven: hashed by an accepted
//! capture, or written by a materialization while it held the file with
//! exclusive write sharing. It is trusted again only when the listed stat key
//! is identical and the file's modification time precedes the moment of proof
//! by at least the volume's timestamp margin (otherwise the entry is "racy",
//! as in git, and the file is read again). A missing index disables the
//! mechanism; an unreadable one is treated as empty (everything is read).
//!
//! An index may be bound to the checkout projection of one root Tree
//! (council s3, D1): an entry whose FileId, length and executable bit equal
//! that projection's entry is written without them, and the file records the
//! Tree and a digest of the exact projection compared against. Such an entry
//! is trusted only once that very projection is supplied again (`resolve`);
//! until then its file is read, as for an entry that does not exist.
use super::checkout::{CheckoutProjectionContent, CheckoutProjectionEntry};
use layrs_core::scm::{FileId, TreeId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SCHEMA: &str = "layrs.change-index.v1";
const DIRECTORY: &str = "change-index";
/// Bound on what a corrupt or hostile file can make a capture allocate.
const MAX_INDEX_BYTES: u64 = 256 * 1024 * 1024;
/// NTFS and ReFS stamp times from the system clock (one tick is <= 16 ms).
const FINE_MARGIN_NANOS: i64 = 100_000_000;
/// FAT keeps 2 s modification times; unknown volumes get the same margin.
const COARSE_MARGIN_NANOS: i64 = 2_000_000_000;

/// The listing metadata that identifies one observed version of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatKey {
    pub length: u64,
    pub modified: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}

impl StatKey {
    /// `None` when the platform reports no modification time (never trusted).
    pub fn of(metadata: &fs::Metadata) -> Option<Self> {
        if !metadata.is_file() {
            return None;
        }
        let modified = nanos(metadata.modified().ok()?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self {
                length: metadata.len(),
                modified,
                created: None,
                inode: Some(metadata.ino()),
                changed: metadata
                    .ctime()
                    .checked_mul(1_000_000_000)
                    .and_then(|s| s.checked_add(metadata.ctime_nsec())),
                mode: Some(metadata.mode()),
            })
        }
        #[cfg(not(unix))]
        {
            Some(Self {
                length: metadata.len(),
                modified,
                created: metadata.created().ok().and_then(nanos),
                inode: None,
                changed: None,
                mode: Some(u32::from(metadata.permissions().readonly())),
            })
        }
    }
}

impl StatKey {
    /// From a Windows directory listing (FILETIME values, attributes): the
    /// same key `StatKey::of` derives from that file's std metadata.
    pub fn from_listing(
        length: u64,
        last_write: i64,
        creation: i64,
        attributes: u32,
    ) -> Option<Self> {
        Some(Self {
            length,
            modified: filetime_nanos(last_write)?,
            created: Some(filetime_nanos(creation)?),
            inode: None,
            changed: None,
            mode: Some(attributes & 1),
        })
    }
}

/// FILETIME (100 ns since 1601) to nanoseconds since the Unix epoch.
fn filetime_nanos(value: i64) -> Option<i64> {
    const EPOCH_DIFFERENCE: i64 = 116_444_736_000_000_000;
    value.checked_sub(EPOCH_DIFFERENCE)?.checked_mul(100)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexEntry {
    pub file_id: FileId,
    pub length: u64,
    pub executable: bool,
    pub stat: StatKey,
    /// When this content was proven (nanoseconds since the Unix epoch),
    /// taken no later than the observation that produced `stat`.
    pub verified_at: i64,
    /// Native file number of a virtual placeholder created for this path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    schema: String,
    worktree_id: WorktreeId,
    margin_nanos: i64,
    entries: BTreeMap<String, IndexEntry>,
}

#[derive(Clone, Debug)]
pub struct ChangeIndex {
    worktree_id: WorktreeId,
    margin_nanos: i64,
    entries: BTreeMap<String, Slot>,
    binding: Option<Binding>,
    /// Loaded empty because the stored index could not be read (a busy or
    /// unusable cache database): never saved over it (council s12 r1).
    unreadable: bool,
}

/// The projection an index is bound to: its `projection_digest`, which
/// alone decides whether a supplied projection is that one, and a Tree
/// whose checkout projection it is (where to find it again).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    tree: TreeId,
    digest: [u8; 32],
}

/// One recorded path.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Slot {
    /// A proven entry; `bound` when its FileId, length and executable bit
    /// equal the bound projection's entry for the path (written without
    /// them).
    Known { entry: IndexEntry, bound: bool },
    /// Read without its FileId, length and executable bit, before the bound
    /// projection was supplied: never trusted.
    Unresolved(Omitted),
}

/// What an entry written against the bound projection keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Omitted {
    /// `None` when the listed length is the projection's length.
    stat_length: Option<u64>,
    /// The listing key; its `length` is set from the projection on resolve.
    stat: StatKey,
    verified_at: i64,
    native: Option<u64>,
}

impl Slot {
    fn known(&self) -> Option<&IndexEntry> {
        match self {
            Slot::Known { entry, .. } => Some(entry),
            Slot::Unresolved(_) => None,
        }
    }
}

/// Digest of a checkout projection, entries in path order (council s3, D1):
/// what binds an index to exactly the projection it was compared against.
pub fn projection_digest(projection: &[CheckoutProjectionEntry]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs.change-index.projection.v1\0");
    let mut buffer = Vec::with_capacity(256);
    for entry in projection {
        put_varint(&mut buffer, entry.path.len() as u64);
        buffer.extend_from_slice(entry.path.as_bytes());
        match &entry.content {
            CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            } => {
                buffer.push(0);
                buffer.extend_from_slice(&file_id.digest_bytes());
                put_varint(&mut buffer, *byte_length);
                buffer.push(u8::from(*executable));
            }
            CheckoutProjectionContent::Symlink { target } => {
                buffer.push(1);
                put_varint(&mut buffer, target.len() as u64);
                buffer.extend_from_slice(target.as_bytes());
            }
        }
        if buffer.len() >= 64 * 1024 {
            hasher.update(&buffer);
            buffer.clear();
        }
    }
    hasher.update(&buffer);
    *hasher.finalize().as_bytes()
}

/// Looks up a projection's files by path, for paths asked in increasing
/// order (a merge walk over two sorted sequences).
struct ProjectionCursor<'a> {
    projection: &'a [CheckoutProjectionEntry],
    at: usize,
}

impl<'a> ProjectionCursor<'a> {
    fn new(projection: &'a [CheckoutProjectionEntry]) -> Self {
        Self { projection, at: 0 }
    }

    /// The file content at `path`: FileId, length, executable bit.
    fn file(&mut self, path: &str) -> Option<(&'a FileId, u64, bool)> {
        while let Some(entry) = self.projection.get(self.at) {
            match entry.path.as_str().cmp(path) {
                std::cmp::Ordering::Less => self.at += 1,
                std::cmp::Ordering::Greater => return None,
                std::cmp::Ordering::Equal => {
                    return match &entry.content {
                        CheckoutProjectionContent::File {
                            file_id,
                            byte_length,
                            executable,
                        } => Some((file_id, *byte_length, *executable)),
                        CheckoutProjectionContent::Symlink { .. } => None,
                    };
                }
            }
        }
        None
    }
}

impl ChangeIndex {
    /// Where an index was kept before the derived cache database: still read
    /// when the database has none, and removed once it is written there.
    fn file(store_root: &Path, worktree_id: &WorktreeId) -> PathBuf {
        let name: String = worktree_id
            .as_str()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        store_root
            .join("metadata")
            .join(DIRECTORY)
            .join(format!("{name}.json"))
    }

    /// Turns the mechanism on for a Worktree whose checkout is `checkout_root`
    /// (an empty index). Existing entries are kept.
    pub fn enable(
        store_root: &Path,
        worktree_id: &WorktreeId,
        checkout_root: &Path,
    ) -> io::Result<()> {
        if Self::is_enabled(store_root, worktree_id) {
            return Ok(());
        }
        Self {
            worktree_id: worktree_id.clone(),
            margin_nanos: margin_nanos(checkout_root),
            entries: BTreeMap::new(),
            binding: None,
            unreadable: false,
        }
        .save(store_root)
    }

    /// Whether an index exists for this Worktree (an unreadable cache
    /// database may hold one: it counts as existing, read as empty).
    pub fn is_enabled(store_root: &Path, worktree_id: &WorktreeId) -> bool {
        match super::derived_cache::change_index_presence(store_root, worktree_id.as_str()) {
            super::derived_cache::Presence::Present | super::derived_cache::Presence::Unknown => {
                true
            }
            super::derived_cache::Presence::Missing => {
                fs::symlink_metadata(Self::file(store_root, worktree_id)).is_ok_and(|m| m.is_file())
            }
        }
    }

    /// `None` when the mechanism is off for this Worktree. A damaged index is
    /// returned empty, with the conservative margin: every file is read.
    pub fn load(store_root: &Path, worktree_id: &WorktreeId) -> Option<Self> {
        let _span = crate::perf::span("change_index.load");
        let empty = || Self {
            worktree_id: worktree_id.clone(),
            margin_nanos: COARSE_MARGIN_NANOS,
            entries: BTreeMap::new(),
            binding: None,
            unreadable: false,
        };
        let read = crate::perf::span("change_index.read");
        let bytes = match super::derived_cache::change_index_get(
            store_root,
            worktree_id.as_str(),
            MAX_INDEX_BYTES,
        ) {
            super::derived_cache::Stored::Present(bytes) => Some(bytes),
            super::derived_cache::Stored::Unreadable => {
                return Some(Self {
                    unreadable: true,
                    ..empty()
                });
            }
            // A damaged value: read as empty, and replaced by the next save.
            super::derived_cache::Stored::Malformed => None,
            super::derived_cache::Stored::Missing => {
                let path = Self::file(store_root, worktree_id);
                let metadata = fs::symlink_metadata(&path).ok()?;
                if !metadata.is_file() || metadata.len() > MAX_INDEX_BYTES {
                    return Some(empty());
                }
                fs::read(&path).ok()
            }
        };
        drop(read);
        let _decode = crate::perf::span("change_index.decode");
        let parsed = bytes
            .and_then(|bytes| {
                if bytes.starts_with(BOUND_MAGIC) {
                    decode_bound(&bytes)
                } else {
                    let document = if bytes.starts_with(COMPACT_MAGIC) {
                        decode_compact(&bytes)
                    } else if bytes.starts_with(BINARY_MAGIC) {
                        decode_binary(&bytes)
                    } else {
                        // An index written before the binary format (a cache).
                        serde_json::from_slice::<Document>(&bytes).ok()
                    }?;
                    Some(Decoded::from(document))
                }
            })
            .filter(|document| {
                document.schema == SCHEMA
                    && &document.worktree_id == worktree_id
                    && document.margin_nanos >= FINE_MARGIN_NANOS
            });
        Some(match parsed {
            Some(document) => Self {
                worktree_id: document.worktree_id,
                margin_nanos: document.margin_nanos,
                entries: document.entries,
                binding: document.binding,
                unreadable: false,
            },
            None => empty(),
        })
    }

    /// Replaces the stored index in the derived cache database, without
    /// sync: this is a cache, and a lost or torn update only makes the next
    /// capture read more (a torn value fails its checksum).
    pub fn save(&self, store_root: &Path) -> io::Result<()> {
        let _span = crate::perf::span("change_index.save");
        if self.unreadable {
            crate::perf::count("change_index.save_skipped_unreadable", 1);
            return Ok(());
        }
        let bytes = encode_bound(self);
        super::derived_cache::change_index_put(store_root, self.worktree_id.as_str(), &bytes)?;
        let legacy = Self::file(store_root, &self.worktree_id);
        if fs::symlink_metadata(&legacy).is_ok() {
            let _ = fs::remove_file(&legacy);
        }
        Ok(())
    }

    pub fn remove(store_root: &Path, worktree_id: &WorktreeId) -> io::Result<()> {
        super::derived_cache::change_index_delete(store_root, worktree_id.as_str());
        match fs::remove_file(Self::file(store_root, worktree_id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// The stored bytes of an index (tests that damage them).
    #[cfg(test)]
    fn stored_bytes(store_root: &Path, worktree_id: &WorktreeId) -> Vec<u8> {
        match super::derived_cache::change_index_get(store_root, worktree_id.as_str(), u64::MAX) {
            super::derived_cache::Stored::Present(bytes) => bytes,
            _ => panic!("no stored index"),
        }
    }

    #[cfg(test)]
    fn store_bytes(store_root: &Path, worktree_id: &WorktreeId, bytes: &[u8]) {
        super::derived_cache::change_index_put(store_root, worktree_id.as_str(), bytes).unwrap();
    }

    /// The proven entry for `path` if `stat` still identifies it.
    pub fn trusted(&self, path: &str, stat: &StatKey) -> Option<&IndexEntry> {
        let entry = self.entries.get(path)?.known()?;
        (entry.stat == *stat
            && entry.length == stat.length
            && stat
                .modified
                .checked_add(self.margin_nanos)
                .is_some_and(|limit| limit < entry.verified_at))
        .then_some(entry)
    }

    /// The proven entry for `path`; `None` also for an entry whose bound
    /// projection was not supplied (`resolve`).
    pub fn entry(&self, path: &str) -> Option<&IndexEntry> {
        self.entries.get(path)?.known()
    }
    /// Same recorded entries and binding (and so nothing to write).
    pub fn same_entries(&self, other: &Self) -> bool {
        self.worktree_id == other.worktree_id
            && self.margin_nanos == other.margin_nanos
            && self.binding == other.binding
            && self.entries == other.entries
    }
    /// Every recorded path, resolved or not.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
    pub fn insert(&mut self, path: String, entry: IndexEntry) {
        self.entries.insert(
            path,
            Slot::Known {
                entry,
                bound: false,
            },
        );
    }
    pub fn remove_path(&mut self, path: &str) {
        self.entries.remove(path);
    }
    /// Keeps the proven entries `keep` accepts, and every unresolved one.
    pub fn retain(&mut self, mut keep: impl FnMut(&str, &IndexEntry) -> bool) {
        self.entries.retain(|path, slot| match slot {
            Slot::Known { entry, .. } => keep(path, entry),
            Slot::Unresolved(_) => true,
        });
    }

    /// The root Tree this index is bound to, if any.
    pub fn bound_tree(&self) -> Option<&TreeId> {
        self.binding.as_ref().map(|binding| &binding.tree)
    }

    /// Whether some entries wait for the bound projection (`resolve`).
    pub fn has_unresolved(&self) -> bool {
        self.entries
            .values()
            .any(|slot| matches!(slot, Slot::Unresolved(_)))
    }

    /// Whether the index is bound to the projection of this digest.
    pub fn is_bound_to(&self, digest: &[u8; 32]) -> bool {
        self.binding
            .as_ref()
            .is_some_and(|binding| &binding.digest == digest)
    }

    /// Binds the index to `projection`, the checkout projection of `tree`:
    /// each proven entry equal to the projection's entry for its path is
    /// then written without its FileId, length and executable bit. Entries
    /// still waiting for another projection are dropped (their files are
    /// read again).
    pub fn bind(&mut self, tree: &TreeId, projection: &[CheckoutProjectionEntry]) {
        let digest = projection_digest(projection);
        self.bind_digested(tree, projection, &digest);
    }

    /// `bind` with `digest`, which must be `projection_digest(projection)`.
    pub fn bind_digested(
        &mut self,
        tree: &TreeId,
        projection: &[CheckoutProjectionEntry],
        digest: &[u8; 32],
    ) {
        let _span = crate::perf::span("change_index.bind");
        if !self.is_bound_to(digest) {
            self.entries
                .retain(|_, slot| !matches!(slot, Slot::Unresolved(_)));
        }
        let mut cursor = ProjectionCursor::new(projection);
        for (path, slot) in &mut self.entries {
            if let Slot::Known { entry, bound } = slot {
                *bound =
                    cursor.file(path) == Some((&entry.file_id, entry.length, entry.executable));
            }
        }
        self.binding = Some(Binding {
            tree: tree.clone(),
            digest: *digest,
        });
    }

    /// Supplies a checkout projection. If it is exactly the projection the
    /// index is bound to (same digest), every unresolved entry takes its
    /// FileId, length and executable bit from it, and this returns true.
    /// Otherwise nothing changes: unresolved entries stay untrusted.
    pub fn resolve(&mut self, projection: &[CheckoutProjectionEntry]) -> bool {
        if self.binding.is_none() {
            return false;
        }
        let digest = projection_digest(projection);
        self.resolve_digested(projection, &digest)
    }

    /// `resolve` with `digest`, which must be `projection_digest(projection)`.
    pub fn resolve_digested(
        &mut self,
        projection: &[CheckoutProjectionEntry],
        digest: &[u8; 32],
    ) -> bool {
        if !self.is_bound_to(digest) {
            if self.binding.is_some() {
                crate::perf::count("change_index.other_projection", 1);
            }
            return false;
        }
        if !self.has_unresolved() {
            return true;
        }
        let _span = crate::perf::span("change_index.resolve");
        let mut cursor = ProjectionCursor::new(projection);
        let mut lost = Vec::new();
        for (path, slot) in &mut self.entries {
            let Slot::Unresolved(omitted) = *slot else {
                continue;
            };
            match cursor.file(path) {
                Some((file_id, length, executable)) => {
                    *slot = Slot::Known {
                        entry: IndexEntry {
                            file_id: file_id.clone(),
                            length,
                            executable,
                            stat: StatKey {
                                length: omitted.stat_length.unwrap_or(length),
                                ..omitted.stat
                            },
                            verified_at: omitted.verified_at,
                            native: omitted.native,
                        },
                        bound: true,
                    };
                }
                // Written against this very projection, which has no file
                // there: not a sound entry.
                None => lost.push(path.clone()),
            }
        }
        for path in lost {
            self.entries.remove(&path);
        }
        true
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    /// This index without its entries (same Worktree, margin and binding),
    /// without copying them.
    pub fn emptied(&self) -> Self {
        Self {
            worktree_id: self.worktree_id.clone(),
            margin_nanos: self.margin_nanos,
            entries: BTreeMap::new(),
            binding: self.binding.clone(),
            unreadable: self.unreadable,
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn worktree_id(&self) -> &WorktreeId {
        &self.worktree_id
    }
}

/// Compact encoding (ADR 0028, about 3 times smaller than `LYRSIDX1`): the
/// same document with 32-byte FileIds, each path stored as the length it
/// shares with the previous path plus its own suffix, unsigned and zigzag
/// varints, and times as differences (modification from the previous
/// entry's, the others from the entry's own). Every field is kept. A blake3
/// trailer covers it; a damaged file decodes to nothing (treated as empty).
const COMPACT_MAGIC: &[u8; 8] = b"LYRSIDX2";

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

#[cfg(test)]
fn file_digest(id: &FileId) -> Option<[u8; 32]> {
    Some(id.digest_bytes())
}

#[cfg(test)]
fn encode_compact(document: &Document) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + document.entries.len() * 64);
    out.extend_from_slice(COMPACT_MAGIC);
    for text in [document.schema.as_str(), document.worktree_id.as_str()] {
        put_varint(&mut out, text.len() as u64);
        out.extend_from_slice(text.as_bytes());
    }
    out.extend_from_slice(&document.margin_nanos.to_le_bytes());
    put_varint(&mut out, document.entries.len() as u64);
    let mut previous_path: &[u8] = &[];
    let mut previous_modified = 0_i64;
    for (path, entry) in &document.entries {
        let Some(digest) = file_digest(&entry.file_id) else {
            // Not representable compactly: write the former encoding.
            return encode_binary(document);
        };
        let path = path.as_bytes();
        let shared = path
            .iter()
            .zip(previous_path)
            .take_while(|(a, b)| a == b)
            .count();
        put_varint(&mut out, shared as u64);
        put_varint(&mut out, (path.len() - shared) as u64);
        out.extend_from_slice(&path[shared..]);
        previous_path = path;
        out.extend_from_slice(&digest);
        put_varint(&mut out, entry.length);
        let stat = &entry.stat;
        let flags = u8::from(entry.executable)
            | (u8::from(stat.created.is_some()) << 1)
            | (u8::from(stat.inode.is_some()) << 2)
            | (u8::from(stat.changed.is_some()) << 3)
            | (u8::from(stat.mode.is_some()) << 4)
            | (u8::from(entry.native.is_some()) << 5)
            | (u8::from(stat.length != entry.length) << 6);
        out.push(flags);
        if stat.length != entry.length {
            put_varint(&mut out, stat.length);
        }
        put_signed(&mut out, stat.modified.wrapping_sub(previous_modified));
        previous_modified = stat.modified;
        if let Some(created) = stat.created {
            put_signed(&mut out, created.wrapping_sub(stat.modified));
        }
        if let Some(inode) = stat.inode {
            put_varint(&mut out, inode);
        }
        if let Some(changed) = stat.changed {
            put_signed(&mut out, changed.wrapping_sub(stat.modified));
        }
        if let Some(mode) = stat.mode {
            put_varint(&mut out, u64::from(mode));
        }
        put_signed(&mut out, entry.verified_at.wrapping_sub(stat.modified));
        if let Some(native) = entry.native {
            put_varint(&mut out, native);
        }
    }
    let checksum = blake3::hash(&out);
    out.extend_from_slice(checksum.as_bytes());
    out
}

/// Bound encoding (council s3, D1): `LYRSIDX2`'s fields, the index's
/// binding (root Tree and projection digest) in the header, and per entry a
/// flag (0x80) for an entry written without its FileId, length and
/// executable bit, which come from the bound projection. The flags byte
/// precedes the FileId.
const BOUND_MAGIC: &[u8; 8] = b"LYRSIDX3";
const FLAG_BOUND: u8 = 0x80;

/// A decoded index file.
struct Decoded {
    schema: String,
    worktree_id: WorktreeId,
    margin_nanos: i64,
    binding: Option<Binding>,
    entries: BTreeMap<String, Slot>,
}

impl From<Document> for Decoded {
    fn from(document: Document) -> Self {
        Self {
            schema: document.schema,
            worktree_id: document.worktree_id,
            margin_nanos: document.margin_nanos,
            binding: None,
            entries: document
                .entries
                .into_iter()
                .map(|(path, entry)| {
                    (
                        path,
                        Slot::Known {
                            entry,
                            bound: false,
                        },
                    )
                })
                .collect(),
        }
    }
}

fn encode_bound(index: &ChangeIndex) -> Vec<u8> {
    let mut out = Vec::with_capacity(96 + index.entries.len() * 24);
    out.extend_from_slice(BOUND_MAGIC);
    for text in [SCHEMA, index.worktree_id.as_str()] {
        put_varint(&mut out, text.len() as u64);
        out.extend_from_slice(text.as_bytes());
    }
    out.extend_from_slice(&index.margin_nanos.to_le_bytes());
    match &index.binding {
        None => out.push(0),
        Some(binding) => {
            out.push(1);
            put_varint(&mut out, binding.tree.as_str().len() as u64);
            out.extend_from_slice(binding.tree.as_str().as_bytes());
            out.extend_from_slice(&binding.digest);
        }
    }
    let bound = index.binding.is_some();
    // Unresolved entries exist only under a binding (they were read under
    // one, and another binding drops them).
    let count = index
        .entries
        .values()
        .filter(|slot| bound || matches!(slot, Slot::Known { .. }))
        .count();
    put_varint(&mut out, count as u64);
    let mut previous_path: &[u8] = &[];
    let mut previous_modified = 0_i64;
    for (path, slot) in &index.entries {
        // (flags of the FileId part, explicit entry, listed length if it
        // differs, listing key, proof time, native number)
        let (head, explicit, stat_length, stat, verified_at, native) = match slot {
            Slot::Known { entry, bound: omit } => {
                let omit = *omit && bound;
                let differs = entry.stat.length != entry.length;
                (
                    if omit {
                        FLAG_BOUND
                    } else {
                        u8::from(entry.executable)
                    },
                    (!omit).then_some(entry),
                    differs.then_some(entry.stat.length),
                    entry.stat,
                    entry.verified_at,
                    entry.native,
                )
            }
            Slot::Unresolved(omitted) if bound => (
                FLAG_BOUND,
                None,
                omitted.stat_length,
                omitted.stat,
                omitted.verified_at,
                omitted.native,
            ),
            Slot::Unresolved(_) => continue,
        };
        let path = path.as_bytes();
        let shared = path
            .iter()
            .zip(previous_path)
            .take_while(|(a, b)| a == b)
            .count();
        put_varint(&mut out, shared as u64);
        put_varint(&mut out, (path.len() - shared) as u64);
        out.extend_from_slice(&path[shared..]);
        previous_path = path;
        let flags = head
            | (u8::from(stat.created.is_some()) << 1)
            | (u8::from(stat.inode.is_some()) << 2)
            | (u8::from(stat.changed.is_some()) << 3)
            | (u8::from(stat.mode.is_some()) << 4)
            | (u8::from(native.is_some()) << 5)
            | (u8::from(stat_length.is_some()) << 6);
        out.push(flags);
        if let Some(entry) = explicit {
            out.extend_from_slice(&entry.file_id.digest_bytes());
            put_varint(&mut out, entry.length);
        }
        if let Some(length) = stat_length {
            put_varint(&mut out, length);
        }
        put_signed(&mut out, stat.modified.wrapping_sub(previous_modified));
        previous_modified = stat.modified;
        if let Some(created) = stat.created {
            put_signed(&mut out, created.wrapping_sub(stat.modified));
        }
        if let Some(inode) = stat.inode {
            put_varint(&mut out, inode);
        }
        if let Some(changed) = stat.changed {
            put_signed(&mut out, changed.wrapping_sub(stat.modified));
        }
        if let Some(mode) = stat.mode {
            put_varint(&mut out, u64::from(mode));
        }
        put_signed(&mut out, verified_at.wrapping_sub(stat.modified));
        if let Some(native) = native {
            put_varint(&mut out, native);
        }
    }
    let checksum = blake3::hash(&out);
    out.extend_from_slice(checksum.as_bytes());
    out
}

fn decode_bound(bytes: &[u8]) -> Option<Decoded> {
    if bytes.len() < BOUND_MAGIC.len() + 32 {
        return None;
    }
    let (body, trailer) = bytes.split_at(bytes.len() - 32);
    if blake3::hash(body).as_bytes() != trailer {
        return None;
    }
    let mut reader = CompactReader(&body[BOUND_MAGIC.len()..]);
    let schema = reader.text()?.to_owned();
    let worktree_id = WorktreeId::parse(reader.text()?).ok()?;
    let margin_nanos = i64::from_le_bytes(reader.take(8)?.try_into().ok()?);
    let binding = match reader.take(1)?[0] {
        0 => None,
        1 => Some(Binding {
            tree: TreeId::parse(reader.text()?).ok()?,
            digest: reader.take(32)?.try_into().ok()?,
        }),
        _ => return None,
    };
    let count = usize::try_from(reader.varint()?).ok()?;
    let mut entries: Vec<(String, Slot)> = Vec::with_capacity(count.min(1 << 20));
    let mut previous_path: Vec<u8> = Vec::new();
    let mut previous_modified = 0_i64;
    for index in 0..count {
        let shared = usize::try_from(reader.varint()?).ok()?;
        let suffix = usize::try_from(reader.varint()?).ok()?;
        if shared > previous_path.len() {
            return None;
        }
        let tail = reader.take(suffix)?;
        // Strictly greater than the previous path.
        if index > 0 && tail <= &previous_path[shared..] {
            return None;
        }
        previous_path.truncate(shared);
        previous_path.extend_from_slice(tail);
        let path = std::str::from_utf8(&previous_path).ok()?.to_owned();
        let flags = reader.take(1)?[0];
        let omitted = flags & FLAG_BOUND != 0;
        // A bound entry needs a binding, which also gives its executable bit.
        if omitted && (binding.is_none() || flags & 0x01 != 0) {
            return None;
        }
        let explicit = if omitted {
            None
        } else {
            let digest: [u8; 32] = reader.take(32)?.try_into().ok()?;
            Some((FileId::from_digest_bytes(&digest), reader.varint()?))
        };
        let stat_length = reader.when(flags & 0x40 != 0)?;
        let modified = previous_modified.wrapping_add(reader.signed()?);
        previous_modified = modified;
        let created = if flags & 0x02 != 0 {
            Some(modified.wrapping_add(reader.signed()?))
        } else {
            None
        };
        let inode = reader.when(flags & 0x04 != 0)?;
        let changed = if flags & 0x08 != 0 {
            Some(modified.wrapping_add(reader.signed()?))
        } else {
            None
        };
        let mode = match reader.when(flags & 0x10 != 0)? {
            Some(mode) => Some(u32::try_from(mode).ok()?),
            None => None,
        };
        let verified_at = modified.wrapping_add(reader.signed()?);
        let native = reader.when(flags & 0x20 != 0)?;
        let slot = match explicit {
            Some((file_id, length)) => {
                // One encoding per document: the flag means "differs".
                if stat_length == Some(length) {
                    return None;
                }
                Slot::Known {
                    entry: IndexEntry {
                        file_id,
                        length,
                        executable: flags & 0x01 != 0,
                        stat: StatKey {
                            length: stat_length.unwrap_or(length),
                            modified,
                            created,
                            inode,
                            changed,
                            mode,
                        },
                        verified_at,
                        native,
                    },
                    bound: false,
                }
            }
            None => Slot::Unresolved(Omitted {
                stat_length,
                stat: StatKey {
                    length: 0,
                    modified,
                    created,
                    inode,
                    changed,
                    mode,
                },
                verified_at,
                native,
            }),
        };
        entries.push((path, slot));
    }
    reader.0.is_empty().then(|| Decoded {
        schema,
        worktree_id,
        margin_nanos,
        binding,
        entries: entries.into_iter().collect(),
    })
}

struct CompactReader<'a>(&'a [u8]);

impl<'a> CompactReader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.0.len() < count {
            return None;
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Some(head)
    }
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.take(1)?[0];
            value |= u64::from(byte & 0x7f).checked_shl(shift)?;
            if byte & 0x80 == 0 {
                // Minimal encodings only: one input, one document.
                return (shift == 0 || byte != 0).then_some(value);
            }
        }
        None
    }
    fn signed(&mut self) -> Option<i64> {
        let value = self.varint()?;
        Some(((value >> 1) as i64) ^ -((value & 1) as i64))
    }
    fn text(&mut self) -> Option<&'a str> {
        let length = usize::try_from(self.varint()?).ok()?;
        std::str::from_utf8(self.take(length)?).ok()
    }
    fn when(&mut self, present: bool) -> Option<Option<u64>> {
        if present {
            self.varint().map(Some)
        } else {
            Some(None)
        }
    }
}

fn decode_compact(bytes: &[u8]) -> Option<Document> {
    if bytes.len() < COMPACT_MAGIC.len() + 32 {
        return None;
    }
    let (body, trailer) = bytes.split_at(bytes.len() - 32);
    if blake3::hash(body).as_bytes() != trailer {
        return None;
    }
    let mut reader = CompactReader(&body[COMPACT_MAGIC.len()..]);
    let schema = reader.text()?.to_owned();
    let worktree_id = WorktreeId::parse(reader.text()?).ok()?;
    let margin_nanos = i64::from_le_bytes(reader.take(8)?.try_into().ok()?);
    let count = usize::try_from(reader.varint()?).ok()?;
    // Written in strictly increasing path order (from a map): checked while
    // decoding, then the map is built in one pass.
    let mut entries: Vec<(String, IndexEntry)> = Vec::with_capacity(count.min(1 << 20));
    let mut previous_path: Vec<u8> = Vec::new();
    let mut previous_modified = 0_i64;
    for index in 0..count {
        let shared = usize::try_from(reader.varint()?).ok()?;
        let suffix = usize::try_from(reader.varint()?).ok()?;
        if shared > previous_path.len() {
            return None;
        }
        let tail = reader.take(suffix)?;
        // Strictly greater than the previous path: at the first differing
        // byte (or, with the previous path as a prefix, by being longer).
        if index > 0 {
            let previous_tail = &previous_path[shared..];
            if tail <= previous_tail {
                return None;
            }
        }
        previous_path.truncate(shared);
        previous_path.extend_from_slice(tail);
        let path = std::str::from_utf8(&previous_path).ok()?.to_owned();
        let digest: [u8; 32] = reader.take(32)?.try_into().ok()?;
        let file_id = FileId::from_digest_bytes(&digest);
        let length = reader.varint()?;
        let flags = reader.take(1)?[0];
        if flags & 0x80 != 0 {
            return None;
        }
        let stat_length = reader.when(flags & 0x40 != 0)?.unwrap_or(length);
        let modified = previous_modified.wrapping_add(reader.signed()?);
        previous_modified = modified;
        let created = if flags & 0x02 != 0 {
            Some(modified.wrapping_add(reader.signed()?))
        } else {
            None
        };
        let inode = reader.when(flags & 0x04 != 0)?;
        let changed = if flags & 0x08 != 0 {
            Some(modified.wrapping_add(reader.signed()?))
        } else {
            None
        };
        let mode = match reader.when(flags & 0x10 != 0)? {
            Some(mode) => Some(u32::try_from(mode).ok()?),
            None => None,
        };
        let verified_at = modified.wrapping_add(reader.signed()?);
        let native = reader.when(flags & 0x20 != 0)?;
        let entry = IndexEntry {
            file_id,
            length,
            executable: flags & 0x01 != 0,
            stat: StatKey {
                length: stat_length,
                modified,
                created,
                inode,
                changed,
                mode,
            },
            verified_at,
            native,
        };
        entries.push((path, entry));
    }
    reader.0.is_empty().then(|| Document {
        schema,
        worktree_id,
        margin_nanos,
        entries: entries.into_iter().collect(),
    })
}

/// Former binary encoding (`LYRSIDX1`): still read, no longer written.
const BINARY_MAGIC: &[u8; 8] = b"LYRSIDX1";

#[cfg(test)]
fn encode_binary(document: &Document) -> Vec<u8> {
    fn string(out: &mut Vec<u8>, value: &str) {
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    fn optional(out: &mut Vec<u8>, value: Option<[u8; 8]>) {
        match value {
            Some(bytes) => {
                out.push(1);
                out.extend_from_slice(&bytes);
            }
            None => out.push(0),
        }
    }
    let mut out = Vec::with_capacity(64 + document.entries.len() * 160);
    out.extend_from_slice(BINARY_MAGIC);
    string(&mut out, &document.schema);
    string(&mut out, document.worktree_id.as_str());
    out.extend_from_slice(&document.margin_nanos.to_le_bytes());
    out.extend_from_slice(&(document.entries.len() as u32).to_le_bytes());
    for (path, entry) in &document.entries {
        string(&mut out, path);
        string(&mut out, entry.file_id.as_str());
        out.extend_from_slice(&entry.length.to_le_bytes());
        out.push(u8::from(entry.executable));
        out.extend_from_slice(&entry.stat.length.to_le_bytes());
        out.extend_from_slice(&entry.stat.modified.to_le_bytes());
        optional(&mut out, entry.stat.created.map(i64::to_le_bytes));
        optional(&mut out, entry.stat.inode.map(u64::to_le_bytes));
        optional(&mut out, entry.stat.changed.map(i64::to_le_bytes));
        optional(
            &mut out,
            entry.stat.mode.map(|mode| u64::from(mode).to_le_bytes()),
        );
        out.extend_from_slice(&entry.verified_at.to_le_bytes());
        optional(&mut out, entry.native.map(u64::to_le_bytes));
    }
    let checksum = blake3::hash(&out);
    out.extend_from_slice(checksum.as_bytes());
    out
}

fn decode_binary(bytes: &[u8]) -> Option<Document> {
    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, count: usize) -> Option<&'a [u8]> {
            if self.0.len() < count {
                return None;
            }
            let (head, tail) = self.0.split_at(count);
            self.0 = tail;
            Some(head)
        }
        fn eight(&mut self) -> Option<[u8; 8]> {
            self.take(8)?.try_into().ok()
        }
        fn u32(&mut self) -> Option<u32> {
            Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
        fn string(&mut self) -> Option<&'a str> {
            let length = usize::try_from(self.u32()?).ok()?;
            std::str::from_utf8(self.take(length)?).ok()
        }
        fn optional(&mut self) -> Option<Option<[u8; 8]>> {
            match self.take(1)?[0] {
                0 => Some(None),
                1 => Some(Some(self.eight()?)),
                _ => None,
            }
        }
    }
    if bytes.len() < BINARY_MAGIC.len() + 32 {
        return None;
    }
    let (body, trailer) = bytes.split_at(bytes.len() - 32);
    if blake3::hash(body).as_bytes() != trailer {
        return None;
    }
    let mut reader = Reader(&body[BINARY_MAGIC.len()..]);
    let schema = reader.string()?.to_owned();
    let worktree_id = WorktreeId::parse(reader.string()?).ok()?;
    let margin_nanos = i64::from_le_bytes(reader.eight()?);
    let count = reader.u32()? as usize;
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        let path = reader.string()?.to_owned();
        let file_id = FileId::parse(reader.string()?).ok()?;
        let length = u64::from_le_bytes(reader.eight()?);
        let executable = match reader.take(1)?[0] {
            0 => false,
            1 => true,
            _ => return None,
        };
        let stat = StatKey {
            length: u64::from_le_bytes(reader.eight()?),
            modified: i64::from_le_bytes(reader.eight()?),
            created: reader.optional()?.map(i64::from_le_bytes),
            inode: reader.optional()?.map(u64::from_le_bytes),
            changed: reader.optional()?.map(i64::from_le_bytes),
            mode: match reader.optional()? {
                Some(bytes) => Some(u32::try_from(u64::from_le_bytes(bytes)).ok()?),
                None => None,
            },
        };
        let verified_at = i64::from_le_bytes(reader.eight()?);
        let native = reader.optional()?.map(u64::from_le_bytes);
        if entries
            .insert(
                path,
                IndexEntry {
                    file_id,
                    length,
                    executable,
                    stat,
                    verified_at,
                    native,
                },
            )
            .is_some()
        {
            return None;
        }
    }
    reader.0.is_empty().then_some(Document {
        schema,
        worktree_id,
        margin_nanos,
        entries,
    })
}

/// Nanoseconds since the Unix epoch (negative before it); `None` on overflow.
/// The key `StatKey::of` gives a regular file of `length` bytes, from its
/// creation and last write times (FILETIME intervals since 1601) and its
/// attributes, as one basic information request returns them.
#[cfg(windows)]
pub fn stat_from_basic(
    length: u64,
    created: i64,
    written: i64,
    attributes: u32,
) -> Option<StatKey> {
    // 100 ns intervals from 1601-01-01 to 1970-01-01.
    const UNIX_EPOCH_INTERVALS: i64 = 116_444_736_000_000_000;
    let since_epoch = |intervals: i64| {
        intervals
            .checked_sub(UNIX_EPOCH_INTERVALS)?
            .checked_mul(100)
    };
    const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
    Some(StatKey {
        length,
        modified: since_epoch(written)?,
        created: since_epoch(created),
        inode: None,
        changed: None,
        mode: Some(u32::from(attributes & FILE_ATTRIBUTE_READONLY != 0)),
    })
}

pub fn nanos(time: SystemTime) -> Option<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_nanos()).ok(),
        Err(before) => i64::try_from(before.duration().as_nanos()).ok().map(|n| -n),
    }
}

/// The current time, for `IndexEntry::verified_at`. Take it before the
/// observation it qualifies.
pub fn now() -> i64 {
    nanos(SystemTime::now()).unwrap_or(i64::MIN)
}

/// Timestamp margin for the volume holding `root`.
pub(crate) fn margin_nanos(root: &Path) -> i64 {
    #[cfg(windows)]
    {
        match volume_file_system(root).as_deref() {
            Some("NTFS" | "ReFS") => FINE_MARGIN_NANOS,
            _ => COARSE_MARGIN_NANOS,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = root;
        COARSE_MARGIN_NANOS / 2
    }
}

#[cfg(windows)]
pub(crate) fn volume_file_system(root: &Path) -> Option<String> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, GetVolumeInformationByHandleW,
    };
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(root)
        .ok()?;
    let mut name = [0u16; 64];
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            directory.as_raw_handle() as _,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return None;
    }
    let end = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..end]))
}

/// Whether `root` is on a local NTFS volume: the condition under which
/// directory updates are ordered by the NTFS journal and one flush makes
/// every earlier one durable. A share that merely reports NTFS (SMB, Samba)
/// is remote: it answers `FileRemoteProtocolInfo`, which a local volume does
/// not (council s2 r3).
#[cfg(windows)]
pub(crate) fn local_ntfs(root: &Path) -> bool {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_REMOTE_PROTOCOL_INFO, FileRemoteProtocolInfo,
        GetFileInformationByHandleEx,
    };
    if volume_file_system(root).as_deref() != Some("NTFS") {
        return false;
    }
    let Ok(directory) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(root)
    else {
        return false;
    };
    // SAFETY: an all-zero FILE_REMOTE_PROTOCOL_INFO is a valid output buffer.
    let mut info: FILE_REMOTE_PROTOCOL_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: `directory` is live; the buffer is writable and sized.
    let remote = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle() as _,
            FileRemoteProtocolInfo,
            (&raw mut info).cast(),
            std::mem::size_of::<FILE_REMOTE_PROTOCOL_INFO>() as u32,
        )
    };
    remote == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_temporary_folder_is_on_a_local_ntfs_volume_here() {
        // The machines that run this suite keep TEMP on a local NTFS disk.
        if volume_file_system(&std::env::temp_dir()).as_deref() == Some("NTFS") {
            assert!(local_ntfs(&std::env::temp_dir()));
        }
    }

    fn temp() -> PathBuf {
        let path = std::env::temp_dir().join(format!("layrs-change-index-{}", uuid_like()));
        fs::create_dir_all(path.join("metadata")).unwrap();
        path
    }
    fn uuid_like() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        format!(
            "{}-{}-{}",
            std::process::id(),
            now(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }
    fn worktree() -> WorktreeId {
        WorktreeId::parse("worktree:00000000-0000-4000-8000-000000000001".to_owned()).unwrap()
    }
    fn entry(stat: StatKey, verified_at: i64) -> IndexEntry {
        IndexEntry {
            file_id: FileId::from_bytes(b"x"),
            length: stat.length,
            executable: false,
            stat,
            verified_at,
            native: None,
        }
    }
    fn stat(modified: i64) -> StatKey {
        StatKey {
            length: 1,
            modified,
            created: Some(1),
            inode: None,
            changed: None,
            mode: None,
        }
    }
    /// The proven entries, as the former encodings hold them.
    fn known(index: &ChangeIndex) -> BTreeMap<String, IndexEntry> {
        index
            .entries
            .iter()
            .filter_map(|(path, slot)| Some((path.clone(), slot.known()?.clone())))
            .collect()
    }
    fn file(path: &str, content: &[u8], length: u64) -> CheckoutProjectionEntry {
        CheckoutProjectionEntry {
            path: path.into(),
            content: CheckoutProjectionContent::File {
                file_id: FileId::from_bytes(content),
                byte_length: length,
                executable: false,
            },
        }
    }
    fn tree(name: &[u8]) -> TreeId {
        TreeId::from_digest_bytes(blake3::hash(name).as_bytes())
    }

    #[test]
    fn binary_round_trips_every_field_reads_legacy_json_and_rejects_damage() {
        let store = temp();
        let id = worktree();
        ChangeIndex::enable(&store, &id, &store).unwrap();
        let mut index = ChangeIndex::load(&store, &id).unwrap();
        let full = StatKey {
            length: 7,
            modified: -5,
            created: Some(3),
            inode: Some(u64::MAX),
            changed: Some(i64::MIN),
            mode: Some(0o755),
        };
        let mut rich = entry(full, 99);
        rich.length = 7;
        rich.executable = true;
        rich.native = Some(42);
        index.insert("d/rich".into(), rich.clone());
        index.insert("plain".into(), entry(stat(10), 20));
        index.insert("plain2".into(), entry(stat(i64::MAX), i64::MIN));
        let mut mismatched = entry(stat(1), 2);
        mismatched.length = 8;
        index.insert("d/rich2".into(), mismatched);
        index.save(&store).unwrap();
        let bound = ChangeIndex::stored_bytes(&store, &id);
        assert!(bound.starts_with(BOUND_MAGIC));
        let reloaded = ChangeIndex::load(&store, &id).unwrap();
        assert!(reloaded.same_entries(&index));
        let document = Document {
            schema: SCHEMA.into(),
            worktree_id: id.clone(),
            margin_nanos: index.margin_nanos,
            entries: known(&index),
        };
        // The former encodings are still read, and are larger.
        let compact = encode_compact(&document);
        let former = encode_binary(&document);
        assert!(bound.len() <= compact.len() + 1);
        assert!(compact.len() < former.len());
        for bytes in [&compact, &former] {
            ChangeIndex::store_bytes(&store, &id, bytes);
            assert!(ChangeIndex::load(&store, &id).unwrap().same_entries(&index));
        }
        // Truncated or extended documents are refused.
        for (encoded, decode) in [
            (
                &compact,
                (|b: &[u8]| decode_compact(b).is_some()) as fn(&[u8]) -> bool,
            ),
            (&bound, |b: &[u8]| decode_bound(b).is_some()),
        ] {
            let body = &encoded[..encoded.len() - 32];
            for damaged in [&body[..body.len() - 1], &[body, &[0][..]].concat()[..]] {
                let mut bytes = damaged.to_vec();
                bytes.extend_from_slice(blake3::hash(damaged).as_bytes());
                assert!(!decode(&bytes));
            }
        }
        // A legacy JSON document is still read.
        let document = Document {
            schema: SCHEMA.into(),
            worktree_id: id.clone(),
            margin_nanos: index.margin_nanos,
            entries: known(&index),
        };
        ChangeIndex::store_bytes(&store, &id, &serde_json::to_vec(&document).unwrap());
        assert!(ChangeIndex::load(&store, &id).unwrap().same_entries(&index));
        // A damaged binary index loads empty: every file is read.
        index.save(&store).unwrap();
        let mut bytes = ChangeIndex::stored_bytes(&store, &id);
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        ChangeIndex::store_bytes(&store, &id, &bytes);
        assert!(ChangeIndex::load(&store, &id).unwrap().is_empty());
        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn disabled_until_enabled_and_round_trips() {
        let store = temp();
        let id = worktree();
        assert!(ChangeIndex::load(&store, &id).is_none());
        ChangeIndex::enable(&store, &id, &store).unwrap();
        let mut index = ChangeIndex::load(&store, &id).unwrap();
        assert!(index.is_empty());
        index.insert("a.txt".into(), entry(stat(10), 10 + 3_000_000_000));
        index.save(&store).unwrap();
        let reloaded = ChangeIndex::load(&store, &id).unwrap();
        assert_eq!(reloaded.entry("a.txt"), index.entry("a.txt"));
        // Enabling again keeps the entries.
        ChangeIndex::enable(&store, &id, &store).unwrap();
        assert_eq!(ChangeIndex::load(&store, &id).unwrap().len(), 1);
        ChangeIndex::remove(&store, &id).unwrap();
        assert!(ChangeIndex::load(&store, &id).is_none());
        // An index kept as a file by an earlier version is still read, and
        // moves to the database when saved again.
        let legacy = ChangeIndex::file(&store, &id);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, encode_bound(&index)).unwrap();
        assert!(ChangeIndex::is_enabled(&store, &id));
        let reloaded = ChangeIndex::load(&store, &id).unwrap();
        assert_eq!(reloaded.entry("a.txt"), index.entry("a.txt"));
        reloaded.save(&store).unwrap();
        assert!(!legacy.exists());
        assert_eq!(ChangeIndex::load(&store, &id).unwrap().len(), 1);
        let _ = fs::remove_dir_all(store);
    }

    #[test]
    fn racy_or_different_stats_are_never_trusted() {
        let store = temp();
        let id = worktree();
        ChangeIndex::enable(&store, &id, &store).unwrap();
        let mut index = ChangeIndex::load(&store, &id).unwrap();
        let margin = index.margin_nanos;
        index.insert("old".into(), entry(stat(0), margin + 1));
        index.insert("racy".into(), entry(stat(0), margin));
        assert!(index.trusted("old", &stat(0)).is_some());
        assert!(
            index.trusted("racy", &stat(0)).is_none(),
            "same tick as proof"
        );
        assert!(index.trusted("old", &stat(1)).is_none(), "other mtime");
        let mut grown = stat(0);
        grown.length = 2;
        assert!(index.trusted("old", &grown).is_none(), "other length");
        assert!(index.trusted("missing", &stat(0)).is_none());
        let _ = fs::remove_dir_all(store);
    }

    #[cfg(windows)]
    #[test]
    fn listing_and_metadata_derive_the_same_key() {
        use std::os::windows::fs::MetadataExt;
        let store = temp();
        let file = store.join("file.txt");
        fs::write(&file, b"content").unwrap();
        let metadata = fs::metadata(&file).unwrap();
        assert_eq!(
            StatKey::of(&metadata),
            StatKey::from_listing(
                metadata.len(),
                metadata.last_write_time() as i64,
                metadata.creation_time() as i64,
                metadata.file_attributes(),
            )
        );
        let _ = fs::remove_dir_all(store);
    }

    #[test]
    fn damaged_or_foreign_index_reads_as_empty() {
        let store = temp();
        let id = worktree();
        ChangeIndex::enable(&store, &id, &store).unwrap();
        ChangeIndex::store_bytes(&store, &id, b"{not json");
        let index = ChangeIndex::load(&store, &id).unwrap();
        assert!(index.is_empty());
        assert_eq!(index.margin_nanos, COARSE_MARGIN_NANOS);
        let other =
            WorktreeId::parse("worktree:00000000-0000-4000-8000-000000000002".to_owned()).unwrap();
        let mut foreign = ChangeIndex::load(&store, &id).unwrap();
        foreign.worktree_id = other;
        foreign.insert("a".into(), entry(stat(0), i64::MAX));
        let bytes = serde_json::to_vec(&Document {
            schema: SCHEMA.into(),
            worktree_id: foreign.worktree_id.clone(),
            margin_nanos: FINE_MARGIN_NANOS,
            entries: known(&foreign),
        })
        .unwrap();
        ChangeIndex::store_bytes(&store, &id, &bytes);
        assert!(ChangeIndex::load(&store, &id).unwrap().is_empty());
        let _ = fs::remove_dir_all(store);
    }

    #[test]
    fn a_bound_index_omits_what_the_projection_holds_and_trusts_it_only_once_resolved() {
        let store = temp();
        let id = worktree();
        ChangeIndex::enable(&store, &id, &store).unwrap();
        let mut index = ChangeIndex::load(&store, &id).unwrap();
        let margin = index.margin_nanos;
        let projection = vec![
            file("a.txt", b"a", 1),
            file("b.txt", b"b", 1),
            CheckoutProjectionEntry {
                path: "link".into(),
                content: CheckoutProjectionContent::Symlink {
                    target: "a.txt".into(),
                },
            },
        ];
        let proven = |content: &[u8]| IndexEntry {
            file_id: FileId::from_bytes(content),
            length: 1,
            executable: false,
            stat: stat(0),
            verified_at: margin + 1,
            native: None,
        };
        // a.txt equals the projection, b.txt was edited, c.txt is new.
        index.insert("a.txt".into(), proven(b"a"));
        index.insert("b.txt".into(), proven(b"B"));
        index.insert("c.txt".into(), proven(b"c"));
        let base = tree(b"base");
        // The same header, matching no entry.
        let mut unmatched = index.clone();
        unmatched.bind(&base, &[]);
        index.bind(&base, &projection);
        assert_eq!(index.bound_tree(), Some(&base));
        index.save(&store).unwrap();
        // One FileId (32 bytes) and its length fewer.
        assert!(encode_bound(&index).len() + 32 <= encode_bound(&unmatched).len());

        let mut loaded = ChangeIndex::load(&store, &id).unwrap();
        assert!(loaded.has_unresolved());
        // Not resolved: a.txt is read again; the explicit entries are trusted.
        assert!(loaded.trusted("a.txt", &stat(0)).is_none());
        assert_eq!(loaded.trusted("b.txt", &stat(0)), index.entry("b.txt"));
        assert_eq!(loaded.trusted("c.txt", &stat(0)), index.entry("c.txt"));
        assert_eq!(loaded.len(), 3);
        // Another projection resolves nothing.
        let mut changed = projection.clone();
        changed[0] = file("a.txt", b"x", 1);
        assert!(!loaded.resolve(&changed));
        assert!(loaded.trusted("a.txt", &stat(0)).is_none());
        assert!(!loaded.is_bound_to(&projection_digest(&changed)));
        assert!(loaded.is_bound_to(&projection_digest(&projection)));
        // The exact projection resolves it, back to the saved index.
        assert!(loaded.resolve(&projection));
        assert!(!loaded.has_unresolved());
        assert_eq!(loaded.trusted("a.txt", &stat(0)), index.entry("a.txt"));
        assert!(loaded.same_entries(&index));

        // Saving unresolved entries under the same binding keeps them.
        let unresolved = ChangeIndex::load(&store, &id).unwrap();
        unresolved.save(&store).unwrap();
        let mut again = ChangeIndex::load(&store, &id).unwrap();
        assert!(again.resolve(&projection));
        assert!(again.same_entries(&index));
        // Binding to another projection drops them (read again, never wrong).
        let mut rebound = ChangeIndex::load(&store, &id).unwrap();
        rebound.bind(&tree(b"next"), &changed);
        assert!(rebound.entry("a.txt").is_none());
        assert!(!rebound.has_unresolved());
        assert_eq!(rebound.len(), 2);
        let _ = fs::remove_dir_all(store);
    }

    #[test]
    fn a_bound_entry_keeps_a_listed_length_that_differs_and_a_native_number() {
        let store = temp();
        let id = worktree();
        ChangeIndex::enable(&store, &id, &store).unwrap();
        let mut index = ChangeIndex::load(&store, &id).unwrap();
        let projection = vec![file("p", b"p", 5)];
        let mut listed = stat(7);
        listed.length = 9;
        listed.inode = Some(3);
        listed.mode = Some(1);
        index.insert(
            "p".into(),
            IndexEntry {
                file_id: FileId::from_bytes(b"p"),
                length: 5,
                executable: false,
                stat: listed,
                verified_at: 77,
                native: Some(12),
            },
        );
        let base = tree(b"base");
        index.bind(&base, &projection);
        index.save(&store).unwrap();
        let mut loaded = ChangeIndex::load(&store, &id).unwrap();
        assert!(loaded.entry("p").is_none());
        assert!(loaded.resolve(&projection));
        assert_eq!(loaded.entry("p"), index.entry("p"));
        let _ = fs::remove_dir_all(store);
    }

    #[test]
    fn a_projection_digest_covers_every_field_and_the_order() {
        let a = vec![file("a", b"a", 1), file("b", b"b", 1)];
        let digest = projection_digest(&a);
        let mut executable = a.clone();
        if let CheckoutProjectionContent::File {
            executable: bit, ..
        } = &mut executable[1].content
        {
            *bit = true;
        }
        let variants = [
            vec![file("a", b"a", 1)],
            vec![file("a", b"a", 1), file("b", b"b", 2)],
            vec![file("a", b"a", 1), file("c", b"b", 1)],
            vec![file("b", b"b", 1), file("a", b"a", 1)],
            executable,
        ];
        for variant in variants {
            assert_ne!(projection_digest(&variant), digest);
        }
        assert_eq!(projection_digest(&a.clone()), digest);
    }
}
