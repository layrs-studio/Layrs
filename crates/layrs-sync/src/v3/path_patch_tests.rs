use super::*;
use layrs_core::scm::FileId;
use serde_json::json;

fn request() -> PathPatchRequest {
    let hash = format!("blake3:{}", "a".repeat(64));
    let mut value: PathPatchRequest = serde_json::from_value(json!({
        "schemaVersion":"layrs.path-patch.v1", "spaceId":"space:test", "clientId":"client:test",
        "idempotencyKey":"path-patch-test-0001", "requestDigest":hash, "layerId":"layer:main", "baseStepId":hash,
        "expectedRepositoryViewId":hash, "expectedGeneration":4, "expectedPolicySetId":hash,
        "expectedAuthorizationEpoch":7, "changes":[{"path":"file.txt","expected":null,
            "replacement":{"target":{"kind":"file","fileId":hash},"executable":false}}]
    })).unwrap();
    value.request_digest = value.computed_request_digest().unwrap();
    value
}

#[test]
fn digest_binds_scope_authority_expectations_path_and_exact_replacement() {
    let original = request();
    original.validate().unwrap();
    let mut mutations = vec![];
    let mut r = original.clone();
    r.expected_authorization_epoch += 1;
    mutations.push(r);
    let mut r = original.clone();
    r.expected_generation += 1;
    mutations.push(r);
    let mut r = original.clone();
    r.changes[0].path = RepoPath::parse("other").unwrap();
    mutations.push(r);
    let mut r = original.clone();
    r.changes[0].replacement.0.as_mut().unwrap().executable = true;
    mutations.push(r);
    let mut r = original.clone();
    r.changes[0].expected = r.changes[0].replacement.clone();
    mutations.push(r);
    let mut r = original.clone();
    r.space_id = SpaceId::parse("space:foreign").unwrap();
    mutations.push(r);
    let mut r = original.clone();
    r.changes[0].replacement.0.as_mut().unwrap().target =
        TreeEntryTarget::file(FileId::parse(format!("blake3:{}", "b".repeat(64))).unwrap());
    mutations.push(r);
    for changed in mutations {
        assert_ne!(
            changed.computed_request_digest().unwrap(),
            original.request_digest
        );
        assert!(changed.validate().is_err());
    }
    let bytes = serde_jcs::to_vec(&original).unwrap();
    let decoded: PathPatchRequest = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn missing_expectation_unknown_fields_and_bounded_changes_refused() {
    let original = request();
    let mut value = serde_json::to_value(&original).unwrap();
    value["changes"][0]
        .as_object_mut()
        .unwrap()
        .remove("expected");
    assert!(serde_json::from_value::<PathPatchRequest>(value).is_err());
    let mut value = serde_json::to_value(&original).unwrap();
    value["actorId"] = "actor:forged".into();
    assert!(serde_json::from_value::<PathPatchRequest>(value).is_err());
    let mut changed = original;
    changed.changes = vec![changed.changes[0].clone(); MAX_PATH_PATCH_CHANGES + 1];
    assert!(changed.validate().is_err());
}
