//! Read admitted canonical bytes, releasing all Store capabilities before HTTP.
use super::*;

pub(super) fn read(
    workspace: &NativeWorkspace,
    cap: &DownloadCapability,
) -> RemoteResult<Option<Vec<u8>>> {
    let descriptor = &cap.descriptor;
    let id = descriptor.object_id.as_str();
    let key = match descriptor.kind {
        ObjectKind::Chunk => StoreObjectId::ChunkRaw(ChunkId::parse(id)?),
        ObjectKind::FileManifest => StoreObjectId::FileManifest(FileManifestId::parse(id)?),
        ObjectKind::Tree => StoreObjectId::Tree(TreeId::parse(id)?),
        ObjectKind::Step => StoreObjectId::Step(StepId::parse(id)?),
        ObjectKind::Symlink => StoreObjectId::Symlink(SymlinkId::parse(id)?),
        ObjectKind::Tombstone => StoreObjectId::Tombstone(TombstoneId::parse(id)?),
        ObjectKind::Conflict => StoreObjectId::Conflict(ConflictId::parse(id)?),
        ObjectKind::Product => StoreObjectId::Product(descriptor.object_id.clone()),
        _ => {
            return Err(NativeRemoteError::Invalid(
                "unsupported local transfer object",
            ))
        }
    };
    let store = workspace.open_store()?;
    let reader = NativeScmReader::new(&store)?;
    if !reader.object_admitted_to_space(&key, &cap.space_id)? {
        return Ok(None);
    }
    // Typed readers verify CAS identity, durable verification and projections.
    // Missing/corrupt admitted objects must fail, not silently repair the Store.
    let object = match key {
        StoreObjectId::ChunkRaw(id) => reader.get_chunk(&id)?.map(CanonicalObject::ChunkRaw),
        StoreObjectId::FileManifest(id) => reader
            .get_file_manifest(&id)?
            .map(CanonicalObject::FileManifest),
        StoreObjectId::Tree(id) => reader.get_tree(&id)?.map(CanonicalObject::Tree),
        StoreObjectId::Step(id) => reader.get_step(&id)?.map(CanonicalObject::Step),
        StoreObjectId::Symlink(id) => reader.get_symlink(&id)?.map(CanonicalObject::Symlink),
        StoreObjectId::Tombstone(id) => reader.get_tombstone(&id)?.map(CanonicalObject::Tombstone),
        StoreObjectId::Conflict(id) => reader.get_conflict(&id)?.map(CanonicalObject::Conflict),
        StoreObjectId::Product(id) => reader.get_product(&id)?.map(CanonicalObject::Product),
        _ => unreachable!(),
    }
    .ok_or(NativeRemoteError::Invalid(
        "missing admitted transfer object",
    ))?;
    let (kind, bytes) = encode(&object)?;
    if kind != descriptor.kind
        || object.object_id().as_str() != id
        || bytes.len() as u64 != descriptor.canonical_byte_len
        || bytes.len() as u64 != descriptor.encoded_byte_len
    {
        return Err(NativeRemoteError::Invalid(
            "local transfer descriptor mismatch",
        ));
    }
    Ok(Some(bytes))
}
