//! Packs of small immutable CAS objects (ADR 0026).
//!
//! A pack is one immutable file `objects/packs/pack-<32 hex>.lpk`:
//!
//! 1. header: magic, version, reserved;
//! 2. self-describing records: kind, digest, length, canonical bytes;
//! 3. index sorted by (kind, digest), without duplicates: kind, digest,
//!    record offset, length;
//! 4. footer: index offset and size, entry count, blake3 of header and index,
//!    end magic.
//!
//! It is published by one no-clobber rename and never modified or deleted.
//! The index is untrusted input: it is bounded and checked before use, every
//! member read is exact, and the caller validates the bytes against their
//! content address before serving them. Presence in an index never proves
//! content or admission.
use super::*;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::{Mutex, OnceLock, RwLock};

pub(super) const PACKS_DIR: &str = "packs";
/// A batch packs its new small objects only from this many distinct ones.
pub(super) const PACK_MIN_OBJECTS: usize = 64;
/// Largest canonical object a pack holds.
pub(super) const PACK_MAX_OBJECT_BYTES: usize = 64 * 1024;
/// A pack is closed when its records would exceed this size.
const PACK_MAX_RECORD_BYTES: u64 = 32 * 1024 * 1024;
const PACK_MAX_ENTRIES: u64 = 1_000_000;
const PACK_MAX_INDEX_BYTES: u64 = 64 * 1024 * 1024;

const HEADER_MAGIC: &[u8; 8] = b"LYRSPACK";
const FOOTER_MAGIC: &[u8; 8] = b"LYRSPEND";
const PACK_VERSION: u32 = 1;
const HEADER_LEN: u64 = 16;
const RECORD_HEADER_LEN: u64 = 1 + 32 + 4;
const ENTRY_LEN: u64 = 1 + 32 + 8 + 4;
const FOOTER_LEN: u64 = 8 + 8 + 8 + 32 + 8;
const INVALID_PACK: &str = "validate CAS pack";

/// Identity of an object inside a pack: kind code and raw blake3 digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PackKey {
    kind: u8,
    digest: [u8; 32],
}

impl PackKey {
    pub(super) fn of(object_id: &StoreObjectId) -> CasResult<Self> {
        let hex = digest(object_id)?;
        let mut digest = [0_u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16).map_err(|_| {
                CasError::InvalidObjectId {
                    kind: object_id.kind(),
                    id: object_id.to_string(),
                }
            })?;
        }
        Ok(Self {
            kind: kind_code(object_id.kind()),
            digest,
        })
    }

    pub(super) fn object_id(&self) -> CasResult<StoreObjectId> {
        let kind = kind_from_code(self.kind).ok_or_else(|| CasError::KindMismatch {
            expected: StoreObjectKind::ChunkRaw,
            actual: format!("pack kind code {}", self.kind),
        })?;
        let hex = self
            .digest
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                use std::fmt::Write;
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        parse_object_id(kind, &format!("blake3:{hex}"))
    }
}

fn kind_code(kind: StoreObjectKind) -> u8 {
    match kind {
        StoreObjectKind::ChunkRaw => 1,
        StoreObjectKind::FileManifest => 2,
        StoreObjectKind::Tree => 3,
        StoreObjectKind::Symlink => 4,
        StoreObjectKind::Tombstone => 5,
        StoreObjectKind::Step => 6,
        StoreObjectKind::RepositoryView => 7,
        StoreObjectKind::Conflict => 8,
        StoreObjectKind::Product => 9,
    }
}

fn kind_from_code(code: u8) -> Option<StoreObjectKind> {
    Some(match code {
        1 => StoreObjectKind::ChunkRaw,
        2 => StoreObjectKind::FileManifest,
        3 => StoreObjectKind::Tree,
        4 => StoreObjectKind::Symlink,
        5 => StoreObjectKind::Tombstone,
        6 => StoreObjectKind::Step,
        7 => StoreObjectKind::RepositoryView,
        8 => StoreObjectKind::Conflict,
        9 => StoreObjectKind::Product,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PackEntry {
    pub(super) key: PackKey,
    /// Offset of the record header.
    pub(super) offset: u64,
    /// Length of the canonical bytes.
    pub(super) length: u32,
}

/// A validated pack index. The file it describes is immutable.
#[derive(Debug)]
pub(super) struct PackIndex {
    pub(super) name: OsString,
    pub(super) file_len: u64,
    entries: Vec<PackEntry>,
    /// The pack opened once, through the pinned CAS directories, for the
    /// member reads of this process. A published pack is immutable and the
    /// handle pins the very file whose length matched the index.
    reader: std::sync::OnceLock<crate::secure_fs::PinnedFile>,
}

impl PackIndex {
    pub(super) fn find(&self, key: &PackKey) -> Option<PackEntry> {
        self.entries
            .binary_search_by(|entry| entry.key.cmp(key))
            .ok()
            .map(|index| self.entries[index])
    }

    pub(super) fn entries(&self) -> &[PackEntry] {
        &self.entries
    }
}

pub(super) fn valid_pack_name(name: &OsStr) -> bool {
    let Some(hex) = name
        .to_str()
        .and_then(|text| text.strip_prefix("pack-"))
        .and_then(|text| text.strip_suffix(".lpk"))
    else {
        return false;
    };
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn new_pack_name() -> OsString {
    let hash = blake3::hash(staging_name().as_bytes());
    OsString::from(format!("pack-{}.lpk", &hash.to_hex()[..32]))
}

pub(super) fn invalid_pack(path: &Path, reason: impl Into<String>) -> CasError {
    CasError::Io {
        operation: INVALID_PACK,
        path: path.to_path_buf(),
        message: reason.into(),
    }
}

/// Whether `error` says a pack's bytes are invalid (as opposed to an I/O
/// failure that may be transient).
pub(super) fn is_invalid_pack(error: &CasError) -> bool {
    matches!(error, CasError::Io { operation, .. } if *operation == INVALID_PACK)
}

fn read_exact_at(
    file: &mut fs::File,
    path: &Path,
    offset: u64,
    buffer: &mut [u8],
) -> CasResult<()> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| io_error("seek CAS pack", path, error))?;
    read_exact(file, path, buffer)
}

fn read_exact(reader: &mut impl Read, path: &Path, buffer: &mut [u8]) -> CasResult<()> {
    reader.read_exact(buffer).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            invalid_pack(path, "pack is truncated")
        } else {
            io_error("read CAS pack", path, error)
        }
    })
}

