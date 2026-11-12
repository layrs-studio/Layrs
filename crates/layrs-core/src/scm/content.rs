use super::canonical::{canonical_json, domain_hash, domain_separated_bytes};
use super::error::{Result, ScmError};
use super::ids::{ChunkId, FileId, FileManifestId};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const CHUNK_DOMAIN: &str = "layrs-chunk-v1";
const FILE_DOMAIN: &str = "layrs-file-v1";
const FILE_MANIFEST_DOMAIN: &str = "layrs-file-manifest-v1";
mod stream;
pub use stream::{FileManifestBuilder, FileManifestVerifier};

/// Chunk size of every capture's fixed chunking (ADR 0027): 1 MiB up to
/// 1 GiB, so content captured before large files were supported keeps its
/// chunks; above, the next power of two of length/1024, capped at
/// `MAX_FIXED_CHUNK_BYTES`. A file thus has about 1,024 chunks up to 16 GiB
/// and its manifest stays small; beyond, chunk count grows linearly. The
/// FileId never depends on chunking, only storage and deduplication do.
pub const BASE_CHUNK_BYTES: u64 = 1024 * 1024;
pub const MAX_FIXED_CHUNK_BYTES: u64 = 16 * 1024 * 1024;
pub fn fixed_chunk_bytes(byte_length: u64) -> u64 {
    if byte_length <= 1 << 30 {
        return BASE_CHUNK_BYTES;
    }
    byte_length
        .div_ceil(1024)
        .checked_next_power_of_two()
        .unwrap_or(MAX_FIXED_CHUNK_BYTES)
        .clamp(BASE_CHUNK_BYTES, MAX_FIXED_CHUNK_BYTES)
}
/// The fixed chunking descriptor `fixed_chunk_bytes` selects.
pub fn fixed_chunking(byte_length: u64) -> ChunkingDescriptor {
    ChunkingDescriptor(ChunkingWire::DeterministicFixed {
        version: 1,
        chunk_bytes: fixed_chunk_bytes(byte_length),
    })
}

/// Largest file the `whole` recipe holds (council s13 Q5): one chunk of at
/// most a fixed chunk's size.
pub const WHOLE_MAX_BYTES: u64 = BASE_CHUNK_BYTES;

/// The recipe a capture uses for a file of `byte_length` bytes: `whole` for
/// a non-empty file up to `WHOLE_MAX_BYTES` when the Store holds it
/// (`whole_files`), else the fixed chunking.
pub fn capture_chunking(byte_length: u64, whole_files: bool) -> ChunkingDescriptor {
    if whole_files && (1..=WHOLE_MAX_BYTES).contains(&byte_length) {
        ChunkingDescriptor::whole()
    } else {
        fixed_chunking(byte_length)
    }
}

/// A chunk of content. Its id is a digest of its bytes in the chunk domain
/// (`layrs-chunk-v1`), or, for the single chunk of a `whole` file (council
/// s13 Q5), in the file domain: then it equals the file's FileId digest, so
/// a Store keeps one digest for both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    id: ChunkId,
    bytes: Vec<u8>,
    whole: bool,
}

