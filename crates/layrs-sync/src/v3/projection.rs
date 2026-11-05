//! Structural pages for a partial Worktree. These decisions are observations,
//! never capabilities; every content fetch and mutation is authorized again.
use super::{Nullable, PrincipalId};
use layrs_core::scm::{ObjectId, RepoPath, SpaceId, StepId, TreeEntryTarget, TreeId};
use serde::{Deserialize, Serialize};

/// Classification of exact paths, including absent paths proposed for creation.
/// Names are visible; content decisions never replace authorization at use time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionAccessRequest {
    pub step_id: StepId,
    pub expected_policy_set_id: ObjectId,
    pub expected_authorization_epoch: u64,
    pub paths: Vec<RepoPath>,
}

impl ProjectionAccessRequest {
    pub fn validate(&self) -> super::V3Result<()> {
        if self.expected_authorization_epoch == 0
            || self.paths.is_empty()
            || self.paths.len() > 200
            || self.paths.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .paths
                .iter()
                .any(|path| path.as_str().len() > 4096 || path.as_str().split('/').count() > 128)
        {
            return Err(super::V3ValidationError::new(
                "invalid_projection_paths",
                "paths",
                "requires 1..200 sorted unique bounded paths and a positive epoch",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectionAccessSchema {
    #[serde(rename = "layrs.projection-access.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionAccessEntry {
    pub path: RepoPath,
    pub target: Nullable<TreeEntryTarget>,
    pub executable: bool,
    pub content_readable: bool,
    pub writable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionAccess {
    pub schema_version: ProjectionAccessSchema,
    pub principal_id: PrincipalId,
    pub space_id: SpaceId,
    pub step_id: StepId,
    pub root_tree_id: TreeId,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    pub entries: Vec<ProjectionAccessEntry>,
}

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
