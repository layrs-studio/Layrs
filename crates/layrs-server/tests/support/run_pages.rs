use super::*;
use std::collections::BTreeSet;

pub async fn run(
    app: &Router,
    base: &str,
    admin: &str,
    runner: &str,
    step: &str,
    recipe: &Value,
) -> String {
    let path = format!("{base}/runs");
    let mut expected = BTreeSet::new();
    for n in 0..101 {
        let run = ok(app,"POST",&path,runner,json!({"idempotencyKey":format!("run-pagination-{n:04}"),"stepId":step,"recipeObjectId":recipe})).await;
        expected.insert(run["runId"].as_str().unwrap().to_owned());
    }
    let first = ok(app, "GET", &format!("{path}?limit=37"), admin, Value::Null).await;
    assert_eq!(first["runs"].as_array().unwrap().len(), 37);
    assert_eq!(first["hasMore"], true);
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let late = ok(
        app,
        "POST",
        &path,
        runner,
        json!({"idempotencyKey":"run-pagination-late-0001","stepId":step,"recipeObjectId":recipe}),
    )
    .await;
    let mut page = first;
    let mut seen = BTreeSet::new();
    loop {
        for run in page["runs"].as_array().unwrap() {
            assert!(
                seen.insert(run["runId"].as_str().unwrap().to_owned()),
                "duplicate run across pages"
            );
        }
        let Some(next) = page["nextCursor"].as_str() else {
            break;
        };
        page = ok(
            app,
            "GET",
            &format!("{path}?limit=37&cursor={next}"),
            admin,
            Value::Null,
        )
        .await;
    }
    assert!(expected.is_subset(&seen));
    assert!(!seen.contains(late["runId"].as_str().unwrap()));
    let fresh = ok(app, "GET", &format!("{path}?limit=1"), admin, Value::Null).await;
    assert_eq!(fresh["runs"][0]["runId"], late["runId"]);
    for query in ["limit=0", "limit=101", "cursor=garbage"] {
        assert_eq!(
            call(app, "GET", &format!("{path}?{query}"), admin, Value::Null)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    cursor
}
