use super::*;
use layrs_store_server::v3::{load_space_descriptor, read_canonical_object};
use layrs_weave::{
    graph_merge::{GraphMergeLimits, MergeObjects, plan_structural_merge},
    reconciliation::{FailureAction, FileMergeLimits, FileMergePolicy},
};
use sqlx::Row;

#[path = "review_interference.rs"]
mod review_interference;

async fn bytes(repository: &PgV3Repository, space: &SpaceId, id: &str) -> Vec<u8> {
    let descriptor = load_space_descriptor(repository.pool(), space, &object_id(id))
        .await
        .unwrap();
    read_canonical_object(repository.cas().as_ref(), &descriptor, 1024 * 1024)
        .await
        .unwrap()
}

async fn inputs(repository: &PgV3Repository, space: &SpaceId) -> MergeObjects {
    let mut objects = MergeObjects::default();
    let steps:Vec<String>=sqlx::query_scalar("SELECT st.canonical_record_jcs FROM steps st JOIN space_steps ss USING(step_id) WHERE ss.space_id=$1 LIMIT 101")
        .bind(space.as_str()).fetch_all(repository.pool()).await.unwrap();
    assert!(steps.len() < 101);
    for raw in steps {
        let step: Step = serde_json::from_str(&raw).unwrap();
        objects.steps.insert(step.id().clone(), step);
    }
    let rows=sqlx::query("SELECT o.object_id,o.object_kind FROM canonical_objects o JOIN space_object_availability a USING(object_id) WHERE a.space_id=$1 AND a.availability_state='available' AND o.object_kind IN ('tree','file_manifest') LIMIT 101")
        .bind(space.as_str()).fetch_all(repository.pool()).await.unwrap();
    assert!(rows.len() < 101);
    for row in rows {
        let raw = bytes(repository, space, &row.get::<String, _>("object_id")).await;
        if row.get::<String, _>("object_kind") == "tree" {
            let tree: Tree = serde_json::from_slice(&raw).unwrap();
            objects.trees.insert(tree.id().clone(), tree);
        } else {
            let manifest: FileManifest = serde_json::from_slice(&raw).unwrap();
            let mut content = Vec::new();
            for chunk in manifest.chunks() {
                content.extend(bytes(repository, space, chunk.chunk_id().as_str()).await);
            }
            objects.files.insert(manifest.file_id().clone(), content);
        }
    }
    objects
}

