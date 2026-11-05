//! Unforgeable in-process proof produced only by canonical patch preparation.
use super::*;
use layrs_core::scm::{ObjectId, TreeId};
use layrs_sync::v3::*;

pub(crate) struct ScopedPatch {
    pub(super) request: PathPatchRequest,
    pub(super) root: TreeId,
    pub(super) transaction_digest: ObjectId,
    pub(super) candidate: Option<ScopedCandidateSource>,
    pub(super) integration: Option<ScopedIntegrationBinding>,
}

pub(crate) struct ScopedCandidateSource {
    pub source_request: ScopedPathPatchRequest,
    pub candidate_id: String,
}

pub(crate) struct ScopedIntegrationBinding {
    pub review_id: String,
    pub candidate_id: String,
    pub step_id: layrs_core::scm::StepId,
    pub layer_id: layrs_core::scm::LayerId,
}

impl ScopedPatch {
    pub(crate) async fn decisions(
        &self,
        repo: &PgV3Repository,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        request: &SyncTransactionRequest,
        versions: &[PolicyVersionId],
    ) -> V3StoreResult<Vec<layrs_sync::v3::AuthorizationDecision>> {
        if request.request_digest != self.transaction_digest
            || request.space_id != self.request.space_id
            || request.client_id != self.request.client_id
            || request.idempotency_key != self.request.idempotency_key
        {
            return Err(V3StoreError::InvalidCapability);
        }
        let epoch_matches = authority.policy_set_id == self.request.expected_policy_set_id
            && authority.authorization_epoch == self.request.expected_authorization_epoch;
        let mut decisions = Vec::new();
        for change in &self.request.changes {
            let decision = if epoch_matches {
                repo.authorizer()
                    .authorize(&AuthorizationCheck {
                        principal: principal.clone(),
                        action: V3Action::ScopedPathWrite,
                        target: AuthorizationTarget::ScopedPath {
                            space_id: self.request.space_id.clone(),
                            base_step_id: self.request.base_step_id.clone(),
                            root_tree_id: self.root.clone(),
                            repo_path: change.path.clone(),
                        },
                        policy_set_id: authority.policy_set_id.clone(),
                        authorization_epoch: authority.authorization_epoch,
                    })
                    .await?
            } else {
                super::AuthorizationDecision::deny("authorization_epoch_changed")
            };
            decisions.push(layrs_sync::v3::AuthorizationDecision {
                action: "transaction.apply".into(),
                scope: AuthorizationScope::Path {
                    root: AuthorizationRoot::Step {
                        step_id: self.request.base_step_id.clone(),
                    },
                    repo_path: change.path.clone(),
                },
                allowed: decision.allowed,
                policy_version_ids: versions.to_vec(),
                reason_code: decision.reason_code,
            });
        }
        Ok(decisions)
    }
}
