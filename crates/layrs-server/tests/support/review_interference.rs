//! Whole-View review freshness, exercised through the real PostgreSQL fixture.
use super::*;

// The single-Layer request helper intentionally omits other references. These
// scenarios must preserve them so a refusal can only come from the review gate.
fn advance_request(
    before: &SpaceBootstrap,
    layer: &LayerId,
    step: &Step,
    key: &str,
) -> SyncTransactionRequest {
    let mut transaction = request(before, step, key, LayerMutationMode::Advance);
    let current = &before.current_repository_view.record;
    let heads = current
        .layer_heads()
        .iter()
        .map(|heads| {
            if heads.layer_id() == layer {
                LayerHeadSet::new(layer.clone(), vec![step.id().clone()]).unwrap()
            } else {
                heads.clone()
            }
        })
        .collect();
    let mutation = transaction.view_mutation.as_mut().unwrap();
    mutation.proposed_view = ProposedRepositoryView::Inline {
        view: RepositoryView::new(
            before.space_id.clone(),
            heads,
            current.marker_ids().to_vec(),
            current.release_ids().to_vec(),
            vec![],
        )
        .unwrap(),
    };
    mutation.layer_mutations = vec![LayerMutationIntent {
        layer_id: layer.clone(),
        mode: LayerMutationMode::Advance,
        decision_product_object_id: None,
    }];
    transaction.request_digest = transaction.computed_request_digest().unwrap();
    transaction
}

fn product_identity(mut product: ProductEnvelope) -> ProductEnvelope {
    let id = product.computed_object_id().unwrap();
    match &mut product {
        ProductEnvelope::Layer {
            product_object_id, ..
        }
        | ProductEnvelope::Marker {
            product_object_id, ..
        } => *product_object_id = id,
        _ => panic!("only Layer and Marker fixture products"),
    }
    product.verify_identity().unwrap();
    product
}

async fn gate(app: &Router, path: &str, admin: &str, step: &Step, outcome: &str) -> Value {
    let evaluation = ok(app, "POST", path, admin, json!({"stepId":step.id()})).await;
    assert_eq!(evaluation["outcome"], outcome, "{evaluation}");
    evaluation
}

async fn applied(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    transaction: SyncTransactionRequest,
) -> SyncTransactionReceipt {
    let result = facade.transact(principal, transaction).await.unwrap();
    assert_eq!(
        result.receipt().status,
        ReceiptStatus::Applied,
        "{:?}",
        result.receipt().rejections
    );
    result.receipt().clone()
}

