//! Typed CAS object paths and kind decoding.
use super::*;
pub(super) fn object_relative_path(object_id: &StoreObjectId) -> CasResult<PathBuf> {
    let digest = digest(object_id)?;
    Ok(PathBuf::from(kind_name(object_id.kind()))
        .join(&digest[0..2])
        .join(&digest[2..4])
        .join(digest))
}

pub(super) fn digest(object_id: &StoreObjectId) -> CasResult<&str> {
    let value = object_id.as_str();
    let Some(digest) = value.strip_prefix("blake3:") else {
        return Err(CasError::InvalidObjectId {
            kind: object_id.kind(),
            id: value.into(),
        });
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(CasError::InvalidObjectId {
            kind: object_id.kind(),
            id: value.into(),
        });
    }
    Ok(digest)
}

pub(super) fn object_id_from_path(root: &Path, path: &Path) -> CasResult<StoreObjectId> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CasError::InvalidStagingPath(path.to_path_buf()))?;
    let components: Vec<_> = relative.components().collect();
    if components.len() != 4
        || !components
            .iter()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(CasError::InvalidStagingPath(path.to_path_buf()));
    }
    let kind_text = components[0].as_os_str().to_string_lossy();
    let kind = parse_kind(&kind_text).ok_or_else(|| CasError::KindMismatch {
        expected: StoreObjectKind::ChunkRaw,
        actual: kind_text.into_owned(),
    })?;
    let first = components[1].as_os_str().to_string_lossy();
    let second = components[2].as_os_str().to_string_lossy();
    let digest = components[3].as_os_str().to_string_lossy();
    let valid_digest = digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if !valid_digest {
        return Err(CasError::InvalidObjectId {
            kind,
            id: digest.into_owned(),
        });
    }
    if first != digest[0..2] || second != digest[2..4] {
        return Err(CasError::InvalidObjectId {
            kind,
            id: digest.into_owned(),
        });
    }
    parse_object_id(kind, &format!("blake3:{digest}"))
}

pub(super) fn parse_object_id(kind: StoreObjectKind, value: &str) -> CasResult<StoreObjectId> {
    macro_rules! parse {
        ($type:ty, $variant:path) => {
            <$type>::parse(value.to_owned())
                .map($variant)
                .map_err(|_| CasError::InvalidObjectId {
                    kind,
                    id: value.into(),
                })
        };
    }
    match kind {
        StoreObjectKind::ChunkRaw => parse!(ChunkId, StoreObjectId::ChunkRaw),
        StoreObjectKind::FileManifest => parse!(FileManifestId, StoreObjectId::FileManifest),
        StoreObjectKind::Tree => parse!(TreeId, StoreObjectId::Tree),
        StoreObjectKind::Symlink => parse!(SymlinkId, StoreObjectId::Symlink),
        StoreObjectKind::Tombstone => parse!(TombstoneId, StoreObjectId::Tombstone),
        StoreObjectKind::Step => parse!(StepId, StoreObjectId::Step),
        StoreObjectKind::RepositoryView => {
            parse!(RepositoryViewId, StoreObjectId::RepositoryView)
        }
        StoreObjectKind::Conflict => parse!(ConflictId, StoreObjectId::Conflict),
        StoreObjectKind::Product => parse!(layrs_core::scm::ObjectId, StoreObjectId::Product),
    }
}

pub(super) fn kind_name(kind: StoreObjectKind) -> &'static str {
    match kind {
        StoreObjectKind::ChunkRaw => "chunk_raw",
        StoreObjectKind::FileManifest => "file_manifest",
        StoreObjectKind::Tree => "tree",
        StoreObjectKind::Symlink => "symlink",
        StoreObjectKind::Tombstone => "tombstone",
        StoreObjectKind::Step => "step",
        StoreObjectKind::RepositoryView => "repository_view",
        StoreObjectKind::Conflict => "conflict",
        StoreObjectKind::Product => "product",
    }
}

pub(super) fn parse_kind(value: &str) -> Option<StoreObjectKind> {
    match value {
        "chunk_raw" => Some(StoreObjectKind::ChunkRaw),
        "file_manifest" => Some(StoreObjectKind::FileManifest),
        "tree" => Some(StoreObjectKind::Tree),
        "symlink" => Some(StoreObjectKind::Symlink),
        "tombstone" => Some(StoreObjectKind::Tombstone),
        "step" => Some(StoreObjectKind::Step),
        "repository_view" => Some(StoreObjectKind::RepositoryView),
        "conflict" => Some(StoreObjectKind::Conflict),
        "product" => Some(StoreObjectKind::Product),
        _ => None,
    }
}
