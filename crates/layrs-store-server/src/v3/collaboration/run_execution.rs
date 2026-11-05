//! Explicit producer declaration, not a hosted queue or process supervisor.
use super::*;

impl PgCollaborationStore {
    pub async fn begin_run_execution(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        run: &str,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self.authority(principal, space, V3Action::RunProof).await?;
        let mut tx = self.locked(principal, &authority).await?;
        let row = sqlx::query("SELECT policy_set_id,authorization_epoch,execution_requested,execution_started_at IS NOT NULL AS started,to_char(execution_started_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS started_at,proof_object_id,expires_at>clock_timestamp() AS current FROM layrs_collaboration.runs WHERE run_id=$1 AND space_id=$2 AND runner_principal_id=$3 FOR UPDATE")
            .bind(run).bind(space.as_str()).bind(principal.principal_id.as_str())
            .fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let cancelled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.run_cancellations WHERE space_id=$1 AND run_id=$2)")
            .bind(space.as_str()).bind(run).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if cancelled {
            return Err(types::invalid("run_cancelled"));
        }
        if !row.get::<bool, _>("current")
            || row.get::<String, _>("policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(types::invalid("stale_run"));
        }
        if !row.get::<bool, _>("execution_requested") {
            return Err(types::invalid("run_already_declared_running"));
        }
        if row.get::<Option<String>, _>("proof_object_id").is_some() {
            return Err(types::invalid("run_already_completed"));
        }
        let started: String = if row.get::<bool, _>("started") {
            row.get("started_at")
        } else {
            sqlx::query_scalar("UPDATE layrs_collaboration.runs SET execution_started_at=GREATEST(created_at,clock_timestamp()) WHERE run_id=$1 AND execution_started_at IS NULL RETURNING to_char(execution_started_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')")
                .bind(run).fetch_one(&mut *tx).await.map_err(pg_error)?
        };
        tx.commit().await.map_err(pg_error)?;
        Ok(serde_json::json!({"runId":run,"executionStartedAt":started}))
    }
}