fn header_bytes() -> [u8; HEADER_LEN as usize] {
    let mut header = [0_u8; HEADER_LEN as usize];
    header[..8].copy_from_slice(HEADER_MAGIC);
    header[8..12].copy_from_slice(&PACK_VERSION.to_le_bytes());
    header
}

fn record_header(key: &PackKey, length: u32) -> [u8; RECORD_HEADER_LEN as usize] {
    let mut header = [0_u8; RECORD_HEADER_LEN as usize];
    header[0] = key.kind;
    header[1..33].copy_from_slice(&key.digest);
    header[33..37].copy_from_slice(&length.to_le_bytes());
    header
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut value = [0_u8; 8];
    value.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(value)
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut value = [0_u8; 4];
    value.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(value)
}

/// Loads and validates the index of the pack open in `file`.
pub(super) fn load_index(file: &mut fs::File, path: &Path, name: OsString) -> CasResult<PackIndex> {
    let file_len = file
        .metadata()
        .map_err(|error| io_error("stat CAS pack", path, error))?
        .len();
    if file_len < HEADER_LEN + FOOTER_LEN {
        return Err(invalid_pack(
            path,
            "pack is shorter than its header and footer",
        ));
    }
    let mut header = [0_u8; HEADER_LEN as usize];
    read_exact_at(file, path, 0, &mut header)?;
    if header != header_bytes() {
        return Err(invalid_pack(path, "unknown pack header or version"));
    }
    let mut footer = [0_u8; FOOTER_LEN as usize];
    read_exact_at(file, path, file_len - FOOTER_LEN, &mut footer)?;
    if &footer[56..64] != FOOTER_MAGIC {
        return Err(invalid_pack(path, "pack footer magic is missing"));
    }
    let index_offset = u64_at(&footer, 0);
    let index_len = u64_at(&footer, 8);
    let entries = u64_at(&footer, 16);
    let bounded = entries <= PACK_MAX_ENTRIES
        && index_len <= PACK_MAX_INDEX_BYTES
        && entries.checked_mul(ENTRY_LEN) == Some(index_len)
        && index_offset >= HEADER_LEN
        && index_offset - HEADER_LEN <= PACK_MAX_RECORD_BYTES
        && index_offset.checked_add(index_len) == Some(file_len - FOOTER_LEN);
    if !bounded {
        return Err(invalid_pack(path, "pack footer is out of bounds"));
    }
    let mut index = vec![0_u8; index_len as usize];
    read_exact_at(file, path, index_offset, &mut index)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&header);
    hasher.update(&index);
    if hasher.finalize().as_bytes()[..] != footer[24..56] {
        return Err(invalid_pack(path, "pack index checksum differs"));
    }
    let entries = parse_entries(&index, index_offset, path)?;
    Ok(PackIndex {
        name,
        file_len,
        entries,
        reader: std::sync::OnceLock::new(),
    })
}

fn parse_entries(index: &[u8], records_end: u64, path: &Path) -> CasResult<Vec<PackEntry>> {
    let mut entries: Vec<PackEntry> = Vec::with_capacity(index.len() / ENTRY_LEN as usize);
    for raw in index.chunks_exact(ENTRY_LEN as usize) {
        let kind = raw[0];
        if kind_from_code(kind).is_none() {
            return Err(invalid_pack(
                path,
                format!("pack index has unknown kind {kind}"),
            ));
        }
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&raw[1..33]);
        let entry = PackEntry {
            key: PackKey { kind, digest },
            offset: u64_at(raw, 33),
            length: u32_at(raw, 41),
        };
        if entries.last().is_some_and(|last| last.key >= entry.key) {
            return Err(invalid_pack(path, "pack index is not strictly sorted"));
        }
        if entry.length as usize > PACK_MAX_OBJECT_BYTES {
            return Err(invalid_pack(
                path,
                "pack index entry exceeds the object limit",
            ));
        }
        let end = entry
            .offset
            .checked_add(RECORD_HEADER_LEN + u64::from(entry.length));
        if entry.offset < HEADER_LEN || end.is_none_or(|end| end > records_end) {
            return Err(invalid_pack(
                path,
                "pack index entry is outside the records",
            ));
        }
        entries.push(entry);
    }
    let mut ranges: Vec<(u64, u64)> = entries
        .iter()
        .map(|entry| {
            (
                entry.offset,
                entry.offset + RECORD_HEADER_LEN + u64::from(entry.length),
            )
        })
        .collect();
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(invalid_pack(path, "pack index records overlap"));
    }
    Ok(entries)
}

/// Positional exact read through a shared handle (no seek state).
fn read_exact_at_shared(
    file: &fs::File,
    path: &Path,
    offset: u64,
    buffer: &mut [u8],
) -> CasResult<()> {
    let mut done = 0usize;
    while done < buffer.len() {
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(
            file,
            &mut buffer[done..],
            offset + done as u64,
        );
        #[cfg(unix)]
        let read =
            std::os::unix::fs::FileExt::read_at(file, &mut buffer[done..], offset + done as u64);
        match read {
            Ok(0) => return Err(invalid_pack(path, "pack is truncated")),
            Ok(count) => done += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(io_error("read CAS pack", path, error)),
        }
    }
    Ok(())
}

