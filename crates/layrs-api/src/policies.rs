use crate::common::PermissionMode;
use crate::ids::{PolicyId, PrincipalId, SpaceId, WorkspaceId};
use crate::validation::{ApiResult, Validate, bounded_len, max_items, required};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PolicyEffect {
    Allow,
    Deny,
    RequireProof,
    RequireGate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PolicyOperation {
    Read,
    Create,
    Update,
    Delete,
    Publish,
    Approve,
    Administer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyRule {
    pub rule_id: String,
    pub effect: PolicyEffect,
    pub operation: PolicyOperation,
    pub object_kind: String,
    pub proof_kind: Option<String>,
}

impl PolicyRule {
    pub fn validate(&self) -> ApiResult<()> {
        required("rule_id", &self.rule_id)?;
        required("object_kind", &self.object_kind)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatePolicyRequest {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub rules: Vec<PolicyRule>,
}

impl Validate for CreatePolicyRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        bounded_len("name", &self.name, 2, 128)?;
        max_items("rules", &self.rules, 256)?;
        for rule in &self.rules {
            rule.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluatePolicyRequest {
    pub workspace_id: WorkspaceId,
    pub space_id: Option<SpaceId>,
    pub principal_id: PrincipalId,
    pub operation: PolicyOperation,
    pub object_kind: String,
    pub object_id: String,
}

impl Validate for EvaluatePolicyRequest {
    fn validate(&self) -> ApiResult<()> {
        self.workspace_id.validate_field("workspace_id")?;
        if let Some(space_id) = &self.space_id {
            space_id.validate_field("space_id")?;
        }
        self.principal_id.validate_field("principal_id")?;
        required("object_kind", &self.object_kind)?;
        required("object_id", &self.object_id)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyResponse {
    pub workspace_id: WorkspaceId,
    pub policy_id: PolicyId,
    pub name: String,
    pub rules: Vec<PolicyRule>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyDecisionResponse {
    pub allowed: bool,
    pub effective_permission: Option<PermissionMode>,
    pub required_proofs: Vec<String>,
    pub matched_policy_ids: Vec<PolicyId>,
    pub message: Option<String>,
}
