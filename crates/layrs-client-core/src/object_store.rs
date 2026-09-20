use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const FILE_OBJECT_SCHEMA: &str = "layrs.file_object.v1";
const CHUNK_METADATA_SCHEMA: &str = "layrs.chunk_metadata.v1";
const PACK_SCHEMA: &str = "layrs.chunk_pack.v1";
const COMPRESSION_IDENTITY: &str = "identity";
const COMPRESSION_ZSTD: &str = "zstd";
const ZSTD_LEVEL: i32 = 3;
const MIN_COMPRESSION_SAVINGS_BYTES: usize = 64;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileObjectFile {
    pub schema: String,
    pub hash: String,
    pub size: u64,
    pub chunks: Vec<FileChunkRef>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChunkRef {
    pub chunk_id: String,
    pub size: u64,
    #[serde(default = "identity")]
    pub compression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored_size: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct EncodedChunk {
    pub chunk_id: String,
    pub digest: String,
    pub raw_size: u64,
    pub stored_size: u64,
    pub compression: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactStoreResult {
    pub packed_chunks: usize,
    pub loose_chunks_removed: usize,
    pub raw_bytes: u64,
    pub stored_bytes: u64,
    pub pack_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkMetadataFile {
    schema: String,
    chunk_id: String,
    raw_size: u64,
    stored_size: u64,
    compression: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkPackIndex {
    schema: String,
    pack_id: String,
    entries: Vec<ChunkPackEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkPackEntry {
    chunk_id: String,
    offset: u64,
    raw_size: u64,
    stored_size: u64,
    compression: String,
}

#[derive(Clone, Copy, Debug)]
enum ChunkProfile {
    Text,
    Binary,
}

impl ChunkProfile {
    fn for_media_type(media_type: &str) -> Self {
        let lower = media_type.to_ascii_lowercase();
        if lower.starts_with("text/")
            || lower.contains("json")
            || lower.contains("xml")
            || lower.contains("yaml")
            || lower.contains("toml")
            || lower.contains("javascript")
            || lower.contains("typescript")
        {
            Self::Text
        } else {
            Self::Binary
        }
    }

    fn min(self) -> usize {
        match self {
            Self::Text => 8 * 1024,
            Self::Binary => 64 * 1024,
        }
    }

    fn target(self) -> usize {
        match self {
            Self::Text => 32 * 1024,
            Self::Binary => 1024 * 1024,
        }
    }

    fn max(self) -> usize {
        match self {
            Self::Text => 128 * 1024,
            Self::Binary => 4 * 1024 * 1024,
        }
    }
}

pub fn write_file_object_manifest(
    layrs_dir: &Path,
    hash: &str,
    bytes: &[u8],
    media_type: &str,
) -> Result<(), String> {
    let object_path = layrs_dir
        .join("objects")
        .join("files")
        .join(format!("{}.json", object_file_stem(hash)));
    if object_path.exists() {
        return Ok(());
    }

    let profile = ChunkProfile::for_media_type(media_type);
    let mut chunks = Vec::new();
    for chunk in chunk_file(bytes, profile) {
        chunks.push(write_chunk(layrs_dir, chunk)?);
    }

    let manifest = FileObjectFile {
        schema: FILE_OBJECT_SCHEMA.to_string(),
        hash: hash.to_string(),
        size: bytes.len() as u64,
        chunks,
    };
    write_json(&object_path, &manifest)
}

pub fn read_file_object_manifest(
    layrs_dir: &Path,
    object_path: &Path,
) -> Result<FileObjectFile, String> {
    read_json(&layrs_dir.join(object_path))
}

pub fn read_file_object_bytes(
    layrs_dir: &Path,
    manifest: &FileObjectFile,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(manifest.size as usize);
    for chunk in &manifest.chunks {
        bytes.extend_from_slice(&read_chunk_raw(layrs_dir, &chunk.chunk_id, Some(chunk))?);
    }
    if blake3_id(&bytes) != manifest.hash {
        return Err("Layrs object hash mismatch while reading file object.".to_string());
    }
    Ok(bytes)
}

pub fn read_chunk_encoded(layrs_dir: &Path, chunk: &FileChunkRef) -> Result<EncodedChunk, String> {
    read_encoded_chunk_by_id(layrs_dir, &chunk.chunk_id, Some(chunk))
}

pub fn write_received_encoded_chunk(
    layrs_dir: &Path,
    chunk_id: &str,
    bytes: Vec<u8>,
    compression: &str,
    raw_size: u64,
) -> Result<FileChunkRef, String> {
    validate_blake3_id(chunk_id)?;
    let compression = normalized_compression(compression);
    let raw = decode_chunk(&bytes, &compression, raw_size, chunk_id)?;
    if blake3_id(&raw) != chunk_id {
        return Err(format!(
            "Layrs rejected received chunk {chunk_id} because its raw digest does not match."
        ));
    }

    let path = loose_chunk_path(layrs_dir, chunk_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Layrs could not create received chunk directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(&path, &bytes).map_err(|error| {
        format!(
            "Layrs could not write received chunk object {}: {error}",
            path.display()
        )
    })?;
    write_json(
        &chunk_metadata_path(layrs_dir, chunk_id),
        &ChunkMetadataFile {
            schema: CHUNK_METADATA_SCHEMA.to_string(),
            chunk_id: chunk_id.to_string(),
            raw_size,
            stored_size: bytes.len() as u64,
            compression: compression.clone(),
        },
    )?;

    Ok(FileChunkRef {
        chunk_id: chunk_id.to_string(),
        size: raw_size,
        compression,
        stored_size: Some(bytes.len() as u64),
    })
}

pub fn read_encoded_chunk_by_id(
    layrs_dir: &Path,
    chunk_id: &str,
    hint: Option<&FileChunkRef>,
) -> Result<EncodedChunk, String> {
    validate_blake3_id(chunk_id)?;
    let loose_path = loose_chunk_path(layrs_dir, chunk_id);
    if loose_path.exists() {
        let bytes = fs::read(&loose_path).map_err(|error| {
            format!(
                "Layrs could not read chunk object {}: {error}",
                loose_path.display()
            )
        })?;
        let metadata = chunk_metadata(layrs_dir, chunk_id, hint, bytes.len() as u64)?;
        return encoded_chunk_from_bytes(chunk_id, bytes, metadata);
    }

    let packed = find_packed_chunk(layrs_dir, chunk_id)?
        .ok_or_else(|| format!("Layrs could not find chunk object {chunk_id}."))?;
    encoded_chunk_from_bytes(chunk_id, packed.bytes, packed.metadata)
}

pub fn read_chunk_raw(
    layrs_dir: &Path,
    chunk_id: &str,
    hint: Option<&FileChunkRef>,
) -> Result<Vec<u8>, String> {
    let encoded = read_encoded_chunk_by_id(layrs_dir, chunk_id, hint)?;
    decode_chunk(
        &encoded.bytes,
        &encoded.compression,
        encoded.raw_size,
        &encoded.chunk_id,
    )
}

/// Physical container for the legacy SQL projection. A packed chunk is read
/// through its verified index entry; it no longer has a loose `.chunk` file.
pub(crate) fn chunk_storage_paths(
    layrs_dir: &Path,
    chunk_id: &str,
) -> Result<Vec<PathBuf>, String> {
    validate_blake3_id(chunk_id)?;
    let loose = loose_chunk_path(layrs_dir, chunk_id);
    if loose.is_file() {
        return Ok(vec![loose]);
    }
    let packed = find_packed_chunk(layrs_dir, chunk_id)?
        .ok_or_else(|| format!("Layrs could not find chunk object {chunk_id}."))?;
    decode_chunk(
        &packed.bytes,
        &packed.metadata.compression,
        packed.metadata.raw_size,
        chunk_id,
    )?;
    Ok(vec![packed.path, packed.index_path])
}

pub fn compact_loose_chunks(layrs_dir: &Path) -> Result<CompactStoreResult, String> {
    compact_loose_chunks_with_hook(layrs_dir, |_, _| Ok(()))
}

fn compact_loose_chunks_with_hook<F>(
    layrs_dir: &Path,
    before_verify: F,
) -> Result<CompactStoreResult, String>
where
    F: FnOnce(&Path, &Path) -> Result<(), String>,
{
    let chunks_dir = layrs_dir.join("objects").join("chunks");
    if !chunks_dir.exists() {
        return Ok(CompactStoreResult::default());
    }

    let packed_ids = packed_chunk_ids(layrs_dir)?;
    let mut candidates = Vec::<EncodedChunk>::new();
    for entry in fs::read_dir(&chunks_dir).map_err(|error| {
        format!(
            "Layrs could not read chunk directory {}: {error}",
            chunks_dir.display()
        )
    })? {
        let path = entry
            .map_err(|error| {
                format!(
                    "Layrs could not read chunk directory entry {}: {error}",
                    chunks_dir.display()
                )
            })?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("chunk") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let chunk_id = format!("blake3:{stem}");
        if packed_ids.contains(&chunk_id) {
            continue;
        }
        candidates.push(read_encoded_chunk_by_id(layrs_dir, &chunk_id, None)?);
    }

    if candidates.is_empty() {
        return Ok(CompactStoreResult::default());
    }

    let pack_id = pack_id_for_chunks(&candidates);
    let packs_dir = layrs_dir.join("objects").join("packs");
    fs::create_dir_all(&packs_dir).map_err(|error| {
        format!(
            "Layrs could not create pack directory {}: {error}",
            packs_dir.display()
        )
    })?;
    let pack_path = packs_dir.join(format!("{}.pack", object_file_stem(&pack_id)));
    let index_path = packs_dir.join(format!("{}.json", object_file_stem(&pack_id)));
    if pack_path.exists() && index_path.exists() {
        return Ok(CompactStoreResult::default());
    }
    let temp_suffix = format!(
        "tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temp_pack_path = pack_path.with_extension(format!("pack.{temp_suffix}"));
    let temp_index_path = index_path.with_extension(format!("json.{temp_suffix}"));

    let mut offset = 0u64;
    let mut pack_bytes = Vec::new();
    let mut entries = Vec::new();
    let mut raw_bytes = 0u64;
    let mut stored_bytes = 0u64;
    for chunk in &candidates {
        pack_bytes.extend_from_slice(&chunk.bytes);
        entries.push(ChunkPackEntry {
            chunk_id: chunk.chunk_id.clone(),
            offset,
            raw_size: chunk.raw_size,
            stored_size: chunk.stored_size,
            compression: chunk.compression.clone(),
        });
        offset += chunk.stored_size;
        raw_bytes += chunk.raw_size;
        stored_bytes += chunk.stored_size;
    }

    let index = ChunkPackIndex {
        schema: PACK_SCHEMA.to_string(),
        pack_id: pack_id.clone(),
        entries,
    };
    let expected_ids = candidates
        .iter()
        .map(|chunk| chunk.chunk_id.clone())
        .collect::<BTreeSet<_>>();

    let publish_result = (|| {
        write_synced_file(&temp_pack_path, &pack_bytes)?;
        write_synced_json(&temp_index_path, &index)?;
        before_verify(&temp_pack_path, &temp_index_path)?;

        verify_pack_files(&temp_pack_path, &temp_index_path, &pack_id, &expected_ids)?;

        rename_new_file(&temp_pack_path, &pack_path)?;
        sync_directory_best_effort(&packs_dir);
        if let Err(error) = rename_new_file(&temp_index_path, &index_path) {
            let _ = fs::remove_file(&pack_path);
            sync_directory_best_effort(&packs_dir);
            return Err(error);
        }
        sync_directory_best_effort(&packs_dir);

        if let Err(error) = verify_pack_files(&pack_path, &index_path, &pack_id, &expected_ids) {
            let _ = fs::remove_file(&index_path);
            let _ = fs::remove_file(&pack_path);
            sync_directory_best_effort(&packs_dir);
            return Err(error);
        }
        Ok(())
    })();

    if let Err(error) = publish_result {
        let _ = fs::remove_file(&temp_index_path);
        let _ = fs::remove_file(&temp_pack_path);
        return Err(error);
    }

    let mut removed = 0usize;
    for chunk in &candidates {
        let chunk_path = loose_chunk_path(layrs_dir, &chunk.chunk_id);
        if chunk_path.exists() {
            fs::remove_file(&chunk_path).map_err(|error| {
                format!(
                    "Layrs could not remove loose chunk {}: {error}",
                    chunk_path.display()
                )
            })?;
            removed += 1;
        }
        let metadata_path = chunk_metadata_path(layrs_dir, &chunk.chunk_id);
        if metadata_path.exists() {
            fs::remove_file(&metadata_path).map_err(|error| {
                format!(
                    "Layrs could not remove loose chunk metadata {}: {error}",
                    metadata_path.display()
                )
            })?;
        }
    }
    sync_directory_best_effort(&chunks_dir);

    Ok(CompactStoreResult {
        packed_chunks: candidates.len(),
        loose_chunks_removed: removed,
        raw_bytes,
        stored_bytes,
        pack_path: Some(pack_path.display().to_string()),
    })
}

fn write_synced_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            format!(
                "Layrs could not create temporary pack file {}: {error}",
                path.display()
            )
        })?;
    file.write_all(bytes).map_err(|error| {
        format!(
            "Layrs could not write temporary pack file {}: {error}",
            path.display()
        )
    })?;
    file.sync_all().map_err(|error| {
        format!(
            "Layrs could not flush temporary pack file {}: {error}",
            path.display()
        )
    })
}

fn write_synced_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Layrs could not encode JSON {}: {error}", path.display()))?;
    write_synced_file(path, &body)
}

fn rename_new_file(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "Layrs refused to replace existing pack file {}.",
            destination.display()
        ));
    }
    fs::rename(source, destination).map_err(|error| {
        format!(
            "Layrs could not publish pack file {} as {}: {error}",
            source.display(),
            destination.display()
        )
    })
}

fn verify_pack_files(
    pack_path: &Path,
    index_path: &Path,
    expected_pack_id: &str,
    expected_ids: &BTreeSet<String>,
) -> Result<(), String> {
    let index = read_json::<ChunkPackIndex>(index_path)?;
    if index.schema != PACK_SCHEMA {
        return Err(format!(
            "Layrs compact verification rejected pack index {} with schema {}.",
            index_path.display(),
            index.schema
        ));
    }
    if index.pack_id != expected_pack_id {
        return Err(format!(
            "Layrs compact verification rejected pack index {} for unexpected pack {}.",
            index_path.display(),
            index.pack_id
        ));
    }

    let pack_size = fs::metadata(pack_path)
        .map_err(|error| {
            format!(
                "Layrs could not inspect chunk pack {}: {error}",
                pack_path.display()
            )
        })?
        .len();
    let mut pack = File::open(pack_path).map_err(|error| {
        format!(
            "Layrs could not open chunk pack {} for verification: {error}",
            pack_path.display()
        )
    })?;
    let mut verified_ids = BTreeSet::new();
    let mut expected_offset = 0u64;

    for entry in &index.entries {
        validate_blake3_id(&entry.chunk_id)?;
        if !verified_ids.insert(entry.chunk_id.clone()) {
            return Err(format!(
                "Layrs compact verification found duplicate chunk {} in {}.",
                entry.chunk_id,
                index_path.display()
            ));
        }
        if entry.offset != expected_offset {
            return Err(format!(
                "Layrs compact verification found a non-contiguous offset for chunk {}.",
                entry.chunk_id
            ));
        }
        let end = entry.offset.checked_add(entry.stored_size).ok_or_else(|| {
            format!(
                "Layrs compact verification found an overflowing range for chunk {}.",
                entry.chunk_id
            )
        })?;
        if end > pack_size {
            return Err(format!(
                "Layrs compact verification found chunk {} outside pack bounds.",
                entry.chunk_id
            ));
        }

        pack.seek(SeekFrom::Start(entry.offset)).map_err(|error| {
            format!(
                "Layrs could not seek chunk pack {} during verification: {error}",
                pack_path.display()
            )
        })?;
        let mut encoded = vec![0u8; entry.stored_size as usize];
        pack.read_exact(&mut encoded).map_err(|error| {
            format!(
                "Layrs could not read chunk {} from pack {} during verification: {error}",
                entry.chunk_id,
                pack_path.display()
            )
        })?;
        let compression = normalized_compression(&entry.compression);
        let raw = decode_chunk(&encoded, &compression, entry.raw_size, &entry.chunk_id)?;
        if blake3_id(&raw) != entry.chunk_id {
            return Err(format!(
                "Layrs compact verification failed for chunk {}.",
                entry.chunk_id
            ));
        }
        expected_offset = end;
    }

    if verified_ids != *expected_ids {
        return Err(format!(
            "Layrs compact verification found an incomplete chunk inventory in {}.",
            index_path.display()
        ));
    }
    if expected_offset != pack_size {
        return Err(format!(
            "Layrs compact verification found trailing bytes in pack {}.",
            pack_path.display()
        ));
    }
    Ok(())
}

fn sync_directory_best_effort(path: &Path) {
    #[cfg(unix)]
    {
        let _ = File::open(path).and_then(|directory| directory.sync_all());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        let _ = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
            .and_then(|directory| directory.sync_all());
    }
}

fn write_chunk(layrs_dir: &Path, raw: &[u8]) -> Result<FileChunkRef, String> {
    let chunk_id = blake3_id(raw);
    if let Some(packed) = find_packed_chunk(layrs_dir, &chunk_id)? {
        return Ok(FileChunkRef {
            chunk_id,
            size: packed.metadata.raw_size,
            compression: packed.metadata.compression,
            stored_size: Some(packed.metadata.stored_size),
        });
    }

    let path = loose_chunk_path(layrs_dir, &chunk_id);
    if path.exists() {
        let encoded = read_encoded_chunk_by_id(layrs_dir, &chunk_id, None)?;
        return Ok(FileChunkRef {
            chunk_id,
            size: encoded.raw_size,
            compression: encoded.compression,
            stored_size: Some(encoded.stored_size),
        });
    }

    let encoded = encode_chunk(raw)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Layrs could not create chunk directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(&path, &encoded.bytes).map_err(|error| {
        format!(
            "Layrs could not write chunk object {}: {error}",
            path.display()
        )
    })?;
    write_json(
        &chunk_metadata_path(layrs_dir, &chunk_id),
        &ChunkMetadataFile {
            schema: CHUNK_METADATA_SCHEMA.to_string(),
            chunk_id: chunk_id.clone(),
            raw_size: raw.len() as u64,
            stored_size: encoded.bytes.len() as u64,
            compression: encoded.compression.clone(),
        },
    )?;

    Ok(FileChunkRef {
        chunk_id,
        size: raw.len() as u64,
        compression: encoded.compression,
        stored_size: Some(encoded.bytes.len() as u64),
    })
}

fn encode_chunk(raw: &[u8]) -> Result<EncodedChunk, String> {
    let chunk_id = blake3_id(raw);
    let compressed = zstd::stream::encode_all(Cursor::new(raw), ZSTD_LEVEL)
        .map_err(|error| format!("Layrs could not zstd-compress a chunk: {error}"))?;
    let (bytes, compression) = if compressed.len() + MIN_COMPRESSION_SAVINGS_BYTES < raw.len() {
        (compressed, COMPRESSION_ZSTD.to_string())
    } else {
        (raw.to_vec(), COMPRESSION_IDENTITY.to_string())
    };
    Ok(EncodedChunk {
        chunk_id: chunk_id.clone(),
        digest: chunk_id,
        raw_size: raw.len() as u64,
        stored_size: bytes.len() as u64,
        compression,
        bytes,
    })
}

fn encoded_chunk_from_bytes(
    chunk_id: &str,
    bytes: Vec<u8>,
    metadata: ChunkMetadataFile,
) -> Result<EncodedChunk, String> {
    let raw = decode_chunk(&bytes, &metadata.compression, metadata.raw_size, chunk_id)?;
    if blake3_id(&raw) != chunk_id {
        return Err(format!("Layrs chunk {chunk_id} failed hash verification."));
    }
    Ok(EncodedChunk {
        chunk_id: chunk_id.to_string(),
        digest: chunk_id.to_string(),
        raw_size: metadata.raw_size,
        stored_size: metadata.stored_size,
        compression: metadata.compression,
        bytes,
    })
}

fn decode_chunk(
    bytes: &[u8],
    compression: &str,
    raw_size: u64,
    chunk_id: &str,
) -> Result<Vec<u8>, String> {
    let raw = match compression {
        COMPRESSION_IDENTITY | "" => bytes.to_vec(),
        COMPRESSION_ZSTD => zstd::stream::decode_all(Cursor::new(bytes)).map_err(|error| {
            format!("Layrs could not zstd-decompress chunk {chunk_id}: {error}")
        })?,
        other => {
            return Err(format!(
                "Layrs cannot read chunk {chunk_id} with unsupported compression {other}."
            ));
        }
    };
    if raw.len() as u64 != raw_size {
        return Err(format!(
            "Layrs chunk {chunk_id} size mismatch after decode: got {}, expected {raw_size}.",
            raw.len()
        ));
    }
    Ok(raw)
}

fn chunk_metadata(
    layrs_dir: &Path,
    chunk_id: &str,
    hint: Option<&FileChunkRef>,
    stored_size: u64,
) -> Result<ChunkMetadataFile, String> {
    let path = chunk_metadata_path(layrs_dir, chunk_id);
    if path.exists() {
        return read_json(&path);
    }
    if let Some(hint) = hint {
        return Ok(ChunkMetadataFile {
            schema: CHUNK_METADATA_SCHEMA.to_string(),
            chunk_id: chunk_id.to_string(),
            raw_size: hint.size,
            stored_size: hint.stored_size.unwrap_or(stored_size),
            compression: normalized_compression(&hint.compression),
        });
    }
    Ok(ChunkMetadataFile {
        schema: CHUNK_METADATA_SCHEMA.to_string(),
        chunk_id: chunk_id.to_string(),
        raw_size: stored_size,
        stored_size,
        compression: COMPRESSION_IDENTITY.to_string(),
    })
}

#[derive(Clone, Debug)]
struct PackedChunk {
    path: PathBuf,
    index_path: PathBuf,
    metadata: ChunkMetadataFile,
    bytes: Vec<u8>,
}

fn find_packed_chunk(layrs_dir: &Path, chunk_id: &str) -> Result<Option<PackedChunk>, String> {
    let packs_dir = layrs_dir.join("objects").join("packs");
    if !packs_dir.exists() {
        return Ok(None);
    }
    for entry in fs::read_dir(&packs_dir).map_err(|error| {
        format!(
            "Layrs could not read pack directory {}: {error}",
            packs_dir.display()
        )
    })? {
        let index_path = entry
            .map_err(|error| {
                format!(
                    "Layrs could not read pack directory entry {}: {error}",
                    packs_dir.display()
                )
            })?
            .path();
        if index_path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let index = read_json::<ChunkPackIndex>(&index_path)?;
        let Some(pack_entry) = index
            .entries
            .iter()
            .find(|entry| entry.chunk_id == chunk_id)
        else {
            continue;
        };
        let pack_path = packs_dir.join(format!("{}.pack", object_file_stem(&index.pack_id)));
        let mut file = fs::File::open(&pack_path).map_err(|error| {
            format!(
                "Layrs could not open chunk pack {}: {error}",
                pack_path.display()
            )
        })?;
        file.seek(SeekFrom::Start(pack_entry.offset))
            .map_err(|error| {
                format!(
                    "Layrs could not seek chunk pack {}: {error}",
                    pack_path.display()
                )
            })?;
        let mut bytes = vec![0u8; pack_entry.stored_size as usize];
        file.read_exact(&mut bytes).map_err(|error| {
            format!(
                "Layrs could not read chunk pack {}: {error}",
                pack_path.display()
            )
        })?;
        return Ok(Some(PackedChunk {
            path: pack_path,
            index_path,
            metadata: ChunkMetadataFile {
                schema: CHUNK_METADATA_SCHEMA.to_string(),
                chunk_id: chunk_id.to_string(),
                raw_size: pack_entry.raw_size,
                stored_size: pack_entry.stored_size,
                compression: normalized_compression(&pack_entry.compression),
            },
            bytes,
        }));
    }
    Ok(None)
}

fn packed_chunk_ids(layrs_dir: &Path) -> Result<BTreeSet<String>, String> {
    let packs_dir = layrs_dir.join("objects").join("packs");
    if !packs_dir.exists() {
        return Ok(BTreeSet::new());
    }
    let mut ids = BTreeSet::new();
    for entry in fs::read_dir(&packs_dir).map_err(|error| {
        format!(
            "Layrs could not read pack directory {}: {error}",
            packs_dir.display()
        )
    })? {
        let path = entry
            .map_err(|error| {
                format!(
                    "Layrs could not read pack directory entry {}: {error}",
                    packs_dir.display()
                )
            })?
            .path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            let index = read_json::<ChunkPackIndex>(&path)?;
            ids.extend(index.entries.into_iter().map(|entry| entry.chunk_id));
        }
    }
    Ok(ids)
}

