use layrs_core::scm::{
    LayerHeadSet, LayerId, Metadata, Operation, OperationId, OperationKind, RepositoryView,
    RepositoryViewId, SpaceId, Step, StepId, Timestamp, TreeId,
};
use layrs_graph::scm::{
    OperationDag, ScmGraphError, StepDag, integrate_heads, normalize_heads,
    validate_repository_view,
};

fn oid(seed: u64) -> String {
    format!("blake3:{seed:064x}")
}

fn tree(seed: u64) -> TreeId {
    TreeId::parse(oid(seed)).unwrap()
}

fn step(parents: &[&Step], seed: u64) -> Step {
    Step::new(
        parents.iter().map(|parent| parent.id().clone()).collect(),
        tree(seed),
    )
    .unwrap()
}

fn space(name: &str) -> SpaceId {
    SpaceId::parse(format!("space:{name}")).unwrap()
}

fn operation(
    name: &str,
    space_id: &SpaceId,
    parents: &[&Operation],
    kind: OperationKind,
    view_seed: u64,
) -> Operation {
    operation_with_parent_ids(
        name,
        space_id,
        parents.iter().map(|parent| parent.id().clone()).collect(),
        kind,
        view_seed,
    )
}

fn operation_with_parent_ids(
    name: &str,
    space_id: &SpaceId,
    parent_ids: Vec<OperationId>,
    kind: OperationKind,
    view_seed: u64,
) -> Operation {
    Operation::new(
        OperationId::parse(format!("operation:{name}")).unwrap(),
        space_id.clone(),
        parent_ids,
        RepositoryViewId::parse(oid(view_seed)).unwrap(),
        kind,
        name,
        vec![],
        None,
        None,
        Timestamp::parse("2026-07-19T12:34:56Z").unwrap(),
        Metadata::empty(),
    )
    .unwrap()
}

#[test]
fn step_dag_is_independent_of_batch_and_head_permutations() {
    let root = step(&[], 1);
    let left = step(&[&root], 2);
    let right = step(&[&root], 3);
    let left_tip = step(&[&left], 4);
    let right_tip = step(&[&right], 5);

    let forward = StepDag::from_steps([&root, &left, &right, &left_tip, &right_tip]).unwrap();
    let reverse = StepDag::from_steps([&right_tip, &left_tip, &right, &left, &root]).unwrap();
    let interleaved = StepDag::from_steps([&right, &left_tip, &root, &right_tip, &left]).unwrap();

    let expected = forward.topological().unwrap();
    assert_eq!(reverse.topological().unwrap(), expected);
    assert_eq!(interleaved.topological().unwrap(), expected);
    assert_eq!(
        forward.parents(left_tip.id()).unwrap(),
        vec![left.id().clone()]
    );
    assert_eq!(forward.children(root.id()).unwrap().len(), 2);

    let first_heads = vec![left_tip.id().clone(), right_tip.id().clone()];
    let second_heads = vec![right_tip.id().clone(), left_tip.id().clone()];
    assert_eq!(
        forward.merge_bases(&first_heads).unwrap(),
        forward.merge_bases(&second_heads).unwrap()
    );
    assert_eq!(
        forward.parent_closure_topological(&first_heads).unwrap(),
        forward.parent_closure_topological(&second_heads).unwrap()
    );
}

#[test]
fn step_batch_failures_roll_back_after_missing_and_duplicate_objects() {
    let root = step(&[], 10);
    let valid = step(&[&root], 11);
    let absent_parent = step(&[], 99);
    let orphan = step(&[&absent_parent], 12);
    let mut graph = StepDag::from_steps([&root]).unwrap();
    let before = graph.topological().unwrap();

    assert!(matches!(
        graph.insert_batch([&valid, &orphan]),
        Err(ScmGraphError::MissingStep { .. })
    ));
    assert_eq!(graph.topological().unwrap(), before);
    assert!(!graph.contains(valid.id()));

    assert!(matches!(
        graph.insert_batch([&valid, &root]),
        Err(ScmGraphError::DuplicateStep(_))
    ));
    assert_eq!(graph.topological().unwrap(), before);
    assert!(!graph.contains(valid.id()));
}

