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
use layrs_core::scm::{FileId, WorktreeId};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
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
    entries: BTreeMap<String, IndexEntry>,
}

impl ChangeIndex {
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
        if fs::symlink_metadata(Self::file(store_root, worktree_id)).is_ok() {
            return Ok(());
        }
        Self {
            worktree_id: worktree_id.clone(),
            margin_nanos: margin_nanos(checkout_root),
            entries: BTreeMap::new(),
        }
        .save(store_root)
    }

    pub fn is_enabled(store_root: &Path, worktree_id: &WorktreeId) -> bool {
        fs::symlink_metadata(Self::file(store_root, worktree_id)).is_ok_and(|m| m.is_file())
    }

    /// `None` when the mechanism is off for this Worktree. A damaged index is
    /// returned empty, with the conservative margin: every file is read.
    pub fn load(store_root: &Path, worktree_id: &WorktreeId) -> Option<Self> {
        let _span = crate::perf::span("change_index.load");
        let path = Self::file(store_root, worktree_id);
        let metadata = fs::symlink_metadata(&path).ok()?;
        let empty = || Self {
            worktree_id: worktree_id.clone(),
            margin_nanos: COARSE_MARGIN_NANOS,
            entries: BTreeMap::new(),
        };
        if !metadata.is_file() || metadata.len() > MAX_INDEX_BYTES {
            return Some(empty());
        }
        let parsed = fs::read(&path)
            .ok()
            .and_then(|bytes| {
                if bytes.starts_with(BINARY_MAGIC) {
                    decode_binary(&bytes)
                } else {
                    // An index written before the binary format (a cache).
                    serde_json::from_slice::<Document>(&bytes).ok()
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
            },
            None => empty(),
        })
    }

    /// Atomic replacement, without fsync: this is a cache, and a lost or torn
    /// update only makes the next capture read more.
    pub fn save(&self, store_root: &Path) -> io::Result<()> {
        let path = Self::file(store_root, &self.worktree_id);
        let directory = path.parent().expect("index file has a parent");
        fs::create_dir_all(directory)?;
        let document = Document {
            schema: SCHEMA.into(),
            worktree_id: self.worktree_id.clone(),
            margin_nanos: self.margin_nanos,
            entries: self.entries.clone(),
        };
        let bytes = encode_binary(&document);
        let staged = directory.join(format!(
            ".{}.{}.tmp",
            path.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));
        {
            let mut file = fs::File::create(&staged)?;
            file.write_all(&bytes)?;
        }
        fs::rename(&staged, &path).inspect_err(|_| {
            let _ = fs::remove_file(&staged);
        })
    }

    pub fn remove(store_root: &Path, worktree_id: &WorktreeId) -> io::Result<()> {
        match fs::remove_file(Self::file(store_root, worktree_id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// The proven entry for `path` if `stat` still identifies it.
    pub fn trusted(&self, path: &str, stat: &StatKey) -> Option<&IndexEntry> {
        let entry = self.entries.get(path)?;
        (entry.stat == *stat
            && entry.length == stat.length
            && stat
                .modified
                .checked_add(self.margin_nanos)
                .is_some_and(|limit| limit < entry.verified_at))
        .then_some(entry)
    }

    pub fn entry(&self, path: &str) -> Option<&IndexEntry> {
        self.entries.get(path)
    }
    /// Same recorded entries (and so nothing to write).
    pub fn same_entries(&self, other: &Self) -> bool {
        self.worktree_id == other.worktree_id
            && self.margin_nanos == other.margin_nanos
            && self.entries == other.entries
    }
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
    pub fn insert(&mut self, path: String, entry: IndexEntry) {
        self.entries.insert(path, entry);
    }
    pub fn remove_path(&mut self, path: &str) {
        self.entries.remove(path);
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&str, &IndexEntry) -> bool) {
        self.entries.retain(|path, entry| keep(path, entry));
    }
    pub fn clear(&mut self) {
        self.entries.clear();
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

/// Binary encoding of the index (a cache, read on every `status`): the same
/// document as the former JSON, length-prefixed, with a blake3 trailer. A
/// damaged file decodes to nothing, which the loader treats as empty.
const BINARY_MAGIC: &[u8; 8] = b"LYRSIDX1";

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
fn margin_nanos(root: &Path) -> i64 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        index.save(&store).unwrap();
        let path = ChangeIndex::file(&store, &id);
        assert!(fs::read(&path).unwrap().starts_with(BINARY_MAGIC));
        let reloaded = ChangeIndex::load(&store, &id).unwrap();
        assert!(reloaded.same_entries(&index));
        // A legacy JSON document is still read.
        let document = Document {
            schema: SCHEMA.into(),
            worktree_id: id.clone(),
            margin_nanos: index.margin_nanos,
            entries: index.entries.clone(),
        };
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(ChangeIndex::load(&store, &id).unwrap().same_entries(&index));
        // A damaged binary index loads empty: every file is read.
        index.save(&store).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        fs::write(&path, &bytes).unwrap();
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
        let path = ChangeIndex::file(&store, &id);
        fs::write(&path, b"{not json").unwrap();
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
            entries: foreign.entries.clone(),
        })
        .unwrap();
        fs::write(&path, bytes).unwrap();
        assert!(ChangeIndex::load(&store, &id).unwrap().is_empty());
        let _ = fs::remove_dir_all(store);
    }
}
