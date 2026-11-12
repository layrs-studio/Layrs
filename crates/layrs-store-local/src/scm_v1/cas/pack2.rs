//! Packs v2 of Store format 6 (ADR 0028, `docs/plans/store-format-6.md`).
//!
//! A pack is one immutable file `objects/packs/pack-<32 hex>.lp2`, written by
//! one batch and published by one no-clobber rename:
//!
//! 1. header: magic `LYRSPAK2`, version, reserved;
//! 2. blocks of records: at most 16 KiB of records each, a single record
//!    larger than that has a block of its own. A block is `flags u8,
//!    stored_len u32`, the stored bytes (a zstd frame, or the raw bytes when
//!    zstd does not shrink them, flag bit 0) and an 8-byte checksum (blake3 of
//!    the stored bytes, truncated). A record never spans blocks;
//! 3. metadata: the block table, the explicit records sorted by (kind, id),
//!    the file records (phase 3) with their ChunkId and manifest aliases, and
//!    the FileId section of the explicit manifests;
//! 4. footer: offset and size of the metadata, counts, blake3 of header and
//!    metadata, end magic.
//!
//! A record is `kind u8, codec u8, varint length, body`, without its id: ids
//! live only in the metadata, whose blake3 SQLite registers (`cas_packs`).
//! The metadata is still decoded as untrusted input (bounds, order,
//! uniqueness, ranges within blocks) and every read verifies its content
//! against the requested id (the caller does that, `compact.rs`).
//!
//! Admission ordinals: explicit record `i` (in metadata order) is logical
//! object `i`; file record `f` carries two logical objects, its chunk
//! (`explicit + 2f`) and its recipe manifest (`explicit + 2f + 1`).
//!
//! Version 2 (Store format 7, council s13 Q5) adds the `whole` recipe: a
//! file record whose chunk is named by its FileId stores no ChunkId and is
//! left out of the ChunkId order (its FileId order is that order). Version 1
//! never holds a `whole` record.
use super::*;

pub(super) const PACK2_SUFFIX: &str = ".lp2";
const HEADER_MAGIC: &[u8; 8] = b"LYRSPAK2";
const FOOTER_MAGIC: &[u8; 8] = b"LYRSP2EN";
const PACK2_VERSION: u32 = 1;
/// Packs of Store format 7: `whole` file records (council s13 Q5).
const PACK2_VERSION_WHOLE: u32 = 2;
pub(super) const HEADER_LEN: u64 = 16;
/// index offset, index length, explicit count, file count, digest, magic.
const FOOTER_LEN: u64 = 8 + 8 + 8 + 8 + 32 + 8;
/// Records whose header and body fit in this many bytes share blocks.
pub(super) const BLOCK_TARGET: usize = 16 * 1024;
/// Largest record body a pack holds; larger records stay loose files.
pub(super) const MAX_RECORD_BYTES: usize = 1024 * 1024;
/// A record header is at most 1 + 1 + a 3-byte varint for 1 MiB.
const MAX_RECORD_HEADER: usize = 1 + 1 + 10;
const MAX_RAW_BLOCK: usize = MAX_RECORD_BYTES + MAX_RECORD_HEADER;
const BLOCK_HEADER_LEN: u64 = 1 + 4;
const BLOCK_CHECKSUM_LEN: usize = 8;
/// Explicit records plus file records of one pack (a batch holds at most
/// 262,144 objects; this leaves room without trusting a footer blindly).
pub(super) const MAX_PACK_ENTRIES: u64 = 4_000_000;
pub(super) const MAX_PACK_BLOCKS: u64 = 4_000_000;
const MAX_METADATA_BYTES: u64 = 512 * 1024 * 1024;
/// Kind code of a file record (codes 1..=9 are `StoreObjectKind`s).
pub(super) const FILE_KIND: u8 = 10;
const CODEC_CANONICAL: u8 = 0;
const FLAG_RAW: u8 = 1;
const ZSTD_LEVEL: i32 = 3;

pub(super) fn pack2_corruption(path: &Path, reason: impl Into<String>) -> CasError {
    CasError::PackCorruption {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

/// Where a record is: its block, the offset of its record header inside the
/// decompressed block, and the length of its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Location {
    pub(super) block: u32,
    pub(super) offset: u32,
    pub(super) length: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BlockInfo {
    /// File offset of the block header.
    pub(super) offset: u64,
    pub(super) flags: u8,
    pub(super) stored_len: u32,
    pub(super) raw_len: u32,
}

/// The two manifest recipes a file record can carry (phase 3): the standard
/// fixed chunking of captures, and `single`. Any other recipe is stored as
/// an explicit manifest record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Recipe {
    Fixed,
    Single,
    /// The chunk is named by the FileId (council s13 Q5; packs version 2).
    Whole,
}

impl Recipe {
    fn code(self) -> u8 {
        match self {
            Self::Fixed => 0,
            Self::Single => 1,
            Self::Whole => 2,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Fixed),
            1 => Some(Self::Single),
            2 => Some(Self::Whole),
            _ => None,
        }
    }

    pub(super) fn chunking(self) -> layrs_core::scm::ChunkingDescriptor {
        match self {
            Self::Fixed => layrs_core::scm::fixed_chunking(0),
            Self::Single => layrs_core::scm::ChunkingDescriptor::single(),
            Self::Whole => layrs_core::scm::ChunkingDescriptor::whole(),
        }
    }

    /// The recipe of `manifest` when it can be a file record: exactly one
    /// chunk covering at most `MAX_RECORD_BYTES`, and a recipe of this enum.
    pub(super) fn of(manifest: &FileManifest) -> Option<Self> {
        let [chunk] = manifest.chunks() else {
            return None;
        };
        if chunk.offset() != 0
            || chunk.byte_length() != manifest.byte_length()
            || manifest.byte_length() > MAX_RECORD_BYTES as u64
        {
            return None;
        }
        [Self::Fixed, Self::Single, Self::Whole]
            .into_iter()
            .find(|recipe| &recipe.chunking() == manifest.chunking())
    }
}

