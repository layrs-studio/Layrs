//! Review records confined to an admitted scoped patch candidate.
use super::PrincipalId;
use layrs_core::scm::{LayerId, ObjectId, RepoPath, RepositoryViewId, SpaceId, StepId, TreeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedReviewProposalRequest {
    pub candidate_id: String,
    pub idempotency_key: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedReview {
    pub review_id: String,
    pub candidate_id: String,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
    pub author_principal_id: PrincipalId,
    pub candidate_step_id: StepId,
    pub candidate_root_tree_id: TreeId,
    pub base_repository_view_id: RepositoryViewId,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    pub changed_paths: Vec<RepoPath>,
    pub title: Option<String>,
    pub lifecycle: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopedReviewActionKind {
    Comment,
    Approve,
    RequestChanges,
    Close,
    Reopen,
}

impl ScopedReviewActionKind {
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
pub struct ScopedReviewActionRequest {
    pub idempotency_key: String,
    pub action: ScopedReviewActionKind,
    pub body: String,
    pub anchor_path: Option<RepoPath>,
    pub reply_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedReviewAction {
    pub action_id: String,
    pub review_id: String,
    pub actor_principal_id: PrincipalId,
    pub action: ScopedReviewActionKind,
    pub body: Option<String>,
    pub body_redacted: bool,
    pub anchor_path: Option<RepoPath>,
    pub reply_to: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedReviewListQuery {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedReviewPage {
    pub reviews: Vec<ScopedReview>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedReviewDetail {
    pub review: ScopedReview,
    pub actions: Vec<ScopedReviewAction>,
    pub decision_state: String,
    pub integration: ScopedIntegrationStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedIntegrationStatus {
    pub candidate_id: String,
    pub review_id: String,
    pub state: String,
    pub approvals: u32,
    pub required_approvals: u32,
    pub blocking_decisions: u32,
    pub proof_status: String,
}