pub(super) async fn run(
    app: &Router,
    admin: &str,
    reviewer: &str,
    runner: &str,
    repository: &PgV3Repository,
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
) {
    for case in ["layer", "marker"] {
        eprintln!("review interference scenario: {case}");
        // Separate Spaces keep these additional heads/references out of the
        // original collaboration fixture and its subsequent single-Layer drills.
        let provision = facade
            .provision_space(
                principal,
                ProvisionSpaceRequest {
                    workspace_slug: "team".into(),
                    space_slug: format!("review-interference-{case}"),
                    name: format!("Review interference: {case}"),
                    initial_layer: None,
                },
            )
            .await
            .unwrap();
        let space = &provision.space_id;
        let base = format!("/v3/spaces/{space}");
        let layer_a = &provision.layer_id;
        let layer_b = Layer::new(
            LayerId::parse(format!("layer:review-interference-{case}-b")).unwrap(),
            space.clone(),
            "Independent B",
            LayerKind::Base,
            LayerStatus::Active,
            vec![],
            LayerSettings::new(false, 1000).unwrap(),
            Timestamp::parse("2026-10-24T00:00:00Z").unwrap(),
            Metadata::empty(),
        )
        .unwrap();
        let empty = Tree::new(vec![]).unwrap();
        let tree_a = Tree::new(vec![TreeEntry::new(
            RepoPathComponent::parse("a").unwrap(),
            TreeEntryTarget::tree(empty.id().clone()),
            false,
        )])
        .unwrap();
        let tree_b = Tree::new(vec![TreeEntry::new(
            RepoPathComponent::parse("b").unwrap(),
            TreeEntryTarget::tree(empty.id().clone()),
            false,
        )])
        .unwrap();
        for (index, tree) in [&empty, &tree_a, &tree_b].into_iter().enumerate() {
            upload(
                facade,
                principal,
                space,
                ObjectKind::Tree,
                tree.id().as_str(),
                serde_jcs::to_vec(tree).unwrap(),
                &format!("interference-{case}-tree-{index}"),
            )
            .await;
        }
        let seed = Step::new(vec![], empty.id().clone()).unwrap();
        let proposed = Step::new(vec![seed.id().clone()], tree_a.id().clone()).unwrap();
        let moved_b = Step::new(vec![seed.id().clone()], tree_b.id().clone()).unwrap();
        let initial = facade.bootstrap(principal, space).await.unwrap();
        let mut setup = request(
            &initial,
            &seed,
            "interference-seed",
            LayerMutationMode::Advance,
        );
        setup.steps = vec![seed.clone(), proposed.clone(), moved_b.clone()];
        setup.products = vec![product_identity(ProductEnvelope::Layer {
            product_id: layer_b.id().clone(),
            product_object_id: object_id(&format!("blake3:{}", "0".repeat(64))),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: layer_b.clone(),
        })];
        let mutation = setup.view_mutation.as_mut().unwrap();
        mutation.proposed_view = ProposedRepositoryView::Inline {
            view: RepositoryView::new(
                space.clone(),
                vec![
                    LayerHeadSet::new(layer_a.clone(), vec![seed.id().clone()]).unwrap(),
                    LayerHeadSet::new(layer_b.id().clone(), vec![seed.id().clone()]).unwrap(),
                ],
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
        };
        mutation.layer_mutations.push(LayerMutationIntent {
            layer_id: layer_b.id().clone(),
            mode: LayerMutationMode::Advance,
            decision_product_object_id: None,
        });
        setup.request_digest = setup.computed_request_digest().unwrap();
        applied(facade, principal, setup).await;
        let reviewer_id = facade
            .authenticate_bearer(reviewer)
            .await
            .unwrap()
            .principal_id;
        let runner_id = facade
            .authenticate_bearer(runner)
            .await
            .unwrap()
            .principal_id;
        for (member, role) in [(&reviewer_id, "member"), (&runner_id, "automation")] {
            ok(
                app,
                "POST",
                &format!("{base}/members"),
                admin,
                json!({"principalId":member,"role":role}),
            )
            .await;
        }
        let protection = ok(
            app,
            "PUT",
            &format!("{base}/layers/{layer_a}/protection"),
            admin,
            json!({"minimumApprovals":1,"allowSelfApproval":false,"rejectConflicts":true,
                "recipe":{"command":"cargo test --locked"},"trustedRunnerPrincipalIds":[runner_id],
                "maximumProofAgeSeconds":3600}),
        )
        .await;
        let before = facade.bootstrap(principal, space).await.unwrap();
        let old = ok(
            app,
            "POST",
            &format!("{base}/reviews"),
            admin,
            json!({"idempotencyKey":"interference-review-original","layerId":layer_a,
                "proposedStepId":proposed.id(),"title":"Unchanged Layer A proposal",
                "participants":[],"expectedVersion":0}),
        )
        .await;
        let path = format!("{base}/reviews/{}", old["reviewId"].as_str().unwrap());
        let gate_path = format!("{base}/layers/{layer_a}/gates/evaluate");
        approve(app, &path, reviewer, 1, "interference-approve-original").await;
        if case == "layer" {
            // Unlike the merge fixture's predecessors, B is a sibling of A.
            assert_eq!(proposed.parent_step_ids(), moved_b.parent_step_ids());
            assert_ne!(proposed.id(), moved_b.id());
            prove(
                app,
                &base,
                runner,
                &moved_b,
                &protection["recipeObjectId"],
                "interference-proof-sibling",
            )
            .await;
            let evaluation = gate(app, &gate_path, admin, &proposed, "failed").await;
            assert_eq!(
                evaluation["rejection"]["message"],
                "missing fresh passing Proof for exact Step, recipe and policy"
            );
            let rejected = facade
                .transact(
                    principal,
                    advance_request(
                        &before,
                        layer_a,
                        &proposed,
                        "interference-sibling-proof-refused",
                    ),
                )
                .await
                .unwrap();
            assert_eq!(rejected.receipt().status, ReceiptStatus::Rejected);
            assert_eq!(
                rejected.receipt().rejections[0].code,
                RejectionCode::GateRejected
            );
            assert_eq!(
                rejected.receipt().rejections[0].message,
                "missing fresh passing Proof for exact Step, recipe and policy"
            );
        }
        let proof = prove(
            app,
            &base,
            runner,
            &proposed,
            &protection["recipeObjectId"],
            "interference-proof-original",
        )
        .await;
        assert_eq!(
            ok(app, "GET", &path, admin, Value::Null).await["decisionState"]["state"],
            "approved"
        );
        gate(app, &gate_path, admin, &proposed, "passed").await;

        if case == "layer" {
            applied(
                facade,
                principal,
                advance_request(&before, layer_b.id(), &moved_b, "interference-b-advance"),
            )
            .await;
        } else {
            let marker = Marker::new(
                MarkerId::parse("marker:review-interference-checkpoint").unwrap(),
                space.clone(),
                seed.id().clone(),
                MarkerKind::Checkpoint,
                "Existing Step checkpoint",
                "",
                "",
                None,
                Timestamp::parse("2026-10-24T00:00:00Z").unwrap(),
                vec![],
                Metadata::empty(),
            )
            .unwrap();
            let current = &before.current_repository_view.record;
            let mut transaction = request(
                &before,
                &seed,
                "interference-marker",
                LayerMutationMode::Advance,
            );
            // ADR 0015: an inline reference-only View, with no Steps or Layer intents.
            transaction.steps.clear();
            transaction.products = vec![product_identity(ProductEnvelope::Marker {
                product_id: marker.id().clone(),
                product_object_id: object_id(&format!("blake3:{}", "0".repeat(64))),
                depends_on_step_ids: vec![seed.id().clone()],
                depends_on_product_ids: vec![],
                record: marker.clone(),
            })];
            let mutation = transaction.view_mutation.as_mut().unwrap();
            mutation.layer_mutations.clear();
            mutation.proposed_view = ProposedRepositoryView::Inline {
                view: RepositoryView::new(
                    space.clone(),
                    current.layer_heads().to_vec(),
                    vec![marker.id().clone()],
                    current.release_ids().to_vec(),
                    vec![],
                )
                .unwrap(),
            };
            transaction.request_digest = transaction.computed_request_digest().unwrap();
            applied(facade, principal, transaction).await;
        }

        let after = facade.bootstrap(principal, space).await.unwrap();
        assert_ne!(
            after.current_repository_view, before.current_repository_view,
            "{case}"
        );
        assert_eq!(
            after.current_repository_view_generation,
            before.current_repository_view_generation + 1
        );
        assert_eq!(after.current_policy_set, before.current_policy_set);
        assert_eq!(after.authorization_epoch, before.authorization_epoch);
        assert_eq!(
            after
                .layers
                .iter()
                .find(|layer| layer.record.id() == layer_a)
                .unwrap(),
            before
                .layers
                .iter()
                .find(|layer| layer.record.id() == layer_a)
                .unwrap(),
            "{case}: Layer A definition and head must remain identical"
        );
        if case == "layer" {
            assert_eq!(after.markers, before.markers);
            let b = after
                .layers
                .iter()
                .find(|layer| layer.record.id() == layer_b.id())
                .unwrap();
            assert_eq!(b.record, layer_b);
            assert_eq!(b.head_step_ids, vec![moved_b.id().clone()]);
        } else {
            assert_eq!(
                after.layers, before.layers,
                "Marker must not advance any head"
            );
            assert_eq!(
                after.current_repository_view.record.layer_heads(),
                before.current_repository_view.record.layer_heads()
            );
            assert!(before.markers.is_empty());
            assert_eq!(after.markers.len(), 1);
            assert_eq!(after.markers[0].record.target_step_id(), seed.id());
        }
        let stored: String = sqlx::query_scalar(
            "SELECT st.canonical_record_jcs FROM steps st JOIN space_steps ss USING(step_id) WHERE ss.space_id=$1 AND st.step_id=$2",
        )
        .bind(space.as_str()).bind(proposed.id().as_str()).fetch_one(repository.pool()).await.unwrap();
        assert_eq!(serde_json::from_str::<Step>(&stored).unwrap(), proposed);
        let stale = ok(app, "GET", &path, admin, Value::Null).await;
        assert_eq!(stale["versions"], json!([old.clone()]));
        assert_eq!(stale["decisionState"]["state"], "stale");
        assert_eq!(stale["decisionState"]["allowsIntegration"], false);
        assert_eq!(stale["lifecycle"], "open");
        let evaluation = gate(app, &gate_path, admin, &proposed, "failed").await;
        assert_eq!(
            evaluation["rejection"]["message"],
            "current Step, target View and policy require a review"
        );
        // Use the CURRENT CAS. A stale transaction would only prove a View
        // generation conflict, rather than stale review authorization.
        let rejected = facade
            .transact(
                principal,
                advance_request(&after, layer_a, &proposed, "interference-stale-review"),
            )
            .await
            .unwrap();
        assert_eq!(rejected.receipt().status, ReceiptStatus::Rejected);
        assert_eq!(
            rejected.receipt().rejections[0].code,
            RejectionCode::GateRejected
        );
        assert_eq!(
            rejected.receipt().rejections[0].message,
            "current Step, target View and policy require a review"
        );
        let unchanged = facade.bootstrap(principal, space).await.unwrap();
        assert_eq!(
            unchanged.current_repository_view,
            after.current_repository_view
        );
        assert_eq!(
            unchanged.current_repository_view_generation,
            after.current_repository_view_generation
        );

        let renewed = ok(
            app,
            "POST",
            &format!("{path}/versions"),
            admin,
            json!({"idempotencyKey":"interference-review-renewed","layerId":layer_a,
                "proposedStepId":proposed.id(),"title":"Reapproved against current whole View",
                "participants":[],"expectedVersion":1}),
        )
        .await;
        assert_eq!(renewed["version"], 2);
        assert_eq!(renewed["proposedStepId"], old["proposedStepId"]);
        assert_eq!(renewed["layerId"], old["layerId"]);
        assert_eq!(
            renewed["baseRepositoryViewId"],
            after.current_repository_view.repository_view_id.as_str()
        );
        assert_eq!(
            ok(app, "GET", &path, admin, Value::Null).await["decisionState"]["state"],
            "pending"
        );
        let approval_required = gate(app, &gate_path, admin, &proposed, "failed").await;
        assert_eq!(
            approval_required["rejection"]["message"],
            "review has insufficient current approvals"
        );
        approve(app, &path, reviewer, 2, "interference-approve-renewed").await;
        // No new CI run: the Step, recipe and policy have not changed. Only the
        // whole-View review authorization needed renewal.
        let evaluation = gate(app, &gate_path, admin, &proposed, "passed").await;
        let ProductEnvelope::GateEvaluation { record, .. } =
            serde_json::from_value(evaluation["product"].clone()).unwrap()
        else {
            panic!("expected Gate evaluation Product")
        };
        assert_eq!(record.proof_object_ids, vec![proof.object_id().clone()]);
        assert_eq!(
            record.subject,
            ImmutableSubject::Step {
                object_id: object_id(proposed.id().as_str())
            }
        );
        let accepted = applied(
            facade,
            principal,
            advance_request(
                &after,
                layer_a,
                &proposed,
                "interference-renewed-integration",
            ),
        )
        .await;
        let integrated = ok(app, "GET", &path, admin, Value::Null).await;
        assert_eq!(integrated["versions"][0], old);
        assert_eq!(integrated["lifecycle"], "integrated");
        assert_eq!(integrated["integration"]["record"]["version"], 2);
        assert_eq!(
            integrated["integration"]["record"]["proposedStepId"],
            proposed.id().as_str()
        );
        assert_eq!(
            integrated["integration"]["record"]["transactionId"],
            accepted.transaction_id.as_str()
        );
        let final_state = facade.bootstrap(principal, space).await.unwrap();
        assert_eq!(
            final_state
                .layers
                .iter()
                .find(|layer| layer.record.id() == layer_a)
                .unwrap()
                .head_step_ids,
            vec![proposed.id().clone()]
        );
        assert_eq!(
            final_state
                .layers
                .iter()
                .find(|layer| layer.record.id() == layer_b.id()),
            after
                .layers
                .iter()
                .find(|layer| layer.record.id() == layer_b.id())
        );
        assert_eq!(final_state.markers, after.markers);
    }
}
