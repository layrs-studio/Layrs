//! Closed native whole-Space and content rule documents shared by operator and server.
use super::{PrincipalId, V3ValidationError};
use layrs_core::scm::RepoPath;
use serde::{Deserialize, Serialize};
#[path = "native_policy_v3.rs"]
mod v3;
pub use v3::*;
type PolicyRulesResult<T> = Result<T, V3ValidationError>;
fn invalid() -> V3ValidationError {
    V3ValidationError {
        code: "invalid_policy_rules",
        field: "rules",
        message: "native policy rules exceed bounds or use an unsupported value".into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpacePolicyRules {
    pub schema_version: SpacePolicyRulesVersion,
    pub rules: Vec<SpacePolicyRule>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SpacePolicyRulesVersion {
    #[serde(rename = "layrs.space-policy-rules.v1")]
    V1,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpacePolicyRule {
    pub effect: RuleEffect,
    pub principal_ids: Vec<PrincipalId>,
    /// Explicit policy action names; no wildcard or implicit administrator grant.
    pub actions: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleEffect {
    Allow,
    Deny,
}

impl SpacePolicyRules {
    pub fn evaluate(
        &self,
        principal: &PrincipalId,
        action: &str,
    ) -> PolicyRulesResult<Option<RuleEffect>> {
        self.evaluate_with(principal, action, known_action)
    }

    fn evaluate_with(
        &self,
        principal: &PrincipalId,
        action: &str,
        known: fn(&str) -> bool,
    ) -> PolicyRulesResult<Option<RuleEffect>> {
        if self.rules.len() > 256 {
            return Err(invalid());
        }
        let mut allowed = false;
        let mut denied = false;
        for rule in &self.rules {
            if rule.principal_ids.is_empty()
                || rule.principal_ids.len() > 256
                || rule.actions.is_empty()
                || rule.actions.len() > 32
                || rule.actions.iter().any(|a| !known(a))
            {
                return Err(invalid());
            }
            if rule.principal_ids.contains(principal) && rule.actions.iter().any(|a| a == action) {
                if rule.effect == RuleEffect::Deny {
                    denied = true;
                } else {
                    allowed = true;
                }
            }
        }
        Ok(if denied {
            Some(RuleEffect::Deny)
        } else if allowed {
            Some(RuleEffect::Allow)
        } else {
            None
        })
    }
}
fn known_action(action: &str) -> bool {
    matches!(
        action,
        "space.bootstrap"
            | "object.plan_upload"
            | "object.complete_upload"
            | "object.plan_fetch"
            | "object.fetch"
            | "event.read"
            | "transaction.apply"
            | "policy.administer"
            | "audit.read_evidence"
            | "review.read"
            | "review.write"
            | "review.approve"
            | "proof.run"
            | "release.read"
            | "release.publish"
            | "release.revoke"
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NativePolicyRules {
    V1(SpacePolicyRules),
    V2(ContentPolicyRules),
    V3(PathPolicyRules),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentPolicyRules {
    pub schema_version: ContentPolicyRulesVersion,
    pub rules: Vec<SpacePolicyRule>,
    pub content_policies: Vec<ContentPathPolicy>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ContentPolicyRulesVersion {
    #[serde(rename = "layrs.space-policy-rules.v2")]
    V2,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentPathPolicy {
    pub principal_ids: Vec<PrincipalId>,
    pub default: RuleEffect,
    pub rules: Vec<ContentPathRule>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPathRule {
    pub effect: RuleEffect,
    pub selector: ContentPathSelector,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentPathSelector {
    Exact { path: RepoPath },
    Subtree { path: RepoPath },
}
impl ContentPathSelector {
    fn path(&self) -> &RepoPath {
        match self {
            Self::Exact { path } | Self::Subtree { path } => path,
        }
    }
    fn matches(&self, value: &RepoPath) -> bool {
        let prefix = self.path().as_str();
        value == self.path()
            || matches!(self, Self::Subtree { .. })
                && value
                    .as_str()
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
    }
}
impl NativePolicyRules {
    pub fn evaluate(
        &self,
        principal: &PrincipalId,
        action: &str,
    ) -> PolicyRulesResult<Option<RuleEffect>> {
        match self {
            Self::V1(rules) => rules.evaluate(principal, action),
            Self::V3(rules) => rules.evaluate(principal, action),
            Self::V2(rules) => {
                if rules.content_policies.is_empty() || rules.content_policies.len() > 64 {
                    return Err(invalid());
                }
                let mut total = 0usize;
                for policy in &rules.content_policies {
                    if policy.principal_ids.is_empty()
                        || policy.principal_ids.len() > 256
                        || policy.rules.len() > 256
                    {
                        return Err(invalid());
                    }
                    total += policy.rules.len();
                    for rule in &policy.rules {
                        if rule.selector.path().as_str().len() > 4096 {
                            return Err(invalid());
                        }
                    }
                }
                if total > 1024 {
                    return Err(invalid());
                }
                SpacePolicyRules {
                    schema_version: SpacePolicyRulesVersion::V1,
                    rules: rules.rules.clone(),
                }
                .evaluate(principal, action)
            }
        }
    }
    pub fn restricted(&self, principal: &PrincipalId) -> bool {
        match self {
            Self::V1(_) => false,
            Self::V2(rules) => rules
                .content_policies
                .iter()
                .any(|p| p.principal_ids.contains(principal)),
            Self::V3(rules) => rules
                .path_policies
                .iter()
                .any(|p| p.principal_ids.contains(principal)),
        }
    }
    pub fn content_allowed(&self, principal: &PrincipalId, path: &RepoPath) -> bool {
        if let Self::V3(rules) = self {
            return rules.read_allowed(principal, path);
        }
        let Self::V2(rules) = self else {
            return true;
        };
        rules
            .content_policies
            .iter()
            .filter(|p| p.principal_ids.contains(principal))
            .all(|policy| {
                let mut allowed = policy.default == RuleEffect::Allow;
                for rule in &policy.rules {
                    if rule.selector.matches(path) {
                        if rule.effect == RuleEffect::Deny {
                            return false;
                        }
                        allowed = true;
                    }
                }
                allowed
            })
    }

    /// A v2 content restriction remains read-only even when another Policy grants a patch action.
    /// Every Policy in the captured PolicySet must agree before a scoped write is permitted.
    pub fn write_allowed(&self, principal: &PrincipalId, path: &RepoPath) -> bool {
        match self {
            Self::V1(_) => true,
            Self::V2(_) => !self.restricted(principal),
            Self::V3(rules) => rules.write_allowed(principal, path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_subtree_defaults_deny_and_principal_are_distinct() {
        let principal = PrincipalId::parse("principal:reader").unwrap();
        let rules: NativePolicyRules = serde_json::from_value(serde_json::json!({"schemaVersion":"layrs.space-policy-rules.v2","rules":[],"contentPolicies":[{"principalIds":[principal],"default":"deny","rules":[{"effect":"allow","selector":{"kind":"subtree","path":"public"}},{"effect":"deny","selector":{"kind":"exact","path":"public/private.txt"}}]}]})).unwrap();
        assert!(rules.evaluate(&principal, "object.fetch").is_ok());
        for (path, allowed) in [
            ("public", true),
            ("public/a", true),
            ("publicity/a", false),
            ("public/private.txt", false),
            ("Public/a", false),
            ("secret", false),
        ] {
            assert_eq!(
                rules.content_allowed(&principal, &RepoPath::parse(path).unwrap()),
                allowed
            );
        }
        assert!(rules.restricted(&principal));
        assert!(!rules.restricted(&PrincipalId::parse("principal:other").unwrap()));
        assert!(serde_json::from_value::<NativePolicyRules>(serde_json::json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[],"contentPolicies":[]})).is_err());
    }
    #[test]
    fn deny_wins_across_policies_and_rule_order_without_creating_action_grants() {
        let principal = PrincipalId::parse("principal:reader").unwrap();
        for reverse in [false, true] {
            let mut entries = vec![
                serde_json::json!({"principalIds":[principal],"default":"allow","rules":[]}),
                serde_json::json!({"principalIds":[principal],"default":"allow","rules":[{"effect":"allow","selector":{"kind":"subtree","path":"public"}},{"effect":"deny","selector":{"kind":"subtree","path":"public"}}]}),
            ];
            if reverse {
                entries.reverse();
            }
            let rules:NativePolicyRules=serde_json::from_value(serde_json::json!({"schemaVersion":"layrs.space-policy-rules.v2","rules":[],"contentPolicies":entries})).unwrap();
            assert_eq!(rules.evaluate(&principal, "object.fetch").unwrap(), None);
            assert!(!rules.content_allowed(&principal, &RepoPath::parse("public/a").unwrap()));
            assert!(rules.content_allowed(&principal, &RepoPath::parse("elsewhere").unwrap()));
        }
    }
    #[test]
    fn malformed_selectors_future_schema_and_limits_fail_closed() {
        let principal = PrincipalId::parse("principal:reader").unwrap();
        let valid = serde_json::json!({"schemaVersion":"layrs.space-policy-rules.v2","rules":[],"contentPolicies":[{"principalIds":[principal],"default":"deny","rules":[{"effect":"allow","selector":{"kind":"exact","path":"public"}}]}]});
        for path in [
            "../public",
            "a/../public",
            "/public",
            "a//public",
            "a\\public",
            "",
        ] {
            let mut value = valid.clone();
            value["contentPolicies"][0]["rules"][0]["selector"]["path"] = path.into();
            assert!(
                serde_json::from_value::<NativePolicyRules>(value).is_err(),
                "{path}"
            );
        }
        let mut future = valid.clone();
        future["schemaVersion"] = "layrs.space-policy-rules.v3".into();
        assert!(serde_json::from_value::<NativePolicyRules>(future).is_err());
        let mut regex = valid.clone();
        regex["contentPolicies"][0]["rules"][0]["selector"]["kind"] = "regex".into();
        assert!(serde_json::from_value::<NativePolicyRules>(regex).is_err());
        let mut excessive = valid;
        let rule = excessive["contentPolicies"][0]["rules"][0].clone();
        excessive["contentPolicies"][0]["rules"] = serde_json::json!(vec![rule; 257]);
        let rules: NativePolicyRules = serde_json::from_value(excessive).unwrap();
        assert!(rules.evaluate(&principal, "object.fetch").is_err());
    }
}
