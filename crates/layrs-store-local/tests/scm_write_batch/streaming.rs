use super::*;

fn streamed_batch(root: &Path, label: &str) -> (WriteBatch, Chunk) {
    let initial = initialize(root, label);
    let chunk = Chunk::new(vec![42; 1024 * 1024]).unwrap();
    let manifest =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let mut objects = initial.canonical_objects().to_vec();
    objects.push(CanonicalObject::FileManifest(manifest.clone()));
    let space = initial.admissions()[0].space_id().clone();
    let mut admissions = initial.admissions().to_vec();
    admissions.push(SpaceObjectAdmission::new(
        space.clone(),
        StoreObjectId::ChunkRaw(chunk.id().clone()),
    ));
    admissions.push(SpaceObjectAdmission::new(
        space,
        StoreObjectId::FileManifest(manifest.id().clone()),
    ));
    let batch = WriteBatch::new(
        initial.transaction_id().clone(),
        objects,
        initial.entity_records().to_vec(),
        admissions,
        initial.mutations().to_vec(),
    )
    .unwrap();
    (batch, chunk)
}

#[test]
fn streamed_chunks_are_atomic_replayable_and_exclusively_staged() {
    let root = unique_root("streamed-atomic");
    let (batch, chunk) = streamed_batch(&root, "streamed-atomic");
    let descriptors = vec![(chunk.id().clone(), chunk.bytes().len() as u64)];
    let mut repo = NativeScmRepository::with_fault_injector(
        &root,
        Arc::new(OneFault::new(RepositoryFaultPoint::AfterValidation)),
    );
    let mut calls = 0;
    assert!(
        repo.apply_batch_with_chunks(&batch, &descriptors, |_| {
            calls += 1;
            assert!(
                open_exclusive(&root).is_err(),
                "Store capability must fence the entire local staging"
            );
            Ok(chunk.bytes().to_vec())
        })
        .is_err()
    );
    assert_eq!(calls, 1);
    let db = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    for table in [
        "objects",
        "spaces",
        "operations",
        "file_manifests",
        "applied_transactions",
    ] {
        assert_eq!(
            db.query_row::<i64, _, _>(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap(),
            0,
            "rollback {table}"
        );
    }
    drop(db);
    let mut repo = NativeScmRepository::new(&root);
    assert!(matches!(
        repo.apply_batch_with_chunks(&batch, &descriptors, |_| Ok(chunk.bytes().to_vec()))
            .unwrap(),
        ApplyBatchOutcome::Applied { .. }
    ));
    assert_eq!(repo.get_chunk(chunk.id()).unwrap(), Some(chunk.clone()));
    assert!(matches!(
        repo.apply_batch_with_chunks(&batch, &descriptors, |_| Ok(chunk.bytes().to_vec()))
            .unwrap(),
        ApplyBatchOutcome::AlreadyApplied { .. }
    ));
    assert!(
        fsck_existing(&root, FsckOptions::default())
            .unwrap()
            .is_clean()
    );
    cleanup(&root);
}

#[test]
fn streamed_descriptor_preflight_is_readonly_and_content_errors_never_publish() {
    let root = unique_root("streamed-reject");
    let (batch, chunk) = streamed_batch(&root, "streamed-reject");
    let descriptor = (chunk.id().clone(), chunk.bytes().len() as u64);
    let other = Chunk::new(b"unadmitted".to_vec()).unwrap();
    let mut repo = NativeScmRepository::new(&root);
    for descriptors in [
        vec![descriptor.clone(), descriptor.clone()],
        vec![(descriptor.0.clone(), 1)],
        vec![(other.id().clone(), 10)],
        vec![(descriptor.0.clone(), 65 * 1024 * 1024)],
    ] {
        assert!(
            repo.apply_batch_with_chunks(&batch, &descriptors, |_| panic!(
                "invalid descriptor must fail before content I/O"
            ))
            .is_err()
        );
        assert!(fs::read_dir(root.join("objects")).unwrap().next().is_none());
    }
    for bytes in [vec![43; 1024 * 1024], vec![42; 1024 * 1024 - 1]] {
        assert!(
            repo.apply_batch_with_chunks(&batch, std::slice::from_ref(&descriptor), |_| Ok(
                bytes.clone()
            ))
            .is_err()
        );
        assert!(fs::read_dir(root.join("objects")).unwrap().next().is_none());
    }
    assert!(repo.get_chunk(chunk.id()).unwrap().is_none());
    let db = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT count(*) FROM operations", [], |r| r.get(0))
            .unwrap(),
        0
    );
    drop(db);
    cleanup(&root);
}