/// Rebuilds the one-chunk manifest a file record stands for. Its id is
/// computed from these fields (canonical JCS), never from the record bytes.
pub(super) fn synthesize_manifest(
    file_id: &layrs_core::scm::FileId,
    chunk_id: &ChunkId,
    byte_length: u64,
    recipe: Recipe,
) -> Result<FileManifest, layrs_core::scm::ScmError> {
    FileManifest::new(
        file_id.clone(),
        byte_length,
        recipe.chunking(),
        vec![layrs_core::scm::FileChunk::new(
            chunk_id.clone(),
            0,
            byte_length,
        )?],
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileRecord {
    pub(super) file_id: [u8; 32],
    pub(super) location: Location,
    pub(super) chunk_id: [u8; 32],
    pub(super) recipe: Recipe,
    /// First 4 bytes of the recipe manifest's id: a lookup by manifest id
    /// narrows to these candidates, then recomputes the full id from the
    /// record's metadata (exact, and never hashes content).
    pub(super) manifest_prefix: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileIdEntry {
    pub(super) file_id: [u8; 32],
    /// Position of the manifest in the FileManifest run of the index.
    pub(super) manifest: u32,
    /// The manifest's file length, so a File's length is an index answer.
    pub(super) byte_length: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct KindRun {
    kind: u8,
    start: usize,
    end: usize,
    /// `fanout[b]`: entries of this kind whose first id byte is below `b`.
    fanout: Box<[u32; 257]>,
}

/// A decoded and validated pack v2 metadata area.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Pack2Index {
    /// 1, or 2 for a pack of Store format 7 (`whole` records).
    pub(super) version: u32,
    pub(super) file_len: u64,
    pub(super) digest: [u8; 32],
    pub(super) blocks: Vec<BlockInfo>,
    runs: Vec<KindRun>,
    ids: Vec<[u8; 32]>,
    locations: Vec<Location>,
    pub(super) files: Vec<FileRecord>,
    file_fanout: Box<[u32; 257]>,
    chunk_order: Vec<u32>,
    manifest_order: Vec<u32>,
    pub(super) fileid_section: Vec<FileIdEntry>,
}

fn fanout_of<'a>(ids: impl Iterator<Item = &'a [u8; 32]>) -> Box<[u32; 257]> {
    let mut fanout = Box::new([0_u32; 257]);
    for id in ids {
        fanout[usize::from(id[0]) + 1] += 1;
    }
    for byte in 1..257 {
        fanout[byte] += fanout[byte - 1];
    }
    fanout
}

fn search(ids: &[[u8; 32]], fanout: &[u32; 257], id: &[u8; 32]) -> Option<usize> {
    let low = fanout[usize::from(id[0])] as usize;
    let high = fanout[usize::from(id[0]) + 1] as usize;
    ids.get(low..high)?
        .binary_search(id)
        .ok()
        .map(|found| low + found)
}

impl Pack2Index {
    /// Explicit records (every kind but file records).
    pub(super) fn explicit_count(&self) -> usize {
        self.ids.len()
    }

    /// Logical objects of this pack: the length of an admission bitmap.
    pub(super) fn logical_count(&self) -> u64 {
        self.ids.len() as u64 + 2 * self.files.len() as u64
    }

    pub(super) fn find(&self, kind: u8, id: &[u8; 32]) -> Option<usize> {
        let run = self.runs.iter().find(|run| run.kind == kind)?;
        search(&self.ids[run.start..run.end], &run.fanout, id).map(|found| run.start + found)
    }

    /// Kind, id and location of explicit record `ordinal`.
    pub(super) fn explicit(&self, ordinal: usize) -> Option<(u8, &[u8; 32], Location)> {
        let run = self
            .runs
            .iter()
            .find(|run| (run.start..run.end).contains(&ordinal))?;
        Some((run.kind, &self.ids[ordinal], self.locations[ordinal]))
    }

    /// Explicit ordinals of one kind, in id order.
    pub(super) fn kind_range(&self, kind: u8) -> std::ops::Range<usize> {
        self.runs
            .iter()
            .find(|run| run.kind == kind)
            .map_or(0..0, |run| run.start..run.end)
    }

    pub(super) fn find_file(&self, file_id: &[u8; 32]) -> Option<usize> {
        let low = self.file_fanout[usize::from(file_id[0])] as usize;
        let high = self.file_fanout[usize::from(file_id[0]) + 1] as usize;
        self.files
            .get(low..high)?
            .binary_search_by(|record| record.file_id.cmp(file_id))
            .ok()
            .map(|found| low + found)
    }

    /// The file record standing for the chunk `chunk_id`: through the
    /// ChunkId order, or a `whole` record named by the same digest.
    pub(super) fn find_chunk_alias(&self, chunk_id: &[u8; 32]) -> Option<usize> {
        self.chunk_order
            .binary_search_by(|file| self.files[*file as usize].chunk_id.cmp(chunk_id))
            .ok()
            .map(|position| self.chunk_order[position] as usize)
            .or_else(|| {
                // Only a version 2 pack holds whole records.
                (self.version == PACK2_VERSION_WHOLE)
                    .then(|| self.find_file(chunk_id))
                    .flatten()
                    .filter(|file| self.files[*file].recipe == Recipe::Whole)
            })
    }

    /// File records whose manifest id starts like `manifest_id`; the caller
    /// recomputes each candidate's full id.
    pub(super) fn manifest_alias_candidates(
        &self,
        manifest_id: &[u8; 32],
    ) -> impl Iterator<Item = usize> + '_ {
        let prefix = [
            manifest_id[0],
            manifest_id[1],
            manifest_id[2],
            manifest_id[3],
        ];
        let start = self
            .manifest_order
            .partition_point(|file| self.files[*file as usize].manifest_prefix < prefix);
        self.manifest_order[start..]
            .iter()
            .map(|file| *file as usize)
            .take_while(move |file| self.files[*file].manifest_prefix == prefix)
    }

    /// FileId section entries of `file_id` (explicit manifests), in manifest
    /// position order.
    pub(super) fn fileid_manifests(&self, file_id: &[u8; 32]) -> &[FileIdEntry] {
        let start = self
            .fileid_section
            .partition_point(|entry| &entry.file_id < file_id);
        let end =
            start + self.fileid_section[start..].partition_point(|entry| &entry.file_id == file_id);
        &self.fileid_section[start..end]
    }

    /// Explicit ordinal of the FileManifest at `position` in its run.
    pub(super) fn manifest_ordinal(&self, position: u32) -> Option<usize> {
        let range = self.kind_range(kind_code(StoreObjectKind::FileManifest));
        let ordinal = range.start + position as usize;
        range.contains(&ordinal).then_some(ordinal)
    }

    pub(super) fn chunk_alias_ordinal(&self, file: usize) -> u64 {
        self.ids.len() as u64 + 2 * file as u64
    }

    pub(super) fn manifest_alias_ordinal(&self, file: usize) -> u64 {
        self.ids.len() as u64 + 2 * file as u64 + 1
    }

    /// Every record of each block: explicit ordinals and file record
    /// indexes, for scrub (a damaged block names all of them).
    pub(super) fn records_by_block(&self) -> Vec<Vec<RecordRef>> {
        let mut blocks = vec![Vec::new(); self.blocks.len()];
        for (ordinal, location) in self.locations.iter().enumerate() {
            if let Some(records) = blocks.get_mut(location.block as usize) {
                records.push(RecordRef::Explicit(ordinal));
            }
        }
        for (file, record) in self.files.iter().enumerate() {
            if let Some(records) = blocks.get_mut(record.location.block as usize) {
                records.push(RecordRef::File(file));
            }
        }
        blocks
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordRef {
    Explicit(usize),
    File(usize),
}

pub(super) fn kind_code(kind: StoreObjectKind) -> u8 {
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

pub(super) fn kind_of_code(code: u8) -> Option<StoreObjectKind> {
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

/// The raw 32-byte digest of an object id (`blake3:<64 hex>`).
pub(super) fn raw_digest(object_id: &StoreObjectId) -> CasResult<[u8; 32]> {
    hex_digest(digest(object_id)?).ok_or_else(|| CasError::InvalidObjectId {
        kind: object_id.kind(),
        id: object_id.to_string(),
    })
}

pub(super) fn hex_digest(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let high = (bytes[2 * index] as char).to_digit(16)?;
        let low = (bytes[2 * index + 1] as char).to_digit(16)?;
        if bytes[2 * index].is_ascii_uppercase() || bytes[2 * index + 1].is_ascii_uppercase() {
            return None;
        }
        *byte = (high * 16 + low) as u8;
    }
    Some(digest)
}

pub(super) fn digest_text(digest: &[u8; 32]) -> String {
    let mut text = String::with_capacity(71);
    text.push_str("blake3:");
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

pub(super) fn object_id_of(kind: StoreObjectKind, digest: &[u8; 32]) -> CasResult<StoreObjectId> {
    parse_object_id(kind, &digest_text(digest))
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn record_header(kind: u8, length: usize) -> Vec<u8> {
    let mut header = Vec::with_capacity(MAX_RECORD_HEADER);
    header.push(kind);
    header.push(CODEC_CANONICAL);
    put_varint(&mut header, length as u64);
    header
}

/// Strict little-endian/LEB128 reader over untrusted metadata.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    path: &'a Path,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> CasResult<&'a [u8]> {
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| pack2_corruption(self.path, "pack metadata is truncated"))?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn u8(&mut self) -> CasResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn id(&mut self) -> CasResult<[u8; 32]> {
        let mut id = [0_u8; 32];
        id.copy_from_slice(self.take(32)?);
        Ok(id)
    }

    /// A minimal LEB128 varint of at most `max`.
    fn varint(&mut self, max: u64) -> CasResult<u64> {
        let mut value = 0_u64;
        for index in 0..10 {
            let byte = self.u8()?;
            let bits = u64::from(byte & 0x7f);
            if index == 9 && bits > 1 {
                break;
            }
            value |= bits << (7 * index);
            if byte & 0x80 == 0 {
                if index > 0 && byte == 0 {
                    return Err(pack2_corruption(self.path, "pack varint is not minimal"));
                }
                if value > max {
                    return Err(pack2_corruption(
                        self.path,
                        "pack metadata value is out of bounds",
                    ));
                }
                return Ok(value);
            }
        }
        Err(pack2_corruption(self.path, "pack varint is too long"))
    }

    fn count(&mut self, max: u64) -> CasResult<usize> {
        Ok(self.varint(max)? as usize)
    }
}

fn read_location(reader: &mut Reader<'_>, blocks: &[BlockInfo]) -> CasResult<Location> {
    let block = reader.varint(u64::from(u32::MAX))? as u32;
    let offset = reader.varint(u64::from(u32::MAX))? as u32;
    let length = reader.varint(MAX_RECORD_BYTES as u64)? as u32;
    let info = blocks
        .get(block as usize)
        .ok_or_else(|| pack2_corruption(reader.path, "pack index entry names no block"))?;
    let header = record_header(0, length as usize).len() as u64;
    let end = u64::from(offset) + header + u64::from(length);
    if end > u64::from(info.raw_len) {
        return Err(pack2_corruption(
            reader.path,
            "pack index entry is outside its block",
        ));
    }
    Ok(location_of(block, offset, length))
}

fn location_of(block: u32, offset: u32, length: u32) -> Location {
    Location {
        block,
        offset,
        length,
    }
}

fn write_location(out: &mut Vec<u8>, location: Location) {
    put_varint(out, u64::from(location.block));
    put_varint(out, u64::from(location.offset));
    put_varint(out, u64::from(location.length));
}

fn header_bytes(version: u32) -> [u8; HEADER_LEN as usize] {
    let mut header = [0_u8; HEADER_LEN as usize];
    header[..8].copy_from_slice(HEADER_MAGIC);
    header[8..12].copy_from_slice(&version.to_le_bytes());
    header
}

/// The pack version a Store writes: 2 when it keeps `whole` records.
fn version_for(whole_files: bool) -> u32 {
    if whole_files {
        PACK2_VERSION_WHOLE
    } else {
        PACK2_VERSION
    }
}

fn le_u64(bytes: &[u8]) -> u64 {
    let mut value = [0_u8; 8];
    value.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(value)
}

/// Decodes and validates the metadata of a pack whose whole length is
/// `file_len`, given its header, footer and metadata bytes.
fn decode(
    path: &Path,
    file_len: u64,
    header: &[u8],
    footer: &[u8],
    metadata: &[u8],
) -> CasResult<Pack2Index> {
    let version = [PACK2_VERSION, PACK2_VERSION_WHOLE]
        .into_iter()
        .find(|version| header == header_bytes(*version))
        .ok_or_else(|| pack2_corruption(path, "unknown pack header or version"))?;
    if &footer[64..72] != FOOTER_MAGIC {
        return Err(pack2_corruption(path, "pack footer magic is missing"));
    }
    let index_offset = le_u64(&footer[0..8]);
    let explicit_count = le_u64(&footer[16..24]);
    let file_count = le_u64(&footer[24..32]);
    let mut hasher = blake3::Hasher::new();
    hasher.update(header);
    hasher.update(metadata);
    let digest = *hasher.finalize().as_bytes();
    if digest[..] != footer[32..64] {
        return Err(pack2_corruption(path, "pack metadata checksum differs"));
    }
    let mut reader = Reader {
        bytes: metadata,
        at: 0,
        path,
    };
    let block_count = reader.count(MAX_PACK_BLOCKS)?;
    let mut blocks = Vec::with_capacity(block_count.min(1 << 16));
    let mut offset = HEADER_LEN;
    for _ in 0..block_count {
        let flags = reader.u8()?;
        let raw_len = reader.varint(MAX_RAW_BLOCK as u64)? as u32;
        let stored_len = reader.varint(zstd_bound(MAX_RAW_BLOCK) as u64)? as u32;
        let valid = match flags {
            0 => raw_len > 0 && stored_len as usize <= zstd_bound(raw_len as usize),
            FLAG_RAW => raw_len > 0 && stored_len == raw_len,
            _ => false,
        };
        if !valid {
            return Err(pack2_corruption(path, "pack block table entry is invalid"));
        }
        blocks.push(BlockInfo {
            offset,
            flags,
            stored_len,
            raw_len,
        });
        offset += BLOCK_HEADER_LEN + u64::from(stored_len) + BLOCK_CHECKSUM_LEN as u64;
    }
    if offset != index_offset {
        return Err(pack2_corruption(
            path,
            "pack blocks do not end where the metadata starts",
        ));
    }
    let kind_count = reader.count(9)?;
    let mut runs: Vec<KindRun> = Vec::with_capacity(kind_count);
    let mut ids = Vec::new();
    let mut locations = Vec::new();
    for _ in 0..kind_count {
        let kind = reader.u8()?;
        if kind_of_code(kind).is_none() || runs.last().is_some_and(|run| run.kind >= kind) {
            return Err(pack2_corruption(path, "pack index kinds are invalid"));
        }
        let count = reader.count(MAX_PACK_ENTRIES)?;
        if count == 0 || (ids.len() + count) as u64 > MAX_PACK_ENTRIES {
            return Err(pack2_corruption(path, "pack index kind count is invalid"));
        }
        let start = ids.len();
        for _ in 0..count {
            let id = reader.id()?;
            if ids.len() > start && ids.last().is_some_and(|last: &[u8; 32]| *last >= id) {
                return Err(pack2_corruption(path, "pack index is not strictly sorted"));
            }
            ids.push(id);
        }
        for _ in 0..count {
            locations.push(read_location(&mut reader, &blocks)?);
        }
        let fanout = fanout_of(ids[start..].iter());
        runs.push(KindRun {
            kind,
            start,
            end: ids.len(),
            fanout,
        });
    }
    let files_len = reader.count(MAX_PACK_ENTRIES)?;
    if (ids.len() + files_len) as u64 > MAX_PACK_ENTRIES
        || ids.len() as u64 != explicit_count
        || files_len as u64 != file_count
    {
        return Err(pack2_corruption(
            path,
            "pack index counts differ from the footer",
        ));
    }
    let mut files: Vec<FileRecord> = Vec::with_capacity(files_len);
    for _ in 0..files_len {
        let file_id = reader.id()?;
        if files.last().is_some_and(|last| last.file_id >= file_id) {
            return Err(pack2_corruption(
                path,
                "pack file records are not strictly sorted",
            ));
        }
        let location = read_location(&mut reader, &blocks)?;
        if location.length == 0 {
            return Err(pack2_corruption(path, "pack file record is empty"));
        }
        let (chunk_id, recipe) = if version == PACK2_VERSION {
            let chunk_id = reader.id()?;
            let recipe = Recipe::from_code(reader.u8()?)
                .filter(|recipe| *recipe != Recipe::Whole)
                .ok_or_else(|| pack2_corruption(path, "pack file record has an unknown recipe"))?;
            (chunk_id, recipe)
        } else {
            let recipe = Recipe::from_code(reader.u8()?)
                .ok_or_else(|| pack2_corruption(path, "pack file record has an unknown recipe"))?;
            // A whole record's chunk is named by its FileId.
            let chunk_id = if recipe == Recipe::Whole {
                file_id
            } else {
                reader.id()?
            };
            (chunk_id, recipe)
        };
        let prefix = reader.take(4)?;
        files.push(FileRecord {
            file_id,
            location,
            chunk_id,
            recipe,
            manifest_prefix: [prefix[0], prefix[1], prefix[2], prefix[3]],
        });
    }
    // Version 2 orders the ChunkIds of the records that are not whole only.
    let ordered: Vec<bool> = files
        .iter()
        .map(|file| file.recipe != Recipe::Whole)
        .collect();
    let chunk_order = read_order(&mut reader, &ordered)?;
    if chunk_order
        .windows(2)
        .any(|pair| files[pair[0] as usize].chunk_id >= files[pair[1] as usize].chunk_id)
    {
        return Err(pack2_corruption(
            path,
            "pack chunk aliases are not strictly sorted",
        ));
    }
    // A ChunkId names one record: never a whole record's FileId as well,
    // whether the other is a file record's chunk or an explicit chunk.
    let whole_named = |chunk: &[u8; 32]| {
        files
            .binary_search_by(|record| record.file_id.cmp(chunk))
            .is_ok_and(|found| files[found].recipe == Recipe::Whole)
    };
    let explicit_chunks = runs
        .iter()
        .find(|run| run.kind == kind_code(StoreObjectKind::ChunkRaw))
        .map_or(&ids[0..0], |run| &ids[run.start..run.end]);
    if version == PACK2_VERSION_WHOLE
        && (chunk_order
            .iter()
            .any(|file| whole_named(&files[*file as usize].chunk_id))
            || explicit_chunks.iter().any(whole_named))
    {
        return Err(pack2_corruption(
            path,
            "a pack chunk alias is also a whole file record",
        ));
    }
    let manifest_order = read_permutation(&mut reader, files.len())?;
    if manifest_order.windows(2).any(|pair| {
        (files[pair[0] as usize].manifest_prefix, pair[0])
            >= (files[pair[1] as usize].manifest_prefix, pair[1])
    }) {
        return Err(pack2_corruption(
            path,
            "pack manifest aliases are not sorted",
        ));
    }
    let manifests = runs
        .iter()
        .find(|run| run.kind == kind_code(StoreObjectKind::FileManifest))
        .map_or(0, |run| run.end - run.start);
    let section_len = reader.count(MAX_PACK_ENTRIES)?;
    if section_len != manifests {
        return Err(pack2_corruption(
            path,
            "pack FileId section does not cover every manifest",
        ));
    }
    let mut seen = vec![false; manifests];
    let mut fileid_section: Vec<FileIdEntry> = Vec::with_capacity(section_len);
    for _ in 0..section_len {
        let file_id = reader.id()?;
        let manifest = reader.varint(manifests.saturating_sub(1) as u64)? as u32;
        let byte_length = reader.varint(u64::MAX)?;
        if std::mem::replace(&mut seen[manifest as usize], true) {
            return Err(pack2_corruption(
                path,
                "pack FileId section repeats a manifest",
            ));
        }
        if fileid_section
            .last()
            .is_some_and(|last| (last.file_id, last.manifest) >= (file_id, manifest))
        {
            return Err(pack2_corruption(path, "pack FileId section is not sorted"));
        }
        fileid_section.push(FileIdEntry {
            file_id,
            manifest,
            byte_length,
        });
    }
    if reader.at != metadata.len() {
        return Err(pack2_corruption(path, "pack metadata has trailing bytes"));
    }
    let file_fanout = fanout_of(files.iter().map(|file| &file.file_id));
    Ok(Pack2Index {
        version,
        file_len,
        digest,
        blocks,
        runs,
        ids,
        locations,
        files,
        file_fanout,
        chunk_order,
        manifest_order,
        fileid_section,
    })
}

/// A permutation of the records `ordered` marks (every record in version 1,
/// those that are not whole in version 2).
fn read_order(reader: &mut Reader<'_>, ordered: &[bool]) -> CasResult<Vec<u32>> {
    let count = ordered.iter().filter(|listed| **listed).count();
    let mut seen = vec![false; ordered.len()];
    let mut order = Vec::with_capacity(count);
    for _ in 0..count {
        let file = reader.varint(ordered.len().saturating_sub(1) as u64)? as usize;
        if !ordered[file] || std::mem::replace(&mut seen[file], true) {
            return Err(pack2_corruption(
                reader.path,
                "pack alias order repeats or lists a whole record",
            ));
        }
        order.push(file as u32);
    }
    Ok(order)
}

fn read_permutation(reader: &mut Reader<'_>, length: usize) -> CasResult<Vec<u32>> {
    let mut seen = vec![false; length];
    let mut order = Vec::with_capacity(length);
    for _ in 0..length {
        let file = reader.varint(length.saturating_sub(1) as u64)? as usize;
        if std::mem::replace(&mut seen[file], true) {
            return Err(pack2_corruption(
                reader.path,
                "pack alias order repeats a record",
            ));
        }
        order.push(file as u32);
    }
    Ok(order)
}

fn zstd_bound(raw_len: usize) -> usize {
    zstd::zstd_safe::compress_bound(raw_len)
}

/// Positional exact read; a short read is corruption of a registered pack.
pub(super) fn read_at(
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
            Ok(0) => return Err(pack2_corruption(path, "pack is truncated")),
            Ok(count) => done += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(io_error("read CAS pack", path, error)),
        }
    }
    Ok(())
}

/// Where a pack's bytes are: its file, or its whole image in memory (a
/// small pack registered inline in SQLite, council s7 Q1). The same
/// validation applies to both.
#[derive(Clone, Copy)]
pub(super) enum PackSource<'a> {
    File(&'a fs::File),
    Image(&'a [u8]),
}

impl PackSource<'_> {
    fn len(&self, path: &Path) -> CasResult<u64> {
        match self {
            Self::File(file) => Ok(file
                .metadata()
                .map_err(|error| io_error("stat CAS pack", path, error))?
                .len()),
            Self::Image(image) => Ok(image.len() as u64),
        }
    }

    fn read_at(&self, path: &Path, offset: u64, buffer: &mut [u8]) -> CasResult<()> {
        match self {
            Self::File(file) => read_at(file, path, offset, buffer),
            Self::Image(image) => {
                let bytes = usize::try_from(offset)
                    .ok()
                    .and_then(|start| image.get(start..start.checked_add(buffer.len())?))
                    .ok_or_else(|| pack2_corruption(path, "pack is truncated"))?;
                buffer.copy_from_slice(bytes);
                Ok(())
            }
        }
    }
}

/// Reads and validates the metadata of the pack open in `file`. The caller
/// compares `digest` with the digest SQLite registered.
pub(super) fn load(file: &fs::File, path: &Path) -> CasResult<Pack2Index> {
    load_from(PackSource::File(file), path)
}

/// `load` from either source.
pub(super) fn load_from(source: PackSource<'_>, path: &Path) -> CasResult<Pack2Index> {
    let file_len = source.len(path)?;
    if file_len < HEADER_LEN + FOOTER_LEN {
        return Err(pack2_corruption(
            path,
            "pack is shorter than its header and footer",
        ));
    }
    let mut header = [0_u8; HEADER_LEN as usize];
    source.read_at(path, 0, &mut header)?;
    let mut footer = [0_u8; FOOTER_LEN as usize];
    source.read_at(path, file_len - FOOTER_LEN, &mut footer)?;
    let index_offset = le_u64(&footer[0..8]);
    let index_len = le_u64(&footer[8..16]);
    if index_len > MAX_METADATA_BYTES
        || index_offset < HEADER_LEN
        || index_offset.checked_add(index_len) != Some(file_len - FOOTER_LEN)
    {
        return Err(pack2_corruption(path, "pack footer is out of bounds"));
    }
    let mut metadata = vec![0_u8; index_len as usize];
    source.read_at(path, index_offset, &mut metadata)?;
    decode(path, file_len, &header, &footer, &metadata)
}

/// Reads one block, checks its header against the table and its checksum,
/// and decompresses it into exactly its registered raw length.
pub(super) fn read_block(
    file: &fs::File,
    path: &Path,
    index: &Pack2Index,
    block: u32,
) -> CasResult<Vec<u8>> {
    read_block_from(PackSource::File(file), path, index, block)
}

/// `read_block` from either source.
pub(super) fn read_block_from(
    source: PackSource<'_>,
    path: &Path,
    index: &Pack2Index,
    block: u32,
) -> CasResult<Vec<u8>> {
    let info = index
        .blocks
        .get(block as usize)
        .ok_or_else(|| pack2_corruption(path, "pack block does not exist"))?;
    let stored_len = info.stored_len as usize;
    let mut bytes = vec![0_u8; BLOCK_HEADER_LEN as usize + stored_len + BLOCK_CHECKSUM_LEN];
    source.read_at(path, info.offset, &mut bytes)?;
    if bytes[0] != info.flags || bytes[1..5] != info.stored_len.to_le_bytes() {
        return Err(pack2_corruption(
            path,
            format!("pack block {block} header differs from the block table"),
        ));
    }
    let stored = &bytes[BLOCK_HEADER_LEN as usize..BLOCK_HEADER_LEN as usize + stored_len];
    let checksum = &bytes[BLOCK_HEADER_LEN as usize + stored_len..];
    if blake3::hash(stored).as_bytes()[..BLOCK_CHECKSUM_LEN] != *checksum {
        return Err(pack2_corruption(
            path,
            format!("pack block {block} checksum differs"),
        ));
    }
    let raw = if info.flags & FLAG_RAW != 0 {
        stored.to_vec()
    } else {
        zstd::bulk::decompress(stored, info.raw_len as usize).map_err(|error| {
            pack2_corruption(
                path,
                format!("pack block {block} does not decompress: {error}"),
            )
        })?
    };
    if raw.len() != info.raw_len as usize {
        return Err(pack2_corruption(
            path,
            format!("pack block {block} has another length than registered"),
        ));
    }
    Ok(raw)
}

/// The body of the record at `location` in decompressed `block`, after
/// checking its header against the index (kind, codec, length).
pub(super) fn record_body<'a>(
    block: &'a [u8],
    location: Location,
    kind: u8,
    path: &Path,
) -> CasResult<&'a [u8]> {
    let expected = record_header(kind, location.length as usize);
    let start = location.offset as usize;
    let body_start = start + expected.len();
    let end = body_start + location.length as usize;
    if block.get(start..body_start) != Some(&expected[..]) || end > block.len() {
        return Err(pack2_corruption(
            path,
            "pack record header differs from its index entry",
        ));
    }
    Ok(&block[body_start..end])
}

