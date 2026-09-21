use super::*;
use layrs_core::scm::{ActorId, ObjectId, RepoPath, SpaceId, StepId, TreeId};
use serde_json::json;

fn check(action: V3Action, path: Option<&str>) -> AuthorizationCheck {
    let id = format!("blake3:{}", "a".repeat(64));
    let space = SpaceId::parse("space:scoped").unwrap();
    AuthorizationCheck {
        principal: super::super::PrincipalContext::new(
            PrincipalId::parse("principal:alice").unwrap(),
            ActorId::parse("actor:alice").unwrap(),
            "deployment:test",
            "subject:test",
        )
        .unwrap(),
        action,
        target: match path {
            Some(path) => AuthorizationTarget::ScopedPath {
                space_id: space,
                base_step_id: StepId::parse(&id).unwrap(),
                root_tree_id: TreeId::parse(&id).unwrap(),
                repo_path: RepoPath::parse(path).unwrap(),
            },
            None => AuthorizationTarget::Space(space),
        },
        policy_set_id: ObjectId::parse(id).unwrap(),
        authorization_epoch: 7,
    }
}
fn rules() -> NativePolicyRules {
    serde_json::from_value(json!({"schemaVersion":"layrs.space-policy-rules.v3","rules":[],
        "pathPolicies":[{"principalIds":["principal:alice"],
            "read":{"default":"deny","rules":[{"effect":"allow","selector":{"kind":"subtree","path":"visible"}}]},
            "write":{"default":"deny","rules":[{"effect":"allow","selector":{"kind":"subtree","path":"visible/edit"}}]}}]})).unwrap()
}

#[test]
fn scoped_write_requires_canonical_base_target_even_without_restrictions() {
    assert!(!scoped_target_allowed(
        &check(V3Action::ScopedPathWrite, None),
        &[]
    ));
    let mut wrong = check(V3Action::ScopedPathWrite, Some("visible/edit/a"));
    wrong.target = AuthorizationTarget::Path {
        space_id: SpaceId::parse("space:scoped").unwrap(),
        repo_path: RepoPath::parse("visible/edit/a").unwrap(),
    };
    assert!(!scoped_target_allowed(&wrong, &[]));
    assert!(scoped_target_allowed(
        &check(V3Action::ScopedPathWrite, Some("visible/edit/a")),
        &[]
    ));
}

#[test]
fn projection_envelope_does_not_grant_hidden_leaf_or_write() {
    let rules = [rules()];
    assert!(scoped_target_allowed(
        &check(V3Action::ReadProjection, None),
        &rules
    ));
    for (path, read, write) in [
        ("visible/file", true, false),
        ("visible/edit/file", true, true),
        ("hidden/file", false, false),
        ("visible/editable/file", true, false),
    ] {
        assert_eq!(
            scoped_target_allowed(&check(V3Action::ReadProjection, Some(path)), &rules),
            read
        );
        assert_eq!(
            scoped_target_allowed(&check(V3Action::ScopedPathWrite, Some(path)), &rules),
            write
        );
    }
    assert_eq!(
        V3Action::ReadProjection.as_policy_action(),
        V3Action::Bootstrap.as_policy_action()
    );
    assert_eq!(
        V3Action::ScopedPathWrite.as_policy_action(),
        V3Action::ApplyTransaction.as_policy_action()
    );
}

#[test]
fn v2_intersection_cannot_be_overridden_by_v3_scoped_write() {
    let v2: NativePolicyRules = serde_json::from_value(
        json!({"schemaVersion":"layrs.space-policy-rules.v2","rules":[],
        "contentPolicies":[{"principalIds":["principal:alice"],"default":"allow","rules":[]}]}),
    )
    .unwrap();
    assert!(!scoped_target_allowed(
        &check(V3Action::ScopedPathWrite, Some("visible/edit/file")),
        &[rules(), v2]
    ));
}

#[test]
fn scoped_projection_and_write_require_all_general_read_grants() {
    let grant = |effect, actions: &[&str]| -> NativePolicyRules {
        serde_json::from_value(json!({"schemaVersion":"layrs.space-policy-rules.v1",
            "rules":[{"effect":effect,"principalIds":["principal:alice"],"actions":actions}]}))
        .unwrap()
    };
    let bootstrap = grant("allow", &["space.bootstrap"]);
    let fetch = grant("allow", &["object.fetch", "object.plan_fetch"]);
    let write = grant("allow", &["transaction.apply"]);
    assert_eq!(
        action_grants(&check(V3Action::ReadProjection, None), &[bootstrap.clone()]).unwrap(),
        Some(RuleEffect::Allow)
    );
    let read = check(V3Action::ReadProjection, Some("visible/edit/file"));
    assert_eq!(action_grants(&read, &[bootstrap.clone()]).unwrap(), None);
    assert_eq!(
        action_grants(&read, &[bootstrap.clone(), fetch.clone()]).unwrap(),
        Some(RuleEffect::Allow)
    );
    let edit = check(V3Action::ScopedPathWrite, Some("visible/edit/file"));
    assert_eq!(
        action_grants(&edit, &[bootstrap.clone(), write.clone()]).unwrap(),
        None
    );
    let all = [bootstrap, fetch, write];
    assert_eq!(action_grants(&edit, &all).unwrap(), Some(RuleEffect::Allow));
    for denied in [
        "object.fetch",
        "object.plan_fetch",
        "space.bootstrap",
        "transaction.apply",
    ] {
        let mut policies = all.to_vec();
        policies.push(grant("deny", &[denied]));
        assert_eq!(
            action_grants(&edit, &policies).unwrap(),
            Some(RuleEffect::Deny)
        );
        policies.reverse();
        assert_eq!(
            action_grants(&edit, &policies).unwrap(),
            Some(RuleEffect::Deny)
        );
    }
}
