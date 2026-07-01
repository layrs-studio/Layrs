//! Shared synchronization models for Layrs clients and server-side surfaces.
//!
//! This crate intentionally avoids transport and runtime dependencies. It is a
//! stable contract layer for API crates, store implementations, and future Axum
//! handlers.

use base64::Engine;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt;

pub type SyncResult<T> = Result<T, SyncValidationError>;

pub const SYNC_PROTOCOL_V2: &str = "layrs.sync.v2";
pub const OBJECT_DIGEST_ALGORITHM: &str = "blake3";
pub const OBJECT_DIGEST_PREFIX: &str = "blake3:";
pub const BLAKE3_HEX_LEN: usize = 64;
pub const SMALL_FILE_CHUNK_THRESHOLD: usize = 256 * 1024;
pub const CDC_MIN_CHUNK_BYTES: usize = 64 * 1024;
pub const CDC_TARGET_CHUNK_BYTES: usize = 1024 * 1024;
pub const CDC_MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncValidationError {
    pub field: &'static str,
    pub message: &'static str,
}

impl SyncValidationError {
    pub const fn new(field: &'static str, message: &'static str) -> Self {
        Self { field, message }
    }
}

impl fmt::Display for SyncValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for SyncValidationError {}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ObjectDigest(String);

impl ObjectDigest {
    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        validate_object_digest("object_digest", &value)?;
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn blake3_for(bytes: &[u8]) -> Self {
        Self(format!(
            "{OBJECT_DIGEST_PREFIX}{}",
            blake3::hash(bytes).to_hex()
        ))
    }

    pub fn unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn algorithm(&self) -> &str {
        self.0
            .split_once(':')
            .map(|(algorithm, _)| algorithm)
            .unwrap_or_default()
    }

