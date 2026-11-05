//! Local availability is evidence about an exact version, never remote authority.
use super::{Result, invalid};
use crate::scm::{FileId, FileManifestId, ObjectId, RepoPath, StepId, TreeId, WorktreeId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProjectionVersion {
    Generation {
        worktree_id: WorktreeId,
        generation: u64,
        digest: String,
    },
    Capture {
        worktree_id: WorktreeId,
        sequence: u64,
        digest: String,
    },
    Remote {
        step_id: StepId,
        root_tree_id: TreeId,
        policy_set_id: ObjectId,
        authorization_epoch: u64,
        observation_digest: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OfflineOperation {
    Read,
    Edit,
    Capture,
    CompareMetadata,
    CompareContent,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AvailabilityRequest {
    pub version: ProjectionVersion,
    pub other: Option<ProjectionVersion>,
    pub paths: Vec<RepoPath>,
    pub operation: OfflineOperation,
}
impl AvailabilityRequest {
    pub fn validate(&self) -> Result<()> {
        if self.paths.is_empty()
            || self.paths.len() > 1024
            || self.paths.windows(2).any(|p| p[0] >= p[1])
        {
            return Err(invalid("availability requires 1..1024 sorted unique paths"));
        }
        let comparison = matches!(
            self.operation,
            OfflineOperation::CompareMetadata | OfflineOperation::CompareContent
        );
        if comparison != self.other.is_some() {
            return Err(invalid("comparison requires exactly two version operands"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AvailabilityBlocker {
    UnknownPath,
    MissingContent,
    ProtectedPath,
    ExcludedPath,
    NotMaterialized,
    UnsupportedTarget,
    PendingOperation,
    Quota,
    ChangedCheckout,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionPathAvailability {
    pub path: RepoPath,
    pub operand: u8,
    pub absent: bool,
    pub file_id: Option<FileId>,
    pub manifest_id: Option<FileManifestId>,
    pub selected: bool,
    pub observed_writable: bool,
    pub materialized: bool,
    pub retained: bool,
    pub blockers: Vec<AvailabilityBlocker>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionAvailability {
    pub request: AvailabilityRequest,
    /// Capture currently scans the whole selected working set.
    pub effective_paths: Vec<RepoPath>,
    pub paths: Vec<VersionPathAvailability>,
    pub blockers: Vec<AvailabilityBlocker>,
    pub available: bool,
    pub prepared: bool,
}
