use layrs_core::scm::{ObjectId, RepoPath, StepId};
use layrs_sync::v3::*;

#[test]
fn exact_projection_request_is_bounded_sorted_and_closed() {
    let mut request = ProjectionAccessRequest {
        step_id: StepId::parse(format!("blake3:{}", "1".repeat(64))).unwrap(),
        expected_policy_set_id: ObjectId::parse(format!("blake3:{}", "2".repeat(64))).unwrap(),
        expected_authorization_epoch: 1,
        paths: vec![RepoPath::parse("public/a").unwrap()],
    };
    request.validate().unwrap();
    let mut unknown = serde_json::to_value(&request).unwrap();
    unknown["actor"] = serde_json::json!("principal:forged");
    assert!(serde_json::from_value::<ProjectionAccessRequest>(unknown).is_err());
    request.paths.push(request.paths[0].clone());
    assert!(request.validate().is_err());
    request.paths = (0..200)
        .map(|n| RepoPath::parse(format!("a/{n:03}")).unwrap())
        .collect();
    request.validate().unwrap();
    request.paths.push(RepoPath::parse("b").unwrap());
    assert!(request.validate().is_err());
    request.paths.truncate(2);
    request.paths.reverse();
    assert!(request.validate().is_err());
    request.paths.clear();
    assert!(request.validate().is_err());
    request.paths = vec![RepoPath::parse("a").unwrap()];
    request.expected_authorization_epoch = 0;
    assert!(request.validate().is_err());
}