    pub fn hex(&self) -> &str {
        self.0
            .split_once(':')
            .map(|(_, hex)| hex)
            .unwrap_or_default()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ObjectDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ObjectDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ObjectDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Chunk,
    File,
    Tree,
    LayerState,
    LocalStep,
}

impl ObjectKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chunk => "chunk",
            Self::File => "file",
            Self::Tree => "tree",
            Self::LayerState => "layer_state",
            Self::LocalStep => "local_step",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChunkingStrategy {
    Single,
    DeterministicFixed,
    FastCdcPrepared,
}

impl ChunkingStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::DeterministicFixed => "deterministic-fixed",
            Self::FastCdcPrepared => "fastcdc-prepared",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkSpan {
    pub offset: u64,
    pub byte_len: u64,
}

impl ChunkSpan {
    pub fn validate(&self) -> SyncResult<()> {
        if self.byte_len == 0 {
            return Err(SyncValidationError::new(
                "chunk_span.byte_len",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

pub trait Chunker {
    fn strategy(&self) -> ChunkingStrategy;
    fn chunk(&self, bytes: &[u8]) -> Vec<ChunkSpan>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeterministicChunker {
    pub small_file_threshold: usize,
    pub min_chunk_bytes: usize,
    pub target_chunk_bytes: usize,
    pub max_chunk_bytes: usize,
}

impl DeterministicChunker {
    pub const fn new(
        small_file_threshold: usize,
        min_chunk_bytes: usize,
        target_chunk_bytes: usize,
        max_chunk_bytes: usize,
    ) -> Self {
        Self {
            small_file_threshold,
            min_chunk_bytes,
            target_chunk_bytes,
            max_chunk_bytes,
        }
    }
}

impl Default for DeterministicChunker {
    fn default() -> Self {
        Self::new(
            SMALL_FILE_CHUNK_THRESHOLD,
            CDC_MIN_CHUNK_BYTES,
            CDC_TARGET_CHUNK_BYTES,
            CDC_MAX_CHUNK_BYTES,
        )
    }
}

impl Chunker for DeterministicChunker {
    fn strategy(&self) -> ChunkingStrategy {
        ChunkingStrategy::DeterministicFixed
    }

    fn chunk(&self, bytes: &[u8]) -> Vec<ChunkSpan> {
        if bytes.is_empty() {
            return Vec::new();
        }

        if bytes.len() <= self.small_file_threshold {
            return vec![ChunkSpan {
                offset: 0,
                byte_len: bytes.len() as u64,
            }];
        }

        let target = self
            .target_chunk_bytes
            .clamp(self.min_chunk_bytes, self.max_chunk_bytes);
        let mut spans = Vec::new();
        let mut offset = 0_usize;

        while offset < bytes.len() {
            let remaining = bytes.len() - offset;
            let mut byte_len = remaining.min(target);
            let trailing = remaining.saturating_sub(byte_len);

            if trailing > 0 && trailing < self.min_chunk_bytes {
                byte_len = remaining.min(self.max_chunk_bytes);
            }

            spans.push(ChunkSpan {
                offset: offset as u64,
                byte_len: byte_len as u64,
            });
            offset += byte_len;
        }

        spans
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkObject {
    pub chunk_id: ObjectDigest,
    pub digest: ObjectDigest,
    pub byte_len: u64,
    pub compression: Option<String>,
    pub encryption: Option<String>,
}

impl ChunkObject {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let digest = ObjectDigest::blake3_for(bytes);
        Self {
            chunk_id: digest.clone(),
            digest,
            byte_len: bytes.len() as u64,
            compression: None,
            encryption: None,
        }
    }

    pub fn validate(&self) -> SyncResult<()> {
        validate_object_digest("chunk_id", self.chunk_id.as_str())?;
        validate_object_digest("digest", self.digest.as_str())?;
        if self.chunk_id != self.digest {
            return Err(SyncValidationError::new(
                "chunk_id",
                "must match the content digest for raw chunk objects",
            ));
        }
        if self.byte_len == 0 {
            return Err(SyncValidationError::new(
                "byte_len",
                "must be greater than zero",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_line(&mut bytes, "layrs-chunk-object-v2");
        push_kv(&mut bytes, "chunk_id", self.chunk_id.as_str());
        push_kv(&mut bytes, "digest", self.digest.as_str());
        push_kv(&mut bytes, "byte_len", self.byte_len);
        push_optional_kv(&mut bytes, "compression", self.compression.as_deref());
        push_optional_kv(&mut bytes, "encryption", self.encryption.as_deref());
        bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkStoreObject {
    pub chunk_id: ObjectDigest,
    pub digest: ObjectDigest,
    pub byte_len: u64,
    pub content: Option<ContentBytes>,
    pub compression: Option<String>,
    pub encryption: Option<String>,
}

impl ChunkStoreObject {
    pub fn from_chunk_object(chunk: ChunkObject, content: Option<ContentBytes>) -> Self {
        Self {
            chunk_id: chunk.chunk_id,
            digest: chunk.digest,
            byte_len: chunk.byte_len,
            content,
            compression: chunk.compression,
            encryption: chunk.encryption,
        }
    }

    pub fn validate(&self) -> SyncResult<()> {
        validate_object_digest("chunk_id", self.chunk_id.as_str())?;
        validate_object_digest("digest", self.digest.as_str())?;
        if self.chunk_id != self.digest {
            return Err(SyncValidationError::new(
                "chunk_id",
                "must match digest for raw chunk store objects",
            ));
        }
        if self.byte_len == 0 {
            return Err(SyncValidationError::new(
                "byte_len",
                "must be greater than zero",
            ));
        }
        if let Some(content) = &self.content {
            let decoded = content.decode()?;
            if decoded.len() as u64 != self.byte_len {
                return Err(SyncValidationError::new(
                    "content",
                    "decoded byte length must match byte_len",
                ));
            }
            if ObjectDigest::blake3_for(&decoded) != self.digest {
                return Err(SyncValidationError::new(
                    "content",
                    "decoded bytes must match digest",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "encoding", content = "bytes")]
pub enum ContentBytes {
    #[serde(rename = "base64")]
    Base64(String),
}

impl ContentBytes {
    pub fn base64(bytes: &[u8]) -> Self {
        Self::Base64(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn decode(&self) -> SyncResult<Vec<u8>> {
        match self {
            Self::Base64(value) => base64::engine::general_purpose::STANDARD
                .decode(value)
                .map_err(|_| SyncValidationError::new("content", "must be valid base64")),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileObjectChunk {
    pub chunk_id: ObjectDigest,
    pub digest: ObjectDigest,
    pub offset: u64,
    pub byte_len: u64,
}

impl FileObjectChunk {
    pub fn validate(&self) -> SyncResult<()> {
        validate_object_digest("chunk_id", self.chunk_id.as_str())?;
        validate_object_digest("digest", self.digest.as_str())?;
        if self.byte_len == 0 {
            return Err(SyncValidationError::new(
                "file_chunk.byte_len",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileObject {
    pub file_object_id: ObjectDigest,
    pub digest: ObjectDigest,
    pub byte_len: u64,
    pub chunking: ChunkingStrategy,
    pub chunks: Vec<FileObjectChunk>,
}

impl FileObject {
    pub fn from_chunks(
        digest: ObjectDigest,
        byte_len: u64,
        chunking: ChunkingStrategy,
        chunks: Vec<FileObjectChunk>,
    ) -> SyncResult<Self> {
        let object = Self {
            file_object_id: ObjectDigest::unchecked("blake3:pending"),
            digest,
            byte_len,
            chunking,
            chunks,
        };
        object.validate_without_id()?;
        let file_object_id = ObjectDigest::blake3_for(&object.canonical_bytes_without_id());
        Ok(Self {
            file_object_id,
            ..object
        })
    }

    pub fn from_bytes(bytes: &[u8], chunker: &dyn Chunker) -> SyncResult<(Self, Vec<ChunkObject>)> {
        let spans = chunker.chunk(bytes);
        let mut chunks = Vec::with_capacity(spans.len());
        let mut chunk_objects = Vec::with_capacity(spans.len());

        for span in spans {
            span.validate()?;
            let start = span.offset as usize;
            let end = start + span.byte_len as usize;
            if end > bytes.len() {
                return Err(SyncValidationError::new(
                    "chunk_span",
                    "extends past file bytes",
                ));
            }

            let chunk = ChunkObject::from_bytes(&bytes[start..end]);
            chunks.push(FileObjectChunk {
                chunk_id: chunk.chunk_id.clone(),
                digest: chunk.digest.clone(),
                offset: span.offset,
                byte_len: span.byte_len,
            });
            chunk_objects.push(chunk);
        }

        let file = Self::from_chunks(
            ObjectDigest::blake3_for(bytes),
            bytes.len() as u64,
            chunker.strategy(),
            chunks,
        )?;
        Ok((file, chunk_objects))
    }

    pub fn validate(&self) -> SyncResult<()> {
        validate_object_digest("file_object_id", self.file_object_id.as_str())?;
        self.validate_without_id()
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.canonical_bytes_without_id()
    }

    fn validate_without_id(&self) -> SyncResult<()> {
        validate_object_digest("digest", self.digest.as_str())?;
        if self.byte_len > 0 && self.chunks.is_empty() {
            return Err(SyncValidationError::new(
                "chunks",
                "must contain at least one chunk for a non-empty file",
            ));
        }

        let mut expected_offset = 0_u64;
        for chunk in &self.chunks {
            chunk.validate()?;
            if chunk.offset != expected_offset {
                return Err(SyncValidationError::new(
                    "file_chunk.offset",
                    "must be contiguous and ordered",
                ));
            }
            expected_offset = expected_offset.saturating_add(chunk.byte_len);
        }

        if expected_offset != self.byte_len {
            return Err(SyncValidationError::new(
                "byte_len",
                "must equal the sum of file chunk byte lengths",
            ));
        }
        Ok(())
    }

    fn canonical_bytes_without_id(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_line(&mut bytes, "layrs-file-object-v2");
        push_kv(&mut bytes, "digest", self.digest.as_str());
        push_kv(&mut bytes, "byte_len", self.byte_len);
        push_kv(&mut bytes, "chunking", self.chunking.as_str());
        push_kv(&mut bytes, "chunk_count", self.chunks.len());
        for chunk in &self.chunks {
            push_line(
                &mut bytes,
                format!(
                    "chunk\t{}\t{}\t{}\t{}",
                    chunk.offset,
                    chunk.byte_len,
                    chunk.chunk_id.as_str(),
                    chunk.digest.as_str()
                ),
            );
        }
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TreeEntryKind {
    File,
    Directory,
}

impl TreeEntryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    pub path: String,
    pub kind: TreeEntryKind,
    pub object_id: ObjectDigest,
    pub byte_len: Option<u64>,
    pub executable: bool,
}

impl TreeEntry {
    pub fn file(path: impl Into<String>, file: &FileObject) -> Self {
        Self {
            path: path.into(),
            kind: TreeEntryKind::File,
            object_id: file.file_object_id.clone(),
            byte_len: Some(file.byte_len),
            executable: false,
        }
    }

    pub fn directory(path: impl Into<String>, tree: &TreeObject) -> Self {
        Self {
            path: path.into(),
            kind: TreeEntryKind::Directory,
            object_id: tree.tree_id.clone(),
            byte_len: None,
            executable: false,
        }
    }

    pub fn validate(&self) -> SyncResult<()> {
        validate_path("tree_entry.path", &self.path)?;
        validate_object_digest("tree_entry.object_id", self.object_id.as_str())?;
        if self.kind == TreeEntryKind::Directory && self.byte_len.is_some() {
            return Err(SyncValidationError::new(
                "tree_entry.byte_len",
                "must be empty for directories",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeObject {
    pub tree_id: ObjectDigest,
    pub entries: Vec<TreeEntry>,
}

impl TreeObject {
    pub fn from_entries(mut entries: Vec<TreeEntry>) -> SyncResult<Self> {
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let object = Self {
            tree_id: ObjectDigest::unchecked("blake3:pending"),
            entries,
        };
        object.validate_without_id()?;
        let tree_id = ObjectDigest::blake3_for(&object.canonical_bytes_without_id());
        Ok(Self { tree_id, ..object })
    }

    pub fn validate(&self) -> SyncResult<()> {
        validate_object_digest("tree_id", self.tree_id.as_str())?;
        self.validate_without_id()
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.canonical_bytes_without_id()
    }

    fn validate_without_id(&self) -> SyncResult<()> {
        let mut previous_path: Option<&str> = None;
        for entry in &self.entries {
            entry.validate()?;
            if matches!(previous_path, Some(previous) if previous >= entry.path.as_str()) {
                return Err(SyncValidationError::new(
                    "tree_entry.path",
                    "must be unique and sorted",
                ));
            }
            previous_path = Some(&entry.path);
        }
        Ok(())
    }

    fn canonical_bytes_without_id(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_line(&mut bytes, "layrs-tree-object-v2");
        push_kv(&mut bytes, "entry_count", self.entries.len());
        for entry in &self.entries {
            push_line(
                &mut bytes,
                format!(
                    "entry\t{}\t{}\t{}\t{}\t{}",
                    entry.path,
                    entry.kind.as_str(),
                    entry.object_id.as_str(),
                    entry
                        .byte_len
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    entry.executable
                ),
            );
        }
        bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerStateRef {
    pub layer_id: String,
    pub tree_id: ObjectDigest,
    pub policy_epoch: u64,
    pub parent_layer_id: Option<String>,
}

impl LayerStateRef {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("layer_id", &self.layer_id)?;
        validate_object_digest("tree_id", self.tree_id.as_str())?;
        if let Some(parent_layer_id) = &self.parent_layer_id {
            validate_non_empty("parent_layer_id", parent_layer_id)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStepRef {
    pub step_id: ObjectDigest,
    pub layer_id: String,
    pub tree_id: ObjectDigest,
    pub parent_step_id: Option<ObjectDigest>,
    pub policy_epoch: u64,
    pub created_at: String,
    pub message: Option<String>,
}

impl LocalStepRef {
    pub fn new(
        layer_id: impl Into<String>,
        tree_id: ObjectDigest,
        parent_step_id: Option<ObjectDigest>,
        policy_epoch: u64,
        created_at: impl Into<String>,
        message: Option<String>,
    ) -> SyncResult<Self> {
        let object = Self {
            step_id: ObjectDigest::unchecked("blake3:pending"),
            layer_id: layer_id.into(),
            tree_id,
            parent_step_id,
            policy_epoch,
            created_at: created_at.into(),
            message,
        };
        object.validate_without_id()?;
        let step_id = ObjectDigest::blake3_for(&object.canonical_bytes_without_id());
        Ok(Self { step_id, ..object })
    }

    pub fn validate(&self) -> SyncResult<()> {
        validate_object_digest("step_id", self.step_id.as_str())?;
        self.validate_without_id()
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        self.canonical_bytes_without_id()
    }

    fn validate_without_id(&self) -> SyncResult<()> {
        validate_non_empty("layer_id", &self.layer_id)?;
        validate_object_digest("tree_id", self.tree_id.as_str())?;
        if let Some(parent_step_id) = &self.parent_step_id {
            validate_object_digest("parent_step_id", parent_step_id.as_str())?;
        }
        validate_non_empty("created_at", &self.created_at)?;
        if let Some(message) = &self.message {
            validate_non_empty("message", message)?;
        }
        Ok(())
    }

    fn canonical_bytes_without_id(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_line(&mut bytes, "layrs-local-step-ref-v2");
        push_kv(&mut bytes, "layer_id", &self.layer_id);
        push_kv(&mut bytes, "tree_id", self.tree_id.as_str());
        push_kv(
            &mut bytes,
            "parent_step_id",
            self.parent_step_id
                .as_ref()
                .map(ObjectDigest::as_str)
                .unwrap_or("-"),
        );
        push_kv(&mut bytes, "policy_epoch", self.policy_epoch);
        push_kv(&mut bytes, "created_at", &self.created_at);
        push_optional_kv(&mut bytes, "message", self.message.as_deref());
        bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRef {
    pub object_id: ObjectDigest,
    pub object_kind: ObjectKind,
    pub byte_len: Option<u64>,
}

impl ObjectRef {
    pub fn validate(&self) -> SyncResult<()> {
        validate_object_digest("object_id", self.object_id.as_str())?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TombstoneObject {
    pub path: String,
    pub object_id: Option<ObjectDigest>,
    pub deleted_at: Option<String>,
}

impl TombstoneObject {
    pub fn validate(&self) -> SyncResult<()> {
        validate_path("tombstone.path", &self.path)?;
        if let Some(object_id) = &self.object_id {
            validate_object_digest("tombstone.object_id", object_id.as_str())?;
        }
        if let Some(deleted_at) = &self.deleted_at {
            validate_non_empty("tombstone.deleted_at", deleted_at)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreObjectsV2 {
    pub chunks: Vec<ChunkStoreObject>,
    pub file_objects: Vec<FileObject>,
    pub tree_objects: Vec<TreeObject>,
    pub tombstones: Vec<TombstoneObject>,
    pub deleted_paths: Vec<String>,
}

impl StoreObjectsV2 {
    pub fn validate(&self) -> SyncResult<()> {
        for chunk in &self.chunks {
            chunk.validate()?;
        }
        for file in &self.file_objects {
            file.validate()?;
        }
        for tree in &self.tree_objects {
            tree.validate()?;
        }
        for tombstone in &self.tombstones {
            tombstone.validate()?;
        }
        for path in &self.deleted_paths {
            validate_path("deleted_paths", path)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncManifestV2 {
    pub manifest_id: String,
    pub workspace_id: String,
    pub space_id: String,
    pub source_client_id: String,
    pub base_cursor: Option<String>,
    pub generated_at: String,
    pub layer_states: Vec<LayerStateRef>,
    pub local_steps: Vec<LocalStepRef>,
    pub required_objects: Vec<ObjectRef>,
}

impl SyncManifestV2 {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("manifest_id", &self.manifest_id)?;
        validate_non_empty("workspace_id", &self.workspace_id)?;
        validate_non_empty("space_id", &self.space_id)?;
        validate_non_empty("source_client_id", &self.source_client_id)?;
        validate_non_empty("generated_at", &self.generated_at)?;
        if let Some(cursor) = &self.base_cursor {
            validate_non_empty("base_cursor", cursor)?;
        }
        for layer_state in &self.layer_states {
            layer_state.validate()?;
        }
        for step in &self.local_steps {
            step.validate()?;
        }
        for object in &self.required_objects {
            object.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishRequestV2 {
    pub protocol: String,
    pub layer_id: String,
    pub policy_epoch: u64,
    pub idempotency_key: IdempotencyKey,
    pub source_client_id: String,
    pub base_tree_id: Option<ObjectDigest>,
    pub root_tree_id: ObjectDigest,
    pub changed_paths: Vec<String>,
    pub store_objects: StoreObjectsV2,
}

impl PublishRequestV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        validate_non_empty("layer_id", &self.layer_id)?;
        validate_non_empty("source_client_id", &self.source_client_id)?;
        IdempotencyKey::validate_raw(self.idempotency_key.as_str())?;
        if let Some(base_tree_id) = &self.base_tree_id {
            validate_object_digest("base_tree_id", base_tree_id.as_str())?;
        }
        validate_object_digest("root_tree_id", self.root_tree_id.as_str())?;
        for path in &self.changed_paths {
            validate_path("changed_paths", path)?;
        }
        self.store_objects.validate()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveRequestV2 {
    pub protocol: String,
    pub workspace_id: String,
    pub space_id: String,
    pub layer_id: String,
    pub since_cursor: Option<String>,
    pub client_id: String,
    pub known_tree_id: Option<ObjectDigest>,
    pub max_objects: u32,
}

impl ReceiveRequestV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        validate_non_empty("workspace_id", &self.workspace_id)?;
        validate_non_empty("space_id", &self.space_id)?;
        validate_non_empty("layer_id", &self.layer_id)?;
        validate_non_empty("client_id", &self.client_id)?;
        if let Some(cursor) = &self.since_cursor {
            validate_non_empty("since_cursor", cursor)?;
        }
        if let Some(tree_id) = &self.known_tree_id {
            validate_object_digest("known_tree_id", tree_id.as_str())?;
        }
        if self.max_objects == 0 {
            return Err(SyncValidationError::new(
                "max_objects",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiveResponseV2 {
    pub protocol: String,
    pub manifest: SyncManifestV2,
    pub store_objects: StoreObjectsV2,
    pub server_cursor: String,
    pub has_more: bool,
}

impl ReceiveResponseV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        self.manifest.validate()?;
        self.store_objects.validate()?;
        validate_non_empty("server_cursor", &self.server_cursor)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentRequestV2 {
    pub protocol: String,
    pub object_ids: Vec<ObjectDigest>,
    pub max_objects: u32,
}

impl ContentRequestV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        if self.object_ids.is_empty() {
            return Err(SyncValidationError::new(
                "object_ids",
                "must contain at least one object id",
            ));
        }
        for object_id in &self.object_ids {
            validate_object_digest("object_ids", object_id.as_str())?;
        }
        if self.max_objects == 0 {
            return Err(SyncValidationError::new(
                "max_objects",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentResponseV2 {
    pub protocol: String,
    pub store_objects: StoreObjectsV2,
    pub missing_objects: Vec<ObjectDigest>,
}

impl ContentResponseV2 {
    pub fn validate(&self) -> SyncResult<()> {
        if self.protocol != SYNC_PROTOCOL_V2 {
            return Err(SyncValidationError::new(
                "protocol",
                "must be layrs.sync.v2",
            ));
        }
        self.store_objects.validate()?;
        for object_id in &self.missing_objects {
            validate_object_digest("missing_objects", object_id.as_str())?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub const MIN_LEN: usize = 16;
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        Self::validate_raw(&value)?;
        Ok(Self(value))
    }

    pub fn unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub fn validate_raw(value: &str) -> SyncResult<()> {
        if value.len() < Self::MIN_LEN {
            return Err(SyncValidationError::new(
                "idempotency_key",
                "must be at least 16 bytes",
            ));
        }

        if value.len() > Self::MAX_LEN {
            return Err(SyncValidationError::new(
                "idempotency_key",
                "must be at most 128 bytes",
            ));
        }

        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
        {
            return Err(SyncValidationError::new(
                "idempotency_key",
                "may only contain ASCII letters, numbers, '-', '_', ':' or '.'",
            ));
        }

        Ok(())
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for IdempotencyKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ChunkId(pub String);

impl ChunkId {
    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        validate_non_empty("chunk_id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ContentDigest(pub String);

impl ContentDigest {
    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        validate_digest(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkRef {
    pub chunk_id: ChunkId,
    pub digest: ContentDigest,
    pub byte_len: u64,
    pub media_type: Option<String>,
    pub compression: Option<String>,
    pub encryption: Option<String>,
}

impl ChunkRef {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("chunk_id", self.chunk_id.as_str())?;
        validate_digest(self.digest.as_str())?;
        if self.byte_len == 0 {
            return Err(SyncValidationError::new(
                "byte_len",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncManifest {
    pub manifest_id: String,
    pub workspace_id: String,
    pub space_id: Option<String>,
    pub source_client_id: String,
    pub base_cursor: Option<String>,
    pub capability_epoch: u64,
    pub generated_at: String,
    pub chunks: Vec<ChunkRef>,
    pub operations: Vec<SyncOperationRef>,
}

impl SyncManifest {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("manifest_id", &self.manifest_id)?;
        validate_non_empty("workspace_id", &self.workspace_id)?;
        validate_non_empty("source_client_id", &self.source_client_id)?;
        validate_non_empty("generated_at", &self.generated_at)?;
        if let Some(space_id) = &self.space_id {
            validate_non_empty("space_id", space_id)?;
        }
        for chunk in &self.chunks {
            chunk.validate()?;
        }
        for operation in &self.operations {
            operation.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncOperationRef {
    pub operation_id: String,
    pub entity_kind: SyncEntityKind,
    pub entity_id: String,
    pub operation_kind: SyncOperationKind,
    pub client_sequence: u64,
    pub base_version: Option<String>,
    pub resulting_version: String,
    pub chunks: Vec<ChunkId>,
}

impl SyncOperationRef {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("operation_id", &self.operation_id)?;
        validate_non_empty("entity_id", &self.entity_id)?;
        validate_non_empty("resulting_version", &self.resulting_version)?;
        if self.client_sequence == 0 {
            return Err(SyncValidationError::new(
                "client_sequence",
                "must be greater than zero",
            ));
        }
        for chunk_id in &self.chunks {
            validate_non_empty("chunk_id", chunk_id.as_str())?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SyncEntityKind {
    Workspace,
    Team,
    Space,
    Layer,
    Artifact,
    Weave,
    Proof,
    Policy,
    TimelineEvent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SyncOperationKind {
    Create,
    Update,
    Delete,
    AttachChunk,
    DetachChunk,
    Reorder,
    Decision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishRequest {
    pub idempotency_key: IdempotencyKey,
    pub manifest: SyncManifest,
    pub expected_server_cursor: Option<String>,
    pub dry_run: bool,
}

impl PublishRequest {
    pub fn validate(&self) -> SyncResult<()> {
        IdempotencyKey::validate_raw(self.idempotency_key.as_str())?;
        self.manifest.validate()?;
        if let Some(cursor) = &self.expected_server_cursor {
            validate_non_empty("expected_server_cursor", cursor)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiveRequest {
    pub workspace_id: String,
    pub space_id: Option<String>,
    pub since_cursor: Option<String>,
    pub client_id: String,
    pub max_chunks: u32,
}

impl ReceiveRequest {
    pub fn validate(&self) -> SyncResult<()> {
        validate_non_empty("workspace_id", &self.workspace_id)?;
        validate_non_empty("client_id", &self.client_id)?;
        if let Some(space_id) = &self.space_id {
            validate_non_empty("space_id", space_id)?;
        }
        if let Some(cursor) = &self.since_cursor {
            validate_non_empty("since_cursor", cursor)?;
        }
        if self.max_chunks == 0 {
            return Err(SyncValidationError::new(
                "max_chunks",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncDecision {
    Accepted {
        server_cursor: String,
        accepted_operations: usize,
        missing_chunks: Vec<ChunkId>,
    },
    AlreadyApplied {
        server_cursor: String,
    },
    Conflict {
        server_cursor: String,
        conflicts: Vec<SyncConflict>,
    },
    Rejected {
        reason: SyncRejectionReason,
        message: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncConflict {
    pub entity_kind: SyncEntityKind,
    pub entity_id: String,
    pub client_operation_id: String,
    pub server_version: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SyncRejectionReason {
    PermissionDenied,
    PolicyRejected,
    MissingBase,
    MissingChunk,
    StaleCursor,
    InvalidManifest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishReceipt {
    pub idempotency_key: IdempotencyKey,
    pub decision: SyncDecision,
    pub received_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiveResponse {
    pub manifest: SyncManifest,
    pub server_cursor: String,
    pub has_more: bool,
}

fn validate_non_empty(field: &'static str, value: &str) -> SyncResult<()> {
    if value.trim().is_empty() {
        return Err(SyncValidationError::new(field, "must not be empty"));
    }
    Ok(())
}

fn validate_digest(value: &str) -> SyncResult<()> {
    validate_non_empty("digest", value)?;
    if !value.contains(':') {
        return Err(SyncValidationError::new(
            "digest",
            "must include an algorithm prefix",
        ));
    }
    Ok(())
}

fn validate_object_digest(field: &'static str, value: &str) -> SyncResult<()> {
    validate_non_empty(field, value)?;
    let Some(hex) = value.strip_prefix(OBJECT_DIGEST_PREFIX) else {
        return Err(SyncValidationError::new(
            field,
            "must use the blake3:<hex> digest format",
        ));
    };
    if hex.len() != BLAKE3_HEX_LEN {
        return Err(SyncValidationError::new(
            field,
            "must contain a 64 character BLAKE3 hex digest",
        ));
    }
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(SyncValidationError::new(
            field,
            "must contain only lowercase hex",
        ));
    }
    Ok(())
}

fn validate_path(field: &'static str, value: &str) -> SyncResult<()> {
    validate_non_empty(field, value)?;
    if value.starts_with('/') || value.starts_with('\\') || value.contains("..") {
        return Err(SyncValidationError::new(
            field,
            "must be a relative normalized path",
        ));
    }
    if value
        .bytes()
        .any(|byte| byte == b'\\' || byte == 0 || byte < 0x20)
    {
        return Err(SyncValidationError::new(
            field,
            "contains unsupported characters",
        ));
    }
    Ok(())
}

fn push_line(bytes: &mut Vec<u8>, value: impl AsRef<str>) {
    bytes.extend_from_slice(value.as_ref().as_bytes());
    bytes.push(b'\n');
}

fn push_kv(bytes: &mut Vec<u8>, key: &str, value: impl fmt::Display) {
    push_line(bytes, format!("{key}={value}"));
}

fn push_optional_kv(bytes: &mut Vec<u8>, key: &str, value: Option<&str>) {
    push_kv(bytes, key, value.unwrap_or("-"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_key_accepts_stable_ascii_token() {
        let key = IdempotencyKey::new("client-a:0000000001").expect("valid key");
        assert_eq!(key.as_str(), "client-a:0000000001");
    }

    #[test]
    fn idempotency_key_rejects_short_or_unsafe_values() {
        assert!(IdempotencyKey::new("short").is_err());
        assert!(IdempotencyKey::new("client-a key with spaces").is_err());
    }

    #[test]
    fn publish_request_validates_manifest_and_cursor() {
        let request = PublishRequest {
            idempotency_key: IdempotencyKey::new("client-a:0000000002").unwrap(),
            manifest: SyncManifest {
                manifest_id: "manifest-1".into(),
                workspace_id: "workspace-1".into(),
                space_id: Some("space-1".into()),
                source_client_id: "client-a".into(),
                base_cursor: None,
                capability_epoch: 1,
                generated_at: "2026-06-29T18:00:00Z".into(),
                chunks: vec![ChunkRef {
                    chunk_id: ChunkId::new("chunk-1").unwrap(),
                    digest: ContentDigest::new("sha256:abc").unwrap(),
                    byte_len: 128,
                    media_type: None,
                    compression: None,
                    encryption: None,
                }],
                operations: vec![SyncOperationRef {
                    operation_id: "op-1".into(),
                    entity_kind: SyncEntityKind::Layer,
                    entity_id: "layer-1".into(),
                    operation_kind: SyncOperationKind::Update,
                    client_sequence: 1,
                    base_version: None,
                    resulting_version: "v1".into(),
                    chunks: vec![ChunkId::new("chunk-1").unwrap()],
                }],
            },
            expected_server_cursor: Some("cursor-1".into()),
            dry_run: false,
        };

        assert_eq!(request.validate(), Ok(()));
    }

    #[test]
    fn object_digest_is_blake3_prefixed() {
        let digest = ObjectDigest::blake3_for(b"hello layrs");

        assert!(digest.as_str().starts_with(OBJECT_DIGEST_PREFIX));
        assert_eq!(digest.algorithm(), OBJECT_DIGEST_ALGORITHM);
        assert_eq!(digest.hex().len(), BLAKE3_HEX_LEN);
        assert_eq!(ObjectDigest::new(digest.as_str()), Ok(digest));
        assert!(ObjectDigest::new("sha256:abc").is_err());
        assert!(
            ObjectDigest::new(
                "blake3:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            )
            .is_err()
        );
    }

    #[test]
    fn deterministic_chunker_uses_single_chunk_for_small_files() {
        let chunker = DeterministicChunker::default();
        let bytes = vec![7_u8; SMALL_FILE_CHUNK_THRESHOLD];

        assert_eq!(
            chunker.chunk(&bytes),
            vec![ChunkSpan {
                offset: 0,
                byte_len: SMALL_FILE_CHUNK_THRESHOLD as u64
            }]
        );
    }

    #[test]
    fn deterministic_chunker_splits_large_files_contiguously() {
        let chunker = DeterministicChunker::default();
        let bytes = vec![9_u8; CDC_TARGET_CHUNK_BYTES + CDC_MIN_CHUNK_BYTES + 1];
        let chunks = chunker.chunk(&bytes);

        assert!(chunks.len() >= 2);
        assert_eq!(chunks.first().unwrap().offset, 0);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.byte_len).sum::<u64>(),
            bytes.len() as u64
        );
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].offset + pair[0].byte_len, pair[1].offset);
        }
    }

    #[test]
    fn file_tree_and_step_ids_are_stable_merkle_refs() {
        let chunker = DeterministicChunker::default();
        let (file, chunks) = FileObject::from_bytes(b"hello", &chunker).unwrap();
        let tree = TreeObject::from_entries(vec![TreeEntry::file("src/hello.txt", &file)]).unwrap();
        let step = LocalStepRef::new(
            "layer-main",
            tree.tree_id.clone(),
            None,
            7,
            "2026-06-30T12:00:00Z",
            Some("initial local step".into()),
        )
        .unwrap();

        assert_eq!(chunks.len(), 1);
        assert!(
            file.file_object_id
                .as_str()
                .starts_with(OBJECT_DIGEST_PREFIX)
        );
        assert!(tree.tree_id.as_str().starts_with(OBJECT_DIGEST_PREFIX));
        assert!(step.step_id.as_str().starts_with(OBJECT_DIGEST_PREFIX));
        assert_eq!(file.validate(), Ok(()));
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(step.validate(), Ok(()));
    }

    #[test]
    fn publish_v2_serializes_desktop_server_contract_in_camel_case() {
        let bytes = b"hello";
        let chunk = ChunkObject::from_bytes(bytes);
        let (file, _) = FileObject::from_bytes(bytes, &DeterministicChunker::default()).unwrap();
        let tree = TreeObject::from_entries(vec![TreeEntry::file("src/hello.txt", &file)]).unwrap();
        let request = PublishRequestV2 {
            protocol: SYNC_PROTOCOL_V2.into(),
            layer_id: "layer-main".into(),
            policy_epoch: 9,
            idempotency_key: IdempotencyKey::new("desktop-a:0000001").unwrap(),
            source_client_id: "desktop-a".into(),
            base_tree_id: Some(tree.tree_id.clone()),
            root_tree_id: tree.tree_id.clone(),
            changed_paths: vec!["src/hello.txt".into()],
            store_objects: StoreObjectsV2 {
                chunks: vec![ChunkStoreObject::from_chunk_object(
                    chunk,
                    Some(ContentBytes::base64(bytes)),
                )],
                file_objects: vec![file],
                tree_objects: vec![tree],
                tombstones: vec![],
                deleted_paths: vec!["old/hello.txt".into()],
            },
        };

        request.validate().unwrap();
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["protocol"], SYNC_PROTOCOL_V2);
        assert_eq!(json["layerId"], "layer-main");
        assert_eq!(json["policyEpoch"], 9);
        assert_eq!(json["sourceClientId"], "desktop-a");
        assert!(json["rootTreeId"].as_str().unwrap().starts_with("blake3:"));
        assert!(json.get("root_tree_id").is_none());
        assert_eq!(
            json["storeObjects"]["chunks"][0]["content"]["encoding"],
            "base64"
        );
        assert_eq!(json["storeObjects"]["deletedPaths"][0], "old/hello.txt");

        let round_trip: PublishRequestV2 = serde_json::from_value(json).unwrap();
        assert_eq!(round_trip.validate(), Ok(()));
    }

    #[test]
    fn publish_v2_rejects_non_blake3_or_hex_content_shapes() {
        let valid_digest = ObjectDigest::blake3_for(b"hello");
        let request = serde_json::json!({
            "protocol": SYNC_PROTOCOL_V2,
            "layerId": "layer-main",
            "policyEpoch": 1,
            "idempotencyKey": "desktop-a:0000002",
            "sourceClientId": "desktop-a",
            "baseTreeId": null,
            "rootTreeId": "sha256:abc",
            "changedPaths": ["src/hello.txt"],
            "storeObjects": {
                "chunks": [],
                "fileObjects": [],
                "treeObjects": [],
                "tombstones": [],
                "deletedPaths": []
            }
        });

        assert!(serde_json::from_value::<PublishRequestV2>(request).is_err());

        let request_with_hex_content = serde_json::json!({
            "protocol": SYNC_PROTOCOL_V2,
            "layerId": "layer-main",
            "policyEpoch": 1,
            "idempotencyKey": "desktop-a:0000003",
            "sourceClientId": "desktop-a",
            "baseTreeId": null,
            "rootTreeId": valid_digest.as_str(),
            "changedPaths": ["src/hello.txt"],
            "storeObjects": {
                "chunks": [{
                    "chunkId": valid_digest.as_str(),
                    "digest": valid_digest.as_str(),
                    "byteLen": 5,
                    "content": {"encoding": "hex", "bytes": "68656c6c6f"},
                    "compression": null,
                    "encryption": null
                }],
                "fileObjects": [],
                "treeObjects": [],
                "tombstones": [],
                "deletedPaths": []
            }
        });

        assert!(serde_json::from_value::<PublishRequestV2>(request_with_hex_content).is_err());
    }

    #[test]
    fn content_v2_response_uses_decodable_base64_chunks() {
        let bytes = b"hello";
        let digest = ObjectDigest::blake3_for(bytes);
        let response = ContentResponseV2 {
            protocol: SYNC_PROTOCOL_V2.into(),
            store_objects: StoreObjectsV2 {
                chunks: vec![ChunkStoreObject {
                    chunk_id: digest.clone(),
                    digest,
                    byte_len: bytes.len() as u64,
                    content: Some(ContentBytes::base64(bytes)),
                    compression: None,
                    encryption: None,
                }],
                file_objects: vec![],
                tree_objects: vec![],
                tombstones: vec![],
                deleted_paths: vec![],
            },
            missing_objects: vec![],
        };

        assert_eq!(response.validate(), Ok(()));
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(
            json["storeObjects"]["chunks"][0]["content"]["bytes"],
            "aGVsbG8="
        );
        let decoded = response.store_objects.chunks[0]
            .content
            .as_ref()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(decoded, bytes);
    }
}