fn pack_id_for_chunks(chunks: &[EncodedChunk]) -> String {
    let mut material = Vec::new();
    for chunk in chunks {
        material.extend_from_slice(chunk.chunk_id.as_bytes());
        material.push(0);
        material.extend_from_slice(chunk.compression.as_bytes());
        material.push(0);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    material.extend_from_slice(now.to_string().as_bytes());
    blake3_id(&material)
}

fn loose_chunk_path(layrs_dir: &Path, chunk_id: &str) -> PathBuf {
    layrs_dir
        .join("objects")
        .join("chunks")
        .join(format!("{}.chunk", object_file_stem(chunk_id)))
}

fn chunk_metadata_path(layrs_dir: &Path, chunk_id: &str) -> PathBuf {
    layrs_dir
        .join("objects")
        .join("chunks")
        .join(format!("{}.json", object_file_stem(chunk_id)))
}

fn normalized_compression(value: &str) -> String {
    match value {
        COMPRESSION_ZSTD => COMPRESSION_ZSTD.to_string(),
        _ => COMPRESSION_IDENTITY.to_string(),
    }
}

fn identity() -> String {
    COMPRESSION_IDENTITY.to_string()
}

fn blake3_id(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn validate_blake3_id(value: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("blake3:") else {
        return Err(format!("Layrs expected a blake3 object id, got {value}."));
    };
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "Layrs expected a valid blake3 object id, got {value}."
        ));
    }
    Ok(())
}

fn object_file_stem(object_id: &str) -> &str {
    object_id.strip_prefix("blake3:").unwrap_or(object_id)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Layrs could not create directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let body = serde_json::to_string_pretty(value)
        .map_err(|error| format!("Layrs could not encode JSON {}: {error}", path.display()))?;
    fs::write(path, body).map_err(|error| {
        format!(
            "Layrs could not write JSON file {}: {error}",
            path.display()
        )
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let body = fs::read_to_string(path)
        .map_err(|error| format!("Layrs could not read JSON file {}: {error}", path.display()))?;
    serde_json::from_str(&body)
        .map_err(|error| format!("Layrs JSON file {} is invalid: {error}", path.display()))
}

#[allow(dead_code)]
fn _debug_chunk_lengths(chunks: &[FileChunkRef]) -> BTreeMap<String, u64> {
    chunks
        .iter()
        .map(|chunk| (chunk.chunk_id.clone(), chunk.size))
        .collect()
}

#[cfg(test)]
#[path = "object_store_tests.rs"]
mod tests;

#[path = "object_store/chunking.rs"]
mod chunking;
use chunking::chunk_file;
