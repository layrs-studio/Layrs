use super::*;

#[test]
fn composite_overlay_accepts_forward_step_references_and_rejects_missing_closure() {
    let root = unique_root("overlay-forward");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let tree = Tree::new(vec![]).unwrap();
    let parent = Step::new(vec![], tree.id().clone()).unwrap();
    let child = Step::new(vec![parent.id().clone()], tree.id().clone()).unwrap();
    let batch = WriteBatch::new(
        transaction("forward"),
        vec![
            CanonicalObject::Step(child),
            CanonicalObject::Tree(tree.clone()),
            CanonicalObject::Step(parent),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let resolver = CompositeAdmissionResolver::new(&reader, &batch).unwrap();
    resolver.validate_batch().unwrap();

    let missing = Step::new(
        vec![StepId::parse(format!("blake3:{}", "f".repeat(64))).unwrap()],
        tree.id().clone(),
    )
    .unwrap();
    let missing_batch = WriteBatch::new(
        transaction("missing"),
        vec![CanonicalObject::Tree(tree), CanonicalObject::Step(missing)],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &missing_batch)
            .err()
            .unwrap()
            .code(),
        "not_found"
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn operation_reachability_caches_bounded_closure_for_thousands_of_observations() {
    use layrs_core::scm::AdmissionResolver;
    let root = unique_root("operation-reachability-scale");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let tree = Tree::new(vec![]).unwrap();
    let mut objects = vec![CanonicalObject::Tree(tree.clone())];
    let mut steps = Vec::new();
    for _ in 0..5000 {
        let step = Step::new(
            steps.last().cloned().into_iter().collect(),
            tree.id().clone(),
        )
        .unwrap();
        steps.push(step.id().clone());
        objects.push(CanonicalObject::Step(step));
    }
    let view = RepositoryView::new(
        fixture.space.id().clone(),
        vec![
            LayerHeadSet::new(
                fixture.layer.id().clone(),
                vec![steps.last().unwrap().clone()],
            )
            .unwrap(),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let operation = Operation::new(
        OperationId::parse("operation:scale-observations").unwrap(),
        fixture.space.id().clone(),
        vec![fixture.operation.id().clone()],
        view.id().clone(),
        OperationKind::ReceiveRemoteState,
        "import",
        vec![],
        None,
        None,
        Timestamp::parse("2026-07-19T12:01:00Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap();
    objects.push(CanonicalObject::RepositoryView(view));
    let batch = WriteBatch::new(
        transaction("scale-observations"),
        objects,
        vec![EntityRecord::Operation(operation.clone())],
        vec![],
        vec![],
    )
    .unwrap();
    let resolver = CompositeAdmissionResolver::new(&reader, &batch).unwrap();
    for step in &steps {
        assert!(resolver.operation_reaches_step(operation.id(), step));
    }
    assert!(!resolver.operation_reaches_step(operation.id(), fixture.observation.step_id()));
    assert!(!resolver.operation_reaches_step(fixture.operation.id(), steps.last().unwrap()));
    drop(resolver);
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn admissions_require_real_objects_and_complete_overlay_durable_closure() {
    let root = unique_root("admission-closure");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();

    let missing_step = StepId::parse(format!("blake3:{}", "e".repeat(64))).unwrap();
    let admission_only = WriteBatch::new(
        transaction("admission-only"),
        vec![],
        vec![],
        vec![SpaceObjectAdmission::new(
            fixture.space.id().clone(),
            StoreObjectId::Step(missing_step),
        )],
        vec![],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &admission_only)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );

    let tree = Tree::new(vec![]).unwrap();
    let parent = Step::new(vec![], tree.id().clone()).unwrap();
    let child = Step::new(vec![parent.id().clone()], tree.id().clone()).unwrap();
    let closed_forward = WriteBatch::new(
        transaction("admission-forward-closed"),
        vec![
            CanonicalObject::Step(child.clone()),
            CanonicalObject::Tree(tree.clone()),
            CanonicalObject::Step(parent.clone()),
        ],
        vec![],
        vec![
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::Step(child.id().clone()),
            ),
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::Tree(tree.id().clone()),
            ),
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::Step(parent.id().clone()),
            ),
        ],
        vec![],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &closed_forward)
        .unwrap()
        .validate_batch()
        .unwrap();

    let incomplete = WriteBatch::new(
        transaction("admission-forward-incomplete"),
        vec![
            CanonicalObject::Step(child),
            CanonicalObject::Tree(tree),
            CanonicalObject::Step(parent),
        ],
        vec![],
        vec![SpaceObjectAdmission::new(
            fixture.space.id().clone(),
            StoreObjectId::Step(
                closed_forward
                    .canonical_objects()
                    .iter()
                    .find_map(|object| match object {
                        CanonicalObject::Step(value) if !value.parent_step_ids().is_empty() => {
                            Some(value.id().clone())
                        }
                        _ => None,
                    })
                    .unwrap(),
            ),
        )],
        vec![],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &incomplete)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );

    let durable_step = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::Step(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let durable_child = Step::new(
        vec![durable_step.id().clone()],
        durable_step.root_tree_id().clone(),
    )
    .unwrap();
    let mixed = WriteBatch::new(
        transaction("admission-overlay-durable"),
        vec![CanonicalObject::Step(durable_child.clone())],
        vec![],
        vec![SpaceObjectAdmission::new(
            fixture.space.id().clone(),
            StoreObjectId::Step(durable_child.id().clone()),
        )],
        vec![],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &mixed)
        .unwrap()
        .validate_batch()
        .unwrap();

    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn composite_validation_rejects_cross_space_antichain_and_incomplete_rewrite() {
    let root = unique_root("overlay-invalid");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let timestamp = Timestamp::parse("2026-07-19T12:00:00Z").unwrap();

    let foreign_layer = Layer::new(
        LayerId::parse("layer:foreign").unwrap(),
        SpaceId::parse("space:missing").unwrap(),
        "foreign",
        LayerKind::Base,
        LayerStatus::Active,
        vec![],
        LayerSettings::new(true, 1).unwrap(),
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let cross_space = WriteBatch::new(
        transaction("cross-space"),
        vec![],
        vec![EntityRecord::Layer(foreign_layer)],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &cross_space)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );

    let tree = Tree::new(vec![]).unwrap();
    let parent = Step::new(vec![], tree.id().clone()).unwrap();
    let child = Step::new(vec![parent.id().clone()], tree.id().clone()).unwrap();
    let view = RepositoryView::new(
        fixture.space.id().clone(),
        vec![
            LayerHeadSet::new(
                fixture.layer.id().clone(),
                vec![parent.id().clone(), child.id().clone()],
            )
            .unwrap(),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let antichain = WriteBatch::new(
        transaction("antichain"),
        vec![
            CanonicalObject::Tree(tree),
            CanonicalObject::Step(parent.clone()),
            CanonicalObject::Step(child.clone()),
            CanonicalObject::RepositoryView(view),
        ],
        vec![],
        vec![
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::Step(parent.id().clone()),
            ),
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::Step(child.id().clone()),
            ),
        ],
        vec![],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &antichain)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );

    let result_view = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::RepositoryView(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap();
    let initialize = Operation::new(
        OperationId::parse("operation:overlay-init").unwrap(),
        fixture.space.id().clone(),
        vec![],
        result_view.id().clone(),
        OperationKind::Initialize,
        "init",
        vec![],
        None,
        None,
        timestamp.clone(),
        Metadata::empty(),
    )
    .unwrap();
    let rewrite = RewriteTarget::new(parent.id().clone(), vec![child.id().clone()]).unwrap();
    let rewrite_operation = Operation::new(
        OperationId::parse("operation:overlay-rewrite").unwrap(),
        fixture.space.id().clone(),
        vec![initialize.id().clone()],
        result_view.id().clone(),
        OperationKind::RewriteSteps,
        "rewrite",
        vec![rewrite],
        None,
        None,
        timestamp,
        Metadata::empty(),
    )
    .unwrap();
    let rewrite_batch = WriteBatch::new(
        transaction("rewrite"),
        vec![
            CanonicalObject::Step(parent.clone()),
            CanonicalObject::Step(child),
        ],
        vec![
            EntityRecord::Operation(rewrite_operation),
            EntityRecord::Operation(initialize),
        ],
        vec![SpaceObjectAdmission::new(
            fixture.space.id().clone(),
            StoreObjectId::Step(parent.id().clone()),
        )],
        vec![],
    )
    .unwrap();
    let rewrite_error = match CompositeAdmissionResolver::new(&reader, &rewrite_batch) {
        Ok(resolver) => resolver.validate_batch().unwrap_err(),
        Err(error) => error,
    };
    assert_eq!(rewrite_error.code(), "admission_rejected");
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn tree_file_closure_selects_a_same_space_manifest_with_admitted_chunks() {
    let root = unique_root("file-space-closure");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();

    let full = Chunk::new(b"abcdef".to_vec()).unwrap();
    let left = Chunk::new(b"abc".to_vec()).unwrap();
    let right = Chunk::new(b"def".to_vec()).unwrap();
    let single =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&full))
            .unwrap();
    let split = FileManifest::from_chunks(
        ChunkingDescriptor::deterministic_fixed(3).unwrap(),
        &[left.clone(), right.clone()],
    )
    .unwrap();
    assert_eq!(single.file_id(), split.file_id());
    assert_ne!(single.id(), split.id());
    let tree = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("file.txt").unwrap(),
        TreeEntryTarget::file(single.file_id().clone()),
        false,
    )])
    .unwrap();
    let foreign = Space::new(
        SpaceId::parse("space:file-foreign").unwrap(),
        "Foreign",
        Timestamp::parse("2026-07-19T12:00:00Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap();
    let objects = vec![
        CanonicalObject::ChunkRaw(full.clone()),
        CanonicalObject::ChunkRaw(left.clone()),
        CanonicalObject::ChunkRaw(right.clone()),
        CanonicalObject::FileManifest(single.clone()),
        CanonicalObject::FileManifest(split.clone()),
        CanonicalObject::Tree(tree.clone()),
    ];
    let foreign_only = WriteBatch::new(
        transaction("file-foreign-only"),
        objects.clone(),
        vec![EntityRecord::Space(foreign.clone())],
        vec![
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::Tree(tree.id().clone()),
            ),
            SpaceObjectAdmission::new(
                foreign.id().clone(),
                StoreObjectId::FileManifest(split.id().clone()),
            ),
            SpaceObjectAdmission::new(
                foreign.id().clone(),
                StoreObjectId::ChunkRaw(left.id().clone()),
            ),
            SpaceObjectAdmission::new(
                foreign.id().clone(),
                StoreObjectId::ChunkRaw(right.id().clone()),
            ),
        ],
        vec![],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &foreign_only)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );

    let closed = WriteBatch::new(
        transaction("file-same-space-closed"),
        objects,
        vec![EntityRecord::Space(foreign)],
        vec![
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::Tree(tree.id().clone()),
            ),
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::FileManifest(single.id().clone()),
            ),
            SpaceObjectAdmission::new(
                fixture.space.id().clone(),
                StoreObjectId::ChunkRaw(full.id().clone()),
            ),
            SpaceObjectAdmission::new(
                SpaceId::parse("space:file-foreign").unwrap(),
                StoreObjectId::FileManifest(split.id().clone()),
            ),
            SpaceObjectAdmission::new(
                SpaceId::parse("space:file-foreign").unwrap(),
                StoreObjectId::ChunkRaw(left.id().clone()),
            ),
            SpaceObjectAdmission::new(
                SpaceId::parse("space:file-foreign").unwrap(),
                StoreObjectId::ChunkRaw(right.id().clone()),
            ),
        ],
        vec![],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &closed)
        .unwrap()
        .validate_batch()
        .unwrap();
    drop(reader);
    drop(store);
    cleanup(&root);
}

