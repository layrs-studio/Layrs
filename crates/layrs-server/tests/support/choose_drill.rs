use super::*;
use std::collections::BTreeSet;

fn decision(
    bootstrap: &SpaceBootstrap,
    actor: &ActorId,
    chosen: &[StepId],
    discarded: &[StepId],
    label: &str,
) -> ProductEnvelope {
    let id = DecisionId::parse(format!(
        "decision:{}",
        blake3::hash(label.as_bytes()).to_hex()
    ))
    .unwrap();
    let record = DecisionRecord {
        schema_version: DecisionSchemaVersion::V1,
        decision_id: id.clone(),
        space_id: bootstrap.space_id.clone(),
        layer_id: bootstrap.layers[0].record.id().clone(),
        discarded_head_step_ids: discarded
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        chosen_head_step_ids: chosen
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        reason: "Explicit selection; discarded history remains retained".into(),
        decided_by: actor.clone(),
        decided_at: Timestamp::parse("2026-09-20T00:00:00Z").unwrap(),
    };
    let mut product = ProductEnvelope::Decision {
        product_id: id,
        product_object_id: object_id(&format!("blake3:{}", "0".repeat(64))),
        depends_on_step_ids: chosen
            .iter()
            .chain(discarded)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        depends_on_product_ids: vec![ProductLogicalId::parse(record.layer_id.as_str()).unwrap()],
        record,
    };
    let id = product.computed_object_id().unwrap();
    if let ProductEnvelope::Decision {
        product_object_id, ..
    } = &mut product
    {
        *product_object_id = id;
    }
    product.validate_v3().unwrap();
    product
}
fn choose(
    bootstrap: &SpaceBootstrap,
    seed: &Step,
    actor: &ActorId,
    chosen: &[StepId],
    discarded: &[StepId],
    label: &str,
) -> SyncTransactionRequest {
    let product = decision(bootstrap, actor, chosen, discarded, label);
    let mut value = request(bootstrap, seed, label, LayerMutationMode::Choose);
    value.steps.clear();
    let mutation = value.view_mutation.as_mut().unwrap();
    mutation.layer_mutations[0].decision_product_object_id = Some(product.object_id().clone());
    mutation.proposed_view = ProposedRepositoryView::Inline {
        view: RepositoryView::new(
            bootstrap.space_id.clone(),
            vec![
                LayerHeadSet::new(
                    bootstrap.layers[0].record.id().clone(),
                    chosen
                        .iter()
                        .cloned()
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                )
                .unwrap(),
            ],
            vec![],
            vec![],
            vec![],
        )
        .unwrap(),
    };
    value.products = vec![product];
    value.request_digest = value.computed_request_digest().unwrap();
    value
}
async fn rejected(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    value: SyncTransactionRequest,
    code: RejectionCode,
) {
    let before = facade.bootstrap(principal, &value.space_id).await.unwrap();
    let receipt = facade.transact(principal, value.clone()).await.unwrap();
    assert_eq!(receipt.receipt().status, ReceiptStatus::Rejected);
    assert!(
        receipt.receipt().rejections.iter().any(|r| r.code == code),
        "{:?}",
        receipt.receipt().rejections
    );
    assert_eq!(
        facade.transact(principal, value).await.unwrap().receipt(),
        receipt.receipt(),
        "refusal is idempotently persisted"
    );
    let after = facade.bootstrap(principal, &before.space_id).await.unwrap();
    assert_eq!(
        after.current_repository_view,
        before.current_repository_view
    );
    assert_eq!(
        after.current_repository_view_generation,
        before.current_repository_view_generation
    );
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: &Router,
    admin: &str,
    identity: &PgIdentity,
    repository: &PgV3Repository,
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    cas_root: &std::path::Path,
) {
    let provision = facade
        .provision_space(
            principal,
            ProvisionSpaceRequest {
                workspace_slug: "team".into(),
                space_slug: "choose-heads".into(),
                name: "Choose exact heads".into(),
                initial_layer: None,
            },
        )
        .await
        .unwrap();
    let space = &provision.space_id;
    let base = format!("/v3/spaces/{space}");
    let actor: String = sqlx::query_scalar("SELECT actor_id FROM principals WHERE principal_id=$1")
        .bind(principal.principal_id.as_str())
        .fetch_one(repository.pool())
        .await
        .unwrap();
    let actor = ActorId::parse(actor).unwrap();
    let empty = Tree::new(vec![]).unwrap();
    let nested = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("retained").unwrap(),
        TreeEntryTarget::tree(empty.id().clone()),
        false,
    )])
    .unwrap();
    for (index, tree) in [&empty, &nested].into_iter().enumerate() {
        upload(
            facade,
            principal,
            space,
            ObjectKind::Tree,
            tree.id().as_str(),
            serde_jcs::to_vec(tree).unwrap(),
            &format!("choose-tree-{index}"),
        )
        .await;
    }
    let seed = Step::new(vec![], empty.id().clone()).unwrap();
    let a = Step::new(vec![seed.id().clone()], empty.id().clone()).unwrap();
    let b = Step::new(vec![seed.id().clone()], nested.id().clone()).unwrap();
    let descendant = Step::new(vec![a.id().clone()], empty.id().clone()).unwrap();
    let initial = facade.bootstrap(principal, space).await.unwrap();
    let heads: Vec<_> = [a.id().clone(), b.id().clone()]
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut seed_request = request(&initial, &seed, "choose-seed", LayerMutationMode::Advance);
    seed_request.steps = vec![seed.clone(), a.clone(), b.clone(), descendant.clone()];

    seed_request.view_mutation.as_mut().unwrap().proposed_view = ProposedRepositoryView::Inline {
        view: RepositoryView::new(
            space.clone(),
            vec![LayerHeadSet::new(provision.layer_id.clone(), heads.clone()).unwrap()],
            vec![],
            vec![],
            vec![],
        )
        .unwrap(),
    };
    seed_request.request_digest = seed_request.computed_request_digest().unwrap();
    let seeded = facade.transact(principal, seed_request).await.unwrap();
    assert_eq!(
        seeded.receipt().status,
        ReceiptStatus::Applied,
        "{:?}",
        seeded.receipt().rejections
    );
    let session = identity
        .login(layrs_server::identity::Credentials {
            login: "teams-eve@example.com".into(),
            password: "teams eve strong password phrase".into(),
        })
        .await
        .unwrap();
    let member = facade
        .authenticate_bearer(&session.access_token)
        .await
        .unwrap();
    ok(
        app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":member.principal_id,"role":"member"}),
    )
    .await;
    let current = facade.bootstrap(principal, space).await.unwrap();
    let member_actor: String =
        sqlx::query_scalar("SELECT actor_id FROM principals WHERE principal_id=$1")
            .bind(member.principal_id.as_str())
            .fetch_one(repository.pool())
            .await
            .unwrap();
    let member_actor = ActorId::parse(member_actor).unwrap();
    let member_current = facade.bootstrap(&member, space).await.unwrap();
    rejected(
        facade,
        &member,
        choose(
            &member_current,
            &seed,
            &member_actor,
            std::slice::from_ref(a.id()),
            std::slice::from_ref(b.id()),
            "choose-member-denied",
        ),
        RejectionCode::PermissionDenied,
    )
    .await;
    rejected(
        facade,
        principal,
        choose(
            &current,
            &seed,
            &member_actor,
            std::slice::from_ref(a.id()),
            std::slice::from_ref(b.id()),
            "choose-forged-author",
        ),
        RejectionCode::PermissionDenied,
    )
    .await;
    rejected(
        facade,
        principal,
        choose(
            &current,
            &seed,
            &actor,
            std::slice::from_ref(descendant.id()),
            &heads,
            "choose-false-discard-ancestor",
        ),
        RejectionCode::InvalidViewMutation,
    )
    .await;
    rejected(
        facade,
        principal,
        choose(
            &current,
            &seed,
            &actor,
            std::slice::from_ref(descendant.id()),
            &[],
            "choose-missing-discard",
        ),
        RejectionCode::InvalidViewMutation,
    )
    .await;
    let mut correct = choose(
        &current,
        &seed,
        &actor,
        std::slice::from_ref(descendant.id()),
        std::slice::from_ref(b.id()),
        "choose-descendant-keeps-ancestor",
    );
    extra_refusals(
        facade, principal, repository, &current, &seed, &actor, &correct, cas_root,
    )
    .await;
    // The Decision was admitted separately by extra_refusals; resolve it by its
    // exact object ID, exercising verified stored-Product admission.
    correct.products.clear();
    correct.request_digest = correct.computed_request_digest().unwrap();
    let applied = facade.transact(principal, correct.clone()).await.unwrap();
    assert_eq!(applied.receipt().status, ReceiptStatus::Applied);
    let result = applied.receipt().view_result.as_ref().unwrap();
    assert_eq!(result.outcome, ViewOutcome::Chosen);
    assert_eq!(
        result.layer_results[0].decision_product_object_id.as_ref(),
        correct.view_mutation.as_ref().unwrap().layer_mutations[0]
            .decision_product_object_id
            .as_ref()
    );
    assert_eq!(
        facade.transact(principal, correct).await.unwrap().receipt(),
        applied.receipt()
    );
    rejected(
        facade,
        principal,
        choose(
            &current,
            &seed,
            &actor,
            std::slice::from_ref(a.id()),
            std::slice::from_ref(b.id()),
            "choose-stale-generation",
        ),
        RejectionCode::ViewGenerationConflict,
    )
    .await;
    let current = facade.bootstrap(principal, space).await.unwrap();
    let clear = choose(
        &current,
        &seed,
        &actor,
        &[],
        std::slice::from_ref(descendant.id()),
        "choose-clear-heads",
    );
    assert_eq!(
        facade
            .transact(principal, clear)
            .await
            .unwrap()
            .receipt()
            .status,
        ReceiptStatus::Applied
    );
    let current = facade.bootstrap(principal, space).await.unwrap();
    assert!(
        current.current_repository_view.record.layer_heads()[0]
            .step_ids()
            .is_empty()
    );
    let restore = choose(
        &current,
        &seed,
        &actor,
        &heads,
        &[],
        "choose-restore-retained",
    );
    assert_eq!(
        facade
            .transact(principal, restore)
            .await
            .unwrap()
            .receipt()
            .status,
        ReceiptStatus::Applied
    );
    let current = facade.bootstrap(principal, space).await.unwrap();
    let left = choose(
        &current,
        &seed,
        &actor,
        std::slice::from_ref(a.id()),
        std::slice::from_ref(b.id()),
        "choose-race-left",
    );
    let right = choose(
        &current,
        &seed,
        &actor,
        std::slice::from_ref(b.id()),
        std::slice::from_ref(a.id()),
        "choose-race-right",
    );
    let (left, right) = tokio::join!(
        facade.transact(principal, left),
        facade.transact(principal, right)
    );
    let results = [left.unwrap(), right.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|r| r.receipt().status == ReceiptStatus::Applied)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| r
                .receipt()
                .rejections
                .iter()
                .any(|e| e.code == RejectionCode::ViewGenerationConflict))
            .count(),
        1
    );
    let current = facade.bootstrap(principal, space).await.unwrap();
    let current_heads = current.current_repository_view.record.layer_heads()[0]
        .step_ids()
        .to_vec();
    ok(app,"PUT",&format!("{base}/layers/{}/protection",provision.layer_id),admin,json!({"minimumApprovals":1,"allowSelfApproval":false,"rejectConflicts":true,"recipe":null,"trustedRunnerPrincipalIds":[],"maximumProofAgeSeconds":3600})).await;
    let current = facade.bootstrap(principal, space).await.unwrap();
    rejected(
        facade,
        principal,
        choose(
            &current,
            &seed,
            &actor,
            &[],
            &current_heads,
            "choose-gate-empty-refused",
        ),
        RejectionCode::GateRejected,
    )
    .await;
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM space_steps WHERE space_id=$1")
        .bind(space.as_str())
        .fetch_one(repository.pool())
        .await
        .unwrap();
    assert_eq!(
        retained, 4,
        "choosing and clearing heads never delete historical Steps"
    );
    eprintln!(
        "Choose: administrative authorization, exact discard/ancestor semantics, empty heads, retained history, concurrency, gates and replay passed"
    );
}

