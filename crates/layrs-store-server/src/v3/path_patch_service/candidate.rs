use super::*;
use layrs_core::scm::{LayerId, RepoPath, RepositoryViewId, SpaceId, StepId, TreeId};
use sqlx::Row;

/// A server-owned source binding. Several captures may yield the same Step;
/// callers must select the exact candidate ID, never infer one from a Step ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedPatchCandidate {
    pub candidate_id: String,
    pub space_id: SpaceId,
    pub candidate_step_id: StepId,
    pub candidate_root_tree_id: TreeId,
    pub source_request_digest: ObjectId,
    pub capture_digest: ObjectId,
    pub author_principal_id: PrincipalId,
    pub layer_id: LayerId,
    pub base_step_id: StepId,
    pub base_root_tree_id: TreeId,
    pub expected_view_id: RepositoryViewId,
    pub expected_policy_set_id: ObjectId,
    pub expected_authorization_epoch: u64,
    pub changed_paths: Vec<RepoPath>,
    pub transaction_id: TransactionId,
}

pub async fn load_candidate(
    repo: &PgV3Repository,
    space: &SpaceId,
    candidate_id: &str,
) -> V3StoreResult<Option<ScopedPatchCandidate>> {
    if candidate_id.len() != 74
        || !candidate_id.starts_with("candidate:")
        || !candidate_id[10..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(V3StoreError::InvalidCapability);
    }
    let row = sqlx::query("SELECT c.*,s.root_tree_id AS stored_root FROM layrs_collaboration.scoped_patch_candidates c JOIN steps s ON s.step_id=c.candidate_step_id JOIN sync_transactions t ON t.space_id=c.space_id AND t.transaction_id=c.transaction_id WHERE c.space_id=$1 AND c.candidate_id=$2")
        .bind(space.as_str()).bind(candidate_id).fetch_optional(repo.pool()).await.map_err(pg_error)?;
    let Some(row) = row else { return Ok(None) };
    let source_request_digest =
        ObjectId::parse(row.get::<String, _>("source_request_digest")).map_err(invalid)?;
    if candidate_id != format!("candidate:{}", &source_request_digest.as_str()[7..])
        || row.get::<String, _>("candidate_root_tree_id") != row.get::<String, _>("stored_root")
    {
        return Err(V3StoreError::IdentityMismatch);
    }
    let changed_paths_jcs: String = row.get("changed_paths_jcs");
    if changed_paths_jcs.len() > 1024 * 4096 * 6 {
        return Err(V3StoreError::IdentityMismatch);
    }
    let changed_paths: Vec<RepoPath> = serde_json::from_str(&changed_paths_jcs).map_err(invalid)?;
    if changed_paths.is_empty()
        || changed_paths.len() > 1024
        || changed_paths.windows(2).any(|pair| pair[0] >= pair[1])
        || serde_jcs::to_string(&changed_paths).map_err(invalid)? != changed_paths_jcs
    {
        return Err(V3StoreError::IdentityMismatch);
    }
    Ok(Some(ScopedPatchCandidate {
        candidate_id: candidate_id.into(),
        space_id: space.clone(),
        candidate_step_id: StepId::parse(row.get::<String, _>("candidate_step_id"))
            .map_err(invalid)?,
        candidate_root_tree_id: TreeId::parse(row.get::<String, _>("candidate_root_tree_id"))
            .map_err(invalid)?,
        source_request_digest,
        capture_digest: ObjectId::parse(row.get::<String, _>("capture_digest")).map_err(invalid)?,
        author_principal_id: PrincipalId::parse(row.get::<String, _>("author_principal_id"))
            .map_err(invalid)?,
        layer_id: LayerId::parse(row.get::<String, _>("layer_id")).map_err(invalid)?,
        base_step_id: StepId::parse(row.get::<String, _>("base_step_id")).map_err(invalid)?,
        base_root_tree_id: TreeId::parse(row.get::<String, _>("base_root_tree_id"))
            .map_err(invalid)?,
        expected_view_id: RepositoryViewId::parse(row.get::<String, _>("expected_view_id"))
            .map_err(invalid)?,
        expected_policy_set_id: ObjectId::parse(row.get::<String, _>("expected_policy_set_id"))
            .map_err(invalid)?,
        expected_authorization_epoch: u64::try_from(
            row.get::<i64, _>("expected_authorization_epoch"),
        )
        .map_err(invalid)?,
        changed_paths,
        transaction_id: TransactionId::parse(row.get::<String, _>("transaction_id"))
            .map_err(invalid)?,
    }))
}
