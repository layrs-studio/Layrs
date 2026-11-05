//! Versioned path permissions. A write grant never overrides a read denial.
use super::*;
#[cfg(test)]
#[path = "native_policy_v3_tests.rs"]
mod tests;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathPolicyRules {
    pub schema_version: PathPolicyRulesVersion,
    pub rules: Vec<SpacePolicyRule>,
    pub path_policies: Vec<PathAccessPolicy>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PathPolicyRulesVersion {
    #[serde(rename = "layrs.space-policy-rules.v3")]
    V3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathAccessPolicy {
    pub principal_ids: Vec<PrincipalId>,
    pub read: PathPermission,
    pub write: PathPermission,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathPermission {
    pub default: RuleEffect,
    pub rules: Vec<ContentPathRule>,
}

impl PathPermission {
    fn allowed(&self, path: &RepoPath) -> bool {
        let mut allowed = self.default == RuleEffect::Allow;
        for rule in &self.rules {
            if rule.selector.matches(path) {
                if rule.effect == RuleEffect::Deny {
                    return false;
                }
                allowed = true;
            }
        }
        allowed
    }
}

impl PathPolicyRules {
    fn validate(&self) -> PolicyRulesResult<()> {
        if self.path_policies.is_empty() || self.path_policies.len() > 64 {
            return Err(invalid());
        }
        let mut total = 0usize;
        for policy in &self.path_policies {
            if policy.principal_ids.is_empty() || policy.principal_ids.len() > 256 {
                return Err(invalid());
            }
            for permission in [&policy.read, &policy.write] {
                if permission.rules.len() > 256 {
                    return Err(invalid());
                }
                total += permission.rules.len();
                if total > 1024
                    || permission
                        .rules
                        .iter()
                        .any(|r| r.selector.path().as_str().len() > 4096)
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }

    pub(super) fn evaluate(
        &self,
        principal: &PrincipalId,
        action: &str,
    ) -> PolicyRulesResult<Option<RuleEffect>> {
        self.validate()?;
        SpacePolicyRules {
            schema_version: SpacePolicyRulesVersion::V1,
            rules: self.rules.clone(),
        }
        .evaluate(principal, action)
    }

    pub(super) fn read_allowed(&self, principal: &PrincipalId, path: &RepoPath) -> bool {
        self.validate().is_ok()
            && self
                .path_policies
                .iter()
                .filter(|p| p.principal_ids.contains(principal))
                .all(|p| p.read.allowed(path))
    }

    pub(super) fn write_allowed(&self, principal: &PrincipalId, path: &RepoPath) -> bool {
        self.read_allowed(principal, path)
            && self
                .path_policies
                .iter()
                .filter(|p| p.principal_ids.contains(principal))
                .all(|p| p.write.allowed(path))
    }
}
