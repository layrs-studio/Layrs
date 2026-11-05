use super::*;

pub async fn run(
    app: &Router,
    base: &str,
    admin: &str,
    runner: &str,
    step: &str,
    recipe: &Value,
    bob: &str,
) {
    let path = format!("{base}/runs");
    let start =
        json!({"idempotencyKey":"run-concurrent-start-0001","stepId":step,"recipeObjectId":recipe});
    let (a, b) = tokio::join!(
        call(app, "POST", &path, runner, start.clone()),
        call(app, "POST", &path, runner, start)
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    assert_eq!(a.1, b.1);
    let complete = format!("{path}/{}/complete", a.1["runId"].as_str().unwrap());
    let result = json!({"outcome":"passed","log":"same completion despite lost response"});
    let (a, b) = tokio::join!(
        call(app, "POST", &complete, runner, result.clone()),
        call(app, "POST", &complete, runner, result)
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    assert_eq!(a.1, b.1);
    let independent = ok(
        app,
        "POST",
        &path,
        runner,
        json!({"idempotencyKey":"run-concurrent-attempt-02","stepId":step,"recipeObjectId":recipe}),
    )
    .await;
    let next = format!("{path}/{}/complete", independent["runId"].as_str().unwrap());
    let (passed, failed) = tokio::join!(
        call(
            app,
            "POST",
            &next,
            runner,
            json!({"outcome":"passed","log":"first competing verdict"})
        ),
        call(
            app,
            "POST",
            &next,
            runner,
            json!({"outcome":"failed","log":"contradictory competing verdict"})
        )
    );
    assert!(matches!(
        (passed.0, failed.0),
        (StatusCode::OK, StatusCode::BAD_REQUEST) | (StatusCode::BAD_REQUEST, StatusCode::OK)
    ));
    let winner = if passed.0 == StatusCode::OK {
        &passed.1
    } else {
        &failed.1
    };
    assert_ne!(
        a.1["proofObjectId"], winner["proofObjectId"],
        "distinct attempt must create a distinct immutable proof"
    );
    // Two different executions coexist, rather than two deliveries of one key.
    let (left, right) = tokio::join!(
        call(
            app,
            "POST",
            &path,
            runner,
            json!({"idempotencyKey":"run-distinct-overlap-left", "stepId":step,"recipeObjectId":recipe})
        ),
        call(
            app,
            "POST",
            &path,
            runner,
            json!({"idempotencyKey":"run-distinct-overlap-right","stepId":step,"recipeObjectId":recipe})
        )
    );
    assert_eq!(left.0, StatusCode::OK);
    assert_eq!(right.0, StatusCode::OK);
    assert_ne!(left.1["runId"], right.1["runId"]);
    let left_path = format!("{path}/{}", left.1["runId"].as_str().unwrap());
    let right_path = format!("{path}/{}", right.1["runId"].as_str().unwrap());
    for detail in [&left_path, &right_path] {
        let value = ok(app, "GET", detail, admin, Value::Null).await;
        assert_eq!(value["status"], "running");
        assert!(value["proofObjectId"].is_null());
    }
    let left_body = json!({"outcome":"passed","log":"distinct left completion"});
    let right_body = json!({"outcome":"failed","log":"distinct right completion"});
    let left_complete = format!("{left_path}/complete");
    let right_complete = format!("{right_path}/complete");
    let (left_done, right_done) = tokio::join!(
        call(app, "POST", &left_complete, runner, left_body.clone()),
        call(app, "POST", &right_complete, runner, right_body.clone())
    );
    assert_eq!(left_done.0, StatusCode::OK);
    assert_eq!(right_done.0, StatusCode::OK);
    assert_ne!(left_done.1["proofObjectId"], right_done.1["proofObjectId"]);
    for (detail, expected, body) in [
        (&left_path, &left_done.1, left_body),
        (&right_path, &right_done.1, right_body),
    ] {
        assert_eq!(
            ok(app, "POST", &format!("{detail}/complete"), runner, body).await,
            *expected
        );
        let value = ok(app, "GET", detail, admin, Value::Null).await;
        assert_eq!(value["proofObjectId"], expected["proofObjectId"]);
        assert_eq!(value["outcome"], expected["outcome"]);
        assert_eq!(value["evidence"][0]["runId"], expected["runId"]);
    }
    let pending = ok(
        app,
        "POST",
        &path,
        runner,
        json!({"idempotencyKey":"run-policy-changes-0001","stepId":step,"recipeObjectId":recipe}),
    )
    .await;
    ok(
        app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":bob,"role":"auditor"}),
    )
    .await;
    let response = call(
        app,
        "POST",
        &format!("{path}/{}/complete", pending["runId"].as_str().unwrap()),
        runner,
        json!({"outcome":"passed","log":"policy changed while running"}),
    )
    .await;
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    assert_eq!(response.1["error"]["code"], "invalid_descriptor");
    assert_eq!(
        call(app, "POST", "/v3/auth/logout", runner, json!({}))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            app,
            "POST",
            &complete,
            runner,
            json!({"outcome":"passed","log":"same completion despite lost response"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(call(app,"POST",&format!("{path}/{}/cancel",pending["runId"].as_str().unwrap()),runner,json!({"idempotencyKey":"cancel-revoked-session-001","reason":"revoked session must fail"})).await.0,StatusCode::FORBIDDEN);
}