#[test]
fn deep_step_dag_traversal_is_iterative_and_complete() {
    const DEPTH: usize = 2_048;
    let mut steps = vec![step(&[], 1_000)];
    for offset in 1..DEPTH {
        let next = step(&[steps.last().unwrap()], 1_000 + offset as u64);
        steps.push(next);
    }

    let graph = StepDag::from_steps(steps.iter()).unwrap();
    let root = steps.first().unwrap();
    let tip = steps.last().unwrap();
    assert_eq!(graph.len(), DEPTH);
    assert_eq!(graph.ancestors(tip.id()).unwrap().len(), DEPTH - 1);
    assert_eq!(graph.descendants(root.id()).unwrap().len(), DEPTH - 1);
    assert_eq!(
        graph
            .parent_closure_topological(&[tip.id().clone()])
            .unwrap()
            .len(),
        DEPTH
    );
}

#[test]
fn criss_cross_merge_bases_returns_every_maximal_common_ancestor() {
    let root = step(&[], 20);
    let left_base = step(&[&root], 21);
    let right_base = step(&[&root], 22);
    let left_tip = step(&[&left_base, &right_base], 23);
    let right_tip = step(&[&left_base, &right_base], 24);
    let graph =
        StepDag::from_steps([&right_tip, &left_base, &root, &left_tip, &right_base]).unwrap();

    let mut expected = vec![left_base.id().clone(), right_base.id().clone()];
    expected.sort();
    assert_eq!(
        graph
            .merge_bases(&[left_tip.id().clone(), right_tip.id().clone()])
            .unwrap(),
        expected
    );
    assert_eq!(
        graph
            .merge_bases(&[right_tip.id().clone(), left_tip.id().clone()])
            .unwrap(),
        expected
    );
}

#[test]
fn head_normalization_is_idempotent_and_integration_never_loses_divergence() {
    let root = step(&[], 30);
    let left = step(&[&root], 31);
    let right = step(&[&root], 32);
    let left_tip = step(&[&left], 33);
    let graph = StepDag::from_steps([&left_tip, &right, &root, &left]).unwrap();

    let normalized = normalize_heads(
        &graph,
        &[
            root.id().clone(),
            right.id().clone(),
            left.id().clone(),
            left_tip.id().clone(),
        ],
    )
    .unwrap();
    assert_eq!(
        normalize_heads(&graph, normalized.as_slice()).unwrap(),
        normalized
    );
    assert_eq!(normalized.len(), 2);
    assert!(normalized.as_slice().contains(left_tip.id()));
    assert!(normalized.as_slice().contains(right.id()));

    let current = normalize_heads(&graph, &[left.id().clone()]).unwrap();
    let integrated = integrate_heads(&graph, &current, &[right.id().clone()]).unwrap();
    assert!(integrated.as_slice().contains(left.id()));
    assert!(integrated.as_slice().contains(right.id()));
    assert_eq!(
        integrate_heads(&graph, &integrated, &[right.id().clone()]).unwrap(),
        integrated
    );

    let advanced = integrate_heads(&graph, &integrated, &[left_tip.id().clone()]).unwrap();
    assert!(!advanced.as_slice().contains(left.id()));
    assert!(advanced.as_slice().contains(left_tip.id()));
    assert!(advanced.as_slice().contains(right.id()));
}

