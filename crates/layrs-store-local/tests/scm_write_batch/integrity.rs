use super::*;

#[test]
fn missing_object_closure_is_rejected_before_metadata() {
    let root = unique_root("missing-closure");
    drop(init(&root, StoreId::parse("store:missing-closure").unwrap()).unwrap());
    let missing = TreeId::parse(format!("blake3:{}", "0".repeat(64))).unwrap();
    let step = Step::new(vec![], missing).unwrap();
    let space = Space::new(
        SpaceId::parse("space:missing-closure").unwrap(),
        "Missing",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:missing-closure").unwrap(),
        vec![CanonicalObject::Step(step.clone())],
        vec![EntityRecord::Space(space.clone())],
        vec![SpaceObjectAdmission::new(
            space.id().clone(),
            StoreObjectId::Step(step.id().clone()),
        )],
        vec![],
    )
    .unwrap();
    let mut repository = NativeScmRepository::new(&root);
    assert!(repository.apply_batch(&batch).is_err());
    drop(repository);
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM objects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(connection);
    cleanup(&root);
}

#[test]
fn fault_after_second_cas_leaves_complete_orphans_and_zero_metadata() {
    let root = unique_root("multi-cas-fault");
    drop(init(&root, StoreId::parse("store:multi-cas-fault").unwrap()).unwrap());
    let space = Space::new(
        SpaceId::parse("space:multi-cas-fault").unwrap(),
        "Multi",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let first = Chunk::new(b"first".to_vec()).unwrap();
    let second = Chunk::new(b"second".to_vec()).unwrap();
    let objects = vec![
        CanonicalObject::ChunkRaw(first.clone()),
        CanonicalObject::ChunkRaw(second.clone()),
    ];
    let admissions = objects
        .iter()
        .map(|o| SpaceObjectAdmission::new(space.id().clone(), o.object_id()))
        .collect();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:multi-cas-fault").unwrap(),
        objects,
        vec![EntityRecord::Space(space)],
        admissions,
        vec![],
    )
    .unwrap();
    let mut repository = NativeScmRepository::with_fault_injector(
        &root,
        Arc::new(NthFault {
            point: RepositoryFaultPoint::AfterEachCas,
            target: 2,
            count: AtomicU64::new(0),
        }),
    );
    assert!(repository.apply_batch(&batch).is_err());
    drop(repository);
    let store = open(&root).unwrap();
    let cas = NativeCas::open(&store).unwrap();
    assert!(
        cas.contains(&StoreObjectId::ChunkRaw(first.id().clone()))
            .unwrap()
    );
    assert!(
        cas.contains(&StoreObjectId::ChunkRaw(second.id().clone()))
            .unwrap()
    );
    drop(cas);
    drop(store);
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM objects", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(connection);
    cleanup(&root);
}

