//! Authorization predicates shared by mutation handlers, the authorized Space
//! catalog and the UI capability summary. Each predicate is the single
//! definition of its rule: handlers call it to guard a mutation and the
//! capability summary calls the same function, so a hint cannot drift from the
//! check that actually protects the server.
use super::{AuthorizationDecision, PrincipalContext, V3StoreResult, pg_error};
use layrs_core::scm::SpaceId;
use sqlx::PgExecutor;

/// Workspace roles allowed to administer Space members, Teams, invitations and
/// policy roots in addition to holding `policy.administer`.
pub(crate) const WORKSPACE_ADMINISTRATOR_ROLES: [&str; 2] = ["owner", "administrator"];

pub(crate) fn administers_workspace(role: &str) -> bool {
    WORKSPACE_ADMINISTRATOR_ROLES.contains(&role)
}

/// Collaboration endpoints that are not scoped by path refuse content-restricted
/// principals even when the action itself is granted.
pub(crate) fn full_access(decision: &AuthorizationDecision) -> bool {
    decision.allowed && !decision.content_restricted
}

/// Active owner/administrator membership of the Workspace containing `space`.
pub(crate) async fn workspace_administrator<'e, E>(
    executor: E,
    principal: &PrincipalContext,
    space: &SpaceId,
) -> V3StoreResult<bool>
where
    E: PgExecutor<'e>,
{
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspace_memberships m JOIN spaces s USING(workspace_id) WHERE s.space_id=$1 AND m.principal_id=$2 AND m.state='active' AND m.role=ANY($3))")
        .bind(space.as_str())
        .bind(principal.principal_id.as_str())
        .bind(WORKSPACE_ADMINISTRATOR_ROLES.as_slice())
        .fetch_one(executor)
        .await
        .map_err(pg_error)
}

/// Only human identities may record review approvals or change requests.
pub(crate) async fn human_principal<'e, E>(
    executor: E,
    principal: &PrincipalContext,
) -> V3StoreResult<bool>
where
    E: PgExecutor<'e>,
{
    sqlx::query_scalar(
        "SELECT principal_kind IN ('account','bootstrap_admin') FROM principals WHERE principal_id=$1",
    )
    .bind(principal.principal_id.as_str())
    .fetch_one(executor)
    .await
    .map_err(pg_error)
}

/// Only service identities may register or execute proof runs.
pub(crate) async fn service_principal<'e, E>(
    executor: E,
    principal: &PrincipalContext,
) -> V3StoreResult<bool>
where
    E: PgExecutor<'e>,
{
    sqlx::query_scalar("SELECT principal_kind='service' FROM principals WHERE principal_id=$1")
        .bind(principal.principal_id.as_str())
        .fetch_one(executor)
        .await
        .map_err(pg_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_owner_and_administrator_roles_administer() {
        assert!(administers_workspace("owner"));
        assert!(administers_workspace("administrator"));
        assert!(!administers_workspace("member"));
        assert!(!administers_workspace("auditor"));
        assert!(!administers_workspace("Owner"));
    }

    #[test]
    fn restricted_or_denied_decisions_are_not_full_access() {
        let mut decision = AuthorizationDecision::allow("explicit_policy_allow");
        assert!(full_access(&decision));
        decision.content_restricted = true;
        assert!(!full_access(&decision));
        assert!(!full_access(&AuthorizationDecision::deny(
            "no_policy_grant"
        )));
    }
}
