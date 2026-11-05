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

#[test]
fn restore_budget_bound_dominates_worst_case_checkout_requirements() {
    // Three classes of at most 1024 distinct files and 4GiB each (changed
    // result, preimages, replacements), every file rounded up one chunk,
    // plus old+new staging bytes: the largest restore reservation.
    let per_file = MAX_CAPTURE_BYTES / 1024;
    let files = (0..3 * 1024u32)
        .map(|n| CapturedFile {
            file_id: FileId::from_bytes(&n.to_le_bytes()),
            executable: false,
            bytes: per_file + 1 - u64::from(n % 2),
        })
        .collect::<Vec<_>>();
    let capped = files
        .iter()
        .map(|f| CapturedFile {
            bytes: f.bytes.min(per_file),
            ..f.clone()
        })
        .collect::<Vec<_>>();
    let worst = budget_for(&capped, 2 * MAX_CAPTURE_BYTES).unwrap();
    assert!(
        worst <= MAX_RESTORE_BUDGET_BYTES,
        "{worst} > {MAX_RESTORE_BUDGET_BYTES}"
    );
    // One extra byte per file in every class exceeds the per-class 4GiB scan
    // cap, so the bound is not loose by more than one rounding chunk a file.
    assert!(MAX_RESTORE_BUDGET_BYTES - worst <= 3 * 1024 * 256 + 3 * 256);
}