#[test]
fn repository_view_validation_checks_each_layer_head_antichain() {
    let root = step(&[], 40);
    let left = step(&[&root], 41);
    let right = step(&[&root], 42);
    let graph = StepDag::from_steps([&right, &root, &left]).unwrap();
    let target_space = space("view-tests");

    let valid = RepositoryView::new(
        target_space.clone(),
        vec![
            LayerHeadSet::new(
                LayerId::parse("layer:feature").unwrap(),
                vec![right.id().clone(), left.id().clone()],
            )
            .unwrap(),
            LayerHeadSet::new(
                LayerId::parse("layer:base").unwrap(),
                vec![root.id().clone()],
            )
            .unwrap(),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    validate_repository_view(&graph, &valid).unwrap();

    let non_antichain = RepositoryView::new(
        target_space.clone(),
        vec![
            LayerHeadSet::new(
                LayerId::parse("layer:invalid").unwrap(),
                vec![root.id().clone(), left.id().clone()],
            )
            .unwrap(),
        ],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        validate_repository_view(&graph, &non_antichain)
            .unwrap_err()
            .code(),
        "layer_heads_not_antichain"
    );

    let missing = StepId::parse(oid(999)).unwrap();
    let missing_head = RepositoryView::new(
        target_space,
        vec![LayerHeadSet::new(LayerId::parse("layer:missing").unwrap(), vec![missing]).unwrap()],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    assert!(matches!(
        validate_repository_view(&graph, &missing_head),
        Err(ScmGraphError::MissingStep { .. })
    ));
}

#[test]
fn operation_forks_integrate_exact_heads_and_are_permutation_deterministic() {
    let target_space = space("operations");
    let root = operation("init", &target_space, &[], OperationKind::Initialize, 50);
    let left = operation(
        "left",
        &target_space,
        &[&root],
        OperationKind::CaptureWorktree,
        51,
    );
    let right = operation(
        "right",
        &target_space,
        &[&root],
        OperationKind::SwitchLayer,
        52,
    );

    let mut graph = OperationDag::from_operations([&right, &root, &left]).unwrap();
    let permuted = OperationDag::from_operations([&left, &right, &root]).unwrap();
    assert_eq!(
        graph.topological().unwrap(),
        permuted.topological().unwrap()
    );
    assert_eq!(graph.heads(), graph.heads_in_space(&target_space));
    assert_eq!(graph.heads_in_space(&target_space).len(), 2);
    assert_eq!(graph.children(root.id()).unwrap().len(), 2);

    let integration = operation(
        "integrate",
        &target_space,
        &[&right, &left],
        OperationKind::IntegrateOperations,
        53,
    );
    let current_heads = graph.heads_in_space(&target_space);
    graph
        .validate_integration_parents(&integration, &current_heads)
        .unwrap();
    assert!(matches!(
        graph.validate_integration_parents(&integration, &[left.id().clone()]),
        Err(ScmGraphError::IntegrationParentsMismatch { .. })
    ));

    graph
        .insert_integration(&integration, &current_heads)
        .unwrap();
    assert_eq!(
        graph.heads_in_space(&target_space),
        vec![integration.id().clone()]
    );
    assert!(graph.is_ancestor(root.id(), integration.id()).unwrap());
    assert_eq!(graph.ancestors(integration.id()).unwrap().len(), 3);
}

#[test]
fn operation_batch_cross_space_missing_and_duplicate_failures_are_atomic() {
    let alpha = space("alpha");
    let root = operation("alpha-init", &alpha, &[], OperationKind::Initialize, 60);
    let valid = operation(
        "alpha-valid",
        &alpha,
        &[&root],
        OperationKind::CaptureWorktree,
        61,
    );
    let beta = space("beta");
    let cross_space = operation(
        "beta-cross",
        &beta,
        &[&root],
        OperationKind::CaptureWorktree,
        62,
    );
    let mut graph = OperationDag::from_operations([&root]).unwrap();
    let before = graph.topological().unwrap();

    assert!(matches!(
        graph.insert_batch([&valid, &cross_space]),
        Err(ScmGraphError::OperationSpaceMismatch { .. })
    ));
    assert_eq!(graph.topological().unwrap(), before);
    assert!(!graph.contains(valid.id()));

    let absent = operation("alpha-absent", &alpha, &[&root], OperationKind::Undo, 63);
    let orphan = operation_with_parent_ids(
        "alpha-orphan",
        &alpha,
        vec![absent.id().clone()],
        OperationKind::Undo,
        64,
    );
    assert!(matches!(
        graph.insert_batch([&valid, &orphan]),
        Err(ScmGraphError::MissingOperation { .. })
    ));
    assert_eq!(graph.topological().unwrap(), before);
    assert!(!graph.contains(valid.id()));

    assert!(matches!(
        graph.insert_batch([&valid, &root]),
        Err(ScmGraphError::DuplicateOperation(_))
    ));
    assert_eq!(graph.topological().unwrap(), before);
    assert!(!graph.contains(valid.id()));
}

#[test]
fn public_integration_api_is_the_only_atomic_exact_head_admission_path() {
    let target_space = space("integration-admission");
    let root = operation(
        "cas-init",
        &target_space,
        &[],
        OperationKind::Initialize,
        70,
    );
    let left = operation("cas-left", &target_space, &[&root], OperationKind::Undo, 71);
    let right = operation(
        "cas-right",
        &target_space,
        &[&root],
        OperationKind::Redo,
        72,
    );
    let third = operation(
        "cas-third",
        &target_space,
        &[&root],
        OperationKind::Restore,
        73,
    );
    let incomplete = operation(
        "cas-incomplete",
        &target_space,
        &[&left, &right],
        OperationKind::IntegrateOperations,
        74,
    );

    assert_eq!(
        OperationDag::from_operations([&root, &left, &right, &third, &incomplete])
            .unwrap_err()
            .code(),
        "integration_requires_dedicated_admission"
    );

    let mut forward_graph = OperationDag::from_operations([&root]).unwrap();
    let second_incomplete = operation(
        "cas-incomplete-second",
        &target_space,
        &[&left, &right],
        OperationKind::IntegrateOperations,
        75,
    );
    let forward_before = forward_graph.topological().unwrap();
    assert_eq!(
        forward_graph
            .insert_batch([&incomplete, &right, &second_incomplete, &left])
            .unwrap_err()
            .code(),
        "integration_requires_dedicated_admission"
    );
    assert_eq!(forward_graph.topological().unwrap(), forward_before);
    assert!(!forward_graph.contains(left.id()));
    assert!(!forward_graph.contains(right.id()));

    let mut graph = OperationDag::from_operations([&root, &left, &right, &third]).unwrap();
    let before = graph.topological().unwrap();
    assert!(matches!(
        graph.insert_integration(&incomplete, &[left.id().clone(), right.id().clone()]),
        Err(ScmGraphError::IntegrationParentsMismatch { .. })
    ));
    assert_eq!(graph.topological().unwrap(), before);
    assert!(matches!(
        graph.insert_integration(
            &incomplete,
            &[
                root.id().clone(),
                left.id().clone(),
                right.id().clone(),
                third.id().clone(),
            ],
        ),
        Err(ScmGraphError::IntegrationParentsMismatch { .. })
    ));
    assert!(matches!(
        graph.insert_integration(
            &incomplete,
            &[
                left.id().clone(),
                left.id().clone(),
                right.id().clone(),
                third.id().clone(),
            ],
        ),
        Err(ScmGraphError::DuplicateIntegrationParent(_))
    ));
    assert_eq!(graph.topological().unwrap(), before);

    let complete = operation(
        "cas-complete",
        &target_space,
        &[&left, &right, &third],
        OperationKind::IntegrateOperations,
        76,
    );
    let heads = graph.heads_in_space(&target_space);
    graph.insert_integration(&complete, &heads).unwrap();
    assert_eq!(
        graph.heads_in_space(&target_space),
        vec![complete.id().clone()]
    );

    let next_left = operation(
        "cas-next-left",
        &target_space,
        &[&complete],
        OperationKind::Undo,
        77,
    );
    let next_right = operation(
        "cas-next-right",
        &target_space,
        &[&complete],
        OperationKind::Redo,
        78,
    );
    graph.insert_batch([&next_right, &next_left]).unwrap();
    let second = operation(
        "cas-second",
        &target_space,
        &[&next_left, &next_right],
        OperationKind::IntegrateOperations,
        79,
    );
    let heads = graph.heads_in_space(&target_space);
    graph.insert_integration(&second, &heads).unwrap();
    assert_eq!(
        graph.heads_in_space(&target_space),
        vec![second.id().clone()]
    );
}
