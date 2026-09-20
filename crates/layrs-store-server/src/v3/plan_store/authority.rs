use super::*;

pub(super) async fn revalidate_plan_authority(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    principal: &PrincipalContext,
    context: &PlanContext,
) -> V3StoreResult<()> {
    let valid: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM spaces s
            JOIN workspaces w ON w.workspace_id=s.workspace_id
            JOIN principals p ON p.principal_id=$2 AND p.deployment_id=w.deployment_id
            JOIN workspace_memberships wm
              ON wm.workspace_id=w.workspace_id AND wm.principal_id=p.principal_id
            WHERE s.space_id=$1 AND s.lifecycle_state='ready'
              AND s.authorization_epoch=$3
              AND p.actor_id=$4 AND p.subject_digest=$5 AND p.status='active'
              AND wm.state='active'
            FOR SHARE OF s, p, wm
        )
        "#,
    )
    .bind(context.actor.space_id.as_str())
    .bind(principal.principal_id.as_str())
    .bind(i64_from_u64(context.actor.authorization_epoch)?)
    .bind(principal.actor_id.as_str())
    .bind(&principal.subject_digest)
    .fetch_one(&mut **tx)
    .await
    .map_err(pg_error)?;
    if valid {
        Ok(())
    } else {
        Err(V3StoreError::AuthorizationEpochChanged)
    }
}