/// One block compressed (or kept raw), ready to append.
pub(super) struct EncodedBlock {
    flags: u8,
    stored: Vec<u8>,
    raw_len: u32,
    raw_hash: blake3::Hash,
}

impl EncodedBlock {
    pub(super) fn encode(raw: &[u8]) -> io::Result<Self> {
        let compressed = zstd::bulk::compress(raw, ZSTD_LEVEL)?;
        let (flags, stored) = if compressed.len() < raw.len() {
            (0, compressed)
        } else {
            (FLAG_RAW, raw.to_vec())
        };
        Ok(Self {
            flags,
            stored,
            raw_len: u32::try_from(raw.len()).map_err(io::Error::other)?,
            raw_hash: blake3::hash(raw),
        })
    }

    /// One record alone in its block (bodies larger than a shared block).
    pub(super) fn single_record(kind: u8, body: &[u8]) -> io::Result<Self> {
        let mut raw = record_header(kind, body.len());
        raw.extend_from_slice(body);
        Self::encode(&raw)
    }
}

/// Whether a record body gets a block of its own.
pub(super) fn is_large(body_len: usize) -> bool {
    record_header(0, body_len).len() + body_len > BLOCK_TARGET
}

/// The identities a file record stands for (raw digests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileIdentity {
    pub(super) file_id: [u8; 32],
    pub(super) chunk_id: [u8; 32],
    pub(super) recipe: Recipe,
    pub(super) manifest_id: [u8; 32],
}

