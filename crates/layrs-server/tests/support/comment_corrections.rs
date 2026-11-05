use super::*;

pub async fn run(
    app: &Router,
    base: &str,
    admin: &str,
    author: &str,
    review: &str,
    pool: &sqlx::PgPool,
) -> String {
    let detail_path = format!("{base}/reviews/{review}");
    let before = ok(app, "GET", &detail_path, author, Value::Null).await;
    assert_eq!(before["lifecycle"], "integrated");
    let original = before["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|action| action["action"] == "comment" && action["version"] == 1)
        .unwrap();
    let action = original["actionId"].as_str().unwrap();
    let path = format!("{detail_path}/actions/{action}/corrections");
    let first = json!({"idempotencyKey":"comment-correction-first-001","version":1,"expectedRevision":0,"body":"Corrected explanation of the original capture","reason":"Clarify an inaccurate phrase"});
    assert_eq!(
        call(app, "POST", &path, admin, first.clone()).await.0,
        StatusCode::FORBIDDEN,
        "administrator cannot impersonate author"
    );
    for patch in [
        json!({"body":""}),
        json!({"body":"é".repeat(32769)}),
        json!({"body":"x\u{0001}".repeat(20000)}),
        json!({"reason":" "}),
        json!({"reason":"x".repeat(4097)}),
        json!({"expectedRevision":100}),
        json!({"version":0}),
    ] {
        let mut invalid = first.clone();
        invalid
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert_eq!(
            call(app, "POST", &path, author, invalid).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut reanchor = first.clone();
    reanchor["anchor"] = json!({"path":"elsewhere","startLine":1,"endLine":1});
    assert_eq!(
        call(app, "POST", &path, author, reanchor).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut wrong_version = first.clone();
    wrong_version["version"] = json!(2);
    assert_eq!(
        call(app, "POST", &path, author, wrong_version).await.0,
        StatusCode::NOT_FOUND
    );
    let other_review = format!("{base}/reviews/review:unrelated/actions/{action}/corrections");
    assert_eq!(
        call(app, "POST", &other_review, author, first.clone())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let author_decision = before["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["action"] == "approve"
                && item["actorPrincipalId"] == original["actorPrincipalId"]
                && item["version"] == 1
        })
        .unwrap();
    let decision_path = format!(
        "{detail_path}/actions/{}/corrections",
        author_decision["actionId"].as_str().unwrap()
    );
    assert_eq!(
        call(app, "POST", &decision_path, author, first.clone())
            .await
            .0,
        StatusCode::BAD_REQUEST,
        "decisions cannot be corrected as comments"
    );
    let applied = ok(app, "POST", &path, author, first.clone()).await;
    assert_eq!(applied["record"]["revision"], 1);
    assert_eq!(applied["record"]["version"], 1);
    assert_eq!(
        applied["record"]["actorPrincipalId"],
        original["actorPrincipalId"]
    );
    assert!(applied["record"]["previousCorrectionObjectId"].is_null());
    Timestamp::parse(applied["record"]["correctedAt"].as_str().unwrap()).unwrap();
    assert_eq!(ok(app, "POST", &path, author, first.clone()).await, applied);
    let mut changed = first.clone();
    changed["body"] = json!("Different request using the same key");
    assert_eq!(
        call(app, "POST", &path, author, changed).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut stale = first.clone();
    stale["idempotencyKey"] = json!("comment-correction-stale-001");
    assert_eq!(
        call(app, "POST", &path, author, stale).await.0,
        StatusCode::BAD_REQUEST
    );
    let next = |suffix: &str| json!({"idempotencyKey":format!("comment-correction-race-{suffix}"),"version":1,"expectedRevision":1,"body":format!("Competing revision {suffix}"),"reason":"Concurrent editing"});
    let (left, right) = tokio::join!(
        call(app, "POST", &path, author, next("left")),
        call(app, "POST", &path, author, next("right"))
    );
    assert!(matches!(
        (left.0, right.0),
        (StatusCode::OK, StatusCode::BAD_REQUEST) | (StatusCode::BAD_REQUEST, StatusCode::OK)
    ));
    let winner = if left.0 == StatusCode::OK {
        left.1
    } else {
        right.1
    };
    assert_eq!(winner["record"]["revision"], 2);
    assert_eq!(
        winner["record"]["previousCorrectionObjectId"],
        applied["evidenceObjectId"]
    );
    let third = json!({"idempotencyKey":"comment-correction-same-001","version":1,"expectedRevision":2,"body":"Final accurate explanation","reason":"Resolve the wording"});
    let (left, right) = tokio::join!(
        call(app, "POST", &path, author, third.clone()),
        call(app, "POST", &path, author, third)
    );
    assert_eq!(left.0, StatusCode::OK);
    assert_eq!(right.0, StatusCode::OK);
    assert_eq!(left.1, right.1);
    assert_eq!(left.1["record"]["revision"], 3);
    assert_eq!(
        left.1["record"]["previousCorrectionObjectId"],
        winner["evidenceObjectId"]
    );
    let after = ok(app, "GET", &detail_path, author, Value::Null).await;
    assert_eq!(
        after["actions"], before["actions"],
        "all original text, authors, dates, replies and anchors remain unchanged"
    );
    assert_eq!(after["versions"], before["versions"]);
    assert_eq!(after["integration"], before["integration"]);
    assert_eq!(after["decisionState"], before["decisionState"]);
    assert_eq!(after["commentCorrections"].as_array().unwrap().len(), 3);
    let original_object: String = sqlx::query_scalar(
        "SELECT evidence_object_id FROM layrs_collaboration.review_actions WHERE action_id=$1",
    )
    .bind(action)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(applied["record"]["originalActionObjectId"], original_object);
    assert!(
        sqlx::query(
            "UPDATE layrs_collaboration.comment_corrections SET revision=revision WHERE false"
        )
        .execute(pool)
        .await
        .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM layrs_collaboration.comment_corrections WHERE false")
            .execute(pool)
            .await
            .is_err()
    );
    let notifications = ok(
        app,
        "GET",
        &format!("{base}/notifications"),
        author,
        Value::Null,
    )
    .await;
    let notices: Vec<_> = notifications["notifications"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["kind"] == "review.comment_corrected" && event["subjectId"] == review)
        .collect();
    assert_eq!(notices.len(), 3);
    assert!(
        notices
            .iter()
            .all(|event| event["reviewContext"] == json!({"reviewId":review,"version":1}))
    );
    action.to_owned()
}
