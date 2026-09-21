use super::{PathPatchRequest, SyncTransactionReceipt};
use layrs_core::scm::{ObjectId, RepoPath};
use serde::{Deserialize, Serialize};

/// All replacement bytes must be supplied, including for an already known ID.
/// Possession of an ID is never proof that a member can copy its content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathPatchFile {
    pub path: RepoPath,
    pub bytes_base64: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathPatchSubmission {
    pub request: PathPatchRequest,
    pub files: Vec<PathPatchFile>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathPatchReceipt {
    pub request_digest: ObjectId,
    /// Deterministically reconstructed Sync request. Its mutation ID commits to
    /// requestDigest above, and normal Sync idempotency persists this receipt.
    pub receipt: SyncTransactionReceipt,
}
