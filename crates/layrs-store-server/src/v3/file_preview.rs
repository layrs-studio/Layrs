//! Content selected through the existing exact-path projection authority.
use super::*;
use layrs_sync::v3::ProjectionAccessRequest;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilePreviewRequest {
    pub step_id: StepId,
    pub path: RepoPath,
    pub expected_policy_set_id: ObjectId,
    pub expected_authorization_epoch: u64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreviewResponse {
    pub step_id: StepId,
    pub path: RepoPath,
    pub text: Option<String>,
    pub omitted: bool,
}
impl PgV3Repository {
    pub async fn file_preview(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        request: FilePreviewRequest,
    ) -> V3StoreResult<FilePreviewResponse> {
        let access_request = ProjectionAccessRequest {
            step_id: request.step_id.clone(),
            paths: vec![request.path.clone()],
            expected_policy_set_id: request.expected_policy_set_id.clone(),
            expected_authorization_epoch: request.expected_authorization_epoch,
        };
        let access = self
            .projection_access(principal, authority, access_request.clone())
            .await?;
        let entry = access.entries.first().ok_or(V3StoreError::NotFound)?;
        if !entry.content_readable {
            return Err(V3StoreError::InvalidCapability);
        }
        let Some(TreeEntryTarget::File { file_id }) = &entry.target.0 else {
            return Err(V3StoreError::NotFound);
        };
        let source = PgSource {
            repo: self,
            principal,
            authority,
        };
        let mut reader = Reader::new(&source, &authority.space_id);
        let bytes = match reader.file_bounded(file_id, 1024 * 1024).await {
            Ok(bytes) => Some(bytes),
            Err(V3StoreError::LimitExceeded { .. }) => None,
            Err(error) => return Err(error),
        };
        // Recheck account, membership and policy while holding the same locks as writes.
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        super::super::transaction_store::lock_identity(&mut tx, principal, &authority.space_id)
            .await?;
        let row = sqlx::query("SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 FOR SHARE")
            .bind(authority.space_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let final_access = self
            .projection_access(principal, authority, access_request)
            .await?;
        if !final_access
            .entries
            .first()
            .is_some_and(|value| value.content_readable && value.target == entry.target)
        {
            return Err(V3StoreError::InvalidCapability);
        }
        tx.commit().await.map_err(pg_error)?;
        let text = bytes.and_then(|bytes| {
            if bytes.contains(&0) {
                None
            } else {
                std::str::from_utf8(&bytes).ok().map(str::to_owned)
            }
        });
        Ok(FilePreviewResponse {
            step_id: request.step_id,
            path: request.path,
            omitted: text.is_none(),
            text,
        })
    }
}
