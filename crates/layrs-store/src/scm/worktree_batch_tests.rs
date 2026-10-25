use super::*;
use layrs_core::scm::WorktreeId;

#[test]
fn omitted_registration_binding_preserves_original_payload_and_digest() {
    let space = space_id("legacy-digest");
    let op = operation(
        "legacy-digest",
        &space,
        vec![],
        OperationKind::Initialize,
        192,
    );
    let heads = OperationHeadsCas::new(
        space.clone(),
        vec![],
        op.id().clone(),
        vec![op.id().clone()],
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&heads).unwrap(),
        serde_json::json!({
            "spaceId": space, "expectedHeadIds": [], "newOperationId": op.id(), "newHeadIds": [op.id()],
        })
    );
    let batch = WriteBatch::new(
        transaction("legacy-digest"),
        vec![],
        vec![EntityRecord::Operation(op.clone())],
        vec![],
        vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(space, 0, None, op.resulting_view_id().clone()).unwrap(),
            ),
            StoreMutation::OperationHeads(heads),
        ],
    )
    .unwrap();
    let payload = serde_json::json!({ "schemaVersion": "layrs.store-write-batch.v1", "canonicalObjects": [],
        "entityRecords": batch.entity_records(), "admissions": [], "mutations": batch.mutations() });
    assert_eq!(
        batch.request_digest(),
        RequestDigest::compute(&layrs_core::scm::canonical_json(&payload).unwrap())
    );
}

#[test]
fn retained_parent_requires_a_worktree_claim_but_still_derives_exact_frontier() {
    let space = space_id("retained-parent");
    let parent = operation_id("retained-parent");
    let head = operation_id("current-head");
    let op = operation(
        "retained-child",
        &space,
        vec![parent.clone()],
        OperationKind::CaptureWorktree,
        193,
    );
    let batch = |targets: Vec<StoreMutation>, next: Vec<OperationId>| {
        let mut mutations = vec![
            StoreMutation::RepositoryView(
                RepositoryViewCas::new(
                    space.clone(),
                    2,
                    Some(view_id(192)),
                    op.resulting_view_id().clone(),
                )
                .unwrap(),
            ),
            StoreMutation::OperationHeads(
                OperationHeadsCas::new(space.clone(), vec![head.clone()], op.id().clone(), next)
                    .unwrap(),
            ),
        ];
        mutations.extend(targets);
        WriteBatch::new(
            transaction("retained-child"),
            vec![],
            vec![EntityRecord::Operation(op.clone())],
            vec![],
            mutations,
        )
    };
    assert!(batch(vec![], sorted_ids([head.clone(), op.id().clone()])).is_err());
    let binding = StoreMutation::WorktreeActiveOperation(
        WorktreeActiveOperationCas::new(
            WorktreeId::parse("worktree:retained").unwrap(),
            parent,
            op.id().clone(),
        )
        .unwrap(),
    );
    assert!(
        batch(vec![binding.clone()], vec![op.id().clone()]).is_err(),
        "cannot drop an unrelated head"
    );
    batch(vec![binding], sorted_ids([head.clone(), op.id().clone()])).unwrap();
}
