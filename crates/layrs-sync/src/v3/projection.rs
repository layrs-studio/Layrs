//! Structural pages for a partial Worktree. These decisions are observations,
//! never capabilities; every content fetch and mutation is authorized again.
use super::PrincipalId;
use layrs_core::scm::{ObjectId, RepoPath, SpaceId, StepId, TreeEntryTarget, TreeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectionSchema {
    #[serde(rename = "layrs.projection-page.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionPageRequest {
    pub step_id: StepId,
    pub expected_policy_set_id: ObjectId,
    pub expected_authorization_epoch: u64,
    pub cursor: Option<String>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionEntry {
    pub path: RepoPath,
    pub target: TreeEntryTarget,
    pub executable: bool,
    pub content_readable: bool,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionPage {
    pub schema_version: ProjectionSchema,
    pub principal_id: PrincipalId,
    pub space_id: SpaceId,
    pub step_id: StepId,
    pub root_tree_id: TreeId,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    /// A stable depth-first traversal offset, not an authorization grant.
    pub offset: u32,
    pub entries: Vec<ProjectionEntry>,
    pub next_cursor: Option<String>,
}
