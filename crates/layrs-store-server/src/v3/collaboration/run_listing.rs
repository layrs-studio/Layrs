use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunListQuery {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    schema: String,
    space: SpaceId,
    epoch: u64,
    anchor: i64,
    before: i64,
}
impl PgCollaborationStore {
    pub async fn list_runs_page(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        query: RunListQuery,
    ) -> V3StoreResult<serde_json::Value> {
        let limit = query.limit.unwrap_or(100);
        if !(1..=100).contains(&limit) {
            return Err(types::invalid("invalid_run_page_limit"));
        }
        let cursor: Option<Cursor> = query
            .cursor
            .map(|token| {
                if token.len() > 1024 {
                    return Err(types::invalid("invalid_run_cursor"));
                }
                let bytes = URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|_| types::invalid("invalid_run_cursor"))?;
                let cursor: Cursor = serde_json::from_slice(&bytes)
                    .map_err(|_| types::invalid("invalid_run_cursor"))?;
                if cursor.schema != "layrs.run-list-cursor.v1"
                    || cursor.space != *space
                    || cursor.anchor <= 0
                    || cursor.before <= 0
                    || cursor.before > cursor.anchor
                {
                    return Err(types::invalid("invalid_run_cursor"));
                }
                Ok(cursor)
            })
            .transpose()?;
        let authority = self
            .authority(principal, space, V3Action::Bootstrap)
            .await?;
        if cursor
            .as_ref()
            .is_some_and(|c| c.epoch != authority.authorization_epoch)
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        // Run creation and this snapshot use the same Space lock: no later
        // commit can insert a sequence below this anchor. Completion stays live.
        let mut tx = self.locked(principal, &authority).await?;
        let anchor = match &cursor {
            Some(c) => c.anchor,
            None => sqlx::query_scalar("SELECT coalesce(max(list_sequence),0) FROM layrs_collaboration.runs WHERE space_id=$1")
                .bind(space.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?,
        };
        let rows = sqlx::query("SELECT r.list_sequence,r.run_id,r.step_id,r.recipe_object_id,r.runner_principal_id,r.proof_object_id,r.outcome,to_char(r.created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,to_char(r.expires_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS expires_at,to_char(r.completed_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS completed_at,c.evidence_object_id AS cancellation_object_id,c.canonical_document_jcs AS cancellation_document,CASE WHEN c.run_id IS NOT NULL THEN 'cancelled' WHEN r.proof_object_id IS NOT NULL THEN 'completed' WHEN r.expires_at<=clock_timestamp() THEN 'expired' WHEN r.execution_requested AND r.execution_started_at IS NULL THEN 'queued' ELSE 'running' END AS status FROM layrs_collaboration.runs r LEFT JOIN layrs_collaboration.run_cancellations c ON c.run_id=r.run_id AND c.space_id=r.space_id WHERE r.space_id=$1 AND r.list_sequence<=$2 AND ($3::bigint IS NULL OR r.list_sequence<$3) ORDER BY r.list_sequence DESC LIMIT $4")
            .bind(space.as_str()).bind(anchor).bind(cursor.as_ref().map(|c| c.before)).bind(i64::from(limit)+1)
            .fetch_all(&mut *tx).await.map_err(pg_error)?;
        let has_more = rows.len() > usize::from(limit);
        let mut runs = vec![];
        let mut last = 0;
        for row in rows.into_iter().take(usize::from(limit)) {
            last = row.get("list_sequence");
            runs.push(serde_json::json!({
                "runId":row.get::<String,_>("run_id"),"stepId":row.get::<String,_>("step_id"),
                "recipeObjectId":row.get::<String,_>("recipe_object_id"),"runnerPrincipalId":row.get::<String,_>("runner_principal_id"),
                "proofObjectId":row.get::<Option<String>,_>("proof_object_id"),"outcome":row.get::<Option<String>,_>("outcome"),
                "createdAt":row.get::<String,_>("created_at"),"expiresAt":row.get::<String,_>("expires_at"),
                "completedAt":row.get::<Option<String>,_>("completed_at"),"status":row.get::<String,_>("status"),
                "cancellation":super::run_cancellation::cancellation_from_row(&row)?
            }));
        }
        let next = if has_more {
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&Cursor {
                        schema: "layrs.run-list-cursor.v1".into(),
                        space: space.clone(),
                        epoch: authority.authorization_epoch,
                        anchor,
                        before: last,
                    })
                    .map_err(bad)?,
                ),
            )
        } else {
            None
        };
        Ok(serde_json::json!({"runs":runs,"nextCursor":next,"hasMore":has_more}))
    }
}
