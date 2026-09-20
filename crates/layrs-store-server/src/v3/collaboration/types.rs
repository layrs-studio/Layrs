use crate::v3::{V3StoreError, V3StoreResult};
use layrs_core::scm::{LayerId, ObjectId, RepoPath, RepositoryViewId, SpaceId, StepId, Timestamp};
use layrs_sync::v3::PrincipalId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewProposalRequest {
    pub idempotency_key: String,
    pub layer_id: LayerId,
    pub proposed_step_id: StepId,
    pub title: String,
    pub participants: Vec<PrincipalId>,
    pub expected_version: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewVersion {
    pub schema_version: String,
    pub review_id: String,
    pub version: u32,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub author_principal_id: PrincipalId,
    pub proposed_step_id: StepId,
    pub base_repository_view_id: RepositoryViewId,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    pub title: String,
    pub participants: Vec<PrincipalId>,
    pub created_at: Timestamp,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffAnchor {
    pub path: RepoPath,
    pub start_line: u32,
    pub end_line: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewActionKind {
    Comment,
    Approve,
    RequestChanges,
    Close,
    Reopen,
}
impl ReviewActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::Approve => "approve",
            Self::RequestChanges => "request_changes",
            Self::Close => "close",
            Self::Reopen => "reopen",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewActionRequest {
    pub idempotency_key: String,
    pub version: u32,
    pub action: ReviewActionKind,
    pub body: String,
    pub anchor: Option<DiffAnchor>,
    pub reply_to: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewActionDocument {
    pub schema_version: String,
    pub action_id: String,
    pub review_id: String,
    pub version: u32,
    pub proposal_object_id: ObjectId,
    pub actor_principal_id: PrincipalId,
    pub action: ReviewActionKind,
    pub body: String,
    pub anchor: Option<DiffAnchor>,
    pub reply_to: Option<String>,
    pub created_at: Timestamp,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtectionRequest {
    pub minimum_approvals: u32,
    pub allow_self_approval: bool,
    pub reject_conflicts: bool,
    pub recipe: Option<serde_json::Value>,
    pub trusted_runner_principal_ids: Vec<PrincipalId>,
    pub maximum_proof_age_seconds: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerProtection {
    pub schema_version: String,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub minimum_approvals: u32,
    pub allow_self_approval: bool,
    pub reject_conflicts: bool,
    pub recipe_object_id: Option<ObjectId>,
    pub gate_definition_object_id: Option<ObjectId>,
    pub trusted_runner_principal_ids: Vec<PrincipalId>,
    pub maximum_proof_age_seconds: u32,
    pub configured_by: PrincipalId,
    pub configured_at: Timestamp,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRunRequest {
    pub idempotency_key: String,
    pub step_id: StepId,
    pub recipe_object_id: ObjectId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Passed,
    Failed,
}
impl RunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteRunRequest {
    pub outcome: RunOutcome,
    pub log: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantMemberRequest {
    pub principal_id: PrincipalId,
    pub role: MemberRole,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    Member,
    Auditor,
    Automation,
    ReleaseManager,
    Revoked,
}

pub(super) fn validate_key(key: &str) -> V3StoreResult<()> {
    if !(16..=128).contains(&key.len())
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    {
        return Err(invalid("invalid_idempotency_key"));
    }
    Ok(())
}
pub(super) fn invalid(code: &str) -> V3StoreError {
    V3StoreError::InvalidDescriptor(code.into())
}
impl ReviewProposalRequest {
    pub fn validate(&self) -> V3StoreResult<()> {
        validate_key(&self.idempotency_key)?;
        if self.title.trim().is_empty()
            || self.title.len() > 1024
            || self.participants.len() > 100
            || self.expected_version > 10000
        {
            return Err(invalid("invalid_review"));
        }
        Ok(())
    }
}
impl ReviewActionRequest {
    pub fn validate(&self) -> V3StoreResult<()> {
        validate_key(&self.idempotency_key)?;
        if self.version == 0 || self.version > 10000 || self.body.len() > 64 * 1024 {
            return Err(invalid("invalid_review_action"));
        }
        if let Some(anchor) = &self.anchor {
            if anchor.start_line == 0
                || anchor.end_line < anchor.start_line
                || anchor.end_line - anchor.start_line > 10000
                || anchor.end_line > 2_000_000
            {
                return Err(invalid("invalid_diff_anchor"));
            }
        }
        Ok(())
    }
}
impl ProtectionRequest {
    pub fn validate(&self) -> V3StoreResult<()> {
        if self.minimum_approvals > 10
            || self.trusted_runner_principal_ids.len() > 100
            || !(60..=30 * 24 * 3600).contains(&self.maximum_proof_age_seconds)
            || self.recipe.is_some() == self.trusted_runner_principal_ids.is_empty()
        {
            return Err(invalid("invalid_protection"));
        }
        if let Some(recipe) = &self.recipe {
            if serde_jcs::to_vec(recipe)
                .map_err(|_| invalid("invalid_recipe"))?
                .len()
                > 64 * 1024
            {
                return Err(invalid("recipe_too_large"));
            }
        }
        Ok(())
    }
}