#[allow(clippy::too_many_arguments)]
async fn extra_refusals(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    repository: &PgV3Repository,
    current: &SpaceBootstrap,
    seed: &Step,
    actor: &ActorId,
    correct: &SyncTransactionRequest,
    cas_root: &std::path::Path,
) {
    let mut missing = correct.clone();
    missing.idempotency_key = IdempotencyKey::parse("choose-missing-object-00000000").unwrap();
    missing.products.clear();
    missing.view_mutation.as_mut().unwrap().layer_mutations[0].decision_product_object_id =
        Some(object_id(&format!("blake3:{}", "a".repeat(64))));
    missing.request_digest = missing.computed_request_digest().unwrap();
    rejected(facade, principal, missing, RejectionCode::MissingObject).await;
    let mut wrong_type = correct.clone();
    wrong_type.idempotency_key = IdempotencyKey::parse("choose-wrong-kind-00000000").unwrap();
    wrong_type.products.clear();
    wrong_type.view_mutation.as_mut().unwrap().layer_mutations[0].decision_product_object_id =
        Some(current.layers[0].product_object_id.clone());
    wrong_type.request_digest = wrong_type.computed_request_digest().unwrap();
    rejected(
        facade,
        principal,
        wrong_type,
        RejectionCode::InvalidViewMutation,
    )
    .await;
    for (index, code) in [
        (0, RejectionCode::InvalidRequest),
        (1, RejectionCode::InvalidRequest),
        (2, RejectionCode::InvalidRequest),
    ] {
        let mut bad = correct.clone();
        bad.idempotency_key =
            IdempotencyKey::parse(format!("choose-wrong-binding-{index}-00000000")).unwrap();
        if let ProductEnvelope::Decision {
            record,
            depends_on_product_ids,
            ..
        } = &mut bad.products[0]
        {
            if index == 0 {
                record.space_id = SpaceId::parse("space:other-decision-space").unwrap();
            }
            if index == 1 {
                record.layer_id = LayerId::parse("layer:other-decision-layer").unwrap();
                *depends_on_product_ids =
                    vec![ProductLogicalId::parse(record.layer_id.as_str()).unwrap()];
            }
            if index == 2 {
                record.chosen_head_step_ids = vec![];
            }
        }
        // A forged projection is rejected by validation before planning; retain
        // coherent dependencies when testing the chosen-set binding itself.
        if index == 2 {
            if let ProductEnvelope::Decision {
                record,
                depends_on_step_ids,
                ..
            } = &mut bad.products[0]
            {
                *depends_on_step_ids = record.discarded_head_step_ids.clone();
            }
        }
        let id = bad.products[0].computed_object_id().unwrap();
        if let ProductEnvelope::Decision {
            product_object_id, ..
        } = &mut bad.products[0]
        {
            *product_object_id = id.clone();
        }
        bad.view_mutation.as_mut().unwrap().layer_mutations[0].decision_product_object_id =
            Some(id);
        bad.request_digest = bad.computed_request_digest().unwrap();
        rejected(facade, principal, bad, code).await;
    }
    // policy.administer alone is insufficient without the Workspace role.
    sqlx::query("UPDATE workspace_memberships m SET role='member' FROM spaces s WHERE s.workspace_id=m.workspace_id AND s.space_id=$1 AND m.principal_id=$2").bind(current.space_id.as_str()).bind(principal.principal_id.as_str()).execute(repository.pool()).await.unwrap();
    let mut denied = correct.clone();
    denied.idempotency_key = IdempotencyKey::parse("choose-admin-role-required-00000000").unwrap();
    denied.request_digest = denied.computed_request_digest().unwrap();
    rejected(facade, principal, denied, RejectionCode::PermissionDenied).await;
    sqlx::query("UPDATE workspace_memberships m SET role='owner' FROM spaces s WHERE s.workspace_id=m.workspace_id AND s.space_id=$1 AND m.principal_id=$2").bind(current.space_id.as_str()).bind(principal.principal_id.as_str()).execute(repository.pool()).await.unwrap();
    let mut admit = request(
        current,
        seed,
        "choose-decision-admit-only",
        LayerMutationMode::Advance,
    );
    admit.steps.clear();
    admit.view_mutation = None;
    admit.products = correct.products.clone();
    admit.request_digest = admit.computed_request_digest().unwrap();
    assert_eq!(
        facade
            .transact(principal, admit)
            .await
            .unwrap()
            .receipt()
            .status,
        ReceiptStatus::Applied
    );
    let id = correct.products[0].object_id().as_str();
    let hex = &id[7..];
    let path = cas_root.join("objects/product").join(&hex[..2]).join(hex);
    let original = std::fs::read(&path).unwrap();
    for missing in [false, true] {
        let saved = path.with_extension("choose-test-original");
        if missing {
            std::fs::rename(&path, &saved).unwrap();
        } else {
            let mut bytes = original.clone();
            bytes[0] ^= 1;
            std::fs::write(&path, bytes).unwrap();
        }
        let mut bad = correct.clone();
        bad.idempotency_key =
            IdempotencyKey::parse(format!("choose-cas-integrity-{missing}-00000000")).unwrap();
        bad.products.clear();
        bad.request_digest = bad.computed_request_digest().unwrap();
        rejected(
            facade,
            principal,
            bad,
            if missing {
                RejectionCode::MissingObject
            } else {
                RejectionCode::ImmutableObjectMismatch
            },
        )
        .await;
        if missing {
            std::fs::rename(&saved, &path).unwrap();
        } else {
            std::fs::write(&path, &original).unwrap();
        }
    }
    let _ = actor;
}