async fn approve(app: &Router, path: &str, reviewer: &str, version: u64, key: &str) {
    ok(app,"POST",&format!("{path}/actions"),reviewer,json!({"idempotencyKey":key,"version":version,"action":"approve","body":"Reviewed exact recomputed result","anchor":null,"replyTo":null})).await;
}
async fn prove(
    app: &Router,
    base: &str,
    runner: &str,
    step: &Step,
    recipe: &Value,
    key: &str,
) -> ProductEnvelope {
    let run = ok(
        app,
        "POST",
        &format!("{base}/runs"),
        runner,
        json!({"idempotencyKey":key,"stepId":step.id(),"recipeObjectId":recipe}),
    )
    .await;
    ok(
        app,
        "POST",
        &format!("{base}/runs/{}/complete", run["runId"].as_str().unwrap()),
        runner,
        json!({"outcome":"passed","log":"review target advance exact Step fixture"}),
    )
    .await;
    let detail = ok(
        app,
        "GET",
        &format!("{base}/runs/{}", run["runId"].as_str().unwrap()),
        runner,
        Value::Null,
    )
    .await;
    assert_eq!(detail["stepId"], step.id().as_str());
    let product: ProductEnvelope = serde_json::from_value(detail["product"].clone()).unwrap();
    product.verify_identity().unwrap();
    let ProductEnvelope::Proof {
        record,
        depends_on_step_ids,
        ..
    } = &product
    else {
        panic!("completed Run must expose its immutable Proof");
    };
    assert_eq!(
        record.subject,
        ImmutableSubject::Step {
            object_id: object_id(step.id().as_str())
        }
    );
    assert_eq!(depends_on_step_ids, &vec![step.id().clone()]);
    assert_eq!(record.recipe_object_id.as_str(), recipe.as_str().unwrap());
    assert_eq!(detail["evidence"][0]["stepId"], step.id().as_str());
    assert_eq!(&detail["evidence"][0]["recipeObjectId"], recipe);
    assert_eq!(detail["evidence"][0]["outcome"], "passed");
    product
}
async fn admit(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    bootstrap: &SpaceBootstrap,
    step: &Step,
    key: &str,
) {
    let mut value = request(bootstrap, step, key, LayerMutationMode::Advance);
    value.view_mutation = None;
    value.request_digest = value.computed_request_digest().unwrap();
    assert_eq!(
        facade
            .transact(principal, value)
            .await
            .unwrap()
            .receipt()
            .status,
        ReceiptStatus::Applied
    );
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: &Router,
    base: &str,
    admin: &str,
    reviewer: &str,
    runner: &str,
    repository: &PgV3Repository,
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    recipe: &Value,
) {
    let before = facade.bootstrap(principal, space).await.unwrap();
    let layer = before.layers[0].record.id();
    let head = before.layers[0].head_step_ids[0].clone();
    let objects = inputs(repository, space).await;
    let proposed = Step::new(
        vec![head.clone()],
        objects.steps[&head].root_tree_id().clone(),
    )
    .unwrap();
    admit(facade, principal, &before, &proposed, "target-old-admit").await;
    let old=ok(app,"POST",&format!("{base}/reviews"),admin,json!({"idempotencyKey":"target-old-proposal-001","layerId":layer,"proposedStepId":proposed.id(),"title":"Proposal before another integration","participants":[],"expectedVersion":0})).await;
    let old_path = format!("{base}/reviews/{}", old["reviewId"].as_str().unwrap());
    approve(app, &old_path, reviewer, 1, "target-old-approval-001").await;
    prove(app, base, runner, &proposed, recipe, "target-old-proof-001").await;
    let comment=ok(app,"POST",&format!("{old_path}/actions"),reviewer,json!({"idempotencyKey":"target-old-comment-001","version":1,"action":"comment","body":"Keep this original line anchor","anchor":{"path":"docs/readme.txt","startLine":1,"endLine":1},"replyTo":null})).await;
    // A separately reviewed integration deletes the file while the first review waits.
    let empty = Tree::new(vec![]).unwrap();
    if load_space_descriptor(repository.pool(), space, &object_id(empty.id().as_str()))
        .await
        .is_err()
    {
        upload(
            facade,
            principal,
            space,
            ObjectKind::Tree,
            empty.id().as_str(),
            serde_jcs::to_vec(&empty).unwrap(),
            "target-empty-tree",
        )
        .await;
    }
    let moved = Step::new(vec![head.clone()], empty.id().clone()).unwrap();
    admit(facade, principal, &before, &moved, "target-moved-admit").await;
    let competing=ok(app,"POST",&format!("{base}/reviews"),admin,json!({"idempotencyKey":"target-competing-proposal","layerId":layer,"proposedStepId":moved.id(),"title":"Independent deletion","participants":[],"expectedVersion":0})).await;
    let competing_path = format!("{base}/reviews/{}", competing["reviewId"].as_str().unwrap());
    approve(
        app,
        &competing_path,
        reviewer,
        1,
        "target-competing-approve",
    )
    .await;
    prove(app, base, runner, &moved, recipe, "target-competing-proof").await;
    assert_eq!(
        facade
            .transact(
                principal,
                request(
                    &before,
                    &moved,
                    "target-competing-advance",
                    LayerMutationMode::Advance
                )
            )
            .await
            .unwrap()
            .receipt()
            .status,
        ReceiptStatus::Applied
    );
    let stale = ok(app, "GET", &old_path, admin, Value::Null).await;
    assert_eq!(stale["decisionState"]["state"], "stale");
    assert_eq!(stale["decisionState"]["allowsIntegration"], false);
    let refused = facade
        .transact(
            principal,
            request(
                &before,
                &proposed,
                "target-stale-advance",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(refused.receipt().status, ReceiptStatus::Rejected);
    // Invoke the real structural planner on admitted content; no hand-written merge Step.
    let objects = inputs(repository, space).await;
    let plan = plan_structural_merge(
        &objects,
        std::slice::from_ref(proposed.id()),
        std::slice::from_ref(moved.id()),
        FileMergePolicy {
            unsupported: FailureAction::StructuralFallback,
            quota: FailureAction::Reject,
            limits: FileMergeLimits::default(),
        },
        GraphMergeLimits::default(),
    )
    .unwrap();
    assert!(plan.conflicts().is_empty());
    assert_eq!(plan.merge_bases(), &[head]);
    assert_eq!(plan.step().root_tree_id(), empty.id());
    assert_ne!(plan.step().id(), proposed.id());
    assert_ne!(plan.step().id(), moved.id());
    let mut combined_parents = vec![proposed.id().clone(), moved.id().clone()];
    combined_parents.sort();
    assert_eq!(plan.step().parent_step_ids(), combined_parents);
    let current = facade.bootstrap(principal, space).await.unwrap();
    admit(
        facade,
        principal,
        &current,
        plan.step(),
        "target-recomputed-admit",
    )
    .await;
    let gate_path = format!("{base}/layers/{layer}/gates/evaluate");
    let review_required = ok(
        app,
        "POST",
        &gate_path,
        admin,
        json!({"stepId":plan.step().id()}),
    )
    .await;
    assert_eq!(review_required["outcome"], "failed");
    assert_eq!(
        review_required["rejection"]["message"],
        "current Step, target View and policy require a review"
    );
    let newer=ok(app,"POST",&format!("{old_path}/versions"),admin,json!({"idempotencyKey":"target-recomputed-version","layerId":layer,"proposedStepId":plan.step().id(),"title":"Recomputed after target advanced","participants":[],"expectedVersion":1})).await;
    assert_eq!(newer["version"], 2);
    let detail = ok(app, "GET", &old_path, admin, Value::Null).await;
    assert_eq!(detail["versions"][0], old);
    assert_eq!(detail["decisionState"]["state"], "pending");
    assert!(detail["actions"].as_array().unwrap().contains(&comment));
    let approval_required = ok(
        app,
        "POST",
        &gate_path,
        admin,
        json!({"stepId":plan.step().id()}),
    )
    .await;
    assert_eq!(approval_required["outcome"], "failed");
    assert_eq!(
        approval_required["rejection"]["message"],
        "review has insufficient current approvals"
    );
    approve(app, &old_path, reviewer, 2, "target-recomputed-approve").await;
    // The old Proof and the competing Proof still cannot cover the new result.
    let missing_proof = ok(
        app,
        "POST",
        &gate_path,
        admin,
        json!({"stepId":plan.step().id()}),
    )
    .await;
    assert_eq!(missing_proof["outcome"], "failed");
    assert_eq!(
        missing_proof["rejection"]["message"],
        "missing fresh passing Proof for exact Step, recipe and policy"
    );
    let refused = facade
        .transact(
            principal,
            request(
                &current,
                plan.step(),
                "target-old-proofs-refused",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(refused.receipt().status, ReceiptStatus::Rejected);
    assert_eq!(
        refused.receipt().rejections[0].code,
        RejectionCode::GateRejected
    );
    assert_eq!(
        refused.receipt().rejections[0].message,
        "missing fresh passing Proof for exact Step, recipe and policy"
    );
    let combined_proof = prove(
        app,
        base,
        runner,
        plan.step(),
        recipe,
        "target-recomputed-proof",
    )
    .await;
    let evaluation = ok(
        app,
        "POST",
        &gate_path,
        admin,
        json!({"stepId":plan.step().id()}),
    )
    .await;
    assert_eq!(evaluation["outcome"], "passed");
    let ProductEnvelope::GateEvaluation { record, .. } =
        serde_json::from_value(evaluation["product"].clone()).unwrap()
    else {
        panic!("Gate evaluation must expose its immutable Product");
    };
    assert_eq!(
        record.subject,
        ImmutableSubject::Step {
            object_id: object_id(plan.step().id().as_str())
        }
    );
    assert_eq!(
        record.proof_object_ids,
        vec![combined_proof.object_id().clone()]
    );
    let accepted = facade
        .transact(
            principal,
            request(
                &current,
                plan.step(),
                "target-recomputed-advance",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(accepted.receipt().status, ReceiptStatus::Applied);
    let detail = ok(app, "GET", &old_path, admin, Value::Null).await;
    assert_eq!(detail["lifecycle"], "integrated");
    assert_eq!(detail["integration"]["record"]["version"], 2);
    assert_eq!(
        detail["integration"]["record"]["proposedStepId"],
        plan.step().id().as_str()
    );
    assert_eq!(
        detail["integration"]["record"]["transactionId"],
        accepted.receipt().transaction_id.as_str()
    );
    assert!(detail["actions"].as_array().unwrap().contains(&comment));
    review_interference::run(app, admin, reviewer, runner, repository, facade, principal).await;
}
