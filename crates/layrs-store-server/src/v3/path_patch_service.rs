//! Exact restricted publications use the ordinary atomic Sync commit/Gates.
//! Internal preparation supplies all file bytes and reconstructs all changed
//! Trees; clients can never nominate an arbitrary full Tree or privileged actor.
use super::*;
use layrs_core::scm::{
    LayerHeadSet, ObjectId, RepositoryView, Step, Tree, TreeEntryTarget,
};
use layrs_sync::v3::*;
use std::collections::BTreeMap;

mod content;
mod prepare;
use path_patch_scope::ScopedPatch;

pub struct PgPathPatchService {
    repository: PgV3Repository,
    cursors: EventCursorCodec,
    transfer_base_url: TransferBaseUrl,
}
impl PgPathPatchService {
    pub fn new(
        repository: PgV3Repository,
        cursors: EventCursorCodec,
        transfer_base_url: TransferBaseUrl,
    ) -> Self {
        Self {
            repository,
            cursors,
            transfer_base_url,
        }
    }

    pub async fn submit(
        &self,
        principal: &PrincipalContext,
        submission: &PathPatchSubmission,
    ) -> Result<PathPatchReceipt, TransactionServiceError> {
        let patch = &submission.request;
        patch.validate().map_err(invalid)?;
        let authority = PgBootstrapStore::new(self.repository.clone(), self.cursors.clone())
            .load_authority(&patch.space_id)
            .await?;
        require_allowed(
            self.repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action: V3Action::ReadProjection,
                    target: AuthorizationTarget::Space(patch.space_id.clone()),
                    policy_set_id: authority.policy_set_id.clone(),
                    authorization_epoch: authority.authorization_epoch,
                })
                .await?,
        )?;
        // Even known object IDs require the full bytes supplied by this request.
        let mut content = content::decode(submission)?;
        let (request, scope, trees) = self.prepare_request(patch).await?;
        let transactions = PgTransactionStore::new(self.repository.clone(), self.cursors.clone());
        if let Some(stored) = transactions.replay(principal, &request).await? {
            return Ok(PathPatchReceipt {
                request_digest: patch.request_digest.clone(),
                receipt: stored.receipt,
            });
        }
        if scope
            .decisions(&self.repository, principal, &authority, &request, &[])
            .await?
            .iter()
            .any(|d| !d.allowed)
        {
            // Persist the normal terminal rejection; no upload/availability changes.
            let receipt = transactions
                .transact_scoped(principal, &request, &scope)
                .await?;
            return Ok(PathPatchReceipt {
                request_digest: patch.request_digest.clone(),
                receipt: receipt.receipt,
            });
        }
        content.add_trees(trees)?;
        self.install_content(principal, &scope, &request, &content)
            .await?;
        let receipt = transactions
            .transact_scoped(principal, &request, &scope)
            .await?;
        Ok(PathPatchReceipt {
            request_digest: patch.request_digest.clone(),
            receipt: receipt.receipt,
        })
    }
}

fn invalid(error: impl std::fmt::Display) -> V3StoreError {
    V3StoreError::InvalidDescriptor(error.to_string())
}
