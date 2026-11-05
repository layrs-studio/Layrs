use layrs_core::scm::{SpaceId, StepId};
pub fn run(
    origin: &str,
    token: &str,
    ca: &str,
    space: &SpaceId,
    foreign_space: &SpaceId,
    before: &StepId,
    after: &StepId,
) {
    let client = reqwest::blocking::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let compare = |space: &SpaceId, before: &StepId, after: &StepId| {
        client
            .post(format!("{origin}/v3/spaces/{space}/compare"))
            .bearer_auth(token)
            .json(&serde_json::json!({"before":before,"after":after}))
            .send()
            .unwrap()
    };
    let response = compare(space, before, after);
    assert!(
        response.status().is_success(),
        "comparison HTTP {}",
        response.status()
    );
    let value: serde_json::Value = response.json().unwrap();
    assert_eq!(value["before"], before.as_str());
    assert_eq!(value["after"], after.as_str());
    let changes = value["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0]["path"], "alice.txt");
    assert_eq!(changes[0]["status"], "added");
    assert_eq!(changes[0]["textDiff"][0]["text"], "alice concurrent");
    assert_eq!(changes[1]["path"], "bob.txt");
    assert_eq!(changes[2]["path"], "docs/readme.txt");
    assert_eq!(
        compare(space, after, after)
            .json::<serde_json::Value>()
            .unwrap()["changes"],
        serde_json::json!([])
    );
    assert!(
        !compare(foreign_space, before, after).status().is_success(),
        "Steps admitted only to original Space must not leak through another Space"
    );
    let missing = StepId::parse(format!("blake3:{}", "c".repeat(64))).unwrap();
    assert!(!compare(space, &missing, after).status().is_success());
    assert_eq!(
        client
            .post(format!("{origin}/v3/spaces/{space}/compare"))
            .json(&serde_json::json!({"before":before,"after":after}))
            .send()
            .unwrap()
            .status(),
        401
    );
}
