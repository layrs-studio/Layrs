use super::*;
use layrs_core::scm::{Chunk, ChunkingDescriptor};

#[test]
fn missing_exact_private_representation_respects_its_protection() {
    let one = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(6).unwrap(),
        &[Chunk::new(b"abcdef".to_vec()).unwrap()],
    )
    .unwrap();
    let two = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(3).unwrap(),
        &[
            Chunk::new(b"abc".to_vec()).unwrap(),
            Chunk::new(b"def".to_vec()).unwrap(),
        ],
    )
    .unwrap();
    assert_eq!(one.file_id(), two.file_id());
    let empty = BTreeMap::new();
    assert!(
        resolve_private_manifest(one.file_id(), one.id(), 6, &empty, None)
            .unwrap()
            .is_none()
    );
    let roots = BTreeMap::from([(one.id().clone(), one.clone())]);
    assert!(resolve_private_manifest(one.file_id(), one.id(), 6, &roots, None).is_err());
    assert!(
        resolve_private_manifest(one.file_id(), one.id(), 6, &roots, Some(two)).is_err(),
        "another representation of the same FileId cannot replace the private mapping"
    );
    assert!(
        resolve_private_manifest(one.file_id(), one.id(), 7, &roots, Some(one.clone())).is_err()
    );
    assert_eq!(
        resolve_private_manifest(one.file_id(), one.id(), 6, &roots, Some(one.clone())).unwrap(),
        Some(one)
    );
}
