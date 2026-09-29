use super::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
#[path = "choose_drill.rs"]
mod choose_drill;
#[path = "comment_corrections.rs"]
mod comment_corrections;
#[path = "policy_root_drill.rs"]
mod policy_root_drill;
#[path = "proof_integrity.rs"]
mod proof_integrity;
#[path = "review_lifecycle.rs"]
mod review_lifecycle;
#[path = "review_pages.rs"]
mod review_pages;
#[path = "review_target_advance.rs"]
mod review_target_advance;
#[path = "run_cancellation.rs"]
mod run_cancellation;
#[path = "run_concurrency.rs"]
mod run_concurrency;
#[path = "run_execution.rs"]
mod run_execution;
#[path = "run_pages.rs"]
mod run_pages;
#[path = "team_drill.rs"]
mod team_drill;
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn ok(app: &Router, method: &str, path: &str, token: &str, body: Value) -> Value {
    let (status, value) = call(app, method, path, token, body).await;
    assert!(status.is_success(), "{method} {path}: {status} {value}");
    value
}
#[allow(clippy::too_many_arguments)]
pub async fn run(
    repository: PgV3Repository,
    identity: Arc<PgIdentity>,
    admin: &str,
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    first: &Step,
    cas_root: &std::path::Path,
    maintenance: &sqlx::PgPool,
) {
    let app = identity
        .as_ref()
        .clone()
        .router()
        .merge(layrs_server::collaboration::router(
            layrs_server::collaboration::CollaborationHttpState::new(
                repository.clone(),
                identity.clone(),
            ),
        ));
    let who = ok(&app, "GET", "/v3/auth/whoami", admin, Value::Null).await;
    assert_eq!(who["kind"], "bootstrap_admin");
    let bob=ok(&app,"POST","/v3/auth/accounts",admin,json!({"login":"bob@example.com","password":"bob correct horse battery staple","displayName":"Bob"})).await;
    let session = identity
        .login(layrs_server::identity::Credentials {
            login: "bob@example.com".into(),
            password: "bob correct horse battery staple".into(),
        })
        .await
        .unwrap();
    let accounts = ok(&app, "GET", "/v3/auth/accounts", admin, Value::Null).await;
    assert!(
        accounts["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["principalId"] == bob["principalId"] && a["displayName"] == "Bob")
    );
    assert!(!accounts.to_string().contains("password"));
    assert_eq!(
        call(
            &app,
            "GET",
            "/v3/auth/accounts",
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let service = ok(
        &app,
        "POST",
        "/v3/auth/service-accounts",
        admin,
        json!({"displayName":"CI","label":"integration-test","expiresIn":3600}),
    )
    .await;
    let space: SpaceId = sqlx::query_scalar::<_, String>(
        "SELECT space_id FROM space_steps WHERE step_id=$1 ORDER BY space_id LIMIT 1",
    )
    .bind(first.id().as_str())
    .fetch_one(repository.pool())
    .await
    .map(|s| SpaceId::parse(s).unwrap())
    .unwrap();
    let base = format!("/v3/spaces/{}", space.as_str());
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{base}/reviews"),
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    ok(
        &app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":bob["principalId"],"role":"member"}),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":service["principal_id"],"role":"automation"}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{base}/members"),
            &session.access_token,
            json!({"principalId":service["principal_id"],"role":"member"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let bootstrap = facade.bootstrap(principal, &space).await.unwrap();
    let layer = bootstrap.layers[0].record.id().clone();
    let candidate = Step::new(
        bootstrap.layers[0].head_step_ids.clone(),
        first.root_tree_id().clone(),
    )
    .unwrap();
    let mut admit = request(
        &bootstrap,
        &candidate,
        "review-admit",
        LayerMutationMode::Advance,
    );
    admit.view_mutation = None;
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
    let protection=ok(&app,"PUT",&format!("{base}/layers/{layer}/protection"),admin,json!({"minimumApprovals":1,"allowSelfApproval":false,"rejectConflicts":true,"recipe":{"command":"cargo test --locked"},"trustedRunnerPrincipalIds":[service["principal_id"]],"maximumProofAgeSeconds":3600})).await;
    let fresh = facade.bootstrap(principal, &space).await.unwrap();
    let denied = facade
        .transact(
            principal,
            request(
                &fresh,
                &candidate,
                "gate-no-review",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(denied.receipt().status, ReceiptStatus::Rejected);
    assert_eq!(
        denied.receipt().rejections[0].code,
        RejectionCode::GateRejected
    );
    let proposal = json!({"idempotencyKey":"review-create-0000000001","layerId":layer,"proposedStepId":candidate.id(),"title":"Merge both branches","participants":[bob["principalId"]],"expectedVersion":0});
    let review = ok(
        &app,
        "POST",
        &format!("{base}/reviews"),
        admin,
        proposal.clone(),
    )
    .await;
    assert_eq!(
        ok(&app, "POST", &format!("{base}/reviews"), admin, proposal).await,
        review
    );
    let review_id = review["reviewId"].as_str().unwrap();
    review_lifecycle::closed_and_reopened(
        &app,
        &base,
        admin,
        &session.access_token,
        &review,
        &repository,
    )
    .await;
    let detail_path = format!("{base}/reviews/{review_id}");
    assert_eq!(
        ok(&app, "GET", &detail_path, admin, Value::Null).await["decisionState"]["state"],
        "pending"
    );
    let actions = format!("{base}/reviews/{review_id}/actions");
    ok(&app,"POST",&actions,admin,json!({"idempotencyKey":"review-self-approve-001","version":1,"action":"approve","body":"Author decision is recorded but cannot satisfy protection","anchor":null,"replyTo":null})).await;
    let blocked = facade
        .transact(
            principal,
            request(
                &fresh,
                &candidate,
                "gate-self-approval",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(blocked.receipt().status, ReceiptStatus::Rejected);
    ok(&app,"POST",&actions,&session.access_token,json!({"idempotencyKey":"review-anchor-valid-01","version":1,"action":"comment","body":"Line anchored to exact immutable version","anchor":{"path":"docs/readme.txt","startLine":1,"endLine":1},"replyTo":null})).await;
    for (key, path, line) in [
        ("review-anchor-missing1", "missing.txt", 1),
        ("review-anchor-lineout1", "docs/readme.txt", 2),
    ] {
        assert_eq!(call(&app,"POST",&actions,&session.access_token,json!({"idempotencyKey":key,"version":1,"action":"comment","body":"Invalid anchor","anchor":{"path":path,"startLine":line,"endLine":line},"replyTo":null})).await.0,StatusCode::BAD_REQUEST);
    }
    let approve = json!({"idempotencyKey":"review-approve-00000001","version":1,"action":"approve","body":"Looks good","anchor":null,"replyTo":null});
    ok(
        &app,
        "POST",
        &actions,
        &session.access_token,
        approve.clone(),
    )
    .await;
    assert_eq!(
        ok(&app, "GET", &detail_path, admin, Value::Null).await["decisionState"]["state"],
        "approved"
    );
    ok(&app,"POST",&actions,&session.access_token,json!({"idempotencyKey":"review-request-changes01","version":1,"action":"request_changes","body":"Please fix the reviewed version","anchor":null,"replyTo":null})).await;
    assert_eq!(
        ok(&app, "GET", &detail_path, admin, Value::Null).await["decisionState"]["state"],
        "changes_requested"
    );
    ok(&app,"POST",&actions,&session.access_token,json!({"idempotencyKey":"review-approve-again-001","version":1,"action":"approve","body":"Fixed","anchor":null,"replyTo":null})).await;
    assert_eq!(
        call(
            &app,
            "POST",
            &actions,
            service["access_token"].as_str().unwrap(),
            approve
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let denied = facade
        .transact(
            principal,
            request(
                &fresh,
                &candidate,
                "gate-no-proof",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(denied.receipt().status, ReceiptStatus::Rejected);
    let evaluation_path = format!("{base}/layers/{layer}/gates/evaluate");
    let evaluation = ok(
        &app,
        "POST",
        &evaluation_path,
        admin,
        json!({"stepId":candidate.id()}),
    )
    .await;
    assert_eq!(evaluation["outcome"], "failed");
    let runner = service["access_token"].as_str().unwrap();
    run_execution::run(
        &app,
        &base,
        admin,
        runner,
        candidate.id().as_str(),
        &protection["recipeObjectId"],
        repository.pool(),
    )
    .await;
    let run=ok(&app,"POST",&format!("{base}/runs"),runner,json!({"idempotencyKey":"run-start-00000000001","stepId":candidate.id(),"recipeObjectId":protection["recipeObjectId"]})).await;
    let complete = format!("{base}/runs/{}/complete", run["runId"].as_str().unwrap());
    assert_eq!(
        call(
            &app,
            "POST",
            &complete,
            runner,
            json!({"outcome":"passed","log":"x".repeat(256*1024+1)})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let proof = ok(
        &app,
        "POST",
        &complete,
        runner,
        json!({"outcome":"passed","log":"real external runner fixture: 12 assertions passed"}),
    )
    .await;
    assert_eq!(
        ok(
            &app,
            "POST",
            &complete,
            runner,
            json!({"outcome":"passed","log":"real external runner fixture: 12 assertions passed"})
        )
        .await,
        proof
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &complete,
            runner,
            json!({"outcome":"failed","log":"contradictory retry"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let run_detail = ok(
        &app,
        "GET",
        &format!("{base}/runs/{}", run["runId"].as_str().unwrap()),
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(run_detail["evidence"][0]["outcome"], "passed");
    let expired=ok(&app,"POST",&format!("{base}/runs"),runner,json!({"idempotencyKey":"run-expiration-000001","stepId":candidate.id(),"recipeObjectId":protection["recipeObjectId"]})).await;
    sqlx::query("UPDATE layrs_collaboration.runs SET expires_at=clock_timestamp()-interval '1 second' WHERE run_id=$1").bind(expired["runId"].as_str().unwrap()).execute(repository.pool()).await.unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!(
                "{base}/runs/{}/complete",
                expired["runId"].as_str().unwrap()
            ),
            runner,
            json!({"outcome":"passed","log":"too late"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(call(&app,"POST",&format!("{base}/runs"),runner,json!({"idempotencyKey":"run-wrongrecipe-00001","stepId":candidate.id(),"recipeObjectId":format!("blake3:{}","f".repeat(64))})).await.0,StatusCode::BAD_REQUEST);
    // A new immutable review version invalidates the old approval even for the same Step.
    ok(&app,"POST",&format!("{base}/reviews/{review_id}/versions"),admin,json!({"idempotencyKey":"review-version-00000002","layerId":layer,"proposedStepId":candidate.id(),"title":"Updated explanation","participants":[bob["principalId"]],"expectedVersion":1})).await;
    assert_eq!(
        ok(&app, "GET", &detail_path, admin, Value::Null).await["decisionState"]["state"],
        "pending"
    );
    let denied = facade
        .transact(
            principal,
            request(
                &fresh,
                &candidate,
                "gate-stale-approval",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(denied.receipt().status, ReceiptStatus::Rejected);
    assert_eq!(call(&app,"POST",&actions,&session.access_token,json!({"idempotencyKey":"review-oldversion-0001","version":1,"action":"approve","body":"old","anchor":null,"replyTo":null})).await.0,StatusCode::BAD_REQUEST);
    ok(&app,"POST",&actions,&session.access_token,json!({"idempotencyKey":"review-approve-00000002","version":2,"action":"approve","body":"Reviewed exact version","anchor":null,"replyTo":null})).await;
    let evaluation = ok(
        &app,
        "POST",
        &evaluation_path,
        admin,
        json!({"stepId":candidate.id()}),
    )
    .await;
    assert_eq!(evaluation["outcome"], "passed");
    proof_integrity::run(
        &app,
        &base,
        admin,
        runner,
        &repository,
        facade,
        principal,
        &fresh,
        &candidate,
        first,
        &evaluation_path,
        &run_detail,
        cas_root,
    )
    .await;
    let notifications = ok(
        &app,
        "GET",
        &format!("{base}/notifications?after=0"),
        &session.access_token,
        Value::Null,
    )
    .await;
    assert!(
        !notifications["notifications"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Version two already exists. Its earlier notifications must still open
    // version one rather than silently attaching old actions to the latest.
    let events = notifications["notifications"].as_array().unwrap();
    for kind in ["review.proposed", "review.action"] {
        let contexts: std::collections::BTreeSet<u64> = events
            .iter()
            .filter(|event| event["subjectId"] == review_id && event["kind"] == kind)
            .map(|event| {
                assert_eq!(event["reviewContext"]["reviewId"], review_id);
                event["reviewContext"]["version"].as_u64().unwrap()
            })
            .collect();
        assert_eq!(contexts, [1, 2].into_iter().collect());
    }
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "proof.completed")
    );
    assert!(
        events
            .iter()
            .filter(|event| !event["kind"].as_str().unwrap().starts_with("review."))
            .all(|event| event["reviewContext"].is_null())
    );
    let read = ok(
        &app,
        "POST",
        &format!("{base}/notifications/read"),
        &session.access_token,
        json!({"lastSequence":notifications["nextAfter"]}),
    )
    .await;
    assert_eq!(read["lastReadSequence"], notifications["nextAfter"]);
    let accepted = facade
        .transact(
            principal,
            request(
                &fresh,
                &candidate,
                "gate-accepted",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        accepted.receipt().status,
        ReceiptStatus::Applied,
        "{:?}",
        accepted.receipt().rejections
    );
    let detail = ok(
        &app,
        "GET",
        &format!("{base}/reviews/{review_id}"),
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(detail["versions"].as_array().unwrap().len(), 2);
    assert_eq!(detail["lifecycle"], "integrated");
    assert_eq!(
        detail["integration"]["record"]["transactionId"],
        accepted.receipt().transaction_id.as_str()
    );
    assert_eq!(
        detail["integration"]["record"]["receiptDigest"],
        accepted
            .receipt()
            .computed_storage_digest()
            .unwrap()
            .as_str()
    );
    let integrated_notices = ok(
        &app,
        "GET",
        &format!("{base}/notifications?after={}", notifications["nextAfter"]),
        admin,
        Value::Null,
    )
    .await;
    let integration_notice = integrated_notices["notifications"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "review.integrated" && event["subjectId"] == review_id)
        .unwrap();
    assert_eq!(
        integration_notice["reviewContext"],
        json!({"reviewId":review_id,"version":2})
    );
    assert_eq!(
        integration_notice["evidenceObjectId"],
        detail["integration"]["evidenceObjectId"]
    );
    let replay = facade
        .transact(
            principal,
            request(
                &fresh,
                &candidate,
                "gate-accepted",
                LayerMutationMode::Advance,
            ),
        )
        .await
        .unwrap();
    assert_eq!(replay.receipt(), accepted.receipt());
    assert_eq!(
        ok(&app, "GET", &detail_path, admin, Value::Null).await["integration"],
        detail["integration"]
    );
    for action in ["close", "reopen"] {
        assert_eq!(call(&app,"POST",&actions,admin,json!({"idempotencyKey":format!("integrated-review-{action}-001"),"version":2,"action":action,"body":"must preserve integrated state","anchor":null,"replyTo":null})).await.0,StatusCode::BAD_REQUEST);
    }
    review_lifecycle::rebuild(&app, &detail_path, admin, &repository, review_id).await;
    let runs = ok(&app, "GET", &format!("{base}/runs"), admin, Value::Null).await;
    for run in runs["runs"].as_array().unwrap() {
        for field in ["createdAt", "expiresAt"] {
            Timestamp::parse(run[field].as_str().unwrap()).unwrap();
        }
    }
    assert!(
        sqlx::query(
            "UPDATE layrs_collaboration.review_integrations SET version=version WHERE false"
        )
        .execute(repository.pool())
        .await
        .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM layrs_collaboration.review_integrations WHERE false")
            .execute(repository.pool())
            .await
            .is_err()
    );
    let corrected_comment = comment_corrections::run(
        &app,
        &base,
        admin,
        &session.access_token,
        review_id,
        repository.pool(),
    )
    .await;
    review_target_advance::run(
        &app,
        &base,
        admin,
        &session.access_token,
        runner,
        &repository,
        facade,
        principal,
        &space,
        &protection["recipeObjectId"],
    )
    .await;
    run_cancellation::run(
        &app,
        &base,
        admin,
        runner,
        &session.access_token,
        (candidate.id().as_str(), &protection["recipeObjectId"]),
        repository.pool(),
    )
    .await;
    let run_cursor = run_pages::run(
        &app,
        &base,
        admin,
        runner,
        candidate.id().as_str(),
        &protection["recipeObjectId"],
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":bob["principalId"],"role":"revoked"}),
    )
    .await;
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{base}/reviews"),
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{base}/runs?cursor={run_cursor}"),
            admin,
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{base}/runs"),
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{base}/notifications"),
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(call(&app,"POST",&format!("{base}/reviews/{review_id}/actions/{corrected_comment}/corrections"),&session.access_token,json!({"idempotencyKey":"comment-correction-revoked-001","version":1,"expectedRevision":3,"body":"Forbidden after revocation","reason":"test"})).await.0,StatusCode::FORBIDDEN);
    review_pages::run(
        &app,
        &base,
        admin,
        layer.as_str(),
        first.id().as_str(),
        bob["principalId"].as_str().unwrap(),
    )
    .await;
    run_concurrency::run(
        &app,
        &base,
        admin,
        runner,
        candidate.id().as_str(),
        &protection["recipeObjectId"],
        bob["principalId"].as_str().unwrap(),
    )
    .await;
    team_drill::run(
        &app,
        &base,
        admin,
        &identity,
        &repository,
        facade,
        principal,
        &space,
    )
    .await;
    choose_drill::run(
        &app,
        admin,
        &identity,
        &repository,
        facade,
        principal,
        cas_root,
    )
    .await;
    policy_root_drill::run(
        &app,
        admin,
        &identity,
        &repository,
        facade,
        principal,
        cas_root,
        maintenance,
    )
    .await;
}
