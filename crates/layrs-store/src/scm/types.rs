use super::{Result, StoreError};
use layrs_core::scm::{
    Chunk, ChunkId, ConflictId, FileId, FileManifest, FileManifestId, Layer, Marker, Operation,
    Release, ReleasePublication, RepositoryView, RepositoryViewId, Space, SpaceId, Step, StepId,
    StepObservation, Symlink, SymlinkId, Tombstone, TombstoneId, Tree, TreeId, Worktree,
};
use serde::{Serialize, Serializer};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TransactionId(String);

impl TransactionId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 255 {
            return Err(StoreError::InvalidTransactionId {
                reason: "length must be between 1 and 255 bytes".into(),
            });
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
        {
            return Err(StoreError::InvalidTransactionId {
                reason: "only ASCII letters, digits, '.', '_', ':', and '-' are allowed".into(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransactionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Digest of the canonical request payload, excluding the transaction ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RequestDigest([u8; 32]);

impl RequestDigest {
    pub(crate) fn compute(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for RequestDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("blake3:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Canonical object families persisted during Phase 2.
///
/// Conflict storage is opaque; structured conflict semantics remain reserved
/// for Phase 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreObjectKind {
    ChunkRaw,
    FileManifest,
    Tree,
    Symlink,
    Tombstone,
    Step,
    RepositoryView,
    Conflict,
    Product,
}

impl fmt::Display for StoreObjectKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ChunkRaw => "chunk_raw",
            Self::FileManifest => "file_manifest",
            Self::Tree => "tree",
            Self::Symlink => "symlink",
            Self::Tombstone => "tombstone",
            Self::Step => "step",
            Self::RepositoryView => "repository_view",
            Self::Conflict => "conflict",
            Self::Product => "product",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum StoreObjectId {
    ChunkRaw(ChunkId),
    FileManifest(FileManifestId),
    Tree(TreeId),
    Symlink(SymlinkId),
    Tombstone(TombstoneId),
    Step(StepId),
    RepositoryView(RepositoryViewId),
    Conflict(ConflictId),
    Product(layrs_core::scm::ObjectId),
}

impl StoreObjectId {
    pub const fn kind(&self) -> StoreObjectKind {
        match self {
            Self::ChunkRaw(_) => StoreObjectKind::ChunkRaw,
            Self::FileManifest(_) => StoreObjectKind::FileManifest,
            Self::Tree(_) => StoreObjectKind::Tree,
            Self::Symlink(_) => StoreObjectKind::Symlink,
            Self::Tombstone(_) => StoreObjectKind::Tombstone,
            Self::Step(_) => StoreObjectKind::Step,
            Self::RepositoryView(_) => StoreObjectKind::RepositoryView,
            Self::Conflict(_) => StoreObjectKind::Conflict,
            Self::Product(_) => StoreObjectKind::Product,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::ChunkRaw(id) => id.as_str(),
            Self::FileManifest(id) => id.as_str(),
            Self::Tree(id) => id.as_str(),
            Self::Symlink(id) => id.as_str(),
            Self::Tombstone(id) => id.as_str(),
            Self::Step(id) => id.as_str(),
            Self::RepositoryView(id) => id.as_str(),
            Self::Conflict(id) => id.as_str(),
            Self::Product(id) => id.as_str(),
        }
    }
}

impl fmt::Display for StoreObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CanonicalObject {
    ChunkRaw(Chunk),
    FileManifest(FileManifest),
    Tree(Tree),
    Symlink(Symlink),
    Tombstone(Tombstone),
    Step(Step),
    RepositoryView(RepositoryView),
    Conflict(CanonicalConflict),
    Product(super::CanonicalProduct),
}

impl CanonicalObject {
    pub fn object_id(&self) -> StoreObjectId {
        match self {
            Self::ChunkRaw(value) => StoreObjectId::ChunkRaw(value.id().clone()),
            Self::FileManifest(value) => StoreObjectId::FileManifest(value.id().clone()),
            Self::Tree(value) => StoreObjectId::Tree(value.id().clone()),
            Self::Symlink(value) => StoreObjectId::Symlink(value.id().clone()),
            Self::Tombstone(value) => StoreObjectId::Tombstone(value.id().clone()),
            Self::Step(value) => StoreObjectId::Step(value.id().clone()),
            Self::RepositoryView(value) => StoreObjectId::RepositoryView(value.id().clone()),
            Self::Conflict(value) => StoreObjectId::Conflict(value.id().clone()),
            Self::Product(value) => StoreObjectId::Product(value.id().clone()),
        }
    }

    pub const fn kind(&self) -> StoreObjectKind {
        match self {
            Self::ChunkRaw(_) => StoreObjectKind::ChunkRaw,
            Self::FileManifest(_) => StoreObjectKind::FileManifest,
            Self::Tree(_) => StoreObjectKind::Tree,
            Self::Symlink(_) => StoreObjectKind::Symlink,
            Self::Tombstone(_) => StoreObjectKind::Tombstone,
            Self::Step(_) => StoreObjectKind::Step,
            Self::RepositoryView(_) => StoreObjectKind::RepositoryView,
            Self::Conflict(_) => StoreObjectKind::Conflict,
            Self::Product(_) => StoreObjectKind::Product,
        }
    }
}

/// Opaque, identity-verified Conflict storage envelope reserved for Phase 5.
///
/// The store deliberately exposes no structured conflict semantics. It only
/// accepts the complete exact-JCS document and verifies its content identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalConflict {
    id: ConflictId,
    document: serde_json::Value,
    canonical_json: Vec<u8>,
}

impl CanonicalConflict {
    pub fn new(id: ConflictId, canonical_json_full: impl AsRef<[u8]>) -> Result<Self> {
        let bytes = canonical_json_full.as_ref();
        let document: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| StoreError::InvalidConflictDocument {
                reason: error.to_string(),
            })?;
        let object = document
            .as_object()
            .ok_or_else(|| StoreError::InvalidConflictDocument {
                reason: "root must be a JSON object".into(),
            })?;
        let embedded = object
            .get("conflictId")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| StoreError::InvalidConflictDocument {
                reason: "conflictId must be a string".into(),
            })?;
        if embedded != id.as_str() {
            return Err(StoreError::ConflictIdentityMismatch {
                claimed: id.to_string(),
                computed: embedded.to_owned(),
            });
        }
        let exact = layrs_core::scm::canonical_json(&document).map_err(|error| {
            StoreError::InvalidConflictDocument {
                reason: error.to_string(),
            }
        })?;
        if exact != bytes {
            return Err(StoreError::NonCanonicalConflict);
        }

        let mut payload = document.clone();
        payload
            .as_object_mut()
            .expect("Conflict root was checked as an object")
            .remove("conflictId");
        let payload = layrs_core::scm::canonical_json(&payload).map_err(|error| {
            StoreError::InvalidConflictDocument {
                reason: error.to_string(),
            }
        })?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs-conflict-v1");
        hasher.update(&[0]);
        hasher.update(&payload);
        let computed = ConflictId::parse(format!("blake3:{}", hasher.finalize().to_hex()))
            .map_err(|error| StoreError::InvalidConflictDocument {
                reason: error.to_string(),
            })?;
        if computed != id {
            return Err(StoreError::ConflictIdentityMismatch {
                claimed: id.to_string(),
                computed: computed.to_string(),
            });
        }
        Ok(Self {
            id,
            document,
            canonical_json: exact,
        })
    }

    pub fn id(&self) -> &ConflictId {
        &self.id
    }

    pub fn canonical_json(&self) -> &[u8] {
        &self.canonical_json
    }
}

impl Serialize for CanonicalConflict {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.document.serialize(serializer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Space,
    Layer,
    StepObservation,
    Operation,
    Worktree,
    Marker,
    Release,
    ReleasePublication,
}

impl fmt::Display for EntityKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Space => "space",
            Self::Layer => "layer",
            Self::StepObservation => "step_observation",
            Self::Operation => "operation",
            Self::Worktree => "worktree",
            Self::Marker => "marker",
            Self::Release => "release",
            Self::ReleasePublication => "release_publication",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum EntityRecord {
    Space(Space),
    Layer(Layer),
    StepObservation(StepObservation),
    Operation(Operation),
    Worktree(Worktree),
    Marker(Marker),
    Release(Release),
    ReleasePublication(ReleasePublication),
}

impl EntityRecord {
    pub const fn kind(&self) -> EntityKind {
        match self {
            Self::Space(_) => EntityKind::Space,
            Self::Layer(_) => EntityKind::Layer,
            Self::StepObservation(_) => EntityKind::StepObservation,
            Self::Operation(_) => EntityKind::Operation,
            Self::Worktree(_) => EntityKind::Worktree,
            Self::Marker(_) => EntityKind::Marker,
            Self::Release(_) => EntityKind::Release,
            Self::ReleasePublication(_) => EntityKind::ReleasePublication,
        }
    }

    pub fn id_str(&self) -> &str {
        match self {
            Self::Space(value) => value.id().as_str(),
            Self::Layer(value) => value.id().as_str(),
            Self::StepObservation(value) => value.id().as_str(),
            Self::Operation(value) => value.id().as_str(),
            Self::Worktree(value) => value.id().as_str(),
            Self::Marker(value) => value.id().as_str(),
            Self::Release(value) => value.id().as_str(),
            Self::ReleasePublication(value) => value.id().as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceObjectAdmission {
    space_id: SpaceId,
    object_id: StoreObjectId,
}

impl SpaceObjectAdmission {
    pub fn new(space_id: SpaceId, object_id: StoreObjectId) -> Self {
        Self {
            space_id,
            object_id,
        }
    }

    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub fn object_id(&self) -> &StoreObjectId {
        &self.object_id
    }
}

/// A missing file is never represented by the bytes of an empty file.
///
/// `Absent` means no file is stored for the lookup. `Present` names a real
/// `FileId`, including the valid ID of a zero-byte file, and at least one
/// verified manifest that reconstructs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAvailability {
    Absent,
    Present(AvailableFile),
}

impl FileAvailability {
    pub fn present(file_id: FileId, verified_manifest_ids: Vec<FileManifestId>) -> Result<Self> {
        AvailableFile::new(file_id, verified_manifest_ids).map(Self::Present)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableFile {
    file_id: FileId,
    verified_manifest_ids: Vec<FileManifestId>,
}

impl AvailableFile {
    pub fn new(file_id: FileId, verified_manifest_ids: Vec<FileManifestId>) -> Result<Self> {
        if verified_manifest_ids.is_empty() {
            return Err(StoreError::AdmissionRejected {
                kind: "file",
                id: file_id.to_string(),
            });
        }
        ensure_strictly_sorted("verifiedManifestIds", &verified_manifest_ids)?;
        Ok(Self {
            file_id,
            verified_manifest_ids,
        })
    }

    pub fn file_id(&self) -> &FileId {
        &self.file_id
    }

    pub fn verified_manifest_ids(&self) -> &[FileManifestId] {
        &self.verified_manifest_ids
    }
}

pub(crate) fn ensure_strictly_sorted<T: Ord>(field: &'static str, values: &[T]) -> Result<()> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(StoreError::NonCanonicalSet { field });
    }
    Ok(())
}
