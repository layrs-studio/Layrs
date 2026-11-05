//! Metadata-only, resumable exact path patches. V1 wire identities remain unchanged.
use super::{
    ClientId, IdempotencyKey, PathPatchChange, SyncTransactionReceipt, V3ValidationError,
    validate_path_patch_changes,
};
use layrs_core::scm::{
    FileManifest, LayerId, ObjectId, RepoPath, RepositoryViewId, SpaceId, StepId, TreeEntryTarget,
    TreeId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_SCOPED_PATCH_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_SCOPED_CHUNKS: usize = 16_384;
pub const SCOPED_CHUNK_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopedPathPatchSchema {
    #[serde(rename = "layrs.path-patch.v2")]
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScopedPathPatchIntent {
    Advance,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedPatchFile {
    pub path: RepoPath,
    pub manifest: FileManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedPathPatchRequest {
    pub schema_version: ScopedPathPatchSchema,
    pub intent: ScopedPathPatchIntent,
    pub space_id: SpaceId,
    pub client_id: ClientId,
    pub idempotency_key: IdempotencyKey,
    pub request_digest: ObjectId,
    pub capture_digest: ObjectId,
    pub layer_id: LayerId,
    pub base_step_id: StepId,
    pub base_root_tree_id: TreeId,
    pub expected_repository_view_id: RepositoryViewId,
    pub expected_generation: u64,
    pub expected_policy_set_id: ObjectId,
    pub expected_authorization_epoch: u64,
    pub changes: Vec<PathPatchChange>,
    pub files: Vec<ScopedPatchFile>,
}

impl ScopedPathPatchRequest {
    pub fn computed_request_digest(&self) -> Result<ObjectId, serde_json::Error> {
        let mut value = serde_json::to_value(self)?;
        value
            .as_object_mut()
            .expect("request object")
            .remove("requestDigest");
        let canonical = serde_jcs::to_vec(&value)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs-scoped-path-patch-request-v2\0");
        hasher.update(&canonical);
        Ok(
            ObjectId::parse(format!("blake3:{}", hasher.finalize().to_hex()))
                .expect("valid digest"),
        )
    }

    pub fn validate(&self) -> Result<(), V3ValidationError> {
        let invalid = |message: &'static str| V3ValidationError {
            code: "invalid_scoped_path_patch",
            field: "files",
            message: message.into(),
        };
        validate_path_patch_changes(&self.changes)?;
        if self
            .computed_request_digest()
            .map_err(|_| invalid("cannot canonicalize request"))?
            != self.request_digest
        {
            return Err(invalid("request digest mismatch"));
        }
        let mut replacements = BTreeSet::new();
        for change in &self.changes {
            if change.path.as_str().split('/').any(|part| {
                part.eq_ignore_ascii_case(".layrs") || part.eq_ignore_ascii_case(".git")
            }) {
                return Err(invalid("reserved metadata namespace"));
            }
            if change
                .expected
                .0
                .iter()
                .chain(change.replacement.0.iter())
                .any(|entry| !matches!(entry.target, TreeEntryTarget::File { .. }))
            {
                return Err(invalid("ordinary files only"));
            }
            if change.replacement.0.is_some() {
                replacements.insert(&change.path);
            }
        }
        if replacements.len() != self.files.len() {
            return Err(invalid("one manifest required for each replacement"));
        }
        let mut total = 0u64;
        let mut chunks = 0usize;
        let mut previous: Option<&RepoPath> = None;
        for file in &self.files {
            if previous.is_some_and(|path| path >= &file.path) || !replacements.contains(&file.path)
            {
                return Err(invalid("file paths must match sorted replacements"));
            }
            previous = Some(&file.path);
            let change = self
                .changes
                .iter()
                .find(|change| change.path == file.path)
                .expect("replacement path");
            if change
                .replacement
                .0
                .as_ref()
                .and_then(|entry| entry.target.file_id())
                != Some(file.manifest.file_id())
            {
                return Err(invalid("manifest FileId differs from replacement"));
            }
            let fixed =
                layrs_core::scm::ChunkingDescriptor::deterministic_fixed(SCOPED_CHUNK_BYTES)
                    .map_err(|_| invalid("invalid chunking"))?;
            if file.manifest.chunking() != &fixed {
                return Err(invalid("manifest must use fixed 1 MiB chunks"));
            }
            for (ordinal, chunk) in file.manifest.chunks().iter().enumerate() {
                if chunk.byte_length() == 0
                    || chunk.byte_length() > SCOPED_CHUNK_BYTES
                    || (ordinal + 1 != file.manifest.chunks().len()
                        && chunk.byte_length() != SCOPED_CHUNK_BYTES)
                {
                    return Err(invalid("manifest chunk lengths are not fixed 1 MiB"));
                }
            }
            total = total
                .checked_add(file.manifest.byte_length())
                .ok_or_else(|| invalid("patch length overflow"))?;
            chunks = chunks
                .checked_add(file.manifest.chunks().len())
                .ok_or_else(|| invalid("chunk count overflow"))?;
            if total > MAX_SCOPED_PATCH_BYTES || chunks > MAX_SCOPED_CHUNKS {
                return Err(invalid("patch exceeds scoped transport limits"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedPatchProgress {
    pub session_id: String,
    pub request_digest: ObjectId,
    pub missing: Vec<ScopedMissingChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedMissingChunk {
    pub path: RepoPath,
    pub file_index: u32,
    pub ordinal: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedPathPatchReceipt {
    pub request_digest: ObjectId,
    pub client_id: ClientId,
    pub idempotency_key: IdempotencyKey,
    pub capture_digest: ObjectId,
    pub layer_id: LayerId,
    pub base_step_id: StepId,
    pub intent: ScopedPathPatchIntent,
    pub candidate_id: Option<String>,
    pub receipt: SyncTransactionReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScopedReviewIntegrationRequest {
    pub idempotency_key: IdempotencyKey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use layrs_core::scm::{ChunkingDescriptor, FileId};
    use serde_json::json;

    fn request() -> ScopedPathPatchRequest {
        let hash = format!("blake3:{}", "a".repeat(64));
        let manifest = FileManifest::empty(
            ChunkingDescriptor::deterministic_fixed(SCOPED_CHUNK_BYTES).unwrap(),
        )
        .unwrap();
        let mut request: ScopedPathPatchRequest = serde_json::from_value(json!({
            "schemaVersion":"layrs.path-patch.v2", "intent":"candidate",
            "spaceId":"space:test", "clientId":"client:test", "idempotencyKey":"scoped-test-0001",
            "requestDigest":hash, "captureDigest":hash, "layerId":"layer:main",
            "baseStepId":hash, "baseRootTreeId":hash,
            "expectedRepositoryViewId":hash, "expectedGeneration":4,
            "expectedPolicySetId":hash, "expectedAuthorizationEpoch":7,
            "changes":[{"path":"file.txt","expected":null,"replacement":{
                "target":{"kind":"file","fileId":FileId::from_bytes(&[]).as_str()},"executable":false
            }}],
            "files":[{"path":"file.txt","manifest":manifest}]
        })).unwrap();
        request.request_digest = request.computed_request_digest().unwrap();
        request
    }

    #[test]
    fn v2_digest_binds_intent_capture_and_manifest_without_changing_v1() {
        let original = request();
        original.validate().unwrap();
        let mut candidate_to_advance = original.clone();
        candidate_to_advance.intent = ScopedPathPatchIntent::Advance;
        assert!(candidate_to_advance.validate().is_err());
        let mut other_capture = original.clone();
        other_capture.capture_digest =
            ObjectId::parse(format!("blake3:{}", "b".repeat(64))).unwrap();
        assert!(other_capture.validate().is_err());
        let mut no_manifest = original.clone();
        no_manifest.files.clear();
        no_manifest.request_digest = no_manifest.computed_request_digest().unwrap();
        assert!(no_manifest.validate().is_err());
        let encoded = serde_jcs::to_vec(&original).unwrap();
        assert_eq!(
            serde_json::from_slice::<ScopedPathPatchRequest>(&encoded).unwrap(),
            original
        );
    }
}
