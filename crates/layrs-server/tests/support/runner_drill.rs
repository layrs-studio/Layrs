use layrs_core::scm::{LayerId, SpaceId, StepId};
use layrs_runner::{Job, Recipe};
use serde_json::{Value, json};
use std::path::Path;
#[path = "exact_result_runner.rs"]
pub(crate) mod exact_result_runner;
fn call(
    client: &reqwest::blocking::Client,
    method: &str,
    url: &str,
    token: &str,
    value: Value,
) -> Value {
    let response = client
        .request(method.parse().unwrap(), url)
        .bearer_auth(token)
        .json(&value)
        .send()
        .unwrap();
    let status = response.status();
    let value: Value = response.json().unwrap();
    assert!(status.is_success(), "{method} {url}: {status} {value}");
    value
}
pub fn run(
    origin: &str,
    admin: &str,
    ca: &str,
    space: &SpaceId,
    layer: &LayerId,
    step: &StepId,
    root: &Path,
) {
    let client = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("{origin}/v3/spaces/{space}");
    let service = call(
        &client,
        "POST",
        &format!("{origin}/v3/auth/service-accounts"),
        admin,
        json!({"displayName":"Exact Step CI","label":"runner drill","expiresIn":3600}),
    );
    call(
        &client,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":service["principal_id"],"role":"automation"}),
    );
    let recipe=Recipe{image:std::env::var("LAYRS_TEST_RUNNER_IMAGE").unwrap(),command:vec!["/bin/sh".into(),"-c".into(),"test \"$(cat docs/readme.txt)\" = 'initial contents' && test -z \"$LAYRS_RUNNER_TOKEN_FILE\" && printf 'exact historical Step tested'".into()],timeout_seconds:15,maximum_log_bytes:4096};
    call(
        &client,
        "PUT",
        &format!("{base}/layers/{layer}/protection"),
        admin,
        json!({"minimumApprovals":0,"allowSelfApproval":false,"rejectConflicts":true,"recipe":recipe,"trustedRunnerPrincipalIds":[service["principal_id"]],"maximumProofAgeSeconds":3600}),
    );
    let token = service["access_token"].as_str().unwrap();
    let result = layrs_runner::run(
        Job {
            origin: origin.into(),
            space: space.clone(),
            step: step.clone(),
            layer: layer.clone(),
            recipe,
            directory: root.join("ci-run"),
            ca_pem: Some(ca.as_bytes().to_vec()),
        },
        token,
    )
    .unwrap();
    assert_eq!(result.outcome, "passed", "{}", result.log);
    assert!(result.log.contains("exact historical Step tested"));
    assert!(!result.log.contains(token));
    layrs_runner::submit(&result, token, Some(ca.as_bytes())).unwrap();
    let evaluation = call(
        &client,
        "POST",
        &format!("{base}/layers/{layer}/gates/evaluate"),
        admin,
        json!({"stepId":step}),
    );
    assert_eq!(evaluation["outcome"], "passed");
    let bootstrap = call(
        &client,
        "GET",
        &format!("{base}/bootstrap"),
        admin,
        Value::Null,
    );
    let latest: StepId = bootstrap["layers"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|layer| layer["headStepIds"].as_array().unwrap())
        .filter_map(|value| serde_json::from_value::<StepId>(value.clone()).ok())
        .find(|id| id != step)
        .expect("fixture has a newer Step");
    assert_ne!(
        &latest, step,
        "fixture must move branch while testing historical Step"
    );
    let evaluation = call(
        &client,
        "POST",
        &format!("{base}/layers/{layer}/gates/evaluate"),
        admin,
        json!({"stepId":latest}),
    );
    assert_eq!(
        evaluation["outcome"], "failed",
        "proof for historical N must not cover N+1"
    );
    if std::env::var_os("LAYRS_TEST_EMPTY_TREE_EXPORT_ONLY").is_some() {
        exact_result_runner::empty_tree_export(&client, origin, admin, ca);
    } else {
        exact_result_runner::combined(&client, origin, admin, ca, root);
    }
}
