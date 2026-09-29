use super::*;
use serde_json::{Value, json};

fn principal() -> PrincipalId {
    PrincipalId::parse("principal:editor").unwrap()
}
fn policy() -> Value {
    json!({
        "schemaVersion":"layrs.space-policy-rules.v3",
        "rules":[{"principalIds":[principal()],"effect":"allow","actions":["space.bootstrap","transaction.apply"]}],
        "pathPolicies":[{
            "principalIds":[principal()],
            "read":{"default":"deny","rules":[{"effect":"allow","selector":{"kind":"subtree","path":"shared"}}]},
            "write":{"default":"deny","rules":[{"effect":"allow","selector":{"kind":"subtree","path":"shared/edit"}}]}
        }]
    })
}
fn parse(value: Value) -> NativePolicyRules {
    serde_json::from_value(value).unwrap()
}

#[test]
fn member_read_write_permissions_and_namespace_are_distinct() {
    let rules = parse(policy());
    assert!(rules.restricted(&principal()));
    assert_eq!(
        rules.evaluate(&principal(), "transaction.apply").unwrap(),
        Some(RuleEffect::Allow)
    );
    for (path, read, write) in [
        ("shared/read.txt", true, false),
        ("shared/edit/new.txt", true, true),
        ("shared/editor/new.txt", true, false),
        ("private/new.txt", false, false),
        ("Shared/edit/new.txt", false, false),
    ] {
        let path = RepoPath::parse(path).unwrap();
        assert_eq!(rules.content_allowed(&principal(), &path), read);
        assert_eq!(rules.write_allowed(&principal(), &path), write);
    }
    let other = PrincipalId::parse("principal:other").unwrap();
    assert!(!rules.restricted(&other));
    // No matching path restriction does not invent an action grant.
    assert_eq!(rules.evaluate(&other, "transaction.apply").unwrap(), None);
}

#[test]
fn write_allow_never_overrides_read_deny_in_any_policy_order() {
    for reversed in [false, true] {
        let mut value = policy();
        let deny = json!({"principalIds":[principal()],
            "read":{"default":"allow","rules":[{"effect":"deny","selector":{"kind":"exact","path":"shared/edit/secret"}}]},
            "write":{"default":"allow","rules":[]}});
        let policies = value["pathPolicies"].as_array_mut().unwrap();
        policies.push(deny);
        if reversed {
            policies.reverse();
        }
        let rules = parse(value);
        let hidden = RepoPath::parse("shared/edit/secret").unwrap();
        assert!(!rules.content_allowed(&principal(), &hidden));
        assert!(!rules.write_allowed(&principal(), &hidden));
        assert!(rules.write_allowed(&principal(), &RepoPath::parse("shared/edit/ok").unwrap()));
    }
    let mut blind = policy();
    blind["pathPolicies"][0]["write"] = json!({"default":"allow","rules":[]});
    assert!(!parse(blind).write_allowed(&principal(), &RepoPath::parse("private").unwrap()));
}

#[test]
fn write_denial_wins_without_removing_read_access() {
    for reversed in [false, true] {
        let mut value = policy();
        let entries = value["pathPolicies"][0]["write"]["rules"]
            .as_array_mut()
            .unwrap();
        entries
            .push(json!({"effect":"deny","selector":{"kind":"exact","path":"shared/edit/frozen"}}));
        if reversed {
            entries.reverse();
        }
        let rules = parse(value);
        let frozen = RepoPath::parse("shared/edit/frozen").unwrap();
        assert!(rules.content_allowed(&principal(), &frozen));
        assert!(!rules.write_allowed(&principal(), &frozen));
    }
}

#[test]
fn v1_v2_contracts_are_unchanged_and_v2_restriction_stays_read_only() {
    let v1 = parse(json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[]}));
    let v2 = parse(
        json!({"schemaVersion":"layrs.space-policy-rules.v2","rules":[],
        "contentPolicies":[{"principalIds":[principal()],"default":"allow","rules":[]}]}),
    );
    let path = RepoPath::parse("shared/edit/file").unwrap();
    assert!(v1.write_allowed(&principal(), &path));
    assert!(v2.content_allowed(&principal(), &path));
    assert!(!v2.write_allowed(&principal(), &path));
    for version in ["layrs.space-policy-rules.v1", "layrs.space-policy-rules.v2"] {
        let mut invalid_version = policy();
        invalid_version["schemaVersion"] = version.into();
        assert!(serde_json::from_value::<NativePolicyRules>(invalid_version).is_err());
    }
    let canonical = serde_json::to_value(&v1).unwrap();
    assert_eq!(
        canonical,
        json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[]})
    );
}

#[test]
fn malformed_version_fields_paths_actions_and_bounds_fail_closed() {
    for field in ["read", "write"] {
        let mut missing = policy();
        missing["pathPolicies"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<NativePolicyRules>(missing).is_err());
    }
    for path in [
        "../shared",
        "shared/../secret",
        "shared//edit",
        "shared\\edit",
        "e\u{301}",
    ] {
        let mut value = policy();
        value["pathPolicies"][0]["write"]["rules"][0]["selector"]["path"] = path.into();
        assert!(
            serde_json::from_value::<NativePolicyRules>(value).is_err(),
            "{path}"
        );
    }
    let mut future = policy();
    future["schemaVersion"] = "layrs.space-policy-rules.v4".into();
    assert!(serde_json::from_value::<NativePolicyRules>(future).is_err());
    let mut unknown = policy();
    unknown["pathPolicies"][0]["read"]["extra"] = true.into();
    assert!(serde_json::from_value::<NativePolicyRules>(unknown).is_err());
    let mut action = policy();
    action["rules"][0]["actions"] = json!(["*"]);
    assert!(
        parse(action)
            .evaluate(&principal(), "transaction.apply")
            .is_err()
    );
    let mut oversized = policy();
    let rule = oversized["pathPolicies"][0]["write"]["rules"][0].clone();
    oversized["pathPolicies"][0]["write"]["rules"] = json!(vec![rule; 257]);
    let rules = parse(oversized);
    assert!(rules.evaluate(&principal(), "transaction.apply").is_err());
    assert!(!rules.write_allowed(&principal(), &RepoPath::parse("shared/edit/file").unwrap()));
    let mut cumulative = policy();
    let p = cumulative["pathPolicies"][0].clone();
    cumulative["pathPolicies"] = json!(vec![p; 65]);
    assert!(
        parse(cumulative)
            .evaluate(&principal(), "transaction.apply")
            .is_err()
    );
}