#[test]
fn every_mutation_target_must_exist_in_and_be_admitted_to_its_space() {
    let root = unique_root("mutation-closure");
    let fixture = fixture();
    seed(&root, &fixture);
    let store = open(&root).unwrap();
    let reader = NativeScmReader::new(&store).unwrap();
    let step = fixture
        .objects
        .iter()
        .find_map(|object| match object {
            CanonicalObject::Step(value) => Some(value),
            _ => None,
        })
        .unwrap();
    let current_view = fixture.operation.resulting_view_id().clone();
    let candidate_view = RepositoryView::new(
        fixture.space.id().clone(),
        vec![LayerHeadSet::new(fixture.layer.id().clone(), vec![step.id().clone()]).unwrap()],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let next_operation = Operation::new(
        OperationId::parse("operation:mutation-next").unwrap(),
        fixture.space.id().clone(),
        vec![fixture.operation.id().clone()],
        candidate_view.id().clone(),
        OperationKind::CaptureWorktree,
        "next",
        vec![],
        None,
        None,
        Timestamp::parse("2026-07-19T12:01:00Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap();
    let heads = StoreMutation::OperationHeads(
        OperationHeadsCas::new(
            fixture.space.id().clone(),
            vec![fixture.operation.id().clone()],
            next_operation.id().clone(),
            vec![next_operation.id().clone()],
        )
        .unwrap(),
    );
    let view_cas = StoreMutation::RepositoryView(
        RepositoryViewCas::new(
            fixture.space.id().clone(),
            1,
            Some(current_view),
            candidate_view.id().clone(),
        )
        .unwrap(),
    );
    let orphan_result = WriteBatch::new(
        transaction("mutation-orphan-result"),
        vec![CanonicalObject::RepositoryView(candidate_view.clone())],
        vec![EntityRecord::Operation(next_operation.clone())],
        vec![],
        vec![heads.clone(), view_cas.clone()],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &orphan_result)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );

    let admitted_result = WriteBatch::new(
        transaction("mutation-admitted-result"),
        vec![CanonicalObject::RepositoryView(candidate_view.clone())],
        vec![EntityRecord::Operation(next_operation)],
        vec![SpaceObjectAdmission::new(
            fixture.space.id().clone(),
            StoreObjectId::RepositoryView(candidate_view.id().clone()),
        )],
        vec![heads, view_cas],
    )
    .unwrap();
    CompositeAdmissionResolver::new(&reader, &admitted_result)
        .unwrap()
        .validate_batch()
        .unwrap();

    let missing_operation = OperationId::parse("operation:mutation-missing").unwrap();
    let worktree_cas = WriteBatch::new(
        transaction("mutation-missing-operation"),
        vec![],
        vec![],
        vec![],
        vec![StoreMutation::WorktreeActiveOperation(
            WorktreeActiveOperationCas::new(
                fixture.worktree.id().clone(),
                fixture.operation.id().clone(),
                missing_operation,
            )
            .unwrap(),
        )],
    )
    .unwrap();
    assert_eq!(
        CompositeAdmissionResolver::new(&reader, &worktree_cas)
            .unwrap()
            .validate_batch()
            .unwrap_err()
            .code(),
        "admission_rejected"
    );
    drop(reader);
    drop(store);
    cleanup(&root);
}