/// Streams a pack v2: records into blocks, then metadata and footer.
pub(super) struct Pack2Builder {
    /// `PACK2_VERSION`, or `PACK2_VERSION_WHOLE` in Store format 7.
    version: u32,
    block: Vec<u8>,
    /// Records of the open block: (explicit index or file index, offset).
    blocks: Vec<BlockInfo>,
    block_hashes: Vec<blake3::Hash>,
    written: u64,
    explicit: Vec<(u8, [u8; 32], Location)>,
    files: Vec<(FileRecord, [u8; 32])>,
    fileid: Vec<([u8; 32], [u8; 32], u64)>,
}

/// What `Pack2Builder::finish` wrote, for the reread check and registration.
#[derive(Debug)]
pub(super) struct SealedPack2 {
    pub(super) digest: [u8; 32],
    pub(super) logical: u64,
    pub(super) explicit: u64,
    pub(super) files: u64,
    pub(super) block_hashes: Vec<blake3::Hash>,
    /// Its keys row (council s17, B1b), registered with it.
    pub(super) keys: super::keys::PackKeys,
}

impl Pack2Builder {
    /// A pack of a Store that keeps `whole` records (format 7) or not.
    pub(super) fn start(out: &mut impl Write, whole_files: bool) -> io::Result<Self> {
        let version = version_for(whole_files);
        out.write_all(&header_bytes(version))?;
        Ok(Self {
            version,
            block: Vec::with_capacity(BLOCK_TARGET),
            blocks: Vec::new(),
            block_hashes: Vec::new(),
            written: HEADER_LEN,
            explicit: Vec::new(),
            files: Vec::new(),
            fileid: Vec::new(),
        })
    }

