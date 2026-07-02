use crate::chunking::{Chunker, ChunkingStrategy};
use crate::digest::ObjectDigest;
use crate::validation::{
    SyncResult, SyncValidationError, push_kv, push_line, push_optional_kv, validate_non_empty,
    validate_object_digest, validate_path,
};
use base64::Engine;
use serde::{Deserialize, Serialize};

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