#[test]
fn apply_batch_cas_fault_boundaries_never_publish_metadata() {
    for (index, point) in [
        CasFaultPoint::BeforeStagingSync,
        CasFaultPoint::AfterPublication,
        CasFaultPoint::AfterObjectDirectorySync,
    ]
    .into_iter()
    .enumerate()
    {
        let root = unique_root(&format!("cas-boundary-{index}"));
        let batch = initialize(&root, &format!("casboundary{index}"));
        let object = batch.canonical_objects()[0].object_id();
        let mut repository =
            NativeScmRepository::with_cas_fault_injector(&root, Arc::new(OneCasFault { point }));
        assert!(repository.apply_batch(&batch).is_err());
        drop(repository);
        let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
        for table in [
            "objects",
            "spaces",
            "operations",
            "repository_views",
            "space_object_admissions",
            "current_repository_views",
            "operation_heads",
            "gc_roots",
            "applied_transactions",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{point:?}");
        }
        drop(connection);
        let store = open(&root).unwrap();
        let cas = NativeCas::open(&store).unwrap();
        let published = cas.contains(&object).unwrap();
        assert_eq!(
            published,
            point != CasFaultPoint::BeforeStagingSync,
            "{point:?}"
        );
        drop(cas);
        drop(store);
        let exclusive = open_exclusive(&root).unwrap();
        let writer = NativeCas::open_exclusive(&exclusive).unwrap();
        assert!(writer.recover_staging().unwrap() <= 1);
        drop(writer);
        drop(exclusive);
        assert!(
            fs::read_dir(root.join("staging"))
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == ".lease")
        );
        let mut retry = NativeScmRepository::new(&root);
        assert!(matches!(
            retry.apply_batch(&batch).unwrap(),
            ApplyBatchOutcome::Applied { .. }
        ));
        let space = batch
            .entity_records()
            .iter()
            .find_map(|e| {
                if let EntityRecord::Space(v) = e {
                    Some(v)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            retry
                .current_repository_view(space.id())
                .unwrap()
                .unwrap()
                .generation(),
            1
        );
        drop(retry);
        assert_clean_db(&root);
        cleanup(&root);
    }
}

#[test]
fn exact_canonical_object_reapply_is_one_row_and_cas_tamper_is_corruption() {
    let root = unique_root("object-reapply");
    let initial = initialize(&root, "objectreapply");
    let space = initial
        .entity_records()
        .iter()
        .find_map(|e| {
            if let EntityRecord::Space(v) = e {
                Some(v.clone())
            } else {
                None
            }
        })
        .unwrap();
    let object = initial.canonical_objects()[0].clone();
    let object_id = object.object_id();
    let mut repository = NativeScmRepository::new(&root);
    repository.apply_batch(&initial).unwrap();
    let reapply = WriteBatch::new(
        TransactionId::new("transaction:object-reapply").unwrap(),
        vec![object],
        vec![],
        vec![SpaceObjectAdmission::new(
            space.id().clone(),
            object_id.clone(),
        )],
        vec![],
    )
    .unwrap();
    repository.apply_batch(&reapply).unwrap();
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM objects WHERE object_id=?1",
            [object_id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    drop(connection);
    let store = open(&root).unwrap();
    let cas = NativeCas::open(&store).unwrap();
    let path = cas.object_path(&object_id).unwrap();
    drop(cas);
    drop(store);
    let bytes = fs::read(&path).unwrap();
    fs::write(&path, vec![b'x'; bytes.len()]).unwrap();
    assert_eq!(
        repository.apply_batch(&reapply).unwrap_err().code(),
        "corruption"
    );
    drop(repository);
    cleanup(&root);
}

#[test]
fn forward_step_reference_child_sorts_before_parent_and_roundtrips() {
    let root = unique_root("forward-step");
    drop(init(&root, StoreId::parse("store:forward-step").unwrap()).unwrap());
    let space = Space::new(
        SpaceId::parse("space:forward-step").unwrap(),
        "Forward",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let child_tree = Tree::new(vec![]).unwrap();
    let (parent_tree, parent, child) = (0..10000)
        .find_map(|n| {
            let parent_tree = Tree::new(vec![TreeEntry::new(
                RepoPathComponent::parse(format!("n{n}")).unwrap(),
                TreeEntryTarget::tree(child_tree.id().clone()),
                false,
            )])
            .unwrap();
            let parent = Step::new(vec![], parent_tree.id().clone()).unwrap();
            let child = Step::new(vec![parent.id().clone()], child_tree.id().clone()).unwrap();
            (child.id() < parent.id()).then_some((parent_tree, parent, child))
        })
        .expect("find lexical forward-reference order");
    let objects = vec![
        CanonicalObject::Tree(child_tree.clone()),
        CanonicalObject::Tree(parent_tree.clone()),
        CanonicalObject::Step(parent.clone()),
        CanonicalObject::Step(child.clone()),
    ];
    let admissions = objects
        .iter()
        .map(|o| SpaceObjectAdmission::new(space.id().clone(), o.object_id()))
        .collect();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:forward-step").unwrap(),
        objects,
        vec![EntityRecord::Space(space.clone())],
        admissions,
        vec![],
    )
    .unwrap();
    let step_ids = batch
        .canonical_objects()
        .iter()
        .filter_map(|o| {
            if let CanonicalObject::Step(v) = o {
                Some(v.id())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(step_ids, vec![child.id(), parent.id()]);
    let mut repository = NativeScmRepository::new(&root);
    repository.apply_batch(&batch).unwrap();
    assert_eq!(
        repository.get_step(parent.id()).unwrap(),
        Some(parent.clone())
    );
    assert!(
        repository
            .is_step_ancestor(parent.id(), child.id())
            .unwrap()
    );
    drop(repository);
    assert_clean_db(&root);
    cleanup(&root);
}