    fn append_block(&mut self, out: &mut impl Write, block: EncodedBlock) -> io::Result<u32> {
        let index = u32::try_from(self.blocks.len()).map_err(io::Error::other)?;
        let stored_len = u32::try_from(block.stored.len()).map_err(io::Error::other)?;
        out.write_all(&[block.flags])?;
        out.write_all(&stored_len.to_le_bytes())?;
        out.write_all(&block.stored)?;
        out.write_all(&blake3::hash(&block.stored).as_bytes()[..BLOCK_CHECKSUM_LEN])?;
        self.blocks.push(BlockInfo {
            offset: self.written,
            flags: block.flags,
            stored_len,
            raw_len: block.raw_len,
        });
        self.block_hashes.push(block.raw_hash);
        self.written += BLOCK_HEADER_LEN + u64::from(stored_len) + BLOCK_CHECKSUM_LEN as u64;
        Ok(index)
    }

    fn flush(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.block.is_empty() {
            return Ok(());
        }
        let raw = std::mem::take(&mut self.block);
        self.append_block(out, EncodedBlock::encode(&raw)?)?;
        self.block = Vec::with_capacity(BLOCK_TARGET);
        Ok(())
    }

    /// Appends one record and returns where it is. A large body is encoded
    /// here unless the caller already did (`encoded`, outside any lock).
    fn place(
        &mut self,
        out: &mut impl Write,
        kind: u8,
        body: &[u8],
        encoded: Option<EncodedBlock>,
    ) -> io::Result<Location> {
        if body.len() > MAX_RECORD_BYTES {
            return Err(io::Error::other("record exceeds the pack record limit"));
        }
        let length = body.len() as u32;
        if is_large(body.len()) {
            // Flushing first keeps every block index final once assigned.
            self.flush(out)?;
            let block = match encoded {
                Some(block) => block,
                None => EncodedBlock::single_record(kind, body)?,
            };
            let index = self.append_block(out, block)?;
            return Ok(location_of(index, 0, length));
        }
        let header = record_header(kind, body.len());
        if self.block.len() + header.len() + body.len() > BLOCK_TARGET {
            self.flush(out)?;
        }
        let offset = self.block.len() as u32;
        self.block.extend_from_slice(&header);
        self.block.extend_from_slice(body);
        Ok(location_of(self.blocks.len() as u32, offset, length))
    }

    pub(super) fn add_record(
        &mut self,
        out: &mut impl Write,
        kind: StoreObjectKind,
        id: [u8; 32],
        body: &[u8],
        encoded: Option<EncodedBlock>,
    ) -> io::Result<()> {
        let code = kind_code(kind);
        let location = self.place(out, code, body, encoded)?;
        self.explicit.push((code, id, location));
        Ok(())
    }

    /// The FileId section entry of an explicit manifest record.
    pub(super) fn add_manifest_file(
        &mut self,
        file_id: [u8; 32],
        manifest_id: [u8; 32],
        length: u64,
    ) {
        self.fileid.push((file_id, manifest_id, length));
    }

    /// One file record: the raw bytes of a single-chunk File, keyed by its
    /// FileId, standing for its chunk and its recipe manifest.
    pub(super) fn add_file(
        &mut self,
        out: &mut impl Write,
        identity: FileIdentity,
        body: &[u8],
        encoded: Option<EncodedBlock>,
    ) -> io::Result<()> {
        if body.is_empty() {
            return Err(io::Error::other("a file record holds at least one byte"));
        }
        if identity.recipe == Recipe::Whole
            && (self.version != PACK2_VERSION_WHOLE || identity.chunk_id != identity.file_id)
        {
            return Err(io::Error::other(
                "a whole file record is named by its FileId, in a format 7 pack",
            ));
        }
        let location = self.place(out, FILE_KIND, body, encoded)?;
        let manifest_id = identity.manifest_id;
        self.files.push((
            FileRecord {
                file_id: identity.file_id,
                location,
                chunk_id: identity.chunk_id,
                recipe: identity.recipe,
                manifest_prefix: [
                    manifest_id[0],
                    manifest_id[1],
                    manifest_id[2],
                    manifest_id[3],
                ],
            },
            manifest_id,
        ));
        Ok(())
    }

