//! Exact leaf patches. A request is not an authority to supply a complete Tree.
use super::{ClientId, IdempotencyKey, Nullable, V3ValidationError};
use layrs_core::scm::{
    LayerId, ObjectId, RepoPath, RepositoryViewId, SpaceId, StepId, TreeEntryTarget,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[cfg(test)]
#[path = "path_patch_tests.rs"]
mod tests;

pub const MAX_PATH_PATCH_CHANGES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathPatchSchema {
    #[serde(rename = "layrs.path-patch.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathPatchEntry {
    pub target: TreeEntryTarget,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathPatchChange {
    pub path: RepoPath,
    pub expected: Nullable<PathPatchEntry>,
    pub replacement: Nullable<PathPatchEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathPatchRequest {
    pub schema_version: PathPatchSchema,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: ObjectId,
    pub layer_id: LayerId,
    pub base_step_id: StepId,
    pub expected_repository_view_id: RepositoryViewId,
    pub expected_generation: u64,
    pub expected_policy_set_id: ObjectId,
    pub expected_authorization_epoch: u64,
    pub changes: Vec<PathPatchChange>,
}

fn invalid(message: &'static str) -> V3ValidationError {
    V3ValidationError {
        code: "invalid_path_patch",
        field: "changes",
        message: message.into(),
    }
}

/// Change paths must be sorted, unique and pairwise non-overlapping. This allows
/// rename pairs while forbidding implicit overwrites of directories and hidden leaves.
pub fn validate_path_patch_changes(changes: &[PathPatchChange]) -> Result<(), V3ValidationError> {
    if changes.is_empty() || changes.len() > MAX_PATH_PATCH_CHANGES {
        return Err(invalid("patch must contain between 1 and 1024 changes"));
    }
    let mut paths = BTreeSet::new();
    let mut previous = None;
    for change in changes {
        let path = change.path.as_str();
        if path.len() > 4096 || path.split('/').count() > 128 {
            return Err(invalid("patch path exceeds length or depth bound"));
        }
        if previous.is_some_and(|last: &str| last >= path) {
            return Err(invalid("patch paths must be strictly sorted and unique"));
        }
        previous = Some(path);
        for (offset, _) in path.match_indices('/') {
            if paths.contains(&path[..offset]) {
                return Err(invalid("patch paths overlap an ancestor"));
            }
        }
        paths.insert(path);
        if change.expected == change.replacement {
            return Err(invalid("patch change must modify a leaf"));
        }
        for entry in change.expected.0.iter().chain(change.replacement.0.iter()) {
            if matches!(entry.target, TreeEntryTarget::Tree { .. }) {
                return Err(invalid(
                    "directory replacement requires explicit leaf changes",
                ));
            }
        }
    }
    Ok(())
}

impl PathPatchRequest {
    /// Domain-separated JCS of the full request excluding only requestDigest.
    pub fn computed_request_digest(&self) -> Result<ObjectId, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        value
            .as_object_mut()
            .expect("request serializes as object")
            .remove("requestDigest");
        let bytes = serde_jcs::to_vec(&value)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs-path-patch-request-v1\0");
        hasher.update(&bytes);
        Ok(
            ObjectId::parse(format!("blake3:{}", hasher.finalize().to_hex()))
                .expect("canonical digest is a valid ObjectId"),
        )
    }

    pub fn validate(&self) -> Result<(), V3ValidationError> {
        validate_path_patch_changes(&self.changes)?;
        if self
            .computed_request_digest()
            .map_err(|_| invalid("cannot canonicalize request"))?
            != self.request_digest
        {
            return Err(invalid("request digest mismatch"));
        }
        Ok(())
    }
}
