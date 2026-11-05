use std::collections::{BTreeMap, BTreeSet};

use layrs_core::scm::ChunkId;
use layrs_store::scm::{CanonicalObject, Result, StoreError, StoreObjectId, WriteBatch};

const MAX_CHUNK_BYTES: u64 = 64 * 1024 * 1024;

pub(in crate::scm_v1) fn preflight(batch: &WriteBatch, chunks: &[(ChunkId, u64)]) -> Result<()> {
    if chunks.len().saturating_add(batch.canonical_objects().len())
        > super::super::admission::MAX_BATCH_OBJECTS
    {
        return Err(invalid("combined canonical object count exceeds the batch object bound"));
    }
    let admitted: BTreeSet<_> = batch.admissions().iter().map(|a| a.object_id()).collect();
    let inline: BTreeSet<_> = batch
        .canonical_objects()
        .iter()
        .map(|o| o.object_id())
        .collect();
    let mut seen = BTreeMap::new();
    for (id, length) in chunks {
        let object = StoreObjectId::ChunkRaw(id.clone());
        if *length == 0 || *length > MAX_CHUNK_BYTES {
            return Err(invalid("external chunk length is outside 1..=64MiB"));
        }
        if !admitted.contains(&object)
            || inline.contains(&object)
            || seen.insert(id, *length).is_some()
        {
            return Err(invalid(
                "external chunk must be unique, non-inline and admitted",
            ));
        }
    }
    // Reject a descriptor that disagrees with an available manifest before CAS
    // installation, not only later during full FileId validation. Verification
    // streams one chunk at a time, so File and batch bytes are unbounded (ADR
    // 0027); object counts stay bounded by the batch.
    for object in batch.canonical_objects() {
        if let CanonicalObject::FileManifest(manifest) = object {
            for chunk in manifest.chunks() {
                if seen
                    .get(chunk.chunk_id())
                    .is_some_and(|length| *length != chunk.byte_length())
                {
                    return Err(invalid("external chunk length disagrees with FileManifest"));
                }
            }
        }
    }
    Ok(())
}

fn invalid(message: &str) -> StoreError {
    StoreError::Corruption {
        message: format!("streaming batch preflight: {message}"),
    }
}
