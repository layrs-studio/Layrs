use super::*;

pub async fn run(
    app: &Router,
    base: &str,
    admin: &str,
    runner: &str,
    member: &str,
    target: (&str, &Value),
    pool: &sqlx::PgPool,
) {
    let (step, recipe) = target;
    // Another automation principal has proof.run but is not this run's producer.
    let other = ok(
        app,
        "POST",
        "/v3/auth/service-accounts",
        admin,
        json!({"displayName":"Other CI","label":"cancellation-test","expiresIn":3600}),
    )
    .await;
    ok(
        app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":other["principal_id"],"role":"automation"}),
    )
    .await;
    let path = format!("{base}/runs");
    let started = ok(
        app,
        "POST",
        &path,
        runner,
        json!({"idempotencyKey":"cancel-run-start-00001","stepId":step,"recipeObjectId":recipe}),
    )
    .await;
    let run = started["runId"].as_str().unwrap();
    let cancel = format!("{path}/{run}/cancel");
    let complete = format!("{path}/{run}/complete");
    let request =
        json!({"idempotencyKey":"cancel-run-request-001","reason":"Superseded by another capture"});
    for token in [member, other["access_token"].as_str().unwrap()] {
        assert_eq!(
            call(app, "POST", &cancel, token, request.clone()).await.0,
            StatusCode::FORBIDDEN
        );
    }
    for reason in [String::new(), " ".into(), "x".repeat(4097)] {
        assert_eq!(
            call(
                app,
                "POST",
                &cancel,
                admin,
                json!({"idempotencyKey":"cancel-run-invalid-001","reason":reason})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (first, replay) = tokio::join!(
        call(app, "POST", &cancel, admin, request.clone()),
        call(app, "POST", &cancel, admin, request.clone())
    );
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(replay.0, StatusCode::OK);
    assert_eq!(first.1, replay.1);
    assert_eq!(first.1["status"], "cancelled");
    assert_eq!(
        first.1["cancellation"]["record"]["schemaVersion"],
        "layrs.run-cancellation.v1"
    );
    assert_eq!(first.1["cancellation"]["record"]["stepId"], step);
    assert_eq!(ok(app, "POST", &cancel, admin, request).await, first.1);
    assert_eq!(
        call(
            app,
            "POST",
            &cancel,
            admin,
            json!({"idempotencyKey":"cancel-run-request-001","reason":"changed"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            app,
            "POST",
            &complete,
            runner,
            json!({"outcome":"passed","log":"arrived after cancellation"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let detail = ok(app, "GET", &format!("{path}/{run}"), admin, Value::Null).await;
    assert_eq!(detail["status"], "cancelled");
    assert!(
        detail["outcome"].is_null()
            && detail["proofObjectId"].is_null()
            && detail["product"].is_null()
            && detail["completedAt"].is_null()
    );
    assert_eq!(detail["cancellation"], first.1["cancellation"]);
    let listed = ok(app, "GET", &path, admin, Value::Null).await;
    assert_eq!(
        listed["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["runId"] == run)
            .unwrap()["status"],
        "cancelled"
    );
    let notices = ok(
        app,
        "GET",
        &format!("{base}/notifications?after=0"),
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(
        notices["notifications"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["subjectId"] == run && item["kind"] == "proof.cancelled")
            .count(),
        1
    );
    assert!(sqlx::query("UPDATE layrs_collaboration.run_cancellations SET request_digest=request_digest WHERE false").execute(pool).await.is_err());
    assert!(
        sqlx::query("DELETE FROM layrs_collaboration.run_cancellations WHERE false")
            .execute(pool)
            .await
            .is_err()
    );

    let own = ok(
        app,
        "POST",
        &path,
        runner,
        json!({"idempotencyKey":"cancel-run-producer-001","stepId":step,"recipeObjectId":recipe}),
    )
    .await;
    let own_cancel = format!("{path}/{}/cancel", own["runId"].as_str().unwrap());
    let producer_cancel = ok(
        app,
        "POST",
        &own_cancel,
        runner,
        json!({"idempotencyKey":"cancel-run-producer-stop","reason":"Producer stops this attempt"}),
    )
    .await;
    assert_eq!(producer_cancel["status"], "cancelled");

    let finished = ok(
        app,
        "POST",
        &path,
        runner,
        json!({"idempotencyKey":"cancel-run-finished-001","stepId":step,"recipeObjectId":recipe}),
    )
    .await;
    let finished_path = format!("{path}/{}", finished["runId"].as_str().unwrap());
    let proof = ok(
        app,
        "POST",
        &format!("{finished_path}/complete"),
        runner,
        json!({"outcome":"passed","log":"completed first"}),
    )
    .await;
    assert_eq!(
        call(
            app,
            "POST",
            &format!("{finished_path}/cancel"),
            admin,
            json!({"idempotencyKey":"cancel-run-finished-stop","reason":"Too late"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let finished_detail = ok(app, "GET", &finished_path, admin, Value::Null).await;
    assert_eq!(finished_detail["status"], "completed");
    assert_eq!(finished_detail["proofObjectId"], proof["proofObjectId"]);
    assert!(finished_detail["cancellation"].is_null());

    for attempt in 0..6 {
        let racing=ok(app,"POST",&path,runner,json!({"idempotencyKey":format!("cancel-race-attempt-{attempt:04}"),"stepId":step,"recipeObjectId":recipe})).await;
        let racing_path = format!("{path}/{}", racing["runId"].as_str().unwrap());
        let cancel_path = format!("{racing_path}/cancel");
        let complete_path = format!("{racing_path}/complete");
        let (cancelled, completed) = tokio::join!(
            call(
                app,
                "POST",
                &cancel_path,
                admin,
                json!({"idempotencyKey":format!("cancel-race-request-{attempt:04}"),"reason":"Concurrent explicit cancellation"})
            ),
            call(
                app,
                "POST",
                &complete_path,
                runner,
                json!({"outcome":"failed","log":"Concurrent real verdict"})
            )
        );
        assert!(matches!(
            (cancelled.0, completed.0),
            (StatusCode::OK, StatusCode::BAD_REQUEST) | (StatusCode::BAD_REQUEST, StatusCode::OK)
        ));
        let after = ok(app, "GET", &racing_path, admin, Value::Null).await;
        if cancelled.0 == StatusCode::OK {
            assert_eq!(after["status"], "cancelled");
            assert!(after["proofObjectId"].is_null() && after["outcome"].is_null());
        } else {
            assert_eq!(after["status"], "completed");
            assert_eq!(after["outcome"], "failed");
            assert!(after["cancellation"].is_null());
        }
    }
}
