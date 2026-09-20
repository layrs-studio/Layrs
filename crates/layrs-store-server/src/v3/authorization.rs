use async_trait::async_trait;
use layrs_core::scm::{ActorId, ObjectId, RepoPath, SpaceId, StepId, TreeId};
use layrs_sync::v3::{PrincipalId, ProductRoot};
use sqlx::{PgPool, Row};

use super::{V3StoreError, V3StoreResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincipalContext {
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub deployment_id: String,
    /// Stable authentication-subject digest. It is never accepted as a
    /// substitute for the authenticated Principal ID.
    pub subject_digest: String,
}

impl PrincipalContext {
    pub fn new(
        principal_id: PrincipalId,
        actor_id: ActorId,
        deployment_id: impl Into<String>,
        subject_digest: impl Into<String>,
    ) -> V3StoreResult<Self> {
        let deployment_id = deployment_id.into();
        let subject_digest = subject_digest.into();
        if !deployment_id.starts_with("deployment:")
            || deployment_id.len() > 139
            || subject_digest.is_empty()
            || subject_digest.len() > 1024
        {
            return Err(V3StoreError::InvalidCapability);
        }
        Ok(Self {
            principal_id,
            actor_id,
            deployment_id,
            subject_digest,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V3Action {
    Bootstrap,
    PlanUpload,
    CompleteUpload,
    PlanFetch,
    FetchObject,
    ReadEvents,
    ApplyTransaction,
    AdministerPolicy,
    ReadAuditEvidence,
    ReadReviews,
    WriteReviews,
    ApproveReview,
    RunProof,
    ReadReleases,
    PublishRelease,
    RevokeRelease,
}

impl V3Action {
    pub const fn as_policy_action(self) -> &'static str {
        match self {
            Self::Bootstrap => "space.bootstrap",
            Self::PlanUpload => "object.plan_upload",
            Self::CompleteUpload => "object.complete_upload",
            Self::PlanFetch => "object.plan_fetch",
            Self::FetchObject => "object.fetch",
            Self::ReadEvents => "event.read",
            Self::ApplyTransaction => "transaction.apply",
            Self::AdministerPolicy => "policy.administer",
            Self::ReadAuditEvidence => "audit.read_evidence",
            Self::ReadReviews => "review.read",
            Self::WriteReviews => "review.write",
            Self::ApproveReview => "review.approve",
            Self::RunProof => "proof.run",
            Self::ReadReleases => "release.read",
            Self::PublishRelease => "release.publish",
            Self::RevokeRelease => "release.revoke",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationTarget {
    Space(SpaceId),
    Object {
        space_id: SpaceId,
        object_id: ObjectId,
    },
    Path {
        space_id: SpaceId,
        repo_path: RepoPath,
    },
    /// Created only after canonical Step/Tree traversal validates the exact leaf.
    ContentPath {
        space_id: SpaceId,
        step_id: StepId,
        root_tree_id: TreeId,
        repo_path: RepoPath,
        content_id: String,
    },
    Product {
        space_id: SpaceId,
        product: ProductRoot,
    },
    Event {
        space_id: SpaceId,
        event_kind: String,
    },
}

impl AuthorizationTarget {
    pub fn space_id(&self) -> &SpaceId {
        match self {
            Self::Space(space_id)
            | Self::Object { space_id, .. }
            | Self::Path { space_id, .. }
            | Self::ContentPath { space_id, .. }
            | Self::Product { space_id, .. }
            | Self::Event { space_id, .. } => space_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationCheck {
    pub principal: PrincipalContext,
    pub action: V3Action,
    pub target: AuthorizationTarget,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationDecision {
    pub allowed: bool,
    pub reason_code: String,
    pub content_restricted: bool,
}

impl AuthorizationDecision {
    pub fn deny(reason_code: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason_code: reason_code.into(),
            content_restricted: false,
        }
    }

    pub fn allow(reason_code: impl Into<String>) -> Self {
        Self {
            allowed: true,
            reason_code: reason_code.into(),
            content_restricted: false,
        }
    }
}

#[async_trait]
pub trait PolicyAuthorizer: Send + Sync {
    async fn authorize(&self, check: &AuthorizationCheck) -> V3StoreResult<AuthorizationDecision>;
}

/// Authentication/membership gate for a fresh self-hosted deployment. It is
/// deliberately fail-closed: after validating identity, membership and the
/// captured authority tuple, it still denies because it does not interpret
/// Policy Products. Deployments must inject a real `PolicyAuthorizer`.
#[derive(Clone, Debug)]
pub struct PgMembershipAuthorizer {
    pool: PgPool,
}

impl PgMembershipAuthorizer {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PolicyAuthorizer for PgMembershipAuthorizer {
    async fn authorize(&self, check: &AuthorizationCheck) -> V3StoreResult<AuthorizationDecision> {
        let epoch = i64::try_from(check.authorization_epoch)
            .map_err(|_| V3StoreError::InvalidCapability)?;
        let row = sqlx::query(
            r#"
            SELECT p.status AS principal_status,
                   s.lifecycle_state,
                   s.authorization_epoch,
                   s.current_policy_set_id,
                   p.actor_id,
                   p.deployment_id,
                   wm.state AS membership_state
            FROM principals p
            JOIN spaces s ON s.space_id = $2
            JOIN workspace_memberships wm
              ON wm.workspace_id = s.workspace_id
             AND wm.principal_id = p.principal_id
            WHERE p.principal_id = $1
              AND p.subject_digest = $3
              AND p.actor_id = $4
              AND p.deployment_id = $5
            "#,
        )
        .bind(check.principal.principal_id.as_str())
        .bind(check.target.space_id().as_str())
        .bind(&check.principal.subject_digest)
        .bind(check.principal.actor_id.as_str())
        .bind(&check.principal.deployment_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(pg_error)?;

        let Some(row) = row else {
            return Ok(AuthorizationDecision::deny("membership_not_active"));
        };
        let principal_status: String = row.get("principal_status");
        let lifecycle_state: String = row.get("lifecycle_state");
        let stored_epoch: i64 = row.get("authorization_epoch");
        let policy_set_id: String = row.get("current_policy_set_id");
        let membership_state: String = row.get("membership_state");
        if stored_epoch != epoch || policy_set_id != check.policy_set_id.as_str() {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        if principal_status != "active"
            || lifecycle_state != "ready"
            || membership_state != "active"
        {
            return Ok(AuthorizationDecision::deny("principal_or_space_inactive"));
        }

        Ok(AuthorizationDecision::deny(
            "policy_evaluator_not_configured",
        ))
    }
}

pub(crate) fn require_allowed(decision: AuthorizationDecision) -> V3StoreResult<()> {
    if decision.allowed {
        Ok(())
    } else {
        Err(V3StoreError::InvalidCapability)
    }
}

pub(crate) fn pg_error(error: sqlx::Error) -> V3StoreError {
    V3StoreError::Backend(error.to_string())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct DeterministicAuthorizer {
        pub allowed_actions: Vec<V3Action>,
    }

    #[async_trait]
    impl PolicyAuthorizer for DeterministicAuthorizer {
        async fn authorize(
            &self,
            check: &AuthorizationCheck,
        ) -> V3StoreResult<AuthorizationDecision> {
            Ok(if self.allowed_actions.contains(&check.action) {
                AuthorizationDecision::allow("deterministic_test_allow")
            } else {
                AuthorizationDecision::deny("deterministic_test_deny")
            })
        }
    }
}
