//! UI capability summary for one Space.
//!
//! This is a presentation hint, never an authorization: every mutation still
//! runs its own check and the answer can be stale as soon as it is returned.
//! Each flag is computed from the predicates the corresponding handler uses
//! (`authority`/`scoped_authority` decisions, `space_access` and
//! `policy_management::managed`), so the hint cannot silently diverge.
//! Conditions that depend on the target object (review version, candidate
//! paths, Layer protection gates, trusted runners) are evaluated only when the
//! action runs and are not represented here.
use super::*;
use crate::v3::space_access::{
    full_access, human_principal, service_principal, workspace_administrator,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCapabilities {
    pub schema_version: &'static str,
    pub space_id: SpaceId,
    /// Flags are valid for this epoch; any policy change increments it.
    pub authorization_epoch: u64,
    pub access: SpaceAccess,
    pub capabilities: SpaceCapabilityFlags,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCapabilityFlags {
    /// `GET bootstrap`: `space.bootstrap` granted (restricted principals pass).
    pub open: bool,
    /// Projection pages: `ReadProjection` granted at Space level.
    pub browse_files: bool,
    /// Whole-Space transactions (publication, integration): `transaction.apply`, full access.
    pub apply_changes: bool,
    /// Review list/detail/notifications: `review.read`, full access.
    pub read_reviews: bool,
    /// Propose a review or a new version, comment: `review.write`, full access.
    pub propose_reviews: bool,
    /// Approve/request changes: `review.approve`, full access, human principal.
    pub decide_reviews: bool,
    /// Integrating an approved review is a transaction: same predicate as
    /// `apply_changes`; Layer protection gates are evaluated at integration.
    pub integrate_reviews: bool,
    /// Scoped (path-bound) reviews: Space-level grant only; candidate paths
    /// are checked per review.
    pub read_scoped_reviews: bool,
    pub propose_scoped_reviews: bool,
    /// Scoped approve/request changes: `review.approve` grant, human principal.
    pub decide_scoped_reviews: bool,
    /// Scoped integration target: `review.write` grant; path writes checked then.
    pub integrate_scoped_reviews: bool,
    /// Grant/revoke members: `policy.administer` full access, Workspace
    /// owner/administrator and a managed policy root.
    pub manage_members: bool,
    /// Create/edit/archive Teams: same predicate as `manage_members`.
    pub manage_teams: bool,
    /// Create/revoke invitations: same predicate as `manage_members`.
    pub manage_invitations: bool,
    /// Policy management state and policy/choose transactions:
    /// `policy.administer` full access and Workspace owner/administrator.
    pub manage_policy: bool,
    /// Webhooks: `policy.administer`, full access.
    pub manage_webhooks: bool,
    /// Layer protection configuration: `policy.administer`, full access.
    pub configure_protection: bool,
    pub read_releases: bool,
    pub publish_releases: bool,
    pub revoke_releases: bool,
    /// Run list/detail: `space.bootstrap`, full access.
    pub read_runs: bool,
    /// Register/execute proof runs: `proof.run`, full access, service principal.
    pub run_proofs: bool,
}

impl PgCollaborationStore {
    /// Returns `NotFound` both for a missing Space and for a Space the caller
    /// cannot open, so the endpoint is not an existence oracle.
    pub async fn space_capabilities(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<SpaceCapabilities> {
        let current = self.current_authority(space).await?;
        let authority = &current;
        let open = self
            .decide(principal, authority, V3Action::Bootstrap)
            .await?;
        if !open.allowed {
            return Err(V3StoreError::NotFound);
        }
        let access = SpaceAccess::from_open_decision(&open);
        let full = full_access(&open);
        let granted = |action| async move {
            self.decide(principal, authority, action)
                .await
                .map(|decision| decision.allowed)
        };
        let whole = |action| async move {
            self.decide(principal, authority, action)
                .await
                .map(|decision| full_access(&decision))
        };
        let apply_changes = whole(V3Action::ApplyTransaction).await?;
        let administer = whole(V3Action::AdministerPolicy).await?;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        let human = human_principal(&mut *tx, principal).await?;
        let service = service_principal(&mut *tx, principal).await?;
        let administrator =
            administer && workspace_administrator(&mut *tx, principal, space).await?;
        let managed = administrator
            && super::super::policy_management::managed(
                &self.repository,
                &mut tx,
                space,
                &authority.policy_set_id,
            )
            .await?;
        tx.commit().await.map_err(pg_error)?;
        let capabilities = SpaceCapabilityFlags {
            open: true,
            browse_files: granted(V3Action::ReadProjection).await?,
            apply_changes,
            read_reviews: whole(V3Action::ReadReviews).await?,
            propose_reviews: whole(V3Action::WriteReviews).await?,
            decide_reviews: human && whole(V3Action::ApproveReview).await?,
            integrate_reviews: apply_changes,
            read_scoped_reviews: granted(V3Action::ScopedReviewRead).await?,
            propose_scoped_reviews: granted(V3Action::ScopedReviewWrite).await?,
            decide_scoped_reviews: human && granted(V3Action::ScopedReviewApprove).await?,
            integrate_scoped_reviews: granted(V3Action::ScopedReviewWrite).await?,
            manage_members: managed,
            manage_teams: managed,
            manage_invitations: managed,
            manage_policy: administrator,
            manage_webhooks: administer,
            configure_protection: administer,
            read_releases: whole(V3Action::ReadReleases).await?,
            publish_releases: whole(V3Action::PublishRelease).await?,
            revoke_releases: whole(V3Action::RevokeRelease).await?,
            read_runs: full,
            run_proofs: service && whole(V3Action::RunProof).await?,
        };
        Ok(SpaceCapabilities {
            schema_version: "layrs.space-capabilities.v1",
            space_id: space.clone(),
            authorization_epoch: authority.authorization_epoch,
            access,
            capabilities,
        })
    }
}
