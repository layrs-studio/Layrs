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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    id: ChunkId,
    bytes: Vec<u8>,
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
        Ok(Self { id, bytes })
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

    pub fn identity_bytes(&self) -> Vec<u8> {
        domain_separated_bytes(CHUNK_DOMAIN, &self.bytes)
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
            schema_version: "layrs.chunk.v1",
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
        if wire.schema_version != "layrs.chunk.v1" {
            return Err(serde::de::Error::custom("unsupported Chunk schemaVersion"));
        }
        let bytes = STANDARD
            .decode(wire.content.bytes.as_bytes())
            .map_err(serde::de::Error::custom)?;
        if STANDARD.encode(&bytes) != wire.content.bytes {
            return Err(serde::de::Error::custom("non-canonical base64"));
        }
        if bytes.len() as u64 != wire.byte_length {
            return Err(serde::de::Error::custom("Chunk byteLength mismatch"));
        }
        let chunk = Self::new(bytes).map_err(serde::de::Error::custom)?;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case", deny_unknown_fields)]
enum ChunkingWire {
    #[serde(rename = "single")]
    Single { version: u8 },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileManifest {
    id: FileManifestId,
    file_id: FileId,
    byte_length: u64,
    chunking: ChunkingDescriptor,
    chunks: Vec<FileChunk>,
}

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
        })
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
            let computed = ChunkId::from_digest(domain_hash(CHUNK_DOMAIN, &chunk_bytes));
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
