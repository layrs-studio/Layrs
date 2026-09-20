use super::*;
use layrs_store_server::v3::{load_space_descriptor, read_canonical_object};
use sqlx::Row;

pub async fn closed_and_reopened(
    app: &Router,
    base: &str,
    author: &str,
    other: &str,
    review: &Value,
    repository: &PgV3Repository,
) {
    let id = review["reviewId"].as_str().unwrap();
    let path = format!("{base}/reviews/{id}");
    let close = json!({"idempotencyKey":"review-lifecycle-close-01","version":1,"action":"close","body":"Withdraw this proposal temporarily","anchor":null,"replyTo":null});
    assert_eq!(
        call(
            app,
            "POST",
            &format!("{path}/actions"),
            other,
            close.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let result = ok(
        app,
        "POST",
        &format!("{path}/actions"),
        author,
        close.clone(),
    )
    .await;
    assert_eq!(
        ok(app, "POST", &format!("{path}/actions"), author, close).await,
        result
    );
    assert_eq!(
        ok(app, "GET", &path, author, Value::Null).await["lifecycle"],
        "closed"
    );
    for action in ["approve", "request_changes"] {
        assert_eq!(call(app,"POST",&format!("{path}/actions"),other,json!({"idempotencyKey":format!("review-closed-{action}-01"),"version":1,"action":action,"body":"closed decision forbidden","anchor":null,"replyTo":null})).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(call(app,"POST",&format!("{path}/versions"),author,json!({"idempotencyKey":"review-closed-new-version","expectedVersion":1,"layerId":review["layerId"],"proposedStepId":review["proposedStepId"],"title":"must not update closed proposal","participants":[]})).await.0,StatusCode::BAD_REQUEST);
    rebuild(app, &path, author, repository, id).await;
    let reopen = json!({"idempotencyKey":"review-lifecycle-reopen-01","version":1,"action":"reopen","body":"Resume the same immutable proposal","anchor":null,"replyTo":null});
    assert_eq!(
        call(
            app,
            "POST",
            &format!("{path}/actions"),
            other,
            reopen.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let result = ok(
        app,
        "POST",
        &format!("{path}/actions"),
        author,
        reopen.clone(),
    )
    .await;
    assert_eq!(
        ok(app, "POST", &format!("{path}/actions"), author, reopen).await,
        result
    );
    let detail = ok(app, "GET", &path, author, Value::Null).await;
    assert_eq!(detail["lifecycle"], "open");
    assert_eq!(detail["versions"], json!([review]));
    assert_eq!(detail["actions"].as_array().unwrap().len(), 2);
    rebuild(app, &path, author, repository, id).await;
}

/// Maintenance drill, not a new public API: authoritative ordered immutable
/// events and their verified CAS documents suffice to recreate UI projections.
pub async fn rebuild(
    app: &Router,
    path: &str,
    token: &str,
    repository: &PgV3Repository,
    review: &str,
) {
    let before = ok(app, "GET", path, token, Value::Null).await;
    let space: SpaceId = serde_json::from_value(before["versions"][0]["spaceId"].clone()).unwrap();
    let mut latest = 0;
    let mut lifecycle = "open";
    let mut documents = Vec::new();
    let versions = sqlx::query("SELECT proposal_object_id AS object_id,canonical_document_jcs FROM layrs_collaboration.review_versions WHERE review_id=$1 ORDER BY version")
        .bind(review).fetch_all(repository.pool()).await.unwrap();
    for row in versions {
        let value: Value =
            serde_json::from_str(&row.get::<String, _>("canonical_document_jcs")).unwrap();
        latest = latest.max(value["version"].as_u64().unwrap());
        documents.push((row.get::<String, _>("object_id"), value));
    }
    let actions = sqlx::query("SELECT evidence_object_id AS object_id,canonical_document_jcs FROM layrs_collaboration.review_actions WHERE review_id=$1 ORDER BY sequence")
        .bind(review).fetch_all(repository.pool()).await.unwrap();
    for row in actions {
        let value: Value =
            serde_json::from_str(&row.get::<String, _>("canonical_document_jcs")).unwrap();
        match value["action"].as_str() {
            Some("close") => lifecycle = "closed",
            Some("reopen") => lifecycle = "open",
            _ => {}
        }
        documents.push((row.get::<String, _>("object_id"), value));
    }
    let integration = sqlx::query("SELECT evidence_object_id AS object_id,canonical_document_jcs FROM layrs_collaboration.review_integrations WHERE review_id=$1")
        .bind(review).fetch_optional(repository.pool()).await.unwrap();
    if let Some(row) = integration {
        lifecycle = "integrated";
        documents.push((
            row.get::<String, _>("object_id"),
            serde_json::from_str(&row.get::<String, _>("canonical_document_jcs")).unwrap(),
        ));
    }
    for (id, document) in &documents {
        let descriptor = load_space_descriptor(repository.pool(), &space, &object_id(id))
            .await
            .unwrap();
        let bytes = read_canonical_object(repository.cas().as_ref(), &descriptor, 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_jcs::to_vec(document).unwrap(),
            bytes,
            "projection must match verified immutable authority"
        );
    }
    assert_eq!(before["lifecycle"], lifecycle);
    // Inject damage only into the derived fields, never into immutable records.
    sqlx::query(
        "UPDATE layrs_collaboration.reviews SET lifecycle=$2,latest_version=1 WHERE review_id=$1",
    )
    .bind(review)
    .bind(if lifecycle == "open" {
        "closed"
    } else {
        "open"
    })
    .execute(repository.pool())
    .await
    .unwrap();
    assert_ne!(
        ok(app, "GET", path, token, Value::Null).await["lifecycle"],
        before["lifecycle"]
    );
    sqlx::query(
        "UPDATE layrs_collaboration.reviews SET lifecycle=$2,latest_version=$3 WHERE review_id=$1",
    )
    .bind(review)
    .bind(lifecycle)
    .bind(latest as i32)
    .execute(repository.pool())
    .await
    .unwrap();
    assert_eq!(
        ok(app, "GET", path, token, Value::Null).await,
        before,
        "full detail and decision aggregate survive rebuild"
    );
}