    /// Flushes the last block, writes metadata and footer.
    pub(super) fn finish(mut self, out: &mut impl Write) -> io::Result<SealedPack2> {
        self.flush(out)?;
        let invalid = |reason: &str| io::Error::other(reason.to_owned());
        self.explicit
            .sort_unstable_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
        if self
            .explicit
            .windows(2)
            .any(|pair| (pair[0].0, pair[0].1) == (pair[1].0, pair[1].1))
        {
            return Err(invalid("duplicate record in a pack"));
        }
        self.files
            .sort_unstable_by(|left, right| left.0.file_id.cmp(&right.0.file_id));
        if self
            .files
            .windows(2)
            .any(|pair| pair[0].0.file_id == pair[1].0.file_id)
        {
            return Err(invalid("duplicate file record in a pack"));
        }
        let manifest_code = kind_code(StoreObjectKind::FileManifest);
        let manifest_ids: Vec<[u8; 32]> = self
            .explicit
            .iter()
            .filter(|entry| entry.0 == manifest_code)
            .map(|entry| entry.1)
            .collect();
        if self.fileid.len() != manifest_ids.len() {
            return Err(invalid("every explicit manifest needs its FileId entry"));
        }
        let mut section = Vec::with_capacity(self.fileid.len());
        for (file_id, manifest_id, length) in &self.fileid {
            let position = manifest_ids
                .binary_search(manifest_id)
                .map_err(|_| invalid("FileId entry of a manifest outside the pack"))?;
            section.push((*file_id, position as u32, *length));
        }
        section.sort_unstable();
        if section.windows(2).any(|pair| pair[0].1 == pair[1].1) {
            return Err(invalid("a manifest has two FileId entries"));
        }

        let mut metadata = Vec::new();
        put_varint(&mut metadata, self.blocks.len() as u64);
        for block in &self.blocks {
            metadata.push(block.flags);
            put_varint(&mut metadata, u64::from(block.raw_len));
            put_varint(&mut metadata, u64::from(block.stored_len));
        }
        let mut kinds: Vec<u8> = self.explicit.iter().map(|entry| entry.0).collect();
        kinds.dedup();
        put_varint(&mut metadata, kinds.len() as u64);
        for kind in kinds {
            let run: Vec<_> = self
                .explicit
                .iter()
                .filter(|entry| entry.0 == kind)
                .collect();
            metadata.push(kind);
            put_varint(&mut metadata, run.len() as u64);
            for entry in &run {
                metadata.extend_from_slice(&entry.1);
            }
            for entry in &run {
                write_location(&mut metadata, entry.2);
            }
        }
        put_varint(&mut metadata, self.files.len() as u64);
        for (file, _) in &self.files {
            metadata.extend_from_slice(&file.file_id);
            write_location(&mut metadata, file.location);
            if self.version == PACK2_VERSION {
                metadata.extend_from_slice(&file.chunk_id);
                metadata.push(file.recipe.code());
            } else {
                metadata.push(file.recipe.code());
                if file.recipe != Recipe::Whole {
                    metadata.extend_from_slice(&file.chunk_id);
                }
            }
            metadata.extend_from_slice(&file.manifest_prefix);
        }
        // Version 2 orders the ChunkIds of the records that are not whole.
        let mut chunk_order: Vec<u32> = (0..self.files.len() as u32)
            .filter(|file| self.files[*file as usize].0.recipe != Recipe::Whole)
            .collect();
        chunk_order.sort_unstable_by(|left, right| {
            self.files[*left as usize]
                .0
                .chunk_id
                .cmp(&self.files[*right as usize].0.chunk_id)
        });
        let whole_named = |chunk: &[u8; 32]| {
            self.files
                .binary_search_by(|record| record.0.file_id.cmp(chunk))
                .is_ok_and(|found| self.files[found].0.recipe == Recipe::Whole)
        };
        let chunk_code = kind_code(StoreObjectKind::ChunkRaw);
        if chunk_order.windows(2).any(|pair| {
            self.files[pair[0] as usize].0.chunk_id == self.files[pair[1] as usize].0.chunk_id
        }) || chunk_order
            .iter()
            .any(|file| whole_named(&self.files[*file as usize].0.chunk_id))
            || self
                .explicit
                .iter()
                .any(|entry| entry.0 == chunk_code && whole_named(&entry.1))
        {
            return Err(invalid("two file records share a chunk"));
        }
        for file in &chunk_order {
            put_varint(&mut metadata, u64::from(*file));
        }
        let mut manifest_order: Vec<u32> = (0..self.files.len() as u32).collect();
        manifest_order
            .sort_unstable_by_key(|file| (self.files[*file as usize].0.manifest_prefix, *file));
        for file in &manifest_order {
            put_varint(&mut metadata, u64::from(*file));
        }
        put_varint(&mut metadata, section.len() as u64);
        for (file_id, position, length) in &section {
            metadata.extend_from_slice(file_id);
            put_varint(&mut metadata, u64::from(*position));
            put_varint(&mut metadata, *length);
        }
        let keys = self.keys(&section)?;

        let mut hasher = blake3::Hasher::new();
        hasher.update(&header_bytes(self.version));
        hasher.update(&metadata);
        let digest = *hasher.finalize().as_bytes();
        let mut footer = Vec::with_capacity(FOOTER_LEN as usize);
        footer.extend_from_slice(&self.written.to_le_bytes());
        footer.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
        footer.extend_from_slice(&(self.explicit.len() as u64).to_le_bytes());
        footer.extend_from_slice(&(self.files.len() as u64).to_le_bytes());
        footer.extend_from_slice(&digest);
        footer.extend_from_slice(FOOTER_MAGIC);
        out.write_all(&metadata)?;
        out.write_all(&footer)?;
        Ok(SealedPack2 {
            digest,
            logical: self.explicit.len() as u64 + 2 * self.files.len() as u64,
            explicit: self.explicit.len() as u64,
            files: self.files.len() as u64,
            block_hashes: self.block_hashes,
            keys,
        })
    }

    /// The keys row of the sealed pack, from its records in index order:
    /// explicit records sorted by kind and id, then file records by FileId,
    /// each with its chunk and manifest aliases (the ordinals of the
    /// admission bitmaps), and the FileId section.
    fn keys(&self, section: &[([u8; 32], u32, u64)]) -> io::Result<super::keys::PackKeys> {
        use super::keys::{self, Ordinal, SectionEntry};
        let mut ordinals = Vec::with_capacity(self.explicit.len() + 2 * self.files.len());
        let mut tagged =
            Vec::with_capacity(self.explicit.len() + 3 * self.files.len() + section.len());
        for (code, id, _) in &self.explicit {
            ordinals.push(Ordinal::Explicit {
                kind_code: *code,
                id,
            });
            tagged.push(keys::explicit_key(*code, id));
        }
        for (file, manifest_id) in &self.files {
            ordinals.push(Ordinal::ChunkAlias {
                file_id: &file.file_id,
                chunk_id: &file.chunk_id,
                recipe: file.recipe.code(),
                length: u64::from(file.location.length),
            });
            ordinals.push(Ordinal::ManifestAlias {
                file_id: &file.file_id,
                manifest_id,
            });
            tagged.push(keys::key(keys::FILE_RECORD_TAG, &file.file_id));
            // A whole record's chunk is named by its FileId: the record key
            // answers for it.
            if file.recipe != Recipe::Whole {
                tagged.push(keys::key(keys::CHUNK_ALIAS_TAG, &file.chunk_id));
            }
            tagged.push(keys::key(keys::MANIFEST_ALIAS_TAG, manifest_id));
        }
        let entries: Vec<SectionEntry<'_>> = section
            .iter()
            .map(|(file_id, position, length)| SectionEntry {
                file_id,
                manifest_position: *position,
                length: *length,
            })
            .collect();
        for (file_id, _, _) in section {
            tagged.push(keys::key(keys::FILEID_SECTION_TAG, file_id));
        }
        keys::PackKeys::build(&ordinals, &entries, tagged, keys::RICE_BITS)
    }
}

/// Rereads a sealed pack: its metadata must decode to the written digest and
/// every block must decompress to the raw bytes written.
pub(super) fn verify_written(
    file: &fs::File,
    path: &Path,
    sealed: &SealedPack2,
) -> CasResult<Pack2Index> {
    verify_written_from(PackSource::File(file), path, sealed)
}

/// `verify_written` from either source: an inline image is validated as
/// untrusted input exactly as a reread file (council s7 Q1).
pub(super) fn verify_written_from(
    source: PackSource<'_>,
    path: &Path,
    sealed: &SealedPack2,
) -> CasResult<Pack2Index> {
    let index = load_from(source, path)?;
    if index.digest != sealed.digest
        || index.explicit_count() as u64 != sealed.explicit
        || index.files.len() as u64 != sealed.files
        || index.blocks.len() != sealed.block_hashes.len()
    {
        return Err(pack2_corruption(
            path,
            "pack reread differs from the written one",
        ));
    }
    for (block, hash) in sealed.block_hashes.iter().enumerate() {
        if blake3::hash(&read_block_from(source, path, &index, block as u32)?) != *hash {
            return Err(pack2_corruption(
                path,
                "pack block reread differs from the written one",
            ));
        }
    }
    Ok(index)
}