/// `read_member` through a shared handle.
fn read_member_shared(file: &fs::File, path: &Path, entry: &PackEntry) -> CasResult<Vec<u8>> {
    let mut header = [0_u8; RECORD_HEADER_LEN as usize];
    read_exact_at_shared(file, path, entry.offset, &mut header)?;
    if header != record_header(&entry.key, entry.length) {
        return Err(invalid_pack(
            path,
            "pack record differs from its index entry",
        ));
    }
    let mut bytes = vec![0_u8; entry.length as usize];
    read_exact_at_shared(file, path, entry.offset + RECORD_HEADER_LEN, &mut bytes)?;
    Ok(bytes)
}

/// Reads exactly one member's canonical bytes after checking its record
/// header against the index entry. The caller validates the content.
pub(super) fn read_member(
    file: &mut fs::File,
    path: &Path,
    entry: &PackEntry,
) -> CasResult<Vec<u8>> {
    let mut header = [0_u8; RECORD_HEADER_LEN as usize];
    read_exact_at(file, path, entry.offset, &mut header)?;
    if header != record_header(&entry.key, entry.length) {
        return Err(invalid_pack(
            path,
            "pack record differs from its index entry",
        ));
    }
    let mut bytes = vec![0_u8; entry.length as usize];
    read_exact(file, path, &mut bytes)?;
    Ok(bytes)
}

/// Walks the records of a pack whose index cannot be trusted, in file order,
/// so scrub can name damaged objects. Ends where a footer that still has its
/// magic says the index starts, or at the first header that cannot be a
/// record (damage, or the index of a pack without footer).
pub(super) fn scan_records(
    file: &mut fs::File,
    path: &Path,
    mut visit: impl FnMut(PackKey, CasResult<Vec<u8>>),
) -> CasResult<()> {
    let file_len = file
        .metadata()
        .map_err(|error| io_error("stat CAS pack", path, error))?
        .len();
    let mut records_end = file_len;
    if file_len >= HEADER_LEN + FOOTER_LEN {
        let mut footer = [0_u8; FOOTER_LEN as usize];
        read_exact_at(file, path, file_len - FOOTER_LEN, &mut footer)?;
        let index_offset = u64_at(&footer, 0);
        if &footer[56..64] == FOOTER_MAGIC
            && (HEADER_LEN..=file_len - FOOTER_LEN).contains(&index_offset)
        {
            records_end = index_offset;
        }
    }
    let mut offset = HEADER_LEN;
    while offset + RECORD_HEADER_LEN <= records_end {
        let mut header = [0_u8; RECORD_HEADER_LEN as usize];
        read_exact_at(file, path, offset, &mut header)?;
        let length = u32_at(&header, 33);
        if kind_from_code(header[0]).is_none() || length as usize > PACK_MAX_OBJECT_BYTES {
            break;
        }
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&header[1..33]);
        let key = PackKey {
            kind: header[0],
            digest,
        };
        let mut bytes = vec![0_u8; length as usize];
        let read = read_exact(file, path, &mut bytes).map(|()| bytes);
        let truncated = read.is_err();
        visit(key, read);
        if truncated {
            break;
        }
        offset += RECORD_HEADER_LEN + u64::from(length);
    }
    Ok(())
}

/// The packs this process has loaded for one Store, shared by every CAS view
/// of that Store (the VFS host reopens its view every 2 s).
#[derive(Debug, Default)]
pub(super) struct PackCatalog {
    packs: Vec<Arc<PackIndex>>,
    /// Names loaded or found invalid; never reloaded (packs are immutable).
    known: std::collections::BTreeSet<OsString>,
    pub(super) listed: bool,
}

impl PackCatalog {
    pub(super) fn insert(&mut self, index: Arc<PackIndex>) {
        if self.known.insert(index.name.clone()) {
            self.packs.push(index);
        }
    }

    pub(super) fn reject(&mut self, name: OsString) {
        self.known.insert(name);
    }

    pub(super) fn knows(&self, name: &OsStr) -> bool {
        self.known.contains(name)
    }

    pub(super) fn candidates(&self, key: &PackKey) -> Vec<(Arc<PackIndex>, PackEntry)> {
        self.packs
            .iter()
            .filter_map(|index| index.find(key).map(|entry| (Arc::clone(index), entry)))
            .collect()
    }
}

/// The process-wide catalog of one Store, keyed by Store id and root
/// identity.
pub(super) fn shared_catalog(key: &str) -> Arc<RwLock<PackCatalog>> {
    static CATALOGS: OnceLock<Mutex<HashMap<String, Arc<RwLock<PackCatalog>>>>> = OnceLock::new();
    let mut catalogs = CATALOGS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(catalogs.entry(key.to_owned()).or_default())
}

/// One pack being written to a staging file.
pub(super) struct PackWriter {
    file: PinnedFile,
    path: PathBuf,
    /// Records in file order, with the blake3 of their canonical bytes.
    entries: Vec<(PackEntry, blake3::Hash)>,
    next: u64,
    published: bool,
}

impl PackWriter {
    fn create(staging: &PinnedDirectory, staging_dir: &Path) -> CasResult<Self> {
        let mut last_error = None;
        for _ in 0..64 {
            let name = staging_name();
            match staging.create_file(OsStr::new(&name)) {
                Ok(mut file) => {
                    let path = staging_dir.join(&name);
                    file.file_mut()
                        .write_all(&header_bytes())
                        .map_err(|error| io_error("write CAS pack", &path, error))?;
                    return Ok(Self {
                        file,
                        path,
                        entries: Vec::new(),
                        next: HEADER_LEN,
                        published: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                }
                Err(error) => return Err(io_error("create CAS pack", staging_dir, error)),
            }
        }
        Err(io_error(
            "allocate CAS pack",
            staging_dir,
            last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "collision")),
        ))
    }

