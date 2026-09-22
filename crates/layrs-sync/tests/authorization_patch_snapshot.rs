use layrs_core::scm::{ObjectId, RepoPath, StepId};
use layrs_sync::v3::*;

fn snapshot() -> AuthorizationSnapshot {
    let mut snapshot: AuthorizationSnapshot = serde_json::from_str(include_str!(
        "../../../docs/contracts/sync-v3/examples/authorization-snapshot.json"
    ))
    .unwrap();
    snapshot.schema_version = AuthorizationSnapshotSchemaVersion::V2;
    snapshot.request_binding.path_patch_request_digest =
        Some(ObjectId::parse(format!("blake3:{}", "1".repeat(64))).unwrap());
    snapshot.decisions[0].action = "transaction.apply".into();
    snapshot.decisions[0].scope = AuthorizationScope::Path {
        root: AuthorizationRoot::Step {
            step_id: StepId::parse(format!("blake3:{}", "2".repeat(64))).unwrap(),
        },
        repo_path: RepoPath::parse("public/a").unwrap(),
    };
    snapshot.authorization_snapshot_id = snapshot.computed_id().unwrap();
    snapshot
}
fn invalid(mut snapshot: AuthorizationSnapshot) {
    snapshot
        .decisions
        .sort_by_cached_key(|d| serde_jcs::to_string(d).unwrap());
    snapshot.authorization_snapshot_id = snapshot.computed_id().unwrap();
    assert!(snapshot.validate_v3().is_err());
}
#[test]
fn v2_requires_path_evidence_even_after_rehash() {
    let base = snapshot();
    base.validate_v3().unwrap();
    let mut bad = base.clone();
    bad.request_binding.path_patch_request_digest = None;
    invalid(bad);
    let mut bad = base.clone();
    bad.schema_version = AuthorizationSnapshotSchemaVersion::V1;
    invalid(bad);
    let mut bad = base.clone();
    bad.decisions.clear();
    invalid(bad);
    let mut bad = base.clone();
    bad.decisions[0].action = "object.fetch".into();
    invalid(bad);
    let mut bad = base.clone();
    bad.decisions[0].scope = AuthorizationScope::Space {
        space_id: base.space_id.clone(),
    };
    invalid(bad);
    let mut bad = base.clone();
    bad.decisions.push(base.decisions[0].clone());
    bad.decisions[1].allowed = false;
    invalid(bad);
    let mut bad = base.clone();
    bad.decisions.push(base.decisions[0].clone());
    bad.decisions[1].scope = AuthorizationScope::Path {
        root: AuthorizationRoot::Step {
            step_id: StepId::parse(format!("blake3:{}", "3".repeat(64))).unwrap(),
        },
        repo_path: RepoPath::parse("public/b").unwrap(),
    };
    invalid(bad);
}
#[test]
fn v1_remains_unchanged_without_patch_binding() {
    let mut snapshot: AuthorizationSnapshot = serde_json::from_str(include_str!(
        "../../../docs/contracts/sync-v3/examples/authorization-snapshot.json"
    ))
    .unwrap();
    snapshot.authorization_snapshot_id = snapshot.computed_id().unwrap();
    snapshot.validate_v3().unwrap();
    assert!(
        !serde_jcs::to_string(&snapshot)
            .unwrap()
            .contains("pathPatchRequestDigest")
    );
}
