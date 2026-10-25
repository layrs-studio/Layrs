//! Comparison's I/O boundary; admission and authorization remain live checks.
use super::*;
use async_trait::async_trait;
use layrs_sync::v3::ObjectDescriptor;

#[async_trait]
pub(super) trait Source: Sync {
    async fn allowed(&self, target: AuthorizationTarget) -> V3StoreResult<()>;
    async fn admitted(&self, id: &StepId) -> V3StoreResult<()>;
    async fn manifest(&self, id: &FileId) -> V3StoreResult<String>;
    async fn descriptor(&self, id: &ObjectId) -> V3StoreResult<ObjectDescriptor>;
    async fn read(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<Vec<u8>>;
}

pub(super) struct PgSource<'a> {
    pub repo: &'a PgV3Repository,
    pub principal: &'a PrincipalContext,
    pub authority: &'a SpaceAuthority,
}

#[async_trait]
impl Source for PgSource<'_> {
    async fn allowed(&self, target: AuthorizationTarget) -> V3StoreResult<()> {
        require_allowed(
            self.repo
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: self.principal.clone(),
                    action: V3Action::FetchObject,
                    target,
                    policy_set_id: self.authority.policy_set_id.clone(),
                    authorization_epoch: self.authority.authorization_epoch,
                })
                .await?,
        )
    }

    async fn admitted(&self, id: &StepId) -> V3StoreResult<()> {
        let admitted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM space_steps WHERE space_id=$1 AND step_id=$2)",
        )
        .bind(self.authority.space_id.as_str())
        .bind(id.as_str())
        .fetch_one(self.repo.pool())
        .await
        .map_err(pg_error)?;
        if admitted {
            Ok(())
        } else {
            Err(V3StoreError::NotFound)
        }
    }

    async fn manifest(&self, id: &FileId) -> V3StoreResult<String> {
        sqlx::query_scalar("SELECT m.file_manifest_id FROM file_manifests m JOIN space_object_availability a ON a.object_id=m.file_manifest_id WHERE a.space_id=$1 AND m.file_id=$2 AND a.availability_state='available' AND m.reconstruction_verified_at IS NOT NULL ORDER BY m.file_manifest_id LIMIT 1")
            .bind(self.authority.space_id.as_str()).bind(id.as_str())
            .fetch_optional(self.repo.pool()).await.map_err(pg_error)?
            .ok_or(V3StoreError::NotFound)
    }

    async fn descriptor(&self, id: &ObjectId) -> V3StoreResult<ObjectDescriptor> {
        load_space_descriptor(self.repo.pool(), &self.authority.space_id, id).await
    }

    async fn read(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<Vec<u8>> {
        read_canonical_object(self.repo.cas().as_ref(), descriptor, MAX_FILE).await
    }
}
