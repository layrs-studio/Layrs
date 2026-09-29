use super::*;
impl PgCollaborationStore {
    pub async fn notifications(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        after: u64,
    ) -> V3StoreResult<serde_json::Value> {
        let after = i64::try_from(after).map_err(bad)?;
        let authority = self
            .authority(principal, space, V3Action::ReadReviews)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let read:Option<i64>=sqlx::query_scalar("SELECT last_sequence FROM layrs_collaboration.notification_reads WHERE space_id=$1 AND principal_id=$2").bind(space.as_str()).bind(principal.principal_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        // Context comes from the exact immutable evidence associated with this
        // event, never reviews.latest_version. Subject and Space must also match.
        let rows=sqlx::query("SELECT n.sequence,n.actor_principal_id,n.event_kind,n.subject_id,n.evidence_object_id,n.created_at::text,COALESCE(v.version,a.version,i.version,c.version) AS review_version FROM layrs_collaboration.notifications n LEFT JOIN layrs_collaboration.reviews r ON r.review_id=n.subject_id AND r.space_id=n.space_id LEFT JOIN layrs_collaboration.review_versions v ON n.event_kind='review.proposed' AND v.review_id=r.review_id AND v.proposal_object_id=n.evidence_object_id LEFT JOIN layrs_collaboration.review_actions a ON n.event_kind='review.action' AND a.review_id=r.review_id AND a.evidence_object_id=n.evidence_object_id LEFT JOIN layrs_collaboration.review_integrations i ON n.event_kind='review.integrated' AND i.review_id=r.review_id AND i.space_id=n.space_id AND i.evidence_object_id=n.evidence_object_id LEFT JOIN layrs_collaboration.comment_corrections c ON n.event_kind='review.comment_corrected' AND c.review_id=r.review_id AND c.evidence_object_id=n.evidence_object_id WHERE n.space_id=$1 AND n.sequence>$2 ORDER BY n.sequence LIMIT 101").bind(space.as_str()).bind(after).fetch_all(&mut *tx).await.map_err(pg_error)?;
        let has_more = rows.len() > 100;
        let rows = &rows[..rows.len().min(100)];
        Ok(
            serde_json::json!({"notifications":rows.iter().map(|r|serde_json::json!({"sequence":r.get::<i64,_>("sequence"),"actorPrincipalId":r.get::<String,_>("actor_principal_id"),"kind":r.get::<String,_>("event_kind"),"subjectId":r.get::<String,_>("subject_id"),"evidenceObjectId":r.get::<String,_>("evidence_object_id"),"createdAt":r.get::<String,_>("created_at"),"reviewContext":r.get::<Option<i32>,_>("review_version").map(|version|serde_json::json!({"reviewId":r.get::<String,_>("subject_id"),"version":version}))})).collect::<Vec<_>>(),"lastReadSequence":read.unwrap_or(0),"nextAfter":rows.last().map(|r|r.get::<i64,_>("sequence")).unwrap_or(after),"hasMore":has_more}),
        )
    }
    pub async fn mark_notifications_read(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        sequence: u64,
    ) -> V3StoreResult<serde_json::Value> {
        let sequence = i64::try_from(sequence).map_err(bad)?;
        let authority = self
            .authority(principal, space, V3Action::ReadReviews)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let maximum:i64=sqlx::query_scalar("SELECT COALESCE(max(sequence),0) FROM layrs_collaboration.notifications WHERE space_id=$1").bind(space.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if sequence > maximum {
            return Err(types::invalid("notification_sequence_ahead"));
        }
        let saved:i64=sqlx::query_scalar("INSERT INTO layrs_collaboration.notification_reads(space_id,principal_id,last_sequence) VALUES($1,$2,$3) ON CONFLICT(space_id,principal_id) DO UPDATE SET last_sequence=GREATEST(layrs_collaboration.notification_reads.last_sequence,EXCLUDED.last_sequence) RETURNING last_sequence").bind(space.as_str()).bind(principal.principal_id.as_str()).bind(sequence).fetch_one(&mut *tx).await.map_err(pg_error)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(serde_json::json!({"lastReadSequence":saved}))
    }
}
