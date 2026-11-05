use super::*;
impl PgCollaborationStore {
    /// URL policy and key derivation belong to the instance HTTP boundary.
    pub async fn create_webhook(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
        url: &str,
        kinds: &[String],
    ) -> V3StoreResult<()> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM layrs_collaboration.webhooks WHERE space_id=$1",
        )
        .bind(space.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(pg_error)?;
        if count >= 16 {
            return Err(types::invalid("webhook_limit"));
        }
        sqlx::query("INSERT INTO layrs_collaboration.webhooks(webhook_id,space_id,url,event_kinds,start_sequence,created_by) VALUES($1,$2,$3,$4,(SELECT COALESCE(max(sequence),0) FROM layrs_collaboration.notifications),$5)").bind(id).bind(space.as_str()).bind(url).bind(kinds).bind(principal.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        audit(&mut tx, id, principal, "create", None).await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(())
    }
    pub async fn mutate_webhook(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
        action: &str,
        delivery: Option<&str>,
    ) -> V3StoreResult<u64> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let old:Option<i64>=sqlx::query_scalar("SELECT secret_generation FROM layrs_collaboration.webhooks WHERE webhook_id=$1 AND space_id=$2 FOR UPDATE").bind(id).bind(space.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let mut generation = old.ok_or(V3StoreError::NotFound)?;
        match action {
            "rotate" => {
                generation += 1;
                sqlx::query("UPDATE layrs_collaboration.webhooks SET secret_generation=$2 WHERE webhook_id=$1").bind(id).bind(generation).execute(&mut *tx).await.map_err(pg_error)?;
            }
            "disable" => {
                sqlx::query(
                    "UPDATE layrs_collaboration.webhooks SET enabled=FALSE WHERE webhook_id=$1",
                )
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(pg_error)?;
            }
            "replay" => {
                let changed=sqlx::query("UPDATE layrs_collaboration.webhook_deliveries SET state='pending',attempts=0,next_attempt_at=clock_timestamp(),lease_until=NULL,last_error=NULL WHERE webhook_id=$1 AND delivery_id=$2 AND state IN ('delivered','dead')").bind(id).bind(delivery.ok_or(types::invalid("missing_delivery_id"))?).execute(&mut *tx).await.map_err(pg_error)?.rows_affected();
                if changed != 1 {
                    return Err(types::invalid("delivery_not_replayable"));
                }
            }
            _ => return Err(types::invalid("invalid_webhook_action")),
        }
        audit(&mut tx, id, principal, action, delivery).await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(generation as u64)
    }
    pub async fn list_webhooks(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let rows=sqlx::query("SELECT webhook_id,url,event_kinds,secret_generation,enabled,created_at::text FROM layrs_collaboration.webhooks WHERE space_id=$1 ORDER BY created_at").bind(space.as_str()).fetch_all(&mut *tx).await.map_err(pg_error)?;
        Ok(
            serde_json::json!({"webhooks":rows.iter().map(|r|serde_json::json!({"webhookId":r.get::<String,_>("webhook_id"),"url":r.get::<String,_>("url"),"eventKinds":r.get::<Vec<String>,_>("event_kinds"),"secretGeneration":r.get::<i64,_>("secret_generation"),"enabled":r.get::<bool,_>("enabled"),"createdAt":r.get::<String,_>("created_at")})).collect::<Vec<_>>()}),
        )
    }
    pub async fn webhook_deliveries(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let rows=sqlx::query("SELECT d.delivery_id,d.notification_sequence,d.state,d.attempts,d.last_status,d.last_error,d.next_attempt_at::text FROM layrs_collaboration.webhook_deliveries d JOIN layrs_collaboration.webhooks h USING(webhook_id) WHERE h.space_id=$1 AND h.webhook_id=$2 ORDER BY d.notification_sequence DESC LIMIT 100").bind(space.as_str()).bind(id).fetch_all(&mut *tx).await.map_err(pg_error)?;
        Ok(
            serde_json::json!({"deliveries":rows.iter().map(|r|serde_json::json!({"deliveryId":r.get::<String,_>("delivery_id"),"sequence":r.get::<i64,_>("notification_sequence"),"state":r.get::<String,_>("state"),"attempts":r.get::<i32,_>("attempts"),"lastStatus":r.get::<Option<i32>,_>("last_status"),"lastError":r.get::<Option<String>,_>("last_error"),"nextAttemptAt":r.get::<String,_>("next_attempt_at")})).collect::<Vec<_>>()}),
        )
    }
}
async fn audit(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    principal: &PrincipalContext,
    action: &str,
    delivery: Option<&str>,
) -> V3StoreResult<()> {
    sqlx::query("INSERT INTO layrs_collaboration.webhook_audit(webhook_id,actor_principal_id,action,delivery_id) VALUES($1,$2,$3,$4)").bind(id).bind(principal.principal_id.as_str()).bind(action).bind(delivery).execute(&mut **tx).await.map_err(pg_error)?;
    Ok(())
}
