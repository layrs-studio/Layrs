use super::*;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorrectCommentRequest {
    pub idempotency_key: String,
    pub version: u32,
    pub expected_revision: u32,
    pub body: String,
    pub reason: String,
}
const MAX_REVIEW_CORRECTIONS: i64 = 1000;
const MAX_REVIEW_CORRECTION_BYTES: i64 = 4 * 1024 * 1024;

impl PgCollaborationStore {
    pub async fn correct_comment(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review: &str,
        action: &str,
        request: CorrectCommentRequest,
    ) -> V3StoreResult<serde_json::Value> {
        types::validate_key(&request.idempotency_key)?;
        if request.version == 0
            || request.version > 10001
            || request.expected_revision >= 100
            || request.body.trim().is_empty()
            || request.body.len() > 64 * 1024
            || request.body.contains('\0')
            || request.reason.trim().is_empty()
            || request.reason.len() > 4096
            || request.reason.contains('\0')
        {
            return Err(types::invalid("invalid_comment_correction"));
        }
        let authority = self
            .authority(principal, space, V3Action::WriteReviews)
            .await?;
        let correction = key_id(
            "review-comment-correction",
            principal,
            space,
            &request.idempotency_key,
        );
        let request_digest = digest(&(review, action, &request))?;
        let mut tx = self.locked(principal, &authority).await?;
        let original=sqlx::query("SELECT a.actor_principal_id,a.action_kind,a.evidence_object_id,a.canonical_document_jcs FROM layrs_collaboration.review_actions a JOIN layrs_collaboration.reviews r ON r.review_id=a.review_id WHERE r.space_id=$1 AND a.review_id=$2 AND a.action_id=$3 AND a.version=$4")
            .bind(space.as_str()).bind(review).bind(action).bind(request.version as i32).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        // Even policy administrators cannot impersonate another comment author.
        if original.get::<String, _>("actor_principal_id") != principal.principal_id.as_str() {
            return Err(V3StoreError::InvalidCapability);
        }
        if original.get::<String, _>("action_kind") != "comment" {
            return Err(types::invalid("correction_requires_comment"));
        }
        if let Some(row)=sqlx::query("SELECT request_digest,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.comment_corrections WHERE correction_id=$1")
            .bind(&correction).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("request_digest") != request_digest { return Err(types::invalid("idempotency_digest_mismatch")); }
            return correction_document(&row);
        }
        let previous=sqlx::query("SELECT revision,evidence_object_id FROM layrs_collaboration.comment_corrections WHERE action_id=$1 ORDER BY revision DESC LIMIT 1")
            .bind(action).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let revision = previous
            .as_ref()
            .map(|row| row.get::<i32, _>("revision"))
            .unwrap_or(0);
        if revision != request.expected_revision as i32 {
            return Err(types::invalid("stale_comment_revision"));
        }
        let previous_object = previous.map(|row| row.get::<String, _>("evidence_object_id"));
        let original_object: String = original.get("evidence_object_id");
        let original_record: ReviewActionDocument =
            serde_json::from_str(&original.get::<String, _>("canonical_document_jcs"))
                .map_err(bad)?;
        let record = serde_json::json!({
            "schemaVersion":"layrs.review-comment-correction.v1","correctionId":correction,
            "spaceId":space,"reviewId":review,"version":request.version,"actionId":action,"revision":revision+1,
            "body":request.body,"reason":request.reason,"actorPrincipalId":principal.principal_id,"correctedAt":now()?,
            "originalActionObjectId":original_object,"previousCorrectionObjectId":previous_object,
            "proposalObjectId":original_record.proposal_object_id,"policySetId":authority.policy_set_id,"authorizationEpoch":authority.authorization_epoch,
        });
        let encoded = jcs(&record)?;
        if encoded.len() > 96 * 1024 {
            return Err(V3StoreError::LimitExceeded {
                resource: "comment_correction_bytes",
                limit: 96 * 1024,
            });
        }
        let (count,bytes): (i64,i64)=sqlx::query_as("SELECT count(*),coalesce(sum(octet_length(canonical_document_jcs)),0)::bigint FROM layrs_collaboration.comment_corrections WHERE review_id=$1")
            .bind(review).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if count >= MAX_REVIEW_CORRECTIONS
            || bytes + encoded.len() as i64 > MAX_REVIEW_CORRECTION_BYTES
        {
            return Err(V3StoreError::LimitExceeded {
                resource: "review_comment_corrections",
                limit: MAX_REVIEW_CORRECTIONS as u64,
            });
        }
        let object = self.evidence(&record).await?;
        self.install(&mut tx, space, &object).await?;
        let mut references = vec![
            original_object,
            original_record.proposal_object_id.to_string(),
            authority.policy_set_id.to_string(),
        ];
        references.extend(previous_object);
        references.sort();
        references.dedup();
        for (ordinal, reference) in references.iter().enumerate() {
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,$2,'evidence',$3)")
                .bind(object.descriptor.object_id.as_str()).bind(ordinal as i32).bind(reference).execute(&mut *tx).await.map_err(pg_error)?;
        }
        sqlx::query("INSERT INTO layrs_collaboration.comment_corrections(correction_id,action_id,review_id,version,revision,actor_principal_id,request_digest,evidence_object_id,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(&correction).bind(action).bind(review).bind(request.version as i32).bind(revision+1).bind(principal.principal_id.as_str()).bind(request_digest).bind(object.descriptor.object_id.as_str()).bind(encoded).execute(&mut *tx).await.map_err(pg_error)?;
        self.notify(
            &mut tx,
            principal,
            space,
            "review.comment_corrected",
            review,
            &object.descriptor.object_id,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(serde_json::json!({"evidenceObjectId":object.descriptor.object_id,"record":record}))
    }
    pub(super) async fn comment_corrections(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        review: &str,
    ) -> V3StoreResult<Vec<serde_json::Value>> {
        // Preflight before loading text. Projection is bounded even if an
        // operator bypassed the ordinary writer's append budgets.
        let (count,bytes):(i64,i64)=sqlx::query_as("SELECT count(*),coalesce(sum(octet_length(canonical_document_jcs)),0)::bigint FROM layrs_collaboration.comment_corrections WHERE review_id=$1")
            .bind(review).fetch_one(&mut **tx).await.map_err(pg_error)?;
        if count > MAX_REVIEW_CORRECTIONS || bytes > MAX_REVIEW_CORRECTION_BYTES {
            return Err(V3StoreError::LimitExceeded {
                resource: "review_comment_corrections",
                limit: MAX_REVIEW_CORRECTIONS as u64,
            });
        }
        let rows=sqlx::query("SELECT evidence_object_id,canonical_document_jcs FROM layrs_collaboration.comment_corrections WHERE review_id=$1 ORDER BY version,action_id,revision LIMIT 1000")
            .bind(review).fetch_all(&mut **tx).await.map_err(pg_error)?;
        rows.iter().map(correction_document).collect()
    }
}
fn correction_document(row: &sqlx::postgres::PgRow) -> V3StoreResult<serde_json::Value> {
    let record: serde_json::Value =
        serde_json::from_str(&row.get::<String, _>("canonical_document_jcs")).map_err(bad)?;
    Ok(
        serde_json::json!({"evidenceObjectId":row.get::<String,_>("evidence_object_id"),"record":record}),
    )
}
