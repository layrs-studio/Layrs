//! Contextual checks: a canonical Conflict is not proof of its referenced bytes.
use super::*;
use layrs_core::scm::{Step, TreeEntry};
use layrs_weave::{graph_merge::PlannedConflict, reconciliation::NativeLens, wasm_host};
use serde_json::Value;

pub(super) fn supported_pin(document: &Value) -> V3StoreResult<()> {
    if document["lensPin"].is_null() {
        return Ok(());
    }
    let supported = [NativeLens::Text, NativeLens::Code]
        .into_iter()
        .any(|lens| {
            let pin = wasm_host::manifest(lens).pin().expect("built-in manifest");
            serde_json::to_value(pin).ok().as_ref() == Some(&document["lensPin"])
        });
    if !supported || document["lensImplementationDigest"] != wasm_host::module_digest() {
        return Err(V3StoreError::InvalidDescriptor(
            "unsupported Conflict Lens pin".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn entry_at(
    repo: &PgV3Repository,
    space: &SpaceId,
    step: &Step,
    path: &str,
) -> V3StoreResult<Option<TreeEntry>> {
    let components: Vec<_> = path.split('/').collect();
    if components.len() > 128 {
        return Err(V3StoreError::InvalidDescriptor(
            "Conflict path depth".into(),
        ));
    }
    let mut tree_id = ObjectId::parse(step.root_tree_id().as_str()).expect("typed Tree");
    let mut total = 0;
    for (index, name) in components.iter().enumerate() {
        let descriptor = load_space_descriptor(repo.pool(), space, &tree_id).await?;
        if descriptor.kind != ObjectKind::Tree {
            return Err(V3StoreError::IdentityMismatch);
        }
        charge_closure(&mut total, descriptor.canonical_byte_len)?;
        let bytes =
            read_canonical_object(repo.cas().as_ref(), &descriptor, MAX_OBJECT_BYTES).await?;
        let tree: Tree = serde_json::from_slice(&bytes)
            .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
        let Some(entry) = tree
            .entries()
            .iter()
            .find(|entry| entry.name().as_str() == *name)
        else {
            return Ok(None);
        };
        if index + 1 == components.len() {
            return Ok(Some(entry.clone()));
        }
        let TreeEntryTarget::Tree { tree_id: next } = entry.target() else {
            return Ok(None);
        };
        tree_id = ObjectId::parse(next.as_str()).expect("typed Tree");
    }
    Ok(None)
}

pub(super) async fn verify_origins(
    repo: &PgV3Repository,
    space: &SpaceId,
    conflict: &PlannedConflict,
    steps: &BTreeMap<StepId, Step>,
) -> V3StoreResult<()> {
    let document = conflict.document();
    supported_pin(document)?;
    // Structural expansion retains old origins; pinned conflicts must retain
    // the actual original value at this exact immutable Step and placement.
    if document["lensPin"].is_null() {
        return Ok(());
    }
    let path = document["placements"][0]["path"]
        .as_str()
        .expect("validated placement");
    for role in ["bases", "sides"] {
        for term in document[role].as_array().expect("validated terms") {
            let step = StepId::parse(
                term["provenance"]["stepId"]
                    .as_str()
                    .expect("validated origin"),
            )
            .map_err(|_| V3StoreError::IdentityMismatch)?;
            let step = steps.get(&step).ok_or(V3StoreError::NotFound)?;
            let entry = entry_at(repo, space, step, path).await?;
            let value = match entry {
                None => serde_json::json!({"kind":"absent"}),
                Some(entry) => match entry.target() {
                    TreeEntryTarget::File { file_id } => {
                        serde_json::json!({"kind":"file", "objectId":file_id,"executable":entry.executable()})
                    }
                    TreeEntryTarget::Tree { tree_id } => {
                        serde_json::json!({"kind":"tree", "objectId":tree_id,"executable":null})
                    }
                    TreeEntryTarget::Symlink { symlink_id } => {
                        serde_json::json!({"kind":"symlink", "objectId":symlink_id,"executable":null})
                    }
                    _ => return Err(V3StoreError::IdentityMismatch),
                },
            };
            if value != term["values"][0]["value"] {
                return Err(V3StoreError::IdentityMismatch);
            }
        }
    }
    Ok(())
}

pub(super) fn required_files(conflict: &PlannedConflict) -> V3StoreResult<BTreeSet<FileId>> {
    let mut ids = conflict
        .payload_file_ids()
        .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
    if !ids.is_empty() {
        for role in ["bases", "sides"] {
            for term in conflict.document()[role]
                .as_array()
                .expect("validated terms")
            {
                ids.insert(
                    FileId::parse(
                        term["values"][0]["value"]["objectId"]
                            .as_str()
                            .expect("block File"),
                    )
                    .map_err(|_| V3StoreError::IdentityMismatch)?,
                );
            }
        }
    }
    Ok(ids)
}

pub(super) async fn read_payload_file(
    repo: &PgV3Repository,
    space: &SpaceId,
    manifest: &FileManifest,
    budget: &mut u64,
) -> V3StoreResult<Vec<u8>> {
    // Count logical File bytes, not unique chunk bytes, before allocation.
    charge_closure(budget, manifest.byte_length())?;
    let mut bytes = Vec::with_capacity(manifest.byte_length() as usize);
    for chunk in manifest.chunks() {
        let id = ObjectId::parse(chunk.chunk_id().as_str()).expect("typed Chunk");
        let descriptor = load_space_descriptor(repo.pool(), space, &id).await?;
        bytes.extend(
            read_canonical_object(repo.cas().as_ref(), &descriptor, MAX_OBJECT_BYTES).await?,
        );
    }
    if bytes.len() as u64 != manifest.byte_length()
        || FileId::from_bytes(&bytes) != *manifest.file_id()
    {
        return Err(V3StoreError::IdentityMismatch);
    }
    Ok(bytes)
}