impl Chunk {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return Err(ScmError::InvalidChunk(
                "chunks must contain at least one byte".into(),
            ));
        }
        let id = ChunkId::from_digest(domain_hash(CHUNK_DOMAIN, &bytes));
        Ok(Self {
            id,
            bytes,
            whole: false,
        })
    }

    /// The single chunk of a `whole` file: named in the file domain, at
    /// most `WHOLE_MAX_BYTES`.
    pub fn whole(bytes: impl Into<Vec<u8>>) -> Result<Self> {
        let bytes = bytes.into();
        if bytes.is_empty() || bytes.len() as u64 > WHOLE_MAX_BYTES {
            return Err(ScmError::InvalidChunk(
                "a whole file chunk holds 1 byte to 1 MiB".into(),
            ));
        }
        let id = ChunkId::from_digest(domain_hash(FILE_DOMAIN, &bytes));
        Ok(Self {
            id,
            bytes,
            whole: true,
        })
    }

    /// Verifies `bytes` as the chunk `id` in the domain of its recipe.
    pub fn verified(id: &ChunkId, bytes: Vec<u8>, whole: bool) -> Result<Self> {
        let chunk = if whole {
            Self::whole(bytes)?
        } else {
            Self::new(bytes)?
        };
        if &chunk.id != id {
            return Err(ScmError::IdentityMismatch {
                claimed: id.to_string(),
                computed: chunk.id.to_string(),
            });
        }
        Ok(chunk)
    }

    pub fn id(&self) -> &ChunkId {
        &self.id
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn byte_length(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// Whether this is the chunk of a `whole` file (named in the file
    /// domain).
    pub fn is_whole(&self) -> bool {
        self.whole
    }

    pub fn identity_bytes(&self) -> Vec<u8> {
        domain_separated_bytes(
            if self.whole {
                FILE_DOMAIN
            } else {
                CHUNK_DOMAIN
            },
            &self.bytes,
        )
    }
}

impl ChunkId {
    /// The chunk of the `whole` file `file_id`: the same digest.
    pub fn of_whole_file(file_id: &FileId) -> Self {
        Self::from_digest_bytes(&file_id.digest_bytes())
    }
}

impl FileId {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::from_digest(domain_hash(FILE_DOMAIN, bytes))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkRef<'a> {
    schema_version: &'static str,
    chunk_id: &'a ChunkId,
    byte_length: u64,
    content: Base64Content,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Base64Content {
    encoding: Base64Encoding,
    bytes: String,
}

#[derive(Serialize, Deserialize)]
enum Base64Encoding {
    #[serde(rename = "base64")]
    Base64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChunkWire {
    schema_version: String,
    chunk_id: ChunkId,
    byte_length: u64,
    content: Base64Content,
}

impl Serialize for Chunk {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        ChunkRef {
            schema_version: if self.whole {
                WHOLE_CHUNK_SCHEMA
            } else {
                "layrs.chunk.v1"
            },
            chunk_id: &self.id,
            byte_length: self.byte_length(),
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                bytes: STANDARD.encode(&self.bytes),
            },
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Chunk {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = ChunkWire::deserialize(deserializer)?;
        let whole = match wire.schema_version.as_str() {
            "layrs.chunk.v1" => false,
            WHOLE_CHUNK_SCHEMA => true,
            _ => return Err(serde::de::Error::custom("unsupported Chunk schemaVersion")),
        };
        let bytes = STANDARD
            .decode(wire.content.bytes.as_bytes())
            .map_err(serde::de::Error::custom)?;
        if STANDARD.encode(&bytes) != wire.content.bytes {
            return Err(serde::de::Error::custom("non-canonical base64"));
        }
        if bytes.len() as u64 != wire.byte_length {
            return Err(serde::de::Error::custom("Chunk byteLength mismatch"));
        }
        let chunk = if whole {
            Self::whole(bytes)
        } else {
            Self::new(bytes)
        }
        .map_err(serde::de::Error::custom)?;
        if chunk.id != wire.chunk_id {
            return Err(serde::de::Error::custom(ScmError::IdentityMismatch {
                claimed: wire.chunk_id.to_string(),
                computed: chunk.id.to_string(),
            }));
        }
        Ok(chunk)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkingDescriptor(ChunkingWire);

/// The schema of a serialized `whole` chunk: distinct from v1, so no reader
/// ever verifies it in the chunk domain.
const WHOLE_CHUNK_SCHEMA: &str = "layrs.chunk.whole.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case", deny_unknown_fields)]
enum ChunkingWire {
    #[serde(rename = "single")]
    Single { version: u8 },
    /// One chunk named by the file's own digest (council s13 Q5).
    #[serde(rename = "whole")]
    Whole { version: u8 },
    #[serde(rename = "deterministic_fixed", rename_all = "camelCase")]
    DeterministicFixed { version: u8, chunk_bytes: u64 },
    #[serde(rename = "fastcdc", rename_all = "camelCase")]
    FastCdc {
        version: u8,
        min_bytes: u64,
        target_bytes: u64,
        max_bytes: u64,
    },
}

impl ChunkingDescriptor {
    pub fn single() -> Self {
        Self(ChunkingWire::Single { version: 1 })
    }

    /// One chunk covering the whole file, named in the file domain: its
    /// ChunkId digest is the FileId digest (council s13 Q5).
    pub fn whole() -> Self {
        Self(ChunkingWire::Whole { version: 1 })
    }

    /// Whether this is the `whole` recipe, whose chunk is named in the file
    /// domain.
    pub fn is_whole(&self) -> bool {
        matches!(self.0, ChunkingWire::Whole { .. })
    }

    pub fn deterministic_fixed(chunk_bytes: u64) -> Result<Self> {
        let value = Self(ChunkingWire::DeterministicFixed {
            version: 1,
            chunk_bytes,
        });
        value.validate()?;
        Ok(value)
    }

    pub fn fast_cdc(min_bytes: u64, target_bytes: u64, max_bytes: u64) -> Result<Self> {
        let value = Self(ChunkingWire::FastCdc {
            version: 1,
            min_bytes,
            target_bytes,
            max_bytes,
        });
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<()> {
        match &self.0 {
            ChunkingWire::Single { version } if *version == 1 => Ok(()),
            ChunkingWire::Whole { version } if *version == 1 => Ok(()),
            ChunkingWire::DeterministicFixed {
                version: 1,
                chunk_bytes,
            } if *chunk_bytes > 0 => Ok(()),
            ChunkingWire::FastCdc {
                version: 1,
                min_bytes,
                target_bytes,
                max_bytes,
            } if *min_bytes > 0 && min_bytes <= target_bytes && target_bytes <= max_bytes => Ok(()),
            ChunkingWire::FastCdc { version: 1, .. } => Err(ScmError::FastCdcParameterOrder),
            _ => Err(ScmError::InvalidField {
                field: "chunking",
                reason: "unsupported version or zero fixed chunk size".into(),
            }),
        }
    }

    pub fn strategy(&self) -> &'static str {
        match &self.0 {
            ChunkingWire::Single { .. } => "single",
            ChunkingWire::Whole { .. } => "whole",
            ChunkingWire::DeterministicFixed { .. } => "deterministic_fixed",
            ChunkingWire::FastCdc { .. } => "fastcdc",
        }
    }
}

impl Serialize for ChunkingDescriptor {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ChunkingDescriptor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Self(ChunkingWire::deserialize(deserializer)?);
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChunk {
    chunk_id: ChunkId,
    offset: u64,
    byte_length: u64,
}

impl FileChunk {
    pub fn new(chunk_id: ChunkId, offset: u64, byte_length: u64) -> Result<Self> {
        if byte_length == 0 {
            return Err(ScmError::InvalidField {
                field: "byteLength",
                reason: "file chunks must be non-empty".into(),
            });
        }
        Ok(Self {
            chunk_id,
            offset,
            byte_length,
        })
    }

    pub fn chunk_id(&self) -> &ChunkId {
        &self.chunk_id
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn byte_length(&self) -> u64 {
        self.byte_length
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileChunkWire {
    chunk_id: ChunkId,
    offset: u64,
    byte_length: u64,
}

impl<'de> Deserialize<'de> for FileChunk {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = FileChunkWire::deserialize(deserializer)?;
        Self::new(wire.chunk_id, wire.offset, wire.byte_length).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone)]
pub struct FileManifest {
    id: FileManifestId,
    file_id: FileId,
    byte_length: u64,
    chunking: ChunkingDescriptor,
    chunks: Vec<FileChunk>,
    /// Built by `FileManifestBuilder` in this process from the very bytes it
    /// hashed into the FileId and each ChunkId: its FileId is proven, so
    /// admission need not re-read the content (as git trusts its own hashing).
    /// Never serialized: a manifest read or received is not proven.
    proven: bool,
}

impl PartialEq for FileManifest {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.file_id == other.file_id
            && self.byte_length == other.byte_length
            && self.chunking == other.chunking
            && self.chunks == other.chunks
    }
}
impl Eq for FileManifest {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileManifestPayload<'a> {
    schema_version: &'static str,
    file_id: &'a FileId,
    byte_length: u64,
    chunking: &'a ChunkingDescriptor,
    chunks: &'a [FileChunk],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileManifestRef<'a> {
    schema_version: &'static str,
    manifest_id: &'a FileManifestId,
    file_id: &'a FileId,
    byte_length: u64,
    chunking: &'a ChunkingDescriptor,
    chunks: &'a [FileChunk],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileManifestWire {
    schema_version: String,
    manifest_id: FileManifestId,
    file_id: FileId,
    byte_length: u64,
    chunking: ChunkingDescriptor,
    chunks: Vec<FileChunk>,
}

impl FileManifest {
    pub fn new(
        file_id: FileId,
        byte_length: u64,
        chunking: ChunkingDescriptor,
        chunks: Vec<FileChunk>,
    ) -> Result<Self> {
        chunking.validate()?;
        validate_coverage(byte_length, &chunks)?;
        if chunking.is_whole() {
            validate_whole(&file_id, byte_length, &chunks)?;
        }
        let payload = FileManifestPayload {
            schema_version: "layrs.file-manifest.v1",
            file_id: &file_id,
            byte_length,
            chunking: &chunking,
            chunks: &chunks,
        };
        let id = FileManifestId::from_digest(domain_hash(
            FILE_MANIFEST_DOMAIN,
            &canonical_json(&payload)?,
        ));
        Ok(Self {
            id,
            file_id,
            byte_length,
            chunking,
            chunks,
            proven: false,
        })
    }

    /// Whether this value was built in this process from the bytes hashed
    /// into its FileId and ChunkIds (`FileManifestBuilder`). Chunk bytes are
    /// still verified against their ids wherever they are installed or read.
    pub fn is_proven(&self) -> bool {
        self.proven
    }

    pub(crate) fn into_proven(mut self) -> Self {
        self.proven = true;
        self
    }

    pub fn from_chunks(chunking: ChunkingDescriptor, chunks: &[Chunk]) -> Result<Self> {
        let mut builder = FileManifestBuilder::new(chunking, u64::MAX, chunks.len())?;
        for chunk in chunks {
            builder.push_chunk(chunk)?;
        }
        builder.finish()
    }

    pub fn empty(chunking: ChunkingDescriptor) -> Result<Self> {
        let file_id = FileId::from_digest(domain_hash(FILE_DOMAIN, &[]));
        Self::new(file_id, 0, chunking, Vec::new())
    }

    pub fn reconstruct<F>(&self, mut resolve: F) -> Result<Vec<u8>>
    where
        F: FnMut(&ChunkId) -> Option<Vec<u8>>,
    {
        let capacity = usize::try_from(self.byte_length).map_err(|_| ScmError::InvalidField {
            field: "byteLength",
            reason: "file is too large for this platform".into(),
        })?;
        let mut bytes = Vec::with_capacity(capacity);
        for entry in &self.chunks {
            let chunk_bytes = resolve(&entry.chunk_id)
                .ok_or_else(|| ScmError::MissingChunk(entry.chunk_id.to_string()))?;
            let computed = ChunkId::from_digest(domain_hash(self.chunk_domain(), &chunk_bytes));
            if computed != entry.chunk_id || chunk_bytes.len() as u64 != entry.byte_length {
                return Err(ScmError::InvalidChunk(entry.chunk_id.to_string()));
            }
            bytes.extend_from_slice(&chunk_bytes);
        }
        if bytes.len() as u64 != self.byte_length {
            return Err(ScmError::InvalidField {
                field: "byteLength",
                reason: "reconstructed length differs from manifest".into(),
            });
        }
        let computed = FileId::from_digest(domain_hash(FILE_DOMAIN, &bytes));
        if computed != self.file_id {
            return Err(ScmError::IdentityMismatch {
                claimed: self.file_id.to_string(),
                computed: computed.to_string(),
            });
        }
        Ok(bytes)
    }

    /// The `whole` manifest of a non-empty file of at most
    /// `WHOLE_MAX_BYTES`: its chunk is named by `file_id`'s digest.
    pub fn whole(file_id: FileId, byte_length: u64) -> Result<Self> {
        let chunk = FileChunk::new(ChunkId::of_whole_file(&file_id), 0, byte_length)?;
        Self::new(
            file_id,
            byte_length,
            ChunkingDescriptor::whole(),
            vec![chunk],
        )
    }

    /// Whether this manifest's chunks are named in the file domain (the
    /// `whole` recipe) rather than the chunk domain.
    pub fn chunks_are_whole(&self) -> bool {
        self.chunking.is_whole()
    }

    /// The domain this manifest's chunk ids are digests in.
    fn chunk_domain(&self) -> &'static str {
        if self.chunks_are_whole() {
            FILE_DOMAIN
        } else {
            CHUNK_DOMAIN
        }
    }

    /// Verifies `bytes` as this manifest's chunk `entry`, in the domain of
    /// its recipe.
    pub fn chunk(&self, entry: &FileChunk, bytes: Vec<u8>) -> Result<Chunk> {
        if bytes.len() as u64 != entry.byte_length {
            return Err(ScmError::InvalidChunk(entry.chunk_id.to_string()));
        }
        Chunk::verified(&entry.chunk_id, bytes, self.chunks_are_whole())
    }

    pub fn id(&self) -> &FileManifestId {
        &self.id
    }

    pub fn file_id(&self) -> &FileId {
        &self.file_id
    }

    pub fn byte_length(&self) -> u64 {
        self.byte_length
    }

    pub fn chunking(&self) -> &ChunkingDescriptor {
        &self.chunking
    }

    pub fn chunks(&self) -> &[FileChunk] {
        &self.chunks
    }

    pub fn canonical_payload(&self) -> Result<Vec<u8>> {
        canonical_json(&FileManifestPayload {
            schema_version: "layrs.file-manifest.v1",
            file_id: &self.file_id,
            byte_length: self.byte_length,
            chunking: &self.chunking,
            chunks: &self.chunks,
        })
    }

    pub fn identity_bytes(&self) -> Result<Vec<u8>> {
        Ok(domain_separated_bytes(
            FILE_MANIFEST_DOMAIN,
            &self.canonical_payload()?,
        ))
    }
}

/// A `whole` manifest: one chunk covering 1 byte to `WHOLE_MAX_BYTES`,
/// named by the FileId's digest.
fn validate_whole(file_id: &FileId, byte_length: u64, chunks: &[FileChunk]) -> Result<()> {
    let invalid = |reason: &str| ScmError::InvalidField {
        field: "chunking",
        reason: reason.into(),
    };
    if !(1..=WHOLE_MAX_BYTES).contains(&byte_length) {
        return Err(invalid("a whole file holds 1 byte to 1 MiB"));
    }
    match chunks {
        [chunk] if chunk.chunk_id.digest_bytes() == file_id.digest_bytes() => Ok(()),
        [_] => Err(invalid("a whole file's chunk is named by its FileId")),
        _ => Err(invalid("a whole file has exactly one chunk")),
    }
}

fn validate_coverage(byte_length: u64, chunks: &[FileChunk]) -> Result<()> {
    if byte_length == 0 && !chunks.is_empty() {
        return Err(ScmError::FileManifestChunksNotContiguous(
            "empty files must have zero chunks".into(),
        ));
    }
    if byte_length > 0 && chunks.is_empty() {
        return Err(ScmError::FileManifestChunksNotContiguous(
            "non-empty files require chunks".into(),
        ));
    }
    let mut expected = 0_u64;
    for chunk in chunks {
        if chunk.offset != expected {
            return Err(ScmError::FileManifestChunksNotContiguous(
                "chunk coverage must be ordered and contiguous".into(),
            ));
        }
        expected = expected.checked_add(chunk.byte_length).ok_or_else(|| {
            ScmError::FileManifestChunksNotContiguous("chunk coverage overflow".into())
        })?;
    }
    if expected != byte_length {
        return Err(ScmError::FileManifestChunksNotContiguous(
            "chunk coverage differs from byteLength".into(),
        ));
    }
    Ok(())
}

impl Serialize for FileManifest {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        FileManifestRef {
            schema_version: "layrs.file-manifest.v1",
            manifest_id: &self.id,
            file_id: &self.file_id,
            byte_length: self.byte_length,
            chunking: &self.chunking,
            chunks: &self.chunks,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FileManifest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = FileManifestWire::deserialize(deserializer)?;
        if wire.schema_version != "layrs.file-manifest.v1" {
            return Err(serde::de::Error::custom(
                "unsupported FileManifest schemaVersion",
            ));
        }
        let manifest = Self::new(wire.file_id, wire.byte_length, wire.chunking, wire.chunks)
            .map_err(serde::de::Error::custom)?;
        if manifest.id != wire.manifest_id {
            return Err(serde::de::Error::custom(ScmError::IdentityMismatch {
                claimed: wire.manifest_id.to_string(),
                computed: manifest.id.to_string(),
            }));
        }
        Ok(manifest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "fileId", rename_all = "snake_case")]
pub enum FileState {
    Absent,
    Present(FileId),
}

#[cfg(test)]
mod chunking_tests {
    use super::*;

    #[test]
    fn fixed_chunk_size_keeps_one_mib_up_to_one_gib_then_grows_to_sixteen() {
        const MIB: u64 = 1024 * 1024;
        const GIB: u64 = 1024 * MIB;
        assert_eq!(fixed_chunk_bytes(0), MIB);
        assert_eq!(fixed_chunk_bytes(GIB), MIB);
        assert_eq!(fixed_chunk_bytes(GIB + 1), 2 * MIB);
        assert_eq!(fixed_chunk_bytes(2 * GIB), 2 * MIB);
        assert_eq!(fixed_chunk_bytes(5 * GIB), 8 * MIB);
        assert_eq!(fixed_chunk_bytes(16 * GIB), 16 * MIB);
        assert_eq!(fixed_chunk_bytes(u64::MAX), MAX_FIXED_CHUNK_BYTES);
        assert_eq!(
            fixed_chunking(5 * GIB),
            ChunkingDescriptor::deterministic_fixed(8 * MIB).unwrap()
        );
    }

    #[test]
    fn only_a_manifest_built_from_hashed_bytes_is_proven() {
        let chunks = [
            Chunk::new(b"ab".to_vec()).unwrap(),
            Chunk::new(b"c".to_vec()).unwrap(),
        ];
        let built = FileManifest::from_chunks(fixed_chunking(3), &chunks).unwrap();
        assert!(built.is_proven());
        let declared = FileManifest::new(
            built.file_id().clone(),
            built.byte_length(),
            built.chunking().clone(),
            built.chunks().to_vec(),
        )
        .unwrap();
        assert!(!declared.is_proven());
        let read: FileManifest =
            serde_json::from_slice(&serde_json::to_vec(&built).unwrap()).unwrap();
        assert!(!read.is_proven());
        // Provenance is not identity: equality and ids ignore it.
        assert_eq!(built, declared);
        assert_eq!(built, read);
        assert_eq!(built.id(), read.id());
        assert!(built.clone().is_proven());
    }
}

#[cfg(test)]
mod whole_tests {
    use super::*;

    fn whole_manifest(bytes: &[u8]) -> FileManifest {
        let chunk = Chunk::whole(bytes.to_vec()).unwrap();
        FileManifest::from_chunks(ChunkingDescriptor::whole(), &[chunk]).unwrap()
    }

    #[test]
    fn a_whole_file_names_its_chunk_by_its_file_id() {
        let manifest = whole_manifest(b"hello");
        assert_eq!(manifest.file_id(), &FileId::from_bytes(b"hello"));
        assert_eq!(
            manifest.chunks()[0].chunk_id(),
            &ChunkId::of_whole_file(manifest.file_id())
        );
        assert!(manifest.is_proven());
        assert_eq!(
            FileManifest::whole(manifest.file_id().clone(), 5).unwrap(),
            manifest
        );
        // Its chunk is not the v1 chunk of the same bytes.
        assert_ne!(
            Chunk::new(b"hello".to_vec()).unwrap().id(),
            manifest.chunks()[0].chunk_id()
        );
        // Reconstruction and streaming verification use the file domain.
        let whole = Chunk::whole(b"hello".to_vec()).unwrap();
        assert_eq!(
            manifest.reconstruct(|_| Some(b"hello".to_vec())).unwrap(),
            b"hello"
        );
        manifest
            .verify_chunks(|id| (id == whole.id()).then(|| b"hello".to_vec()))
            .unwrap();
        assert!(manifest.reconstruct(|_| Some(b"hellO".to_vec())).is_err());
        assert_eq!(
            manifest
                .chunk(&manifest.chunks()[0], b"hello".to_vec())
                .unwrap(),
            whole
        );
        assert!(
            manifest
                .chunk(&manifest.chunks()[0], b"hellO".to_vec())
                .is_err()
        );
        // The JSON round trip keeps the recipe and the identity.
        let read: FileManifest =
            serde_json::from_slice(&serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_eq!(read, manifest);
        assert!(read.chunks_are_whole());
        let chunk: Chunk = serde_json::from_slice(&serde_json::to_vec(&whole).unwrap()).unwrap();
        assert_eq!(chunk, whole);
        assert!(chunk.is_whole());
    }

    #[test]
    fn a_whole_manifest_is_refused_unless_one_chunk_named_by_the_file() {
        let file_id = FileId::from_bytes(b"abc");
        let v1 = Chunk::new(b"abc".to_vec()).unwrap();
        // A v1 chunk id under the whole recipe.
        assert!(
            FileManifest::new(
                file_id.clone(),
                3,
                ChunkingDescriptor::whole(),
                vec![FileChunk::new(v1.id().clone(), 0, 3).unwrap()],
            )
            .is_err()
        );
        // Two chunks.
        let named = ChunkId::of_whole_file(&file_id);
        assert!(
            FileManifest::new(
                file_id.clone(),
                3,
                ChunkingDescriptor::whole(),
                vec![
                    FileChunk::new(named.clone(), 0, 1).unwrap(),
                    FileChunk::new(named, 1, 2).unwrap(),
                ],
            )
            .is_err()
        );
        // Empty, or over 1 MiB.
        assert!(FileManifest::empty(ChunkingDescriptor::whole()).is_err());
        assert!(FileManifest::whole(file_id, WHOLE_MAX_BYTES + 1).is_err());
        assert!(Chunk::whole(vec![1; WHOLE_MAX_BYTES as usize + 1]).is_err());
        // A builder refuses a chunk of the other domain.
        let mut builder =
            FileManifestBuilder::new(ChunkingDescriptor::whole(), u64::MAX, 1).unwrap();
        assert!(builder.push_chunk(&v1).is_err());
        let mut builder = FileManifestBuilder::new(fixed_chunking(3), u64::MAX, 1).unwrap();
        assert!(
            builder
                .push_chunk(&Chunk::whole(b"abc".to_vec()).unwrap())
                .is_err()
        );
        // The capture recipe.
        assert!(capture_chunking(1, true).is_whole());
        assert!(capture_chunking(WHOLE_MAX_BYTES, true).is_whole());
        assert!(!capture_chunking(WHOLE_MAX_BYTES + 1, true).is_whole());
        assert!(!capture_chunking(0, true).is_whole());
        assert!(!capture_chunking(5, false).is_whole());
    }
}
