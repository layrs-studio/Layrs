use super::*;
use layrs_sync::v3::{ReceiptStatus, SyncTransactionReceipt};

impl PgCollaborationStore {
    /// Called only after final authorization/gates and receipt insertion, inside
    /// the same Space-locked transaction. Matching a current head is insufficient.
    pub(crate) async fn record_review_integrations(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        receipt: &SyncTransactionReceipt,
    ) -> V3StoreResult<()> {
        if receipt.status != ReceiptStatus::Applied {
            return Ok(());
        }
        let Some(view) = receipt
            .view_result
            .as_ref()
            .filter(|v| v.new_generation != v.old_generation)
        else {
            return Ok(());
        };
        let evaluation_policy = receipt
            .policy_root_result
            .as_ref()
            .map(|r| &r.old_policy_set_id)
            .unwrap_or(&receipt.current_policy_set_id);
        for layer in &view.layer_results {
            if layer.resulting_heads.len() != 1
                || layer.previous_heads.contains(&layer.resulting_heads[0])
            {
                continue;
            }
            let step = &layer.resulting_heads[0];
            let rows=sqlx::query("SELECT r.review_id,r.latest_version,v.proposal_object_id FROM layrs_collaboration.reviews r JOIN layrs_collaboration.review_versions v ON v.review_id=r.review_id AND v.version=r.latest_version WHERE r.space_id=$1 AND r.layer_id=$2 AND r.lifecycle='open' AND v.proposed_step_id=$3 AND v.base_repository_view_id=$4 AND v.policy_set_id=$5 AND v.authorization_epoch=$6 ORDER BY r.review_id LIMIT 1025")
                .bind(receipt.space_id.as_str()).bind(layer.layer_id.as_str()).bind(step.as_str()).bind(view.old_repository_view_id.as_str()).bind(evaluation_policy.as_str()).bind(receipt.authorization_epoch as i64).fetch_all(&mut **tx).await.map_err(pg_error)?;
            if rows.len() > 1024 {
                return Err(V3StoreError::LimitExceeded {
                    resource: "reviews_per_integration",
                    limit: 1024,
                });
            }
            for row in rows {
                let review: String = row.get("review_id");
                let version: i32 = row.get("latest_version");
                let proposal: String = row.get("proposal_object_id");
                let document = serde_json::json!({"schemaVersion":"layrs.review-integration.v1","reviewId":review,"version":version,"spaceId":receipt.space_id,"layerId":layer.layer_id,"proposedStepId":step,"proposalObjectId":proposal,"transactionId":receipt.transaction_id,"receiptDigest":receipt.computed_storage_digest().map_err(bad)?,"previousRepositoryViewId":view.old_repository_view_id,"resultingRepositoryViewId":view.new_repository_view_id,"integratedBy":principal.principal_id,"integratedAt":receipt.stored_at});
                let object = self.evidence(&document).await?;
                self.install(tx, &receipt.space_id, &object).await?;
                sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,0,'evidence',$2) ON CONFLICT DO NOTHING").bind(object.descriptor.object_id.as_str()).bind(&proposal).execute(&mut **tx).await.map_err(pg_error)?;
                sqlx::query("INSERT INTO layrs_collaboration.review_integrations(review_id,version,space_id,transaction_id,evidence_object_id,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6)").bind(&review).bind(version).bind(receipt.space_id.as_str()).bind(receipt.transaction_id.as_str()).bind(object.descriptor.object_id.as_str()).bind(jcs(&document)?).execute(&mut **tx).await.map_err(pg_error)?;
                sqlx::query("UPDATE layrs_collaboration.reviews SET lifecycle='integrated' WHERE review_id=$1 AND lifecycle='open'").bind(&review).execute(&mut **tx).await.map_err(pg_error)?;
                self.notify(
                    tx,
                    principal,
                    &receipt.space_id,
                    "review.integrated",
                    &review,
                    &object.descriptor.object_id,
                )
                .await?;
            }
        }
        Ok(())
    }
}