pub(super) fn valid_pack2_name(name: &OsStr) -> bool {
    let Some(hex) = name
        .to_str()
        .and_then(|text| text.strip_prefix("pack-"))
        .and_then(|text| text.strip_suffix(PACK2_SUFFIX))
    else {
        return false;
    };
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn new_pack2_name() -> std::ffi::OsString {
    let hash = blake3::hash(staging_name().as_bytes());
    std::ffi::OsString::from(format!("pack-{}{PACK2_SUFFIX}", &hash.to_hex()[..32]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "layrs-pack2-test-{}-{name}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    struct Written {
        path: PathBuf,
        file: fs::File,
        sealed: SealedPack2,
    }

    impl Drop for Written {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    fn write(name: &str, build: impl FnOnce(&mut Pack2Builder, &mut fs::File)) -> Written {
        write_version(name, false, build)
    }

    /// `write` of a pack version 2 (Store format 7) when `whole_files`.
    fn write_version(
        name: &str,
        whole_files: bool,
        build: impl FnOnce(&mut Pack2Builder, &mut fs::File),
    ) -> Written {
        let path = temp_path(name);
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut builder = Pack2Builder::start(&mut file, whole_files).unwrap();
        build(&mut builder, &mut file);
        let sealed = builder.finish(&mut file).unwrap();
        file.sync_all().unwrap();
        Written { path, file, sealed }
    }

    fn chunk(text: &str) -> Chunk {
        Chunk::new(text.as_bytes().to_vec()).unwrap()
    }

    fn id_of(object: &StoreObjectId) -> [u8; 32] {
        raw_digest(object).unwrap()
    }

    fn read(written: &Written, index: &Pack2Index, kind: u8, location: Location) -> Vec<u8> {
        let block = read_block(&written.file, &written.path, index, location.block).unwrap();
        record_body(&block, location, kind, &written.path)
            .unwrap()
            .to_vec()
    }

    #[test]
    fn mixed_kinds_round_trip_across_block_boundaries() {
        // ~300 records of ~100 bytes overflow two 16 KiB blocks; a 20 KiB
        // record gets a block of its own.
        let chunks: Vec<Chunk> = (0..300)
            .map(|index| chunk(&format!("{index:04} {}", "x".repeat(90 + index % 7))))
            .collect();
        let large = Chunk::new(vec![7_u8; 20 * 1024]).unwrap();
        let written = write("mixed", |builder, file| {
            for value in &chunks {
                let id = id_of(&StoreObjectId::ChunkRaw(value.id().clone()));
                builder
                    .add_record(file, StoreObjectKind::ChunkRaw, id, value.bytes(), None)
                    .unwrap();
            }
            let id = id_of(&StoreObjectId::ChunkRaw(large.id().clone()));
            let encoded = EncodedBlock::single_record(1, large.bytes()).unwrap();
            builder
                .add_record(
                    file,
                    StoreObjectKind::ChunkRaw,
                    id,
                    large.bytes(),
                    Some(encoded),
                )
                .unwrap();
            builder
                .add_record(file, StoreObjectKind::Tree, [3; 32], b"{\"tree\":1}", None)
                .unwrap();
        });
        let index = verify_written(&written.file, &written.path, &written.sealed).unwrap();
        assert!(index.blocks.len() >= 3, "{} blocks", index.blocks.len());
        assert!(
            index
                .blocks
                .iter()
                .all(|block| block.raw_len as usize <= BLOCK_TARGET
                    || block.raw_len as usize > 20 * 1024)
        );
        assert_eq!(index.logical_count(), 302);
        for value in chunks.iter().chain([&large]) {
            let id = id_of(&StoreObjectId::ChunkRaw(value.id().clone()));
            let ordinal = index.find(1, &id).unwrap();
            let (kind, found, location) = index.explicit(ordinal).unwrap();
            assert_eq!((kind, found), (1, &id));
            assert_eq!(read(&written, &index, 1, location), value.bytes());
        }
        let tree = index.find(3, &[3; 32]).unwrap();
        assert_eq!(
            tree, 301,
            "kind-major ordinals: the tree follows every chunk"
        );
        assert!(index.find(3, &[4; 32]).is_none());
        assert!(index.find(2, &[3; 32]).is_none());
    }

    #[test]
    fn incompressible_blocks_are_stored_raw() {
        let mut noise = Vec::with_capacity(30 * 1024);
        let mut state = 0x1234_5678_u32;
        for _ in 0..30 * 1024 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            noise.push((state >> 24) as u8);
        }
        let written = write("raw", |builder, file| {
            builder
                .add_record(file, StoreObjectKind::ChunkRaw, [1; 32], &noise, None)
                .unwrap();
        });
        let index = verify_written(&written.file, &written.path, &written.sealed).unwrap();
        assert_eq!(index.blocks[0].flags, FLAG_RAW);
        let location = index.explicit(0).unwrap().2;
        assert_eq!(read(&written, &index, 1, location), noise);
    }

    #[test]
    fn file_records_answer_by_file_chunk_and_manifest_id() {
        let bytes = b"hello file record".to_vec();
        let value = chunk("hello file record");
        let file_id = layrs_core::scm::FileId::from_bytes(&bytes);
        let manifest =
            FileManifest::from_chunks(Recipe::Fixed.chunking(), &[value.clone()]).unwrap();
        assert_eq!(Recipe::of(&manifest), Some(Recipe::Fixed));
        let other = chunk("explicit manifest file");
        let other_manifest = FileManifest::from_chunks(
            layrs_core::scm::ChunkingDescriptor::deterministic_fixed(512 * 1024).unwrap(),
            &[other.clone()],
        )
        .unwrap();
        assert_eq!(Recipe::of(&other_manifest), None);
        let manifest_id = id_of(&StoreObjectId::FileManifest(manifest.id().clone()));
        let written = write("files", |builder, file| {
            builder
                .add_file(
                    file,
                    FileIdentity {
                        file_id: id_of_file(&file_id),
                        chunk_id: id_of(&StoreObjectId::ChunkRaw(value.id().clone())),
                        recipe: Recipe::Fixed,
                        manifest_id,
                    },
                    &bytes,
                    None,
                )
                .unwrap();
            let other_manifest_id =
                id_of(&StoreObjectId::FileManifest(other_manifest.id().clone()));
            let encoded =
                encode_object(&CanonicalObject::FileManifest(other_manifest.clone())).unwrap();
            builder
                .add_record(
                    file,
                    StoreObjectKind::FileManifest,
                    other_manifest_id,
                    &encoded,
                    None,
                )
                .unwrap();
            builder.add_manifest_file(
                id_of_file(other_manifest.file_id()),
                other_manifest_id,
                other_manifest.byte_length(),
            );
        });
        let index = verify_written(&written.file, &written.path, &written.sealed).unwrap();
        assert_eq!(index.logical_count(), 3);
        let file = index.find_file(&id_of_file(&file_id)).unwrap();
        assert_eq!(index.chunk_alias_ordinal(file), 1);
        assert_eq!(index.manifest_alias_ordinal(file), 2);
        let record = index.files[file];
        assert_eq!(read(&written, &index, FILE_KIND, record.location), bytes);
        assert_eq!(
            index.find_chunk_alias(&id_of(&StoreObjectId::ChunkRaw(value.id().clone()))),
            Some(file)
        );
        assert!(index.find_chunk_alias(&[9; 32]).is_none());
        let candidates: Vec<_> = index.manifest_alias_candidates(&manifest_id).collect();
        assert_eq!(candidates, vec![file]);
        let rebuilt =
            synthesize_manifest(&file_id, value.id(), bytes.len() as u64, record.recipe).unwrap();
        assert_eq!(rebuilt, manifest);
        let entries = index.fileid_manifests(&id_of_file(other_manifest.file_id()));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].byte_length, other_manifest.byte_length());
        assert_eq!(index.manifest_ordinal(entries[0].manifest), Some(0));
        assert!(index.fileid_manifests(&id_of_file(&file_id)).is_empty());
    }

    fn id_of_file(file_id: &layrs_core::scm::FileId) -> [u8; 32] {
        hex_digest(file_id.as_str().strip_prefix("blake3:").unwrap()).unwrap()
    }

    #[test]
    fn a_damaged_block_fails_alone() {
        let first: Vec<Chunk> = (0..200)
            .map(|i| chunk(&format!("first {i} {}", "a".repeat(100))))
            .collect();
        let written = write("damaged", |builder, file| {
            for value in &first {
                let id = id_of(&StoreObjectId::ChunkRaw(value.id().clone()));
                builder
                    .add_record(file, StoreObjectKind::ChunkRaw, id, value.bytes(), None)
                    .unwrap();
            }
        });
        let index = verify_written(&written.file, &written.path, &written.sealed).unwrap();
        assert!(index.blocks.len() >= 2);
        // Flip a stored byte of block 0.
        let at = index.blocks[0].offset + BLOCK_HEADER_LEN + 3;
        let mut byte = [0_u8; 1];
        read_at(&written.file, &written.path, at, &mut byte).unwrap();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&written.path)
            .unwrap();
        file.seek(SeekFrom::Start(at)).unwrap();
        file.write_all(&[byte[0] ^ 0x40]).unwrap();
        drop(file);
        let error = read_block(&written.file, &written.path, &index, 0).unwrap_err();
        assert!(matches!(error, CasError::PackCorruption { .. }), "{error}");
        assert!(read_block(&written.file, &written.path, &index, 1).is_ok());
        let by_block = index.records_by_block();
        assert!(!by_block[0].is_empty() && !by_block[1].is_empty());
        // The metadata is untouched: it still loads.
        assert!(load(&written.file, &written.path).is_ok());
    }

    #[test]
    fn damaged_or_truncated_metadata_is_corruption() {
        let written = write("index", |builder, file| {
            builder
                .add_record(file, StoreObjectKind::Tree, [5; 32], b"tree", None)
                .unwrap();
        });
        let length = written.file.metadata().unwrap().len();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&written.path)
            .unwrap();
        // One flipped metadata byte breaks the checksum.
        file.seek(SeekFrom::Start(length - FOOTER_LEN - 3)).unwrap();
        file.write_all(&[0xee]).unwrap();
        let error = load(&written.file, &written.path).unwrap_err();
        assert!(matches!(error, CasError::PackCorruption { .. }), "{error}");
        file.set_len(length - 5).unwrap();
        drop(file);
        let error = load(&written.file, &written.path).unwrap_err();
        assert!(matches!(error, CasError::PackCorruption { .. }), "{error}");
    }

    #[test]
    fn untrusted_metadata_is_bounded_and_ordered() {
        let path = Path::new("pack");
        let header = header_bytes(PACK2_VERSION);
        let footer = |metadata: &[u8], explicit: u64, files: u64| {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&header);
            hasher.update(metadata);
            let mut footer = Vec::new();
            footer.extend_from_slice(&HEADER_LEN.to_le_bytes());
            footer.extend_from_slice(&(metadata.len() as u64).to_le_bytes());
            footer.extend_from_slice(&explicit.to_le_bytes());
            footer.extend_from_slice(&files.to_le_bytes());
            footer.extend_from_slice(hasher.finalize().as_bytes());
            footer.extend_from_slice(FOOTER_MAGIC);
            footer
        };
        let decode_with = |metadata: Vec<u8>, explicit: u64| {
            decode(
                path,
                HEADER_LEN + metadata.len() as u64 + FOOTER_LEN,
                &header,
                &footer(&metadata, explicit, 0),
                &metadata,
            )
        };
        // No blocks, one tree kind with two unsorted ids.
        let mut unsorted = vec![0, 1, 3, 2];
        unsorted.extend_from_slice(&[2; 32]);
        unsorted.extend_from_slice(&[1; 32]);
        assert!(decode_with(unsorted, 2).is_err());
        // An entry naming a missing block.
        let mut missing = vec![0, 1, 3, 1];
        missing.extend_from_slice(&[1; 32]);
        missing.extend_from_slice(&[0, 0, 1, 0, 0, 0, 0]);
        assert!(decode_with(missing, 1).is_err());
        // A non-minimal varint.
        assert!(decode_with(vec![0x80, 0x00, 0, 0, 0, 0], 0).is_err());
        // Empty but well formed: no block, no kind, no file, no FileId.
        assert!(decode_with(vec![0, 0, 0, 0], 0).is_ok());
        // Trailing bytes.
        assert!(decode_with(vec![0, 0, 0, 0, 0], 0).is_err());
    }

    /// `written`'s metadata decoded under `version`, after `patch`, with a
    /// checksum that matches: only the structural refusals can catch it.
    fn resealed(
        written: &Written,
        version: u32,
        patch: impl FnOnce(&mut Vec<u8>),
    ) -> CasResult<Pack2Index> {
        let bytes = fs::read(&written.path).unwrap();
        let end = bytes.len() - FOOTER_LEN as usize;
        let mut footer = bytes[end..].to_vec();
        let mut metadata = bytes[le_u64(&footer[0..8]) as usize..end].to_vec();
        patch(&mut metadata);
        let header = header_bytes(version);
        let mut hasher = blake3::Hasher::new();
        hasher.update(&header);
        hasher.update(&metadata);
        footer[32..64].copy_from_slice(hasher.finalize().as_bytes());
        decode(
            Path::new("resealed"),
            bytes.len() as u64,
            &header,
            &footer,
            &metadata,
        )
    }

    fn replace_id(metadata: &mut [u8], from: [u8; 32], to: [u8; 32]) -> usize {
        let at = metadata
            .windows(32)
            .position(|window| window == from)
            .expect("the id is in the metadata");
        metadata[at..at + 32].copy_from_slice(&to);
        at
    }

    fn refused(decoded: CasResult<Pack2Index>, reason: &str) {
        match decoded {
            Err(CasError::PackCorruption { reason: found, .. }) => {
                assert!(found.contains(reason), "{found}")
            }
            other => panic!("expected {reason:?}, got {other:?}"),
        }
    }

    /// Packs version 2 (council s13 Q5): a whole record, a single-chunk
    /// record and an explicit chunk round-trip; a ChunkId equal to a whole
    /// record's FileId, as a record's chunk or an explicit chunk, and a whole
    /// recipe in a version 1 pack are refused (council s16 r3).
    #[test]
    fn version_2_records_round_trip_and_ambiguous_ids_are_refused() {
        let id = |label: &str| *blake3::hash(label.as_bytes()).as_bytes();
        let (whole, single, single_chunk, explicit) = (
            id("whole"),
            id("single"),
            id("single chunk"),
            id("explicit"),
        );
        let written = write_version("whole", true, |builder, file| {
            builder
                .add_file(
                    file,
                    FileIdentity {
                        file_id: whole,
                        chunk_id: whole,
                        recipe: Recipe::Whole,
                        manifest_id: id("whole manifest"),
                    },
                    b"whole bytes",
                    None,
                )
                .unwrap();
            builder
                .add_file(
                    file,
                    FileIdentity {
                        file_id: single,
                        chunk_id: single_chunk,
                        recipe: Recipe::Single,
                        manifest_id: id("single manifest"),
                    },
                    b"single bytes",
                    None,
                )
                .unwrap();
            builder
                .add_record(file, StoreObjectKind::ChunkRaw, explicit, b"explicit", None)
                .unwrap();
        });
        let index = verify_written(&written.file, &written.path, &written.sealed).unwrap();
        assert_eq!(index.version, PACK2_VERSION_WHOLE);
        let whole_file = index.find_file(&whole).unwrap();
        assert_eq!(index.files[whole_file].recipe, Recipe::Whole);
        assert_eq!(index.find_chunk_alias(&whole), Some(whole_file));
        assert_eq!(
            index.find_chunk_alias(&single_chunk),
            index.find_file(&single)
        );
        assert!(index.find_chunk_alias(&explicit).is_none());
        assert!(resealed(&written, PACK2_VERSION_WHOLE, |_| ()).is_ok());
        refused(
            resealed(&written, PACK2_VERSION_WHOLE, |metadata| {
                replace_id(metadata, explicit, whole);
            }),
            "also a whole file record",
        );
        refused(
            resealed(&written, PACK2_VERSION_WHOLE, |metadata| {
                replace_id(metadata, single_chunk, whole);
            }),
            "also a whole file record",
        );
        // Version 1: the record's chunk id, then its recipe code.
        let written = write("whole-in-v1", |builder, file| {
            builder
                .add_file(
                    file,
                    FileIdentity {
                        file_id: single,
                        chunk_id: single_chunk,
                        recipe: Recipe::Single,
                        manifest_id: id("single manifest"),
                    },
                    b"single bytes",
                    None,
                )
                .unwrap();
        });
        assert!(resealed(&written, PACK2_VERSION, |_| ()).is_ok());
        refused(
            resealed(&written, PACK2_VERSION, |metadata| {
                let at = replace_id(metadata, single_chunk, single_chunk);
                metadata[at + 32] = Recipe::Whole.code();
            }),
            "unknown recipe",
        );
    }

    #[test]
    fn pack2_names_are_strict() {
        assert!(valid_pack2_name(OsStr::new(
            "pack-0123456789abcdef0123456789abcdef.lp2"
        )));
        assert!(!valid_pack2_name(OsStr::new(
            "pack-0123456789abcdef0123456789abcdef.lpk"
        )));
        assert!(!valid_pack2_name(OsStr::new("pack-0123.lp2")));
        assert!(valid_pack2_name(&new_pack2_name()));
    }
}
