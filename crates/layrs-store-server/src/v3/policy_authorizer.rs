//! Bounded, explicit whole-Space authorization for native self-hosting.
use super::{
    load_space_descriptor, pg_error, read_canonical_object, AuthorizationCheck,
    AuthorizationDecision, AuthorizationTarget, CatalogedCas, PolicyAuthorizer, V3Action,
    V3StoreError, V3StoreResult,
};
use async_trait::async_trait;
use layrs_sync::v3::{NativePolicyRules, RuleEffect};
use layrs_sync::v3::{ObjectKind, PolicySet, ProductEnvelope};
#[cfg(test)]
use layrs_sync::v3::{PrincipalId, SpacePolicyRule, SpacePolicyRules, SpacePolicyRulesVersion};
use sqlx::{PgPool, Row};
use std::sync::Arc;

#[derive(Clone)]
pub struct PgPolicyAuthorizer {
    pool: PgPool,
    cas: Arc<dyn CatalogedCas>,
}
impl PgPolicyAuthorizer {
    pub fn new(pool: PgPool, cas: Arc<dyn CatalogedCas>) -> Self {
        Self { pool, cas }
    }
}

#[async_trait]
impl PolicyAuthorizer for PgPolicyAuthorizer {
    async fn authorize(&self, check: &AuthorizationCheck) -> V3StoreResult<AuthorizationDecision> {
        let row=sqlx::query("SELECT s.authorization_epoch,s.current_policy_set_id,p.status AS principal_status,wm.state AS membership_state,s.lifecycle_state,d.status AS deployment_status FROM principals p JOIN spaces s ON s.space_id=$2 JOIN workspaces w ON w.workspace_id=s.workspace_id AND w.deployment_id=p.deployment_id JOIN deployments d ON d.deployment_id=p.deployment_id JOIN workspace_memberships wm ON wm.workspace_id=w.workspace_id AND wm.principal_id=p.principal_id WHERE p.principal_id=$1 AND p.actor_id=$3 AND p.deployment_id=$4 AND p.subject_digest=$5")
            .bind(check.principal.principal_id.as_str()).bind(check.target.space_id().as_str()).bind(check.principal.actor_id.as_str()).bind(&check.principal.deployment_id).bind(&check.principal.subject_digest).fetch_optional(&self.pool).await.map_err(pg_error)?;
        let Some(row) = row else {
            return Ok(AuthorizationDecision::deny("membership_not_active"));
        };
        if row.get::<i64, _>("authorization_epoch")
            != i64::try_from(check.authorization_epoch)
                .map_err(|_| V3StoreError::InvalidCapability)?
            || row.get::<String, _>("current_policy_set_id") != check.policy_set_id.as_str()
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        if row.get::<String, _>("principal_status") != "active"
            || row.get::<String, _>("membership_state") != "active"
            || row.get::<String, _>("lifecycle_state") != "ready"
            || row.get::<String, _>("deployment_status") != "ready"
        {
            return Ok(AuthorizationDecision::deny("identity_or_space_inactive"));
        }
        let space = check.target.space_id();
        let descriptor = load_space_descriptor(&self.pool, space, &check.policy_set_id).await?;
        if descriptor.kind != ObjectKind::PolicySet {
            return Err(V3StoreError::IdentityMismatch);
        }
        let bytes = read_canonical_object(self.cas.as_ref(), &descriptor, 1024 * 1024).await?;
        let set: PolicySet =
            serde_json::from_slice(&bytes).map_err(|_| V3StoreError::InvalidCapability)?;
        if set.space_id != *space
            || set.policy_set_id != check.policy_set_id
            || set
                .computed_id()
                .map_err(|_| V3StoreError::IdentityMismatch)?
                != check.policy_set_id
            || set.policy_versions.len() > 32
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        let mut allowed = false;
        let mut content_rules = Vec::new();
        for entry in set.policy_versions {
            let descriptor =
                load_space_descriptor(&self.pool, space, &entry.policy_product_object_id).await?;
            if descriptor.kind != ObjectKind::Product {
                return Err(V3StoreError::IdentityMismatch);
            }
            let bytes = read_canonical_object(self.cas.as_ref(), &descriptor, 1024 * 1024).await?;
            let product: ProductEnvelope =
                serde_json::from_slice(&bytes).map_err(|_| V3StoreError::InvalidCapability)?;
            let ProductEnvelope::Policy { record, .. } = product else {
                return Err(V3StoreError::InvalidCapability);
            };
            if record.space_id != *space
                || record.policy_id != entry.policy_id
                || record.version_id != entry.policy_version_id
                || !entry
                    .dependency_object_ids
                    .contains(&record.rules_object_id)
            {
                return Err(V3StoreError::InvalidCapability);
            }
            let descriptor =
                load_space_descriptor(&self.pool, space, &record.rules_object_id).await?;
            if descriptor.kind != ObjectKind::Evidence {
                return Err(V3StoreError::IdentityMismatch);
            }
            let bytes = read_canonical_object(self.cas.as_ref(), &descriptor, 1024 * 1024).await?;
            let rules: NativePolicyRules =
                serde_json::from_slice(&bytes).map_err(|_| V3StoreError::InvalidCapability)?;
            match rules
                .evaluate(
                    &check.principal.principal_id,
                    check.action.as_policy_action(),
                )
                .map_err(|_| V3StoreError::InvalidCapability)?
            {
                Some(RuleEffect::Deny) => {
                    return Ok(AuthorizationDecision::deny("explicit_policy_deny"));
                }
                Some(RuleEffect::Allow) => allowed = true,
                None => {}
            }
            content_rules.push(rules);
        }
        let restricted = content_rules
            .iter()
            .any(|r| r.restricted(&check.principal.principal_id));
        if restricted {
            let content_allowed = match check.action {
                V3Action::Bootstrap | V3Action::PlanFetch | V3Action::ReadEvents => true,
                V3Action::FetchObject => match &check.target {
                    AuthorizationTarget::Space(_) => true, // Envelope check only; no bytes.
                    AuthorizationTarget::ContentPath { repo_path, .. } => content_rules
                        .iter()
                        .all(|r| r.content_allowed(&check.principal.principal_id, repo_path)),
                    AuthorizationTarget::Object { object_id, .. } => {
                        let descriptor =
                            load_space_descriptor(&self.pool, space, object_id).await?;
                        matches!(
                            descriptor.kind,
                            ObjectKind::Step
                                | ObjectKind::Tree
                                | ObjectKind::RepositoryView
                                | ObjectKind::PolicySet
                        )
                    }
                    _ => false,
                },
                _ => false,
            };
            if !content_allowed {
                let mut decision = AuthorizationDecision::deny("content_access_restricted");
                decision.content_restricted = true;
                return Ok(decision);
            }
        }
        let mut decision = if allowed {
            AuthorizationDecision::allow("explicit_policy_allow")
        } else {
            AuthorizationDecision::deny("no_policy_grant")
        };
        decision.content_restricted = restricted;
        Ok(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn principal() -> PrincipalId {
        PrincipalId::parse("principal:alice").unwrap()
    }
    fn rule(effect: RuleEffect) -> SpacePolicyRule {
        SpacePolicyRule {
            effect,
            principal_ids: vec![principal()],
            actions: vec!["transaction.apply".into()],
        }
    }
    #[test]
    fn empty_and_unrelated_rules_grant_nothing() {
        let policy = SpacePolicyRules {
            schema_version: SpacePolicyRulesVersion::V1,
            rules: vec![],
        };
        assert_eq!(
            policy.evaluate(&principal(), "transaction.apply").unwrap(),
            None
        );
        let policy = SpacePolicyRules {
            rules: vec![rule(RuleEffect::Allow)],
            ..policy
        };
        assert_eq!(policy.evaluate(&principal(), "object.fetch").unwrap(), None);
        assert_eq!(
            policy
                .evaluate(
                    &PrincipalId::parse("principal:bob").unwrap(),
                    "transaction.apply"
                )
                .unwrap(),
            None
        );
    }
    #[test]
    fn deny_overrides_allow_in_every_order() {
        for rules in [
            vec![rule(RuleEffect::Allow), rule(RuleEffect::Deny)],
            vec![rule(RuleEffect::Deny), rule(RuleEffect::Allow)],
        ] {
            assert_eq!(
                SpacePolicyRules {
                    schema_version: SpacePolicyRulesVersion::V1,
                    rules
                }
                .evaluate(&principal(), "transaction.apply")
                .unwrap(),
                Some(RuleEffect::Deny)
            );
        }
    }
    #[test]
    fn unknown_versions_actions_and_fields_fail_closed() {
        assert!(serde_json::from_str::<SpacePolicyRules>(
            r#"{"schemaVersion":"future","rules":[]}"#
        )
        .is_err());
        assert!(serde_json::from_str::<SpacePolicyRules>(
            r#"{"schemaVersion":"layrs.space-policy-rules.v1","rules":[],"allowEveryone":true}"#
        )
        .is_err());
        let mut bad = rule(RuleEffect::Allow);
        bad.actions = vec!["*".into()];
        assert!(SpacePolicyRules {
            schema_version: SpacePolicyRulesVersion::V1,
            rules: vec![bad]
        }
        .evaluate(&principal(), "transaction.apply")
        .is_err());
    }
}
