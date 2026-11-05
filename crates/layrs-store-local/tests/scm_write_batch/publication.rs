use super::*;

#[test]
fn complete_bootstrap_then_release_publication_preserves_pointers_and_is_atomic() {
    let root = unique_root("complete-bootstrap");
    let store_id = StoreId::parse("store:complete").unwrap();
    drop(init(&root, store_id.clone()).unwrap());
    let space = Space::new(
        SpaceId::parse("space:complete").unwrap(),
        "Complete",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let layer = Layer::new(
        LayerId::parse("layer:complete").unwrap(),
        space.id().clone(),
        "Base",
        LayerKind::Base,
        LayerStatus::Active,
        vec![],
        LayerSettings::new(true, 100).unwrap(),
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let tree = Tree::new(vec![]).unwrap();
    let step = Step::new(vec![], tree.id().clone()).unwrap();
    let worktree_id = WorktreeId::parse("worktree:complete").unwrap();
    let view = RepositoryView::new(
        space.id().clone(),
        vec![LayerHeadSet::new(layer.id().clone(), vec![step.id().clone()]).unwrap()],
        vec![],
        vec![],
        vec![WorktreeViewState::new(
            worktree_id.clone(),
            Some(layer.id().clone()),
            Some(step.id().clone()),
            Some(tree.id().clone()),
            WorktreeStatus::Ready,
        )],
    )
    .unwrap();
    let operation = Operation::new(
        OperationId::parse("operation:complete").unwrap(),
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
        worktree_id,
        store_id,
        space.id().clone(),
        operation.id().clone(),
        "C:/complete",
        "fs:complete",
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let objects = vec![
        CanonicalObject::Tree(tree.clone()),
        CanonicalObject::Step(step.clone()),
        CanonicalObject::RepositoryView(view.clone()),
    ];
    let admissions = objects
        .iter()
        .map(|o| SpaceObjectAdmission::new(space.id().clone(), o.object_id()))
        .collect();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:complete").unwrap(),
        objects,
        vec![
            EntityRecord::Space(space.clone()),
            EntityRecord::Layer(layer),
            EntityRecord::Operation(operation.clone()),
            EntityRecord::Worktree(worktree.clone()),
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
    assert_eq!(repository.get_step(step.id()).unwrap(), Some(step.clone()));
    assert_eq!(
        repository.get_worktree(worktree.id()).unwrap(),
        Some(worktree)
    );
    let release = Release::new(
        ReleaseId::parse("release:complete").unwrap(),
        space.id().clone(),
        step.id().clone(),
        "1.0.0",
        "Release",
        "",
        None,
        timestamp(),
        vec![],
        Metadata::empty(),
    )
    .unwrap();
    let publication = ReleasePublication::new(
        ReleasePublicationId::parse("release-publication:complete").unwrap(),
        release.id().clone(),
        space.id().clone(),
        Some(operation.id().clone()),
        None,
        None,
        timestamp(),
        Metadata::empty(),
    );
    let release_batch = WriteBatch::new(
        TransactionId::new("transaction:release").unwrap(),
        vec![],
        vec![
            EntityRecord::Release(release.clone()),
            EntityRecord::ReleasePublication(publication.clone()),
        ],
        vec![],
        vec![],
    )
    .unwrap();
    repository.apply_batch(&release_batch).unwrap();
    assert_eq!(repository.get_release(release.id()).unwrap(), Some(release));
    assert_eq!(
        repository
            .get_release_publication(publication.id())
            .unwrap(),
        Some(publication)
    );
    let current = repository
        .current_repository_view(space.id())
        .unwrap()
        .unwrap();
    assert_eq!(current.generation(), 1);
    assert_eq!(
        repository.operation_heads(space.id()).unwrap(),
        vec![operation.id().clone()]
    );
    drop(repository);
    let failed_release = Release::new(
        ReleaseId::parse("release:failed").unwrap(),
        space.id().clone(),
        step.id().clone(),
        "2.0.0",
        "Failed",
        "",
        None,
        timestamp(),
        vec![],
        Metadata::empty(),
    )
    .unwrap();
    let failed_pub = ReleasePublication::new(
        ReleasePublicationId::parse("release-publication:failed").unwrap(),
        failed_release.id().clone(),
        space.id().clone(),
        Some(operation.id().clone()),
        None,
        None,
        timestamp(),
        Metadata::empty(),
    );
    let failed_batch = WriteBatch::new(
        TransactionId::new("transaction:release-failed").unwrap(),
        vec![],
        vec![
            EntityRecord::Release(failed_release.clone()),
            EntityRecord::ReleasePublication(failed_pub),
        ],
        vec![],
        vec![],
    )
    .unwrap();
    let mut faulted = NativeScmRepository::with_fault_injector(
        &root,
        Arc::new(OneFault::new(
            RepositoryFaultPoint::AfterEntitiesAndProjections,
        )),
    );
    assert!(faulted.apply_batch(&failed_batch).is_err());
    drop(faulted);
    let repository = NativeScmRepository::new(&root);
    assert!(
        repository
            .get_release(failed_release.id())
            .unwrap()
            .is_none()
    );
    drop(repository);
    assert_clean_db(&root);
    cleanup(&root);
}

#[test]
fn stale_view_and_stale_heads_are_classified_before_candidate_metadata() {
    let root = unique_root("stale-cas");
    let initial_batch = initialize(&root, "stalecas");
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
    let view_candidate = Operation::new(
        OperationId::parse("operation:stale-view").unwrap(),
        space.id().clone(),
        vec![initial.id().clone()],
        initial.resulting_view_id().clone(),
        OperationKind::CaptureWorktree,
        "Stale view",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let stale_view = WriteBatch::new(
        TransactionId::new("transaction:stale-view").unwrap(),
        vec![],
        vec![EntityRecord::Operation(view_candidate.clone())],
        vec![],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    space.id().clone(),
                    0,
                    None,
                    initial.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![initial.id().clone()],
                    view_candidate.id().clone(),
                    vec![view_candidate.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    let error = repository.apply_batch(&stale_view).unwrap_err();
    assert_eq!(error.code(), "cas_mismatch");
    assert!(error.to_string().contains("repository_view"));
    assert!(
        repository
            .get_operation(view_candidate.id())
            .unwrap()
            .is_none()
    );
    let fake = OperationId::parse("operation:missing-head").unwrap();
    let heads_candidate = Operation::new(
        OperationId::parse("operation:stale-heads").unwrap(),
        space.id().clone(),
        vec![fake.clone()],
        initial.resulting_view_id().clone(),
        OperationKind::CaptureWorktree,
        "Stale heads",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let stale_heads = WriteBatch::new(
        TransactionId::new("transaction:stale-heads").unwrap(),
        vec![],
        vec![EntityRecord::Operation(heads_candidate.clone())],
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
                    vec![fake],
                    heads_candidate.id().clone(),
                    vec![heads_candidate.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    let error = repository.apply_batch(&stale_heads).unwrap_err();
    assert_eq!(error.code(), "cas_mismatch");
    assert!(error.to_string().contains("operation_heads"));
    assert!(
        repository
            .get_operation(heads_candidate.id())
            .unwrap()
            .is_none()
    );
    drop(repository);
    cleanup(&root);
}

#[test]
fn publication_to_absent_view_is_rejected_without_metadata() {
    let root = unique_root("absent-view");
    let initial_batch = initialize(&root, "absentview");
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
    let absent =
        layrs_core::scm::RepositoryViewId::parse(format!("blake3:{}", "9".repeat(64))).unwrap();
    let candidate = Operation::new(
        OperationId::parse("operation:absent-view").unwrap(),
        space.id().clone(),
        vec![initial.id().clone()],
        absent.clone(),
        OperationKind::CaptureWorktree,
        "Absent",
        vec![],
        None,
        None,
        timestamp(),
        Metadata::empty(),
    )
    .unwrap();
    let batch = WriteBatch::new(
        TransactionId::new("transaction:absent-view").unwrap(),
        vec![],
        vec![EntityRecord::Operation(candidate.clone())],
        vec![],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    space.id().clone(),
                    1,
                    Some(initial.resulting_view_id().clone()),
                    absent,
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(
                    space.id().clone(),
                    vec![initial.id().clone()],
                    candidate.id().clone(),
                    vec![candidate.id().clone()],
                )
                .unwrap(),
            ),
        ],
    )
    .unwrap();
    assert!(repository.apply_batch(&batch).is_err());
    assert!(repository.get_operation(candidate.id()).unwrap().is_none());
    assert_eq!(
        repository
            .current_repository_view(space.id())
            .unwrap()
            .unwrap()
            .generation(),
        1
    );
    drop(repository);
    cleanup(&root);
}
