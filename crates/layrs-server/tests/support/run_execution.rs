use super::*;

pub async fn run(
    app: &Router,
    base: &str,
    admin: &str,
    runner: &str,
    step: &str,
    recipe: &Value,
    pool: &sqlx::PgPool,
) {
    let request =
        json!({"idempotencyKey":"run-pending-execution-001","stepId":step,"recipeObjectId":recipe});
    let pending = format!("{base}/runs/pending");
    let created = ok(app, "POST", &pending, runner, request.clone()).await;
    assert_eq!(
        ok(app, "POST", &pending, runner, request.clone()).await,
        created
    );
    assert_eq!(
        call(app, "POST", &format!("{base}/runs"), runner, request)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let run = created["runId"].as_str().unwrap();
    let path = format!("{base}/runs/{run}");
    assert_eq!(
        ok(app, "GET", &path, admin, Value::Null).await["status"],
        "queued"
    );
    let list = ok(app, "GET", &format!("{base}/runs"), admin, Value::Null).await;
    assert!(
        list["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["runId"] == run && r["status"] == "queued")
    );
    let complete = format!("{path}/complete");
    assert_eq!(
        call(
            app,
            "POST",
            &complete,
            runner,
            json!({"outcome":"passed","log":"not started"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(
        !call(app, "POST", &format!("{path}/begin"), admin, json!({}))
            .await
            .0
            .is_success()
    );
    let begin = format!("{path}/begin");
    let (left, right) = tokio::join!(
        call(app, "POST", &begin, runner, json!({})),
        call(app, "POST", &begin, runner, json!({}))
    );
    assert_eq!(left.0, StatusCode::OK);
    assert_eq!(left, right);
    assert!(
        left.1["executionStartedAt"]
            .as_str()
            .unwrap()
            .ends_with('Z')
    );
    assert_eq!(
        ok(app, "GET", &path, admin, Value::Null).await["status"],
        "running"
    );
    ok(
        app,
        "POST",
        &complete,
        runner,
        json!({"outcome":"failed","log":"explicit failed verdict"}),
    )
    .await;
    let detail = ok(app, "GET", &path, admin, Value::Null).await;
    assert_eq!(detail["status"], "completed");
    assert_eq!(detail["outcome"], "failed");
    assert_eq!(
        call(app, "POST", &begin, runner, json!({})).await.0,
        StatusCode::BAD_REQUEST
    );
    for case in ["cancelled", "expired", "pending", "stale"] {
        let next=ok(app,"POST",&pending,runner,json!({"idempotencyKey":format!("run-execution-{case}-002"),"stepId":step,"recipeObjectId":recipe})).await;
        let id = next["runId"].as_str().unwrap();
        let item = format!("{base}/runs/{id}");
        match case {
            "cancelled" => {
                ok(app,"POST",&format!("{item}/cancel"),admin,json!({"idempotencyKey":"cancel-before-begin-001","reason":"Stop before execution"})).await;
            }
            "expired" => {
                sqlx::query("UPDATE layrs_collaboration.runs SET expires_at=clock_timestamp()-interval '1 second' WHERE run_id=$1").bind(id).execute(pool).await.unwrap();
            }
            "stale" => {
                sqlx::query("UPDATE layrs_collaboration.runs SET authorization_epoch=authorization_epoch+1 WHERE run_id=$1").bind(id).execute(pool).await.unwrap();
            }
            _ => continue,
        }
        assert_eq!(
            call(app, "POST", &format!("{item}/begin"), runner, json!({}))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let untouched:bool=sqlx::query_scalar("SELECT execution_started_at IS NULL AND proof_object_id IS NULL FROM layrs_collaboration.runs WHERE run_id=$1").bind(id).fetch_one(pool).await.unwrap();
        assert!(untouched);
    }
}