    fn fits(&self, length: usize) -> bool {
        self.next - HEADER_LEN + RECORD_HEADER_LEN + length as u64 <= PACK_MAX_RECORD_BYTES
    }

    fn append(&mut self, key: PackKey, bytes: &[u8]) -> CasResult<()> {
        let length = u32::try_from(bytes.len())
            .map_err(|_| invalid_pack(&self.path, "object too large for a pack"))?;
        let file = self.file.file_mut();
        file.write_all(&record_header(&key, length))
            .and_then(|()| file.write_all(bytes))
            .map_err(|error| io_error("write CAS pack", &self.path, error))?;
        self.entries.push((
            PackEntry {
                key,
                offset: self.next,
                length,
            },
            blake3::hash(bytes),
        ));
        self.next += RECORD_HEADER_LEN + u64::from(length);
        Ok(())
    }

    /// Writes the index and footer, makes the data durable as `mode` asks,
    /// then rereads the whole file: the index must validate and equal the
    /// written one, and every record must hold the bytes written.
    fn seal(&mut self, mode: Option<BatchDurability>) -> CasResult<PackIndex> {
        let mut sorted: Vec<PackEntry> = self.entries.iter().map(|(entry, _)| *entry).collect();
        sorted.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        let mut index = Vec::with_capacity(sorted.len() * ENTRY_LEN as usize);
        for entry in &sorted {
            index.push(entry.key.kind);
            index.extend_from_slice(&entry.key.digest);
            index.extend_from_slice(&entry.offset.to_le_bytes());
            index.extend_from_slice(&entry.length.to_le_bytes());
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(&header_bytes());
        hasher.update(&index);
        let mut footer = Vec::with_capacity(FOOTER_LEN as usize);
        footer.extend_from_slice(&self.next.to_le_bytes());
        footer.extend_from_slice(&(index.len() as u64).to_le_bytes());
        footer.extend_from_slice(&(sorted.len() as u64).to_le_bytes());
        footer.extend_from_slice(hasher.finalize().as_bytes());
        footer.extend_from_slice(FOOTER_MAGIC);
        let path = self.path.clone();
        let file = self.file.file_mut();
        file.write_all(&index)
            .and_then(|()| file.write_all(&footer))
            .map_err(|error| io_error("write CAS pack index", &path, error))?;
        {
            let _span = crate::perf::span("cas.pack_sync");
            match mode {
                Some(BatchDurability::JournalOrdered) => write_through(file),
                _ => file.sync_all(),
            }
            .map_err(|error| io_error("sync CAS pack", &path, error))?;
        }
        let _span = crate::perf::span("cas.pack_verify");
        let reread = load_index(file, &path, OsString::new())?;
        if reread.entries != sorted {
            return Err(invalid_pack(
                &path,
                "pack index reread differs from the written one",
            ));
        }
        file.seek(SeekFrom::Start(HEADER_LEN))
            .map_err(|error| io_error("seek CAS pack", &path, error))?;
        let mut reader = io::BufReader::with_capacity(1024 * 1024, &mut *file);
        let mut bytes = Vec::with_capacity(PACK_MAX_OBJECT_BYTES);
        for (entry, hash) in &self.entries {
            let mut header = [0_u8; RECORD_HEADER_LEN as usize];
            read_exact(&mut reader, &path, &mut header)?;
            bytes.resize(entry.length as usize, 0);
            read_exact(&mut reader, &path, &mut bytes)?;
            if header != record_header(&entry.key, entry.length) || blake3::hash(&bytes) != *hash {
                return Err(invalid_pack(
                    &path,
                    "pack record reread differs from the written bytes",
                ));
            }
        }
        Ok(reread)
    }
}

impl Drop for PackWriter {
    fn drop(&mut self) {
        // An unpublished staging pack is never referenced. After a Windows
        // rename the handle names the published pack: never unlink it.
        // Elsewhere publication is a hard link and only the staging name is
        // removed (again, harmlessly, after a completed cleanup).
        #[cfg(windows)]
        if self.published {
            return;
        }
        let _ = self.file.unlink();
    }
}

/// Small new objects of one `put_ordered` batch. The first
/// `PACK_MIN_OBJECTS - 1` are kept in memory; from the next one on they are
/// streamed to staging packs and only their index entries stay in memory.
#[derive(Default)]
pub(super) struct Packer {
    members: HashSet<PackKey>,
    memory: Vec<(PackKey, StoreObjectId, Vec<u8>)>,
    writers: Vec<PackWriter>,
    lease: Option<StagingLease>,
}

impl Packer {
    /// Adds validated canonical bytes; `false` when the batch already holds
    /// this object.
    pub(super) fn add(
        &mut self,
        cas: &NativeCas,
        object_id: StoreObjectId,
        bytes: Vec<u8>,
    ) -> CasResult<bool> {
        let key = PackKey::of(&object_id)?;
        if !self.members.insert(key) {
            return Ok(false);
        }
        if self.writers.is_empty() && self.members.len() < PACK_MIN_OBJECTS {
            self.memory.push((key, object_id, bytes));
            return Ok(true);
        }
        if self.writers.is_empty() {
            self.lease = Some(StagingLease::shared(&cas.staging_lease_path)?);
            let staging = cas.pinned_staging()?;
            self.writers
                .push(PackWriter::create(&staging, &cas.staging_dir)?);
            for (key, _, bytes) in std::mem::take(&mut self.memory) {
                self.append(cas, key, &bytes)?;
            }
        }
        self.append(cas, key, &bytes)?;
        Ok(true)
    }

    fn append(&mut self, cas: &NativeCas, key: PackKey, bytes: &[u8]) -> CasResult<()> {
        if !self
            .writers
            .last()
            .is_some_and(|writer| writer.fits(bytes.len()))
        {
            let staging = cas.pinned_staging()?;
            self.writers
                .push(PackWriter::create(&staging, &cas.staging_dir)?);
        }
        match self.writers.last_mut() {
            Some(writer) => writer.append(key, bytes),
            None => Err(invalid_pack(&cas.staging_dir, "no pack writer")),
        }
    }

