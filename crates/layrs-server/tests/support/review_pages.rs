use super::*;
use std::collections::BTreeSet;

pub async fn run(app: &Router, base: &str, admin: &str, layer: &str, step: &str, bob: &str) {
    let path = format!("{base}/reviews");
    let mut expected = BTreeSet::new();
    for n in 0..101 {
        let review=ok(app,"POST",&path,admin,json!({"idempotencyKey":format!("review-pagination-{n:04}"),"layerId":layer,"proposedStepId":step,"title":format!("Paged review {n}"),"participants":[],"expectedVersion":0})).await;
        expected.insert(review["reviewId"].as_str().unwrap().to_owned());
    }
    let first = ok(app, "GET", &format!("{path}?limit=37"), admin, Value::Null).await;
    assert_eq!(first["reviews"].as_array().unwrap().len(), 37);
    assert_eq!(first["hasMore"], true);
    let late=ok(app,"POST",&path,admin,json!({"idempotencyKey":"review-pagination-late-001","layerId":layer,"proposedStepId":step,"title":"Created between pages","participants":[],"expectedVersion":0})).await;
    let mut seen = BTreeSet::new();
    let mut page = first.clone();
    loop {
        for review in page["reviews"].as_array().unwrap() {
            assert!(
                seen.insert(review["proposal"]["reviewId"].as_str().unwrap().to_owned()),
                "duplicate review across pages"
            );
        }
        let Some(cursor) = page["nextCursor"].as_str() else {
            break;
        };
        page = ok(
            app,
            "GET",
            &format!("{path}?limit=37&cursor={cursor}"),
            admin,
            Value::Null,
        )
        .await;
    }
    assert!(expected.is_subset(&seen));
    assert!(!seen.contains(late["reviewId"].as_str().unwrap()));
    let fresh = ok(app, "GET", &format!("{path}?limit=1"), admin, Value::Null).await;
    assert_eq!(
        fresh["reviews"][0]["proposal"]["reviewId"],
        late["reviewId"]
    );
    for query in ["limit=0", "limit=101", "cursor=garbage"] {
        assert_eq!(
            call(app, "GET", &format!("{path}?{query}"), admin, Value::Null)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    ok(
        app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":bob,"role":"revoked"}),
    )
    .await;
    assert_eq!(
        call(
            app,
            "GET",
            &format!("{path}?cursor={}", first["nextCursor"].as_str().unwrap()),
            admin,
            Value::Null
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}
