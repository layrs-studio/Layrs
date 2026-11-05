use super::*;

#[test]
fn repeated_graph_targets_and_multi_layer_view_edges_roundtrip_and_fsck_clean() {
    let root = unique_root("repeated-edges");
    let store_id = StoreId::parse("store:repeated").unwrap();
    drop(init(&root, store_id).unwrap());
    let space = Space::new(
        SpaceId::parse("space:repeated").unwrap(),
        "Repeated",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let layer_a = Layer::new(
        LayerId::parse("layer:repeated-a").unwrap(),
        space.id().clone(),
        "A",
        LayerKind::Base,
        LayerStatus::Active,
        vec![],
        LayerSettings::new(true, 100).unwrap(),
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let layer_b = Layer::new(
        LayerId::parse("layer:repeated-b").unwrap(),
        space.id().clone(),
        "B",
        LayerKind::Exploration,
        LayerStatus::Active,
        vec![],
        LayerSettings::new(true, 100).unwrap(),
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let chunk = Chunk::new(b"same".to_vec()).unwrap();
    let manifest = FileManifest::from_chunks(
        ChunkingDescriptor::single(),
        &[chunk.clone(), chunk.clone()],
    )
    .unwrap();
    let child = Tree::new(vec![]).unwrap();
    let root_tree = Tree::new(vec![
        TreeEntry::new(
            RepoPathComponent::parse("a").unwrap(),
            TreeEntryTarget::tree(child.id().clone()),
            false,
        ),
        TreeEntry::new(
            RepoPathComponent::parse("b").unwrap(),
            TreeEntryTarget::tree(child.id().clone()),
            false,
        ),
        TreeEntry::new(
            RepoPathComponent::parse("file").unwrap(),
            TreeEntryTarget::file(manifest.file_id().clone()),
            false,
        ),
    ])
    .unwrap();
    let step = Step::new(vec![], root_tree.id().clone()).unwrap();
    let view = RepositoryView::new(
        space.id().clone(),
        vec![
            LayerHeadSet::new(layer_a.id().clone(), vec![step.id().clone()]).unwrap(),
            LayerHeadSet::new(layer_b.id().clone(), vec![step.id().clone()]).unwrap(),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let operation = Operation::new(
        OperationId::parse("operation:repeated").unwrap(),
        space.id().clone(),
        vec![],
        view.id().clone(),
        OperationKind::Initialize,
        "Initialize repeated",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let objects = vec![
        CanonicalObject::ChunkRaw(chunk.clone()),
        CanonicalObject::FileManifest(manifest.clone()),
        CanonicalObject::Tree(child.clone()),
        CanonicalObject::Tree(root_tree.clone()),
        CanonicalObject::Step(step.clone()),
        CanonicalObject::RepositoryView(view.clone()),
    ];
    let admissions = objects
        .iter()
        .map(|o| SpaceObjectAdmission::new(space.id().clone(), o.object_id()))
        .collect();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:repeated").unwrap(),
        objects,
        vec![
            EntityRecord::Space(space.clone()),
            EntityRecord::Layer(layer_a),
            EntityRecord::Layer(layer_b),
            EntityRecord::Operation(operation.clone()),
        ],
        admissions,
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![],
                    operation.id().clone(),
                    vec![operation.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    let mut repository = NativeScmRepository::new(&root);
    repository.apply_batch(&batch).unwrap();
    assert_eq!(
        repository.get_file_manifest(manifest.id()).unwrap(),
        Some(manifest)
    );
    assert_eq!(
        repository.get_tree(root_tree.id()).unwrap(),
        Some(root_tree)
    );
    assert_eq!(
        repository.get_repository_view(view.id()).unwrap(),
        Some(view)
    );
    drop(repository);
    let report = fsck_existing(&root, FsckOptions::default()).unwrap();
    assert!(report.is_clean(), "{:#?}", report.findings);
    cleanup(&root);
}

#[test]
fn passive_imports_do_not_move_heads_and_can_share_a_batch_with_publication() {
    let root = unique_root("passive");
    let initial_batch = initialize(&root, "passive");
    let (space, initial) = initial_batch
        .entity_records()
        .iter()
        .fold((None, None), |mut a, e| {
            match e {
                EntityRecord::Space(v) => a.0 = Some(v.clone()),
                EntityRecord::Operation(v) => a.1 = Some(v.clone()),
                _ => {}
            }
            a
        });
    let space = space.unwrap();
    let initial = initial.unwrap();
    let mut repository = NativeScmRepository::new(&root);
    repository.apply_batch(&initial_batch).unwrap();
    let passive = Operation::new(
        OperationId::parse("operation:passive-only").unwrap(),
        space.id().clone(),
        vec![initial.id().clone()],
        initial.resulting_view_id().clone(),
        OperationKind::ReceiveRemoteState,
        "Passive",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let passive_batch = WriteBatch::new(
        TransactionId::new("transaction:passive-only").unwrap(),
        vec![],
        vec![EntityRecord::Operation(passive.clone())],
        vec![],
        vec![],
    )
    .unwrap();
    repository.apply_batch(&passive_batch).unwrap();
    assert_eq!(
        repository.operation_heads(space.id()).unwrap(),
        vec![initial.id().clone()]
    );
    let passive_two = Operation::new(
        OperationId::parse("operation:passive-two").unwrap(),
        space.id().clone(),
        vec![initial.id().clone()],
        initial.resulting_view_id().clone(),
        OperationKind::ReceiveRemoteState,
        "Passive two",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let published = Operation::new(
        OperationId::parse("operation:published").unwrap(),
        space.id().clone(),
        vec![initial.id().clone()],
        initial.resulting_view_id().clone(),
        OperationKind::CaptureWorktree,
        "Published",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let combined = WriteBatch::new(
        TransactionId::new("transaction:combined").unwrap(),
        vec![],
        vec![
            EntityRecord::Operation(passive_two.clone()),
            EntityRecord::Operation(published.clone()),
        ],
        vec![],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    space.id().clone(),
                    1,
                    Some(initial.resulting_view_id().clone()),
                    initial.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![initial.id().clone()],
                    published.id().clone(),
                    vec![published.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    repository.apply_batch(&combined).unwrap();
    assert_eq!(
        repository.operation_heads(space.id()).unwrap(),
        vec![published.id().clone()]
    );
    assert_eq!(
        repository.get_operation(passive_two.id()).unwrap(),
        Some(passive_two)
    );
    drop(repository);
    assert_clean_db(&root);
    cleanup(&root);
}

#[test]
fn exact_immutable_entity_is_idempotent_but_same_id_different_payload_is_corruption() {
    let root = unique_root("immutable");
    let initial = initialize(&root, "immutable");
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
    let mut repository = NativeScmRepository::new(&root);
    repository.apply_batch(&initial).unwrap();
    let exact = WriteBatch::new(
        TransactionId::new("transaction:immutable-exact").unwrap(),
        vec![],
        vec![EntityRecord::Space(space.clone())],
        vec![],
        vec![],
    )
    .unwrap();
    repository.apply_batch(&exact).unwrap();
    let different = Space::new(
        space.id().clone(),
        "Different",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let conflicting = WriteBatch::new(
        TransactionId::new("transaction:immutable-conflict").unwrap(),
        vec![],
        vec![EntityRecord::Space(different)],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        repository.apply_batch(&conflicting).unwrap_err().code(),
        "corruption"
    );
    drop(repository);
    cleanup(&root);
}

#[test]
fn worktree_cas_updates_payload_atomically_and_fault_rolls_everything_back() {
    let root = unique_root("worktree");
    let store_id = StoreId::parse("store:worktree").unwrap();
    drop(init(&root, store_id.clone()).unwrap());
    let space = Space::new(
        SpaceId::parse("space:worktree").unwrap(),
        "Worktree",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let worktree_id = WorktreeId::parse("worktree:test").unwrap();
    let view = RepositoryView::new(
        space.id().clone(),
        vec![],
        vec![],
        vec![],
        vec![WorktreeViewState::new(
            worktree_id.clone(),
            None,
            None,
            None,
            WorktreeStatus::Ready,
        )],
    )
    .unwrap();
    let initial = Operation::new(
        OperationId::parse("operation:worktree-initial").unwrap(),
        space.id().clone(),
        vec![],
        view.id().clone(),
        OperationKind::Initialize,
        "Initialize",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let worktree = Worktree::new(
        worktree_id.clone(),
        store_id,
        space.id().clone(),
        initial.id().clone(),
        "C:/worktree",
        "fs:worktree",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let initial_batch = WriteBatch::new(
        TransactionId::new("transaction:worktree-initial").unwrap(),
        vec![CanonicalObject::RepositoryView(view.clone())],
        vec![
            EntityRecord::Space(space.clone()),
            EntityRecord::Operation(initial.clone()),
            EntityRecord::Worktree(worktree.clone()),
        ],
        vec![SpaceObjectAdmission::new(
            space.id().clone(),
            StoreObjectId::RepositoryView(view.id().clone()),
        )],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(space.id().clone(), 0, None, view.id().clone()).unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![],
                    initial.id().clone(),
                    vec![initial.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    NativeScmRepository::new(&root)
        .apply_batch(&initial_batch)
        .unwrap();
    let next = Operation::new(
        OperationId::parse("operation:worktree-next").unwrap(),
        space.id().clone(),
        vec![initial.id().clone()],
        view.id().clone(),
        OperationKind::CaptureWorktree,
        "Next",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let next_batch = WriteBatch::new(
        TransactionId::new("transaction:worktree-next").unwrap(),
        vec![],
        vec![EntityRecord::Operation(next.clone())],
        vec![],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    space.id().clone(),
                    1,
                    Some(view.id().clone()),
                    view.id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![initial.id().clone()],
                    next.id().clone(),
                    vec![next.id().clone()],
                )
                .unwrap(),
            ),
            StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(
                    worktree_id.clone(),
                    initial.id().clone(),
                    next.id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    let mut faulted = NativeScmRepository::with_fault_injector(
        &root,
        Arc::new(OneFault::new(RepositoryFaultPoint::AfterWorktreeCas)),
    );
    assert!(faulted.apply_batch(&next_batch).is_err());
    drop(faulted);
    let mut repository = NativeScmRepository::new(&root);
    assert_eq!(
        repository.get_worktree(&worktree_id).unwrap(),
        Some(worktree)
    );
    assert!(repository.get_operation(next.id()).unwrap().is_none());
    repository.apply_batch(&next_batch).unwrap();
    let updated = repository.get_worktree(&worktree_id).unwrap().unwrap();
    assert_eq!(updated.active_operation_id(), next.id());
    let third = Operation::new(
        OperationId::parse("operation:worktree-third").unwrap(),
        space.id().clone(),
        vec![initial.id().clone()],
        view.id().clone(),
        OperationKind::CaptureWorktree,
        "Third",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let stale = WriteBatch::new(
        TransactionId::new("transaction:worktree-stale").unwrap(),
        vec![],
        vec![EntityRecord::Operation(third.clone())],
        vec![],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    space.id().clone(),
                    2,
                    Some(view.id().clone()),
                    view.id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![next.id().clone()],
                    third.id().clone(),
                    vec![next.id().clone(), third.id().clone()],
                )
                .unwrap(),
            ),
            StoreMutation::WorktreeActiveOperation(
                WorktreeActiveOperationCas::new(
                    worktree_id.clone(),
                    initial.id().clone(),
                    third.id().clone(),
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    // The parent matches the stale own-Worktree expectation; the current
    // global frontier preserves `next`. Only the durable own-active CAS fails.
    let error = repository.apply_batch(&stale).unwrap_err();
    assert_eq!(error.code(), "cas_mismatch");
    assert!(
        matches!(&error, layrs_store::scm::StoreError::CasMismatch { target }
        if target == &format!("worktree_active_operation:{worktree_id}")),
        "{error:?}"
    );
    assert!(repository.get_operation(third.id()).unwrap().is_none());
    assert_eq!(
        repository
            .current_repository_view(space.id())
            .unwrap()
            .unwrap()
            .generation(),
        2
    );
    assert_eq!(
        repository.operation_heads(space.id()).unwrap(),
        vec![next.id().clone()]
    );
    assert_eq!(
        repository
            .get_worktree(&worktree_id)
            .unwrap()
            .unwrap()
            .active_operation_id(),
        next.id()
    );
    drop(repository);
    assert_clean_db(&root);
    cleanup(&root);
}

#[test]
fn integration_uses_the_complete_seeded_two_head_frontier() {
    let root = unique_root("integrate");
    let initial_batch = initialize(&root, "integrate");
    let space = initial_batch
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
    let initial = initial_batch
        .entity_records()
        .iter()
        .find_map(|e| {
            if let EntityRecord::Operation(v) = e {
                Some(v.clone())
            } else {
                None
            }
        })
        .unwrap();
    let mut repository = NativeScmRepository::new(&root);
    repository.apply_batch(&initial_batch).unwrap();
    let make_branch = |name: &str| {
        Operation::new(
            OperationId::parse(format!("operation:integrate-{name}")).unwrap(),
            space.id().clone(),
            vec![initial.id().clone()],
            initial.resulting_view_id().clone(),
            OperationKind::ReceiveRemoteState,
            name,
            vec![],
            None,
            None,
            timestamp(),
            Metadata::empty(),
        )
        .unwrap()
    };
    let left = make_branch("left");
    let right = make_branch("right");
    let passive = WriteBatch::new(
        TransactionId::new("transaction:integrate-branches").unwrap(),
        vec![],
        vec![
            EntityRecord::Operation(left.clone()),
            EntityRecord::Operation(right.clone()),
        ],
        vec![],
        vec![],
    )
    .unwrap();
    repository.apply_batch(&passive).unwrap();
    drop(repository);
    let connection = Connection::open(root.join("metadata/layrs.sqlite3")).unwrap();
    connection
        .execute(
            "DELETE FROM operation_heads WHERE space_id=?1",
            [space.id().as_str()],
        )
        .unwrap();
    for (i, op) in [&left, &right].into_iter().enumerate() {
        connection.execute("INSERT INTO operation_heads(space_id,operation_id,ordinal,updated_by_transaction_id) VALUES(?1,?2,?3,'seed:test')",rusqlite::params![space.id().as_str(),op.id().as_str(),i]).unwrap();
    }
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(connection);
    let integration = Operation::new(
        OperationId::parse("operation:integrate-final").unwrap(),
        space.id().clone(),
        vec![left.id().clone(), right.id().clone()],
        initial.resulting_view_id().clone(),
        OperationKind::IntegrateOperations,
        "Integrate",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:integrate-final").unwrap(),
        vec![],
        vec![EntityRecord::Operation(integration.clone())],
        vec![],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    space.id().clone(),
                    1,
                    Some(initial.resulting_view_id().clone()),
                    initial.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![left.id().clone(), right.id().clone()],
                    integration.id().clone(),
                    vec![integration.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    let mut repository = NativeScmRepository::new(&root);
    repository.apply_batch(&batch).unwrap();
    assert_eq!(
        repository.operation_heads(space.id()).unwrap(),
        vec![integration.id().clone()]
    );
    drop(repository);
    assert_clean_db(&root);
    cleanup(&root);
}