    /// Publishes everything added: fewer than `PACK_MIN_OBJECTS` objects as
    /// separate files (in parallel), otherwise every pack. Runs inside the
    /// batch, before its durability step.
    pub(super) fn finish(mut self, cas: &NativeCas) -> CasResult<()> {
        if self.writers.is_empty() {
            let memory = std::mem::take(&mut self.memory);
            return publish_loose(cas, memory);
        }
        for writer in &mut self.writers {
            cas.publish_pack(writer)?;
        }
        Ok(())
    }
}

fn publish_loose(
    cas: &NativeCas,
    objects: Vec<(PackKey, StoreObjectId, Vec<u8>)>,
) -> CasResult<()> {
    use std::sync::atomic::AtomicUsize;
    if objects.is_empty() {
        return Ok(());
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8)
        .min(objects.len());
    let next = AtomicUsize::new(0);
    let failure = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((_, object_id, bytes)) = objects.get(index) else {
                        break;
                    };
                    let result = cas
                        .stage_encoded(object_id.clone(), bytes.clone())
                        .and_then(|prepared| cas.install(prepared));
                    if let Err(error) = result {
                        if let Ok(mut failure) = failure.lock() {
                            failure.get_or_insert(error);
                        }
                        break;
                    }
                }
            });
        }
    });
    match failure.into_inner() {
        Ok(Some(error)) => Err(error),
        _ => Ok(()),
    }
}

/// Opens a pack for an fsync (Coalesced batches that reuse a member).
pub(super) fn sync_pack_file(directory: &PinnedDirectory, name: &OsStr) -> io::Result<()> {
    #[cfg(windows)]
    let file = directory.open_file_for_sync(name)?;
    #[cfg(not(windows))]
    let file = directory.open_file(name)?;
    file.file().sync_all()
}

impl PackIndex {
    fn published_as(self, name: OsString) -> Self {
        Self { name, ..self }
    }
}

/// Where a copy of an object is. Presence is not content proof: every read
/// still verifies the bytes against the content address. In a pack v1
/// `offset` is the record's; in a pack v2 (format 6) it is the offset of the
/// stored bytes of the compressed block holding the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasLocation {
    Loose {
        path: PathBuf,
        length: u64,
    },
    Packed {
        pack: PathBuf,
        offset: u64,
        length: u64,
    },
}

impl CasLocation {
    pub fn length(&self) -> u64 {
        match self {
            Self::Loose { length, .. } | Self::Packed { length, .. } => *length,
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Loose { path, .. } => path,
            Self::Packed { pack, .. } => pack,
        }
    }
}

/// The copy `with_copy` served.
pub(super) enum Found<T> {
    Packed(Vec<u8>, CanonicalObject),
    Loose(T),
}

/// One `put_ordered` item once its worker is done.
pub(super) enum Slot {
    Done(InstallOutcome),
    /// Added to the batch's packer; published by `Packer::finish`.
    Packed {
        object_id: StoreObjectId,
        bytes_len: u64,
        new: bool,
    },
}

fn poisoned(path: &Path) -> CasError {
    io_error("lock CAS packs", path, io::Error::other("poisoned"))
}

impl NativeCas {
    fn packs_path(&self) -> PathBuf {
        self.objects_dir.join(PACKS_DIR)
    }

