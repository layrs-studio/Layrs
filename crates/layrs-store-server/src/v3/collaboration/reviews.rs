use super::*;
use types::invalid;

impl PgCollaborationStore {
    async fn validate_anchor(
        &self,
        space: &SpaceId,
        step: &str,
        anchor: &DiffAnchor,
    ) -> V3StoreResult<()> {
        use layrs_core::scm::{FileManifest, Step, Tree, TreeEntryTarget};
        let raw: String =
            sqlx::query_scalar("SELECT canonical_record_jcs FROM steps WHERE step_id=$1")
                .bind(step)
                .fetch_one(self.repository.pool())
                .await
                .map_err(pg_error)?;
        let step: Step = serde_json::from_str(&raw).map_err(bad)?;
        let mut tree_id: ObjectId = parse(step.root_tree_id().as_str().into())?;
        let parts: Vec<&str> = anchor.path.as_str().split('/').collect();
        if parts.len() > 256 {
            return Err(invalid("anchor_path_too_deep"));
        }
        for (index, name) in parts.iter().enumerate() {
            let descriptor = load_space_descriptor(self.repository.pool(), space, &tree_id).await?;
            let bytes =
                read_canonical_object(self.repository.cas().as_ref(), &descriptor, 4 * 1024 * 1024)
                    .await?;
            let tree: Tree = serde_json::from_slice(&bytes).map_err(bad)?;
            let entry = tree
                .entries()
                .iter()
                .find(|e| e.name().as_str() == *name)
                .ok_or(invalid("anchor_path_not_in_step"))?;
            if index + 1 < parts.len() {
                let TreeEntryTarget::Tree { tree_id: next } = entry.target() else {
                    return Err(invalid("anchor_path_not_in_step"));
                };
                tree_id = parse(next.as_str().into())?;
                continue;
            }
            let TreeEntryTarget::File { file_id } = entry.target() else {
                return Err(invalid("anchor_requires_text_file"));
            };
            let manifest:Option<String>=sqlx::query_scalar("SELECT fm.file_manifest_id FROM file_manifests fm JOIN space_object_availability a ON a.object_id=fm.file_manifest_id WHERE fm.file_id=$1 AND fm.reconstruction_verified_at IS NOT NULL AND a.space_id=$2 AND a.availability_state='available' ORDER BY fm.file_manifest_id LIMIT 1").bind(file_id.as_str()).bind(space.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?;
            let id = parse(manifest.ok_or(V3StoreError::NotFound)?)?;
            let descriptor = load_space_descriptor(self.repository.pool(), space, &id).await?;
            let bytes =
                read_canonical_object(self.repository.cas().as_ref(), &descriptor, 4 * 1024 * 1024)
                    .await?;
            let manifest: FileManifest = serde_json::from_slice(&bytes).map_err(bad)?;
            if manifest.byte_length() > 2 * 1024 * 1024 {
                return Err(invalid("anchor_text_too_large"));
            }
            let mut text = vec![];
            for chunk in manifest.chunks() {
                let id = parse(chunk.chunk_id().as_str().into())?;
                let descriptor = load_space_descriptor(self.repository.pool(), space, &id).await?;
                text.extend(
                    read_canonical_object(
                        self.repository.cas().as_ref(),
                        &descriptor,
                        2 * 1024 * 1024,
                    )
                    .await?,
                );
            }
            let text = std::str::from_utf8(&text).map_err(|_| invalid("anchor_requires_utf8"))?;
            if text.contains('\0') || anchor.end_line as usize > text.lines().count().max(1) {
                return Err(invalid("anchor_line_outside_file"));
            }
            return Ok(());
        }
        Err(invalid("anchor_path_not_in_step"))
    }
    /// A new version requires the same author and exact latest-version compare-and-swap.
    pub async fn propose_review(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        existing: Option<&str>,
        request: ReviewProposalRequest,
    ) -> V3StoreResult<ReviewVersion> {
        request.validate()?;
        let authority = self
            .authority(principal, space, V3Action::WriteReviews)
            .await?;
        let review_id = existing
            .map(str::to_owned)
            .unwrap_or_else(|| key_id("review", principal, space, &request.idempotency_key));
        let version = request.expected_version + 1;
        let request_digest = digest(&request)?;
        let document = ReviewVersion {
            schema_version: "layrs.review.v1".into(),
            review_id: review_id.clone(),
            version,
            space_id: space.clone(),
            layer_id: request.layer_id.clone(),
            author_principal_id: principal.principal_id.clone(),
            proposed_step_id: request.proposed_step_id.clone(),
            base_repository_view_id: authority.repository_view_id.clone(),
            policy_set_id: authority.policy_set_id.clone(),
            authorization_epoch: authority.authorization_epoch,
            title: request.title.clone(),
            participants: request.participants.clone(),
            created_at: now()?,
        };
        let object = self.evidence(&document).await?;
        let mut tx = self.locked(principal, &authority).await?;
        if let Some(row)=sqlx::query("SELECT request_digest,canonical_document_jcs FROM layrs_collaboration.review_versions v JOIN layrs_collaboration.reviews r USING(review_id) WHERE r.space_id=$1 AND r.author_principal_id=$2 AND v.review_id=$3 AND v.version=$4").bind(space.as_str()).bind(principal.principal_id.as_str()).bind(&review_id).bind(version as i32).fetch_optional(&mut *tx).await.map_err(pg_error)?{
            if row.get::<String,_>("request_digest")!=request_digest{return Err(invalid("idempotency_digest_mismatch"))}
            return serde_json::from_str(&row.get::<String,_>("canonical_document_jcs")).map_err(bad)
        }
        let admitted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM space_steps ss JOIN steps st USING(step_id) JOIN current_layer_definitions l ON l.space_id=ss.space_id AND l.layer_id=$3 WHERE ss.space_id=$1 AND ss.step_id=$2)").bind(space.as_str()).bind(request.proposed_step_id.as_str()).bind(request.layer_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if !admitted {
            return Err(invalid("review_requires_admitted_step_and_layer"));
        }
        for participant in &request.participants {
            let member:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM spaces s JOIN workspace_memberships m USING(workspace_id) JOIN principals p USING(principal_id) WHERE s.space_id=$1 AND p.principal_id=$2 AND p.status='active' AND m.state='active')").bind(space.as_str()).bind(participant.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
            if !member {
                return Err(invalid("participant_not_active"));
            }
        }
        if existing.is_none() {
            if request.expected_version != 0 {
                return Err(invalid("expected_version_mismatch"));
            }
            sqlx::query("INSERT INTO layrs_collaboration.reviews(review_id,space_id,layer_id,author_principal_id,latest_version) VALUES($1,$2,$3,$4,1)").bind(&review_id).bind(space.as_str()).bind(request.layer_id.as_str()).bind(principal.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        } else {
            let changed=sqlx::query("UPDATE layrs_collaboration.reviews SET latest_version=$1 WHERE review_id=$2 AND space_id=$3 AND author_principal_id=$4 AND layer_id=$5 AND latest_version=$6 AND lifecycle='open'").bind(version as i32).bind(&review_id).bind(space.as_str()).bind(principal.principal_id.as_str()).bind(request.layer_id.as_str()).bind(request.expected_version as i32).execute(&mut *tx).await.map_err(pg_error)?.rows_affected();
            if changed != 1 {
                return Err(invalid("review_version_conflict"));
            }
        }
        self.install(&mut tx, space, &object).await?;
        sqlx::query("INSERT INTO layrs_collaboration.review_versions(review_id,version,proposal_object_id,proposed_step_id,base_repository_view_id,policy_set_id,authorization_epoch,request_digest,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(&review_id).bind(version as i32).bind(object.descriptor.object_id.as_str()).bind(request.proposed_step_id.as_str()).bind(authority.repository_view_id.as_str()).bind(authority.policy_set_id.as_str()).bind(authority.authorization_epoch as i64).bind(&request_digest).bind(jcs(&document)?).execute(&mut *tx).await.map_err(pg_error)?;
        self.notify(
            &mut tx,
            principal,
            space,
            "review.proposed",
            &review_id,
            &object.descriptor.object_id,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(document)
    }
    pub async fn review_action(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review_id: &str,
        request: ReviewActionRequest,
    ) -> V3StoreResult<ReviewActionDocument> {
        request.validate()?;
        let action = if matches!(
            request.action,
            ReviewActionKind::Approve | ReviewActionKind::RequestChanges
        ) {
            V3Action::ApproveReview
        } else {
            V3Action::WriteReviews
        };
        let authority = self.authority(principal, space, action).await?;
        let action_id = key_id("review-action", principal, space, &request.idempotency_key);
        let request_digest = digest(&(review_id, &request))?;
        // Space lock serializes new versions and decisions; Evidence CAS is immutable.
        let mut tx = self.locked(principal, &authority).await?;
        if let Some(row)=sqlx::query("SELECT request_digest,canonical_document_jcs FROM layrs_collaboration.review_actions WHERE action_id=$1").bind(&action_id).fetch_optional(&mut *tx).await.map_err(pg_error)?{if row.get::<String,_>("request_digest")!=request_digest{return Err(invalid("idempotency_digest_mismatch"))}return serde_json::from_str(&row.get::<String,_>("canonical_document_jcs")).map_err(bad)}
        let row=sqlx::query("SELECT r.author_principal_id,r.lifecycle,v.proposal_object_id,v.proposed_step_id,v.policy_set_id,v.authorization_epoch FROM layrs_collaboration.reviews r JOIN layrs_collaboration.review_versions v ON v.review_id=r.review_id AND v.version=r.latest_version WHERE r.review_id=$1 AND r.space_id=$2 AND r.latest_version=$3").bind(review_id).bind(space.as_str()).bind(request.version as i32).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(invalid("stale_review_version"))?;
        if let Some(anchor) = &request.anchor {
            self.validate_anchor(space, &row.get::<String, _>("proposed_step_id"), anchor)
                .await?;
        }
        if matches!(
            request.action,
            ReviewActionKind::Approve | ReviewActionKind::RequestChanges
        ) {
            let human = super::super::space_access::human_principal(&mut *tx, principal).await?;
            if !human
                || row.get::<String, _>("lifecycle") != "open"
                || row.get::<String, _>("policy_set_id") != authority.policy_set_id.as_str()
                || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
            {
                return Err(invalid("review_decision_not_current"));
            }
        }
        if matches!(
            request.action,
            ReviewActionKind::Close | ReviewActionKind::Reopen
        ) && row.get::<String, _>("author_principal_id") != principal.principal_id.as_str()
        {
            return Err(V3StoreError::InvalidCapability);
        }
        if row.get::<String, _>("lifecycle") == "integrated"
            && matches!(
                request.action,
                ReviewActionKind::Close | ReviewActionKind::Reopen
            )
        {
            return Err(invalid("review_already_integrated"));
        }
        if let Some(reply) = &request.reply_to {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.review_actions WHERE action_id=$1 AND review_id=$2 AND version=$3)").bind(reply).bind(review_id).bind(request.version as i32).fetch_one(&mut *tx).await.map_err(pg_error)?;
            if !exists {
                return Err(invalid("reply_not_in_version"));
            }
        }
        let document = ReviewActionDocument {
            schema_version: "layrs.review-action.v1".into(),
            action_id: action_id.clone(),
            review_id: review_id.into(),
            version: request.version,
            proposal_object_id: parse(row.get("proposal_object_id"))?,
            actor_principal_id: principal.principal_id.clone(),
            action: request.action,
            body: request.body,
            anchor: request.anchor,
            reply_to: request.reply_to,
            created_at: now()?,
        };
        let object = self.evidence(&document).await?;
        self.install(&mut tx, space, &object).await?;
        sqlx::query("INSERT INTO layrs_collaboration.review_actions(action_id,review_id,version,actor_principal_id,action_kind,evidence_object_id,request_digest,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(&action_id).bind(review_id).bind(request.version as i32).bind(principal.principal_id.as_str()).bind(request.action.as_str()).bind(object.descriptor.object_id.as_str()).bind(&request_digest).bind(jcs(&document)?).execute(&mut *tx).await.map_err(pg_error)?;
        if matches!(
            request.action,
            ReviewActionKind::Close | ReviewActionKind::Reopen
        ) {
            sqlx::query("UPDATE layrs_collaboration.reviews SET lifecycle=$1 WHERE review_id=$2")
                .bind(if request.action == ReviewActionKind::Close {
                    "closed"
                } else {
                    "open"
                })
                .bind(review_id)
                .execute(&mut *tx)
                .await
                .map_err(pg_error)?;
        }
        self.notify(
            &mut tx,
            principal,
            space,
            "review.action",
            review_id,
            &object.descriptor.object_id,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(document)
    }
    pub async fn list_reviews(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<serde_json::Value> {
        self.list_reviews_page(principal, space, ReviewListQuery::default())
            .await
    }
    pub async fn get_review(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review: &str,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::ReadReviews)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let lifecycle: Option<String> = sqlx::query_scalar(
            "SELECT lifecycle FROM layrs_collaboration.reviews WHERE space_id=$1 AND review_id=$2",
        )
        .bind(space.as_str())
        .bind(review)
        .fetch_optional(&mut *tx)
        .await
        .map_err(pg_error)?;
        let lifecycle = lifecycle.ok_or(V3StoreError::NotFound)?;
        let versions:Vec<String>=sqlx::query_scalar("SELECT canonical_document_jcs FROM layrs_collaboration.review_versions WHERE review_id=$1 ORDER BY version LIMIT 10001").bind(review).fetch_all(&mut *tx).await.map_err(pg_error)?;
        let actions:Vec<String>=sqlx::query_scalar("SELECT canonical_document_jcs FROM layrs_collaboration.review_actions WHERE review_id=$1 ORDER BY sequence LIMIT 10001").bind(review).fetch_all(&mut *tx).await.map_err(pg_error)?;
        if versions.len() > 10000 || actions.len() > 10000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "review_detail_records",
                limit: 10000,
            });
        }
        let latest: ReviewVersion =
            serde_json::from_str(versions.last().ok_or(V3StoreError::NotFound)?).map_err(bad)?;
        let decision = self
            .review_decision_state(&mut tx, &authority, &latest)
            .await?;
        let integration=sqlx::query("SELECT evidence_object_id,canonical_document_jcs FROM layrs_collaboration.review_integrations WHERE space_id=$1 AND review_id=$2").bind(space.as_str()).bind(review).fetch_optional(&mut *tx).await.map_err(pg_error)?.map(|row| {
            let document:serde_json::Value=serde_json::from_str(&row.get::<String,_>("canonical_document_jcs")).map_err(bad)?;
            Ok::<_,V3StoreError>(serde_json::json!({"evidenceObjectId":row.get::<String,_>("evidence_object_id"),"record":document}))
        }).transpose()?;
        let corrections = self.comment_corrections(&mut tx, review).await?;
        Ok(
            serde_json::json!({"lifecycle":lifecycle,"decisionState":decision,"integration":integration,"commentCorrections":corrections,"versions":versions.iter().map(|s|serde_json::from_str::<serde_json::Value>(s).map_err(bad)).collect::<V3StoreResult<Vec<_>>>()?,"actions":actions.iter().map(|s|serde_json::from_str::<serde_json::Value>(s).map_err(bad)).collect::<V3StoreResult<Vec<_>>>()?}),
        )
    }
}