    /// Lists `objects/packs` and loads the packs this process does not know
    /// yet; returns them. An invalid pack is remembered and ignored; one that
    /// cannot be opened now is retried at the next listing.
    pub(super) fn refresh_packs(&self) -> CasResult<Vec<Arc<PackIndex>>> {
        let Some(catalog) = &self.packs else {
            return Ok(Vec::new());
        };
        let directory = self.packs_path();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                catalog.write().map_err(|_| poisoned(&directory))?.listed = true;
                return Ok(Vec::new());
            }
            Err(error) => return Err(io_error("list CAS packs", &directory, error)),
        };
        let mut unknown = Vec::new();
        {
            let known = catalog.read().map_err(|_| poisoned(&directory))?;
            for entry in entries {
                let entry = entry.map_err(|error| io_error("list CAS packs", &directory, error))?;
                let name = entry.file_name();
                if valid_pack_name(&name) && !known.knows(&name) {
                    unknown.push(name);
                }
            }
        }
        let mut loaded = Vec::new();
        let mut rejected = Vec::new();
        if !unknown.is_empty() {
            let packs = self
                .pinned_objects_for_read()?
                .open_child(OsStr::new(PACKS_DIR))
                .map_err(|error| io_error("pin CAS packs", &directory, error))?;
            for name in unknown {
                let path = directory.join(&name);
                let index = packs
                    .open_file(&name)
                    .map_err(|error| io_error("open CAS pack", &path, error))
                    .and_then(|mut file| load_index(file.file_mut(), &path, name.clone()));
                match index {
                    Ok(index) => loaded.push(Arc::new(index)),
                    Err(error) if is_invalid_pack(&error) => rejected.push(name),
                    Err(_) => {}
                }
            }
        }
        let mut catalog = catalog.write().map_err(|_| poisoned(&directory))?;
        for index in &loaded {
            catalog.insert(Arc::clone(index));
        }
        for name in rejected {
            catalog.reject(name);
        }
        catalog.listed = true;
        Ok(loaded)
    }

    /// Pack copies of `object_id`: in the loaded catalog (listed once per
    /// process), or with `fresh` only in those packs.
    fn pack_candidates(
        &self,
        object_id: &StoreObjectId,
        fresh: Option<&[Arc<PackIndex>]>,
    ) -> CasResult<Vec<(Arc<PackIndex>, PackEntry)>> {
        let Some(catalog) = &self.packs else {
            return Ok(Vec::new());
        };
        let key = PackKey::of(object_id)?;
        if let Some(fresh) = fresh {
            return Ok(fresh
                .iter()
                .filter_map(|index| index.find(&key).map(|entry| (Arc::clone(index), entry)))
                .collect());
        }
        let listed = catalog
            .read()
            .map_err(|_| poisoned(&self.objects_dir))?
            .listed;
        if !listed {
            // A listing failure leaves the catalog unlisted: later lookups
            // retry, and the separate file and the fresh listing still run.
            let _ = self.refresh_packs();
        }
        Ok(catalog
            .read()
            .map_err(|_| poisoned(&self.objects_dir))?
            .candidates(&key))
    }

    /// The exact canonical bytes of one pack member, unvalidated.
    fn read_pack_member(&self, index: &PackIndex, entry: &PackEntry) -> CasResult<Vec<u8>> {
        let path = self.packs_path().join(&index.name);
        if let Some(file) = index.reader.get() {
            return read_member_shared(file.file(), &path, entry);
        }
        let packs = self
            .pinned_objects_for_read()?
            .open_child(OsStr::new(PACKS_DIR))
            .map_err(|error| io_error("pin CAS packs", &path, error))?;
        let file = packs
            .open_file(&index.name)
            .map_err(|error| io_error("open CAS pack", &path, error))?;
        let length = file
            .file()
            .metadata()
            .map_err(|error| io_error("stat CAS pack", &path, error))?
            .len();
        if length != index.file_len {
            return Err(invalid_pack(
                &path,
                "pack length changed since its index was read",
            ));
        }
        let bytes = read_member_shared(file.file(), &path, entry)?;
        let _ = index.reader.set(file);
        Ok(bytes)
    }

    /// The first pack copy of `object_id` that validates; the others' errors
    /// go to `first_error`.
    fn verified_pack_copy(
        &self,
        object_id: &StoreObjectId,
        fresh: Option<&[Arc<PackIndex>]>,
        first_error: &mut Option<CasError>,
    ) -> CasResult<Option<(Vec<u8>, CanonicalObject)>> {
        for (index, entry) in self.pack_candidates(object_id, fresh)? {
            let checked = self.read_pack_member(&index, &entry).and_then(|bytes| {
                validate_stored_bytes(object_id, &bytes).map(|object| (bytes, object))
            });
            match checked {
                Ok(found) => return Ok(Some(found)),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Ok(None)
    }

    /// Tries every copy of `object_id`: loaded packs, the separate file
    /// (`loose`), then packs published since the last listing. The first copy
    /// that verifies wins; an error is returned only if none does. With
    /// `loose_is_final`, a separate file that fails is the answer (its
    /// consumer may already have seen some of its bytes).
    pub(super) fn with_copy<T>(
        &self,
        object_id: &StoreObjectId,
        loose_is_final: bool,
        loose: impl FnOnce() -> CasResult<T>,
    ) -> CasResult<Found<T>> {
        if self.packs.is_none() {
            return loose().map(Found::Loose);
        }
        let mut first_error = None;
        if let Some((bytes, object)) = self.verified_pack_copy(object_id, None, &mut first_error)? {
            return Ok(Found::Packed(bytes, object));
        }
        match loose() {
            Ok(value) => return Ok(Found::Loose(value)),
            Err(CasError::NotFound { .. }) => {}
            Err(error) if loose_is_final => return Err(error),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
        let fresh = self.refresh_packs().unwrap_or_else(|error| {
            first_error.get_or_insert(error);
            Vec::new()
        });
        if let Some((bytes, object)) =
            self.verified_pack_copy(object_id, Some(&fresh), &mut first_error)?
        {
            return Ok(Found::Packed(bytes, object));
        }
        Err(first_error.unwrap_or_else(|| CasError::NotFound {
            kind: object_id.kind(),
            id: object_id.to_string(),
        }))
    }

    pub(super) fn contains_any(
        &self,
        object_id: &StoreObjectId,
        loose: impl FnOnce() -> CasResult<bool>,
    ) -> CasResult<bool> {
        if !self.pack_candidates(object_id, None)?.is_empty() || loose()? {
            return Ok(true);
        }
        let fresh = self.refresh_packs()?;
        Ok(!self.pack_candidates(object_id, Some(&fresh))?.is_empty())
    }

    /// Where a copy of `object_id` is (see `CasLocation`), or `None`.
    pub fn locate(&self, object_id: &StoreObjectId) -> CasResult<Option<CasLocation>> {
        if let Some(catalog) = &self.compact {
            if let Some(location) = catalog.locate(self, object_id)? {
                return Ok(Some(location));
            }
            if is_content(object_id.kind()) {
                return Ok(None);
            }
        }
        let packed = |candidates: Vec<(Arc<PackIndex>, PackEntry)>| {
            candidates
                .first()
                .map(|(index, entry)| CasLocation::Packed {
                    pack: self.packs_path().join(&index.name),
                    offset: entry.offset,
                    length: u64::from(entry.length),
                })
        };
        if let Some(location) = packed(self.pack_candidates(object_id, None)?) {
            return Ok(Some(location));
        }
        let relative = object_relative_path(object_id)?;
        let path = self.objects_dir.join(&relative);
        match self
            .pinned_objects_for_read()?
            .open_relative_file(&relative)
        {
            Ok(file) => {
                let length = file
                    .file()
                    .metadata()
                    .map_err(|error| io_error("inspect CAS object", &path, error))?
                    .len();
                return Ok(Some(CasLocation::Loose { path, length }));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect CAS object", &path, error)),
        }
        let fresh = self.refresh_packs()?;
        Ok(packed(self.pack_candidates(object_id, Some(&fresh))?))
    }

    /// Reuses a pack copy whose bytes equal `bytes` (already validated): the
    /// pack is recorded for the batch's durability step. A copy that does not
    /// verify is ignored. Never lists the directory (the batch's inventory
    /// was taken at its start).
    pub(super) fn reuse_packed(&self, object_id: &StoreObjectId, bytes: &[u8]) -> CasResult<bool> {
        let Some(catalog) = &self.packs else {
            return Ok(false);
        };
        let key = PackKey::of(object_id)?;
        let candidates = catalog
            .read()
            .map_err(|_| poisoned(&self.objects_dir))?
            .candidates(&key);
        for (index, entry) in candidates {
            if self
                .read_pack_member(&index, &entry)
                .is_ok_and(|stored| stored == bytes)
            {
                let mut batch = self.batch.lock().map_err(|_| poisoned(&self.objects_dir))?;
                if let Some(state) = batch.as_mut() {
                    if state.mode == BatchDurability::Coalesced {
                        state.reused_packs.insert(index.name.clone());
                    }
                }
                crate::perf::count("cas.pack_reuse", 1);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Seals one staging pack, publishes it without replacement into
    /// `objects/packs`, checks the published file is the one written, records
    /// the directories for the batch's durability step and adds the pack to
    /// the catalog.
    pub(super) fn publish_pack(&self, writer: &mut PackWriter) -> CasResult<()> {
        let _span = crate::perf::span("cas.pack_publish");
        let sealed = writer.seal(self.batch_mode()?)?;
        self.hit(CasFaultPoint::BeforePackPublication)?;
        let objects = self.pinned_objects()?;
        let packs_path = self.packs_path();
        let packs = objects
            .ensure_child_unsynced(OsStr::new(PACKS_DIR))
            .map_err(|error| io_error("create CAS packs directory", &packs_path, error))?;
        let objects_dir = self.objects_dir.clone();
        let deferred = self.defer_directory(&objects_dir, &objects)?
            && self.defer_directory(&packs_path, &packs)?;
        let name = new_pack_name();
        let target = packs_path.join(&name);
        let existed = publish_no_clobber(&writer.file, &packs, &name)?;
        if existed {
            return Err(io_error(
                "publish CAS pack",
                &target,
                io::Error::new(io::ErrorKind::AlreadyExists, "pack name collision"),
            ));
        }
        // From here the file is the published pack (a Windows rename moved
        // the handle): never unlink it.
        writer.published = true;
        let installed = packs
            .open_file(&name)
            .map_err(|error| io_error("reread published CAS pack", &target, error))?;
        if !writer.file.same_file(&installed) {
            return Err(invalid_pack(
                &target,
                "published pack is not the file written",
            ));
        }
        self.hit(CasFaultPoint::AfterPackPublication)?;
        if !deferred {
            for directory in [&packs, &objects] {
                sync_directory_compatible(directory.sync())
                    .map_err(|error| io_error("sync CAS packs directory", &packs_path, error))?;
            }
        }
        self.cleanup_published_staging(&writer.file, &writer.path, false)?;
        crate::perf::count("cas.pack_objects", sealed.entries.len() as u64);
        if let Some(catalog) = &self.packs {
            catalog
                .write()
                .map_err(|_| poisoned(&packs_path))?
                .insert(Arc::new(sealed.published_as(name)));
        }
        Ok(())
    }

    /// Scrub of `objects/packs`: every record of every pack is verified at
    /// its own place (never through the lookup by id, so a sound copy
    /// elsewhere cannot hide a damaged member). Without a valid index, the
    /// self-describing records still name the damaged objects.
    pub(super) fn scrub_packs(&self, report: &mut CasScrubReport) -> CasResult<()> {
        let directory = self.packs_path();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error("read CAS packs", &directory, error)),
        };
        let packs = self
            .pinned_objects_for_read()?
            .open_child(OsStr::new(PACKS_DIR))
            .map_err(|error| io_error("pin CAS packs", &directory, error))?;
        let code = |error: &CasError| {
            if is_invalid_pack(error) {
                "cas_pack_invalid"
            } else {
                error.code()
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| io_error("read CAS packs", &directory, error))?;
            let path = entry.path();
            let name = entry.file_name();
            let file_type = entry
                .file_type()
                .map_err(|error| io_error("inspect CAS pack", &path, error))?;
            if !file_type.is_file() || !valid_pack_name(&name) {
                report.checked_objects += 1;
                report.issues.push(CasScrubIssue {
                    path,
                    code: "cas_pack_unknown_entry",
                    message: "entry in objects/packs is not a pack".into(),
                });
                continue;
            }
            let mut file = match packs.open_file(&name) {
                Ok(file) => file,
                Err(error) => {
                    let error = io_error("open CAS pack", &path, error);
                    report.checked_objects += 1;
                    report.issues.push(CasScrubIssue {
                        path,
                        code: error.code(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            match load_index(file.file_mut(), &path, name) {
                Ok(index) => {
                    for entry in index.entries() {
                        report.checked_objects += 1;
                        let (id, checked) = match entry.key.object_id() {
                            Ok(object_id) => {
                                let checked = read_member(file.file_mut(), &path, entry)
                                    .and_then(|bytes| validate_stored_bytes(&object_id, &bytes));
                                (object_id.to_string(), checked.map(|_| ()))
                            }
                            Err(error) => (String::from("pack entry"), Err(error)),
                        };
                        if let Err(error) = checked {
                            report.issues.push(CasScrubIssue {
                                path: path.clone(),
                                code: code(&error),
                                message: format!("{id}: {error}"),
                            });
                        }
                    }
                }
                Err(error) => {
                    report.checked_objects += 1;
                    report.issues.push(CasScrubIssue {
                        path: path.clone(),
                        code: code(&error),
                        message: error.to_string(),
                    });
                    let mut damaged = Vec::new();
                    let scanned = scan_records(file.file_mut(), &path, |key, read| {
                        let checked = key.object_id().and_then(|object_id| {
                            read.and_then(|bytes| validate_stored_bytes(&object_id, &bytes))
                                .map(|_| ())
                                .map_err(|error| CasError::Corruption {
                                    kind: object_id.kind(),
                                    id: object_id.to_string(),
                                    reason: error.to_string(),
                                })
                        });
                        if let Err(error) = checked {
                            damaged.push(error);
                        }
                    });
                    for error in damaged.into_iter().chain(scanned.err()) {
                        report.issues.push(CasScrubIssue {
                            path: path.clone(),
                            code: code(&error),
                            message: error.to_string(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> (PathBuf, fs::File) {
        let path = std::env::temp_dir().join(format!(
            "layrs-pack-test-{}-{name}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        (path, file)
    }

    fn key(byte: u8) -> PackKey {
        PackKey {
            kind: 1,
            digest: [byte; 32],
        }
    }

    fn write_pack(file: &mut fs::File, records: &[(PackKey, Vec<u8>)]) -> Vec<PackEntry> {
        file.write_all(&header_bytes()).unwrap();
        let mut offset = HEADER_LEN;
        let mut entries = Vec::new();
        for (key, bytes) in records {
            file.write_all(&record_header(key, bytes.len() as u32))
                .unwrap();
            file.write_all(bytes).unwrap();
            entries.push(PackEntry {
                key: *key,
                offset,
                length: bytes.len() as u32,
            });
            offset += RECORD_HEADER_LEN + bytes.len() as u64;
        }
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        let mut index = Vec::new();
        for entry in &entries {
            index.push(entry.key.kind);
            index.extend_from_slice(&entry.key.digest);
            index.extend_from_slice(&entry.offset.to_le_bytes());
            index.extend_from_slice(&entry.length.to_le_bytes());
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(&header_bytes());
        hasher.update(&index);
        file.write_all(&index).unwrap();
        file.write_all(&offset.to_le_bytes()).unwrap();
        file.write_all(&(index.len() as u64).to_le_bytes()).unwrap();
        file.write_all(&(entries.len() as u64).to_le_bytes())
            .unwrap();
        file.write_all(hasher.finalize().as_bytes()).unwrap();
        file.write_all(FOOTER_MAGIC).unwrap();
        entries
    }

    #[test]
    fn index_round_trips_and_members_read_exactly() {
        let (path, mut file) = temp_file("round");
        let records = vec![(key(9), b"nine".to_vec()), (key(2), b"two!".to_vec())];
        let entries = write_pack(&mut file, &records);
        let index = load_index(&mut file, &path, OsString::from("x")).unwrap();
        assert_eq!(index.entries(), &entries[..]);
        let found = index.find(&key(9)).unwrap();
        assert_eq!(read_member(&mut file, &path, &found).unwrap(), b"nine");
        assert!(index.find(&key(3)).is_none());
        drop(file);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn damaged_index_or_truncation_is_refused() {
        let (path, mut file) = temp_file("damage");
        write_pack(
            &mut file,
            &[(key(1), b"one".to_vec()), (key(2), b"two".to_vec())],
        );
        let len = file.metadata().unwrap().len();
        // One flipped index byte breaks the checksum.
        file.seek(SeekFrom::Start(len - FOOTER_LEN - 5)).unwrap();
        file.write_all(&[0xff]).unwrap();
        let error = load_index(&mut file, &path, OsString::new()).unwrap_err();
        assert!(is_invalid_pack(&error), "{error}");
        // Truncation loses the footer.
        file.set_len(len - 3).unwrap();
        assert!(is_invalid_pack(
            &load_index(&mut file, &path, OsString::new()).unwrap_err()
        ));
        drop(file);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_damaged_record_is_named_by_its_header() {
        let (path, mut file) = temp_file("record");
        write_pack(
            &mut file,
            &[(key(1), b"one".to_vec()), (key(2), b"two".to_vec())],
        );
        let index = load_index(&mut file, &path, OsString::new()).unwrap();
        // Change the length in the first record header: the index no longer
        // matches it.
        file.seek(SeekFrom::Start(HEADER_LEN + 33)).unwrap();
        file.write_all(&7_u32.to_le_bytes()).unwrap();
        let first = index.find(&key(1)).unwrap();
        assert!(is_invalid_pack(
            &read_member(&mut file, &path, &first).unwrap_err()
        ));
        let second = index.find(&key(2)).unwrap();
        assert_eq!(read_member(&mut file, &path, &second).unwrap(), b"two");
        drop(file);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn overlapping_or_unsorted_entries_are_refused() {
        let path = Path::new("pack");
        let entry = |byte, offset, length: u32| {
            let mut raw = vec![1_u8];
            raw.extend_from_slice(&[byte; 32]);
            raw.extend_from_slice(&u64::to_le_bytes(offset));
            raw.extend_from_slice(&length.to_le_bytes());
            raw
        };
        let unsorted = [entry(2, 16, 1), entry(1, 54, 1)].concat();
        assert!(parse_entries(&unsorted, 1000, path).is_err());
        let overlapping = [entry(1, 16, 10), entry(2, 20, 1)].concat();
        assert!(parse_entries(&overlapping, 1000, path).is_err());
        let outside = [entry(1, 16, 100)].concat();
        assert!(parse_entries(&outside, 60, path).is_err());
        let fine = [entry(1, 16, 1), entry(2, 54, 1)].concat();
        assert_eq!(parse_entries(&fine, 92, path).unwrap().len(), 2);
    }

    #[test]
    fn pack_names_are_strict() {
        assert!(valid_pack_name(OsStr::new(
            "pack-0123456789abcdef0123456789abcdef.lpk"
        )));
        assert!(!valid_pack_name(OsStr::new(
            "pack-0123456789ABCDEF0123456789abcdef.lpk"
        )));
        assert!(!valid_pack_name(OsStr::new("pack-01.lpk")));
        assert!(valid_pack_name(&new_pack_name()));
    }
}
