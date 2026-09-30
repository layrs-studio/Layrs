//! Candidate-bound review access. Generic collaboration endpoints remain whole-Space only.
use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use layrs_core::scm::RepoPath;
use layrs_sync::v3::{Rejection, RejectionCode};
use layrs_sync::v3::{
    ScopedIntegrationStatus, ScopedReview, ScopedReviewAction, ScopedReviewActionKind,
    ScopedReviewActionRequest, ScopedReviewDetail, ScopedReviewListQuery, ScopedReviewPage,
    ScopedReviewProposalRequest,
};
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    epoch: u64,
    principal: String,
    anchor: String,
    offset: usize,
}

#[derive(Debug, Clone)]
pub struct ScopedIntegrationTarget {
    pub candidate_id: String,
    pub candidate_step_id: layrs_core::scm::StepId,
    pub layer_id: layrs_core::scm::LayerId,
    pub expected_view_id: layrs_core::scm::RepositoryViewId,
    pub expected_generation: u64,
    pub expected_policy_set_id: ObjectId,
    pub expected_epoch: u64,
}

impl PgCollaborationStore {
    async fn scoped_authority(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        action: V3Action,
    ) -> V3StoreResult<SpaceAuthority> {
        // Unlike `authority`, content-restricted principals pass: every scoped
        // operation then checks the candidate's exact paths.
        let authority = self.current_authority(space).await?;
        require_allowed(self.decide(principal, &authority, action).await?)?;
        Ok(authority)
    }

    async fn scoped_paths(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        candidate: &ScopedPatchCandidate,
        write: bool,
    ) -> V3StoreResult<()> {
        if candidate.space_id != authority.space_id
            || candidate.changed_paths.is_empty()
            || candidate.changed_paths.len() > 1024
            || candidate
                .changed_paths
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(V3StoreError::InvalidCapability);
        }
        for path in &candidate.changed_paths {
            require_allowed(
                self.repository
                    .authorizer()
                    .authorize(&AuthorizationCheck {
                        principal: principal.clone(),
                        action: if write {
                            V3Action::ScopedPathWrite
                        } else {
                            V3Action::ReadProjection
                        },
                        target: AuthorizationTarget::ScopedPath {
                            space_id: authority.space_id.clone(),
                            base_step_id: candidate.base_step_id.clone(),
                            root_tree_id: candidate.base_root_tree_id.clone(),
                            repo_path: path.clone(),
                        },
                        policy_set_id: authority.policy_set_id.clone(),
                        authorization_epoch: authority.authorization_epoch,
                    })
                    .await?,
            )?;
        }
        Ok(())
    }

    async fn scoped_candidate(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        candidate_id: &str,
        write: bool,
    ) -> V3StoreResult<ScopedPatchCandidate> {
        let candidate = load_candidate(&self.repository, &authority.space_id, candidate_id)
            .await?
            .ok_or(V3StoreError::NotFound)?;
        self.scoped_paths(principal, authority, &candidate, false)
            .await?;
        if write {
            self.scoped_paths(principal, authority, &candidate, true)
                .await?;
        }
        Ok(candidate)
    }

    async fn scoped_restricted(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        action: V3Action,
    ) -> V3StoreResult<bool> {
        let decision = self.decide(principal, authority, action).await?;
        let restricted = decision.content_restricted;
        require_allowed(decision)?;
        Ok(restricted)
    }

    async fn scoped_fingerprint(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        action: V3Action,
    ) -> V3StoreResult<Option<String>> {
        self.repository
            .authorizer()
            .read_visibility_fingerprint(&AuthorizationCheck {
                principal: principal.clone(),
                action,
                target: AuthorizationTarget::Space(authority.space_id.clone()),
                policy_set_id: authority.policy_set_id.clone(),
                authorization_epoch: authority.authorization_epoch,
            })
            .await
    }

    pub async fn propose_scoped_review(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        request: ScopedReviewProposalRequest,
    ) -> V3StoreResult<ScopedReview> {
        types::validate_key(&request.idempotency_key)?;
        if request.title.trim().is_empty() || request.title.len() > 1024 {
            return Err(types::invalid("invalid_scoped_review_title"));
        }
        let authority = self
            .scoped_authority(principal, space, V3Action::ScopedReviewWrite)
            .await?;
        let candidate = self
            .scoped_candidate(principal, &authority, &request.candidate_id, true)
            .await?;
        if candidate.expected_view_id != authority.repository_view_id
            || candidate.expected_policy_set_id != authority.policy_set_id
            || candidate.expected_authorization_epoch != authority.authorization_epoch
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let review_id = key_id("scoped-review", principal, space, &request.idempotency_key);
        let request_digest = digest(&request)?;
        let restricted = self
            .scoped_restricted(principal, &authority, V3Action::ScopedReviewWrite)
            .await?;
        let fingerprint = self
            .scoped_fingerprint(principal, &authority, V3Action::ScopedReviewWrite)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        if let Some(row) = sqlx::query("SELECT candidate_id,request_digest FROM layrs_collaboration.scoped_reviews WHERE review_id=$1 AND space_id=$2")
            .bind(&review_id).bind(space.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("candidate_id") != request.candidate_id
                || row.get::<String,_>("request_digest") != request_digest
            { return Err(types::invalid("idempotency_digest_mismatch")); }
        } else {
            sqlx::query("INSERT INTO layrs_collaboration.scoped_reviews(review_id,space_id,candidate_id,author_principal_id,request_digest,title,author_content_restricted,author_visibility_fingerprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
                .bind(&review_id).bind(space.as_str()).bind(&request.candidate_id)
                .bind(principal.principal_id.as_str()).bind(&request_digest).bind(&request.title)
                .bind(restricted).bind(&fingerprint).execute(&mut *tx).await.map_err(pg_error)?;
        }
        let row = sqlx::query("SELECT title,lifecycle,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,author_content_restricted,author_visibility_fingerprint FROM layrs_collaboration.scoped_reviews WHERE review_id=$1")
            .bind(&review_id).fetch_one(&mut *tx).await.map_err(pg_error)?;
        let review = scoped_review_from(
            &review_id,
            &candidate,
            principal.principal_id.clone(),
            &row,
            restricted,
            &principal.principal_id,
            fingerprint.as_deref(),
        )?;
        tx.commit().await.map_err(pg_error)?;
        Ok(review)
    }

    pub async fn act_on_scoped_review(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review_id: &str,
        request: ScopedReviewActionRequest,
    ) -> V3StoreResult<ScopedReviewAction> {
        types::validate_key(&request.idempotency_key)?;
        if request.body.len() > 64 * 1024
            || request.reply_to.as_ref().is_some_and(|id| id.len() > 100)
        {
            return Err(types::invalid("invalid_scoped_review_action"));
        }
        if request.action == ScopedReviewActionKind::Comment && request.body.trim().is_empty() {
            return Err(types::invalid("empty_scoped_review_comment"));
        }
        let grant = if matches!(
            request.action,
            ScopedReviewActionKind::Approve | ScopedReviewActionKind::RequestChanges
        ) {
            V3Action::ScopedReviewApprove
        } else {
            V3Action::ScopedReviewWrite
        };
        let authority = self.scoped_authority(principal, space, grant).await?;
        let mut tx = self.locked(principal, &authority).await?;
        let review = sqlx::query("SELECT candidate_id,author_principal_id,lifecycle FROM layrs_collaboration.scoped_reviews WHERE space_id=$1 AND review_id=$2")
            .bind(space.as_str()).bind(review_id).fetch_optional(&mut *tx).await.map_err(pg_error)?
            .ok_or(V3StoreError::NotFound)?;
        let candidate_id: String = review.get("candidate_id");
        let candidate = self
            .scoped_candidate(principal, &authority, &candidate_id, false)
            .await?;
        if candidate.expected_view_id != authority.repository_view_id
            || candidate.expected_policy_set_id != authority.policy_set_id
            || candidate.expected_authorization_epoch != authority.authorization_epoch
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let action_id = key_id(
            "scoped-review-action",
            principal,
            space,
            &request.idempotency_key,
        );
        let request_digest = digest(&(review_id, &request))?;
        let existing = sqlx::query("SELECT action_id,review_id,actor_principal_id,action_kind,body,anchor_path,reply_to,request_digest,writer_content_restricted,writer_read_paths_jcs,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at FROM layrs_collaboration.scoped_review_actions WHERE action_id=$1")
            .bind(&action_id).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("request_digest") != request_digest
                || row.get::<String, _>("review_id") != review_id
            {
                return Err(types::invalid("idempotency_digest_mismatch"));
            }
            return action_from(&row, true);
        }
        let lifecycle: String = review.get("lifecycle");
        if lifecycle == "integrated"
            || lifecycle != "open" && request.action != ScopedReviewActionKind::Reopen
            || lifecycle != "closed" && request.action == ScopedReviewActionKind::Reopen
        {
            return Err(types::invalid("scoped_review_not_open"));
        }
        if matches!(
            request.action,
            ScopedReviewActionKind::Close | ScopedReviewActionKind::Reopen
        ) && review.get::<String, _>("author_principal_id") != principal.principal_id.as_str()
        {
            return Err(V3StoreError::InvalidCapability);
        }
        if matches!(
            request.action,
            ScopedReviewActionKind::Approve | ScopedReviewActionKind::RequestChanges
        ) {
            let human = super::super::space_access::human_principal(&mut *tx, principal).await?;
            if !human {
                return Err(V3StoreError::InvalidCapability);
            }
        }
        if let Some(anchor) = &request.anchor_path {
            if !candidate.changed_paths.contains(anchor) {
                return Err(types::invalid("anchor_outside_candidate"));
            }
        }
        if let Some(reply) = &request.reply_to {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.scoped_review_actions WHERE action_id=$1 AND review_id=$2)")
                .bind(reply).bind(review_id).fetch_one(&mut *tx).await.map_err(pg_error)?;
            if !exists {
                return Err(types::invalid("reply_outside_review"));
            }
        }
        let restricted = self.scoped_restricted(principal, &authority, grant).await?;
        let fingerprint = self
            .scoped_fingerprint(principal, &authority, grant)
            .await?;
        sqlx::query("INSERT INTO layrs_collaboration.scoped_review_actions(action_id,review_id,actor_principal_id,action_kind,body,anchor_path,reply_to,request_digest,writer_content_restricted,writer_visibility_fingerprint,writer_read_paths_jcs,writer_policy_set_id,writer_authorization_epoch) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
            .bind(&action_id).bind(review_id).bind(principal.principal_id.as_str())
            .bind(request.action.as_str()).bind(&request.body)
            .bind(request.anchor_path.as_ref().map(RepoPath::as_str)).bind(&request.reply_to)
            .bind(&request_digest).bind(restricted).bind(&fingerprint).bind(jcs(&candidate.changed_paths)?)
            .bind(authority.policy_set_id.as_str()).bind(authority.authorization_epoch as i64)
            .execute(&mut *tx).await.map_err(pg_error)?;
        if matches!(
            request.action,
            ScopedReviewActionKind::Close | ScopedReviewActionKind::Reopen
        ) {
            sqlx::query(
                "UPDATE layrs_collaboration.scoped_reviews SET lifecycle=$1 WHERE review_id=$2",
            )
            .bind(if request.action == ScopedReviewActionKind::Close {
                "closed"
            } else {
                "open"
            })
            .bind(review_id)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        }
        let row = sqlx::query("SELECT action_id,review_id,actor_principal_id,action_kind,body,anchor_path,reply_to,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at FROM layrs_collaboration.scoped_review_actions WHERE action_id=$1")
            .bind(&action_id).fetch_one(&mut *tx).await.map_err(pg_error)?;
        let action = action_from(&row, true)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(action)
    }

    pub async fn list_scoped_reviews(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        query: ScopedReviewListQuery,
    ) -> V3StoreResult<ScopedReviewPage> {
        let limit = query.limit.unwrap_or(50);
        if !(1..=100).contains(&limit) {
            return Err(types::invalid("invalid_scoped_review_limit"));
        }
        let authority = self
            .scoped_authority(principal, space, V3Action::ScopedReviewRead)
            .await?;
        let cursor = query
            .cursor
            .map(|token| {
                if token.len() > 2048 {
                    return Err(types::invalid("invalid_scoped_review_cursor"));
                }
                let bytes = URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|_| types::invalid("invalid_scoped_review_cursor"))?;
                let value: Cursor = serde_json::from_slice(&bytes).map_err(bad)?;
                if value.epoch != authority.authorization_epoch
                    || value.principal != principal.principal_id.as_str()
                {
                    return Err(V3StoreError::AuthorizationEpochChanged);
                }
                Ok(value)
            })
            .transpose()?;
        let mut tx = self.locked(principal, &authority).await?;
        let anchor = if let Some(cursor) = &cursor {
            cursor.anchor.clone()
        } else {
            sqlx::query_scalar::<_,String>("SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')")
                .fetch_one(&mut *tx).await.map_err(pg_error)?
        };
        let rows = sqlx::query("SELECT r.review_id,r.candidate_id,r.author_principal_id,r.title,r.lifecycle,to_char(r.created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,r.author_content_restricted,r.author_visibility_fingerprint FROM layrs_collaboration.scoped_reviews r WHERE r.space_id=$1 AND r.created_at <= $2::timestamptz ORDER BY r.created_at,r.review_id LIMIT 10001")
            .bind(space.as_str()).bind(&anchor).fetch_all(&mut *tx).await.map_err(pg_error)?;
        if rows.len() > 10000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "scoped_reviews",
                limit: 10000,
            });
        }
        let restricted = self
            .scoped_restricted(principal, &authority, V3Action::ScopedReviewRead)
            .await?;
        let fingerprint = self
            .scoped_fingerprint(principal, &authority, V3Action::ScopedReviewRead)
            .await?;
        let mut visible = Vec::new();
        for row in rows {
            let id: String = row.get("candidate_id");
            let Some(candidate) = load_candidate(&self.repository, space, &id).await? else {
                continue;
            };
            if self
                .scoped_paths(principal, &authority, &candidate, false)
                .await
                .is_err()
            {
                continue;
            }
            let author = parse(row.get("author_principal_id"))?;
            visible.push(scoped_review_from(
                &row.get::<String, _>("review_id"),
                &candidate,
                author,
                &row,
                restricted,
                &principal.principal_id,
                fingerprint.as_deref(),
            )?);
        }
        let start = cursor.as_ref().map_or(0, |c| c.offset);
        if start > visible.len() {
            return Err(types::invalid("invalid_scoped_review_cursor"));
        }
        let next = (start + usize::from(limit) < visible.len()).then(|| {
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&Cursor {
                    epoch: authority.authorization_epoch,
                    principal: principal.principal_id.as_str().into(),
                    anchor,
                    offset: start + usize::from(limit),
                })
                .expect("cursor serializable"),
            )
        });
        let reviews = visible
            .into_iter()
            .skip(start)
            .take(usize::from(limit))
            .collect();
        Ok(ScopedReviewPage {
            reviews,
            next_cursor: next,
        })
    }

    pub async fn scoped_review_detail(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review_id: &str,
    ) -> V3StoreResult<ScopedReviewDetail> {
        let authority = self
            .scoped_authority(principal, space, V3Action::ScopedReviewRead)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let row = sqlx::query("SELECT review_id,candidate_id,author_principal_id,title,lifecycle,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,author_content_restricted,author_visibility_fingerprint FROM layrs_collaboration.scoped_reviews WHERE space_id=$1 AND review_id=$2")
            .bind(space.as_str()).bind(review_id).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let candidate = self
            .scoped_candidate(
                principal,
                &authority,
                &row.get::<String, _>("candidate_id"),
                false,
            )
            .await?;
        let restricted = self
            .scoped_restricted(principal, &authority, V3Action::ScopedReviewRead)
            .await?;
        let fingerprint = self
            .scoped_fingerprint(principal, &authority, V3Action::ScopedReviewRead)
            .await?;
        let review = scoped_review_from(
            review_id,
            &candidate,
            parse(row.get("author_principal_id"))?,
            &row,
            restricted,
            &principal.principal_id,
            fingerprint.as_deref(),
        )?;
        let rows = sqlx::query("SELECT action_id,review_id,actor_principal_id,action_kind,body,anchor_path,reply_to,writer_content_restricted,writer_visibility_fingerprint,writer_read_paths_jcs,to_char(created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at FROM layrs_collaboration.scoped_review_actions WHERE review_id=$1 ORDER BY sequence LIMIT 10001")
            .bind(review_id).fetch_all(&mut *tx).await.map_err(pg_error)?;
        if rows.len() > 10000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "scoped_review_actions",
                limit: 10000,
            });
        }
        let mut actions = Vec::with_capacity(rows.len());
        for row in rows {
            let writer_paths: Vec<RepoPath> =
                serde_json::from_str(&row.get::<String, _>("writer_read_paths_jcs"))
                    .map_err(bad)?;
            let writer: String = row.get("actor_principal_id");
            let share_body = writer_paths == candidate.changed_paths
                && scoped_text_visible(
                    restricted,
                    row.get("writer_content_restricted"),
                    writer == principal.principal_id.as_str(),
                    fingerprint.as_deref(),
                    row.get::<Option<String>, _>("writer_visibility_fingerprint")
                        .as_deref(),
                );
            let action = action_from(&row, share_body)?;
            actions.push(action);
        }
        let integration = scoped_status(self, &mut tx, &authority, &candidate, review_id).await?;
        Ok(ScopedReviewDetail {
            decision_state: integration.state.clone(),
            review,
            actions,
            integration,
        })
    }

    pub async fn scoped_integration_status(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review_id: &str,
    ) -> V3StoreResult<ScopedIntegrationStatus> {
        let authority = self
            .scoped_authority(principal, space, V3Action::ScopedReviewRead)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let candidate_id: String = sqlx::query_scalar("SELECT candidate_id FROM layrs_collaboration.scoped_reviews WHERE space_id=$1 AND review_id=$2")
            .bind(space.as_str()).bind(review_id).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let candidate = self
            .scoped_candidate(principal, &authority, &candidate_id, false)
            .await?;
        scoped_status(self, &mut tx, &authority, &candidate, review_id).await
    }

    /// Resolves the server-owned integration target. The caller supplies only
    /// a review ID; all transaction roots come from immutable candidate data.
    pub async fn load_scoped_integration_target(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review_id: &str,
    ) -> V3StoreResult<ScopedIntegrationTarget> {
        let authority = self
            .scoped_authority(principal, space, V3Action::ScopedReviewWrite)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let row = sqlx::query("SELECT candidate_id,lifecycle FROM layrs_collaboration.scoped_reviews WHERE space_id=$1 AND review_id=$2")
            .bind(space.as_str()).bind(review_id).fetch_optional(&mut *tx).await.map_err(pg_error)?
            .ok_or(V3StoreError::NotFound)?;
        if row.get::<String, _>("lifecycle") != "open" {
            return Err(types::invalid("scoped_review_not_open"));
        }
        let candidate = self
            .scoped_candidate(
                principal,
                &authority,
                &row.get::<String, _>("candidate_id"),
                true,
            )
            .await?;
        if candidate.expected_view_id != authority.repository_view_id
            || candidate.expected_policy_set_id != authority.policy_set_id
            || candidate.expected_authorization_epoch != authority.authorization_epoch
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let status = scoped_status(self, &mut tx, &authority, &candidate, review_id).await?;
        if status.state != "approved"
            || !matches!(status.proof_status.as_str(), "passed" | "not_required")
        {
            return Err(types::invalid("scoped_review_gate_pending"));
        }
        Ok(ScopedIntegrationTarget {
            candidate_id: candidate.candidate_id,
            candidate_step_id: candidate.candidate_step_id,
            layer_id: candidate.layer_id,
            expected_view_id: candidate.expected_view_id,
            expected_generation: authority.repository_view_generation,
            expected_policy_set_id: candidate.expected_policy_set_id,
            expected_epoch: candidate.expected_authorization_epoch,
        })
    }

    /// Called by the trusted candidate integration path under its Space lock.
    /// The candidate ID must come from that path's verified request binding.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn enforce_scoped_candidate_gates(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        authority: &SpaceAuthority,
        review_id: &str,
        candidate_id: &str,
        step: &layrs_core::scm::StepId,
        layer: &layrs_core::scm::LayerId,
        locked_reviewers: &BTreeSet<String>,
    ) -> V3StoreResult<Option<Rejection>> {
        let reject = |reason: &str| {
            Some(super::super::transaction_plan::reject(
                RejectionCode::GateRejected,
                reason,
            ))
        };
        let Some(candidate) =
            load_candidate(&self.repository, &authority.space_id, candidate_id).await?
        else {
            return Ok(reject("scoped candidate provenance missing"));
        };
        if &candidate.candidate_step_id != step
            || &candidate.layer_id != layer
            || candidate.expected_view_id != authority.repository_view_id
            || candidate.expected_policy_set_id != authority.policy_set_id
            || candidate.expected_authorization_epoch != authority.authorization_epoch
        {
            return Ok(reject("scoped candidate root, view or policy changed"));
        }
        let row = sqlx::query("SELECT review_id,lifecycle FROM layrs_collaboration.scoped_reviews WHERE space_id=$1 AND candidate_id=$2 AND review_id=$3")
            .bind(authority.space_id.as_str()).bind(candidate_id).bind(review_id)
            .fetch_optional(&mut **tx).await.map_err(pg_error)?;
        let Some(row) = row else {
            return Ok(reject("scoped candidate requires review"));
        };
        if row.get::<String, _>("lifecycle") != "open" {
            return Ok(reject("scoped review is not open"));
        }
        let status = scoped_status_with_locked(
            self,
            tx,
            authority,
            &candidate,
            review_id,
            Some(locked_reviewers),
        )
        .await?;
        if status.state != "approved" {
            return Ok(reject(
                "scoped review has insufficient current approvals or blocking decisions",
            ));
        }
        if status.proof_status != "passed" && status.proof_status != "not_required" {
            return Ok(reject("missing current passing Proof for scoped candidate"));
        }
        Ok(None)
    }

    /// Record the exact transaction binding only after a successful candidate
    /// view mutation and receipt insertion in the same transaction.
    pub(crate) async fn record_scoped_integration(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        review_id: &str,
        candidate_id: &str,
        receipt: &layrs_sync::v3::SyncTransactionReceipt,
    ) -> V3StoreResult<()> {
        let candidate = load_candidate(&self.repository, &authority.space_id, candidate_id)
            .await?
            .ok_or(V3StoreError::NotFound)?;
        let Some(view) = receipt.view_result.as_ref() else {
            return Err(V3StoreError::InvalidCapability);
        };
        if receipt.status != layrs_sync::v3::ReceiptStatus::Applied
            || view.new_generation == view.old_generation
            || view.old_repository_view_id != candidate.expected_view_id
            || receipt.current_policy_set_id != candidate.expected_policy_set_id
            || receipt.authorization_epoch != candidate.expected_authorization_epoch
            || !view.layer_results.iter().any(|result| {
                result.layer_id == candidate.layer_id
                    && result.resulting_heads.as_slice()
                        == std::slice::from_ref(&candidate.candidate_step_id)
            })
        {
            return Err(V3StoreError::InvalidCapability);
        }
        let found: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.scoped_reviews WHERE space_id=$1 AND candidate_id=$2 AND review_id=$3 AND lifecycle='open')")
            .bind(authority.space_id.as_str()).bind(candidate_id).bind(review_id)
            .fetch_one(&mut **tx).await.map_err(pg_error)?;
        if !found {
            return Err(V3StoreError::InvalidCapability);
        }
        sqlx::query("INSERT INTO layrs_collaboration.scoped_review_integrations(review_id,space_id,candidate_id,transaction_id,integrated_by) VALUES($1,$2,$3,$4,$5)")
            .bind(review_id).bind(authority.space_id.as_str()).bind(candidate_id)
            .bind(receipt.transaction_id.as_str()).bind(principal.principal_id.as_str())
            .execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query("UPDATE layrs_collaboration.scoped_reviews SET lifecycle='integrated' WHERE review_id=$1 AND lifecycle='open'")
            .bind(review_id).execute(&mut **tx).await.map_err(pg_error)?;
        Ok(())
    }
}

fn scoped_review_from(
    review_id: &str,
    candidate: &ScopedPatchCandidate,
    author: layrs_sync::v3::PrincipalId,
    row: &sqlx::postgres::PgRow,
    reader_restricted: bool,
    reader: &layrs_sync::v3::PrincipalId,
    reader_fingerprint: Option<&str>,
) -> V3StoreResult<ScopedReview> {
    let title = scoped_text_visible(
        reader_restricted,
        row.get("author_content_restricted"),
        author == *reader,
        reader_fingerprint,
        row.get::<Option<String>, _>("author_visibility_fingerprint")
            .as_deref(),
    )
    .then(|| row.get::<String, _>("title"));
    Ok(ScopedReview {
        review_id: review_id.into(),
        candidate_id: candidate.candidate_id.clone(),
        space_id: candidate.space_id.clone(),
        layer_id: candidate.layer_id.clone(),
        author_principal_id: author,
        candidate_step_id: candidate.candidate_step_id.clone(),
        candidate_root_tree_id: candidate.candidate_root_tree_id.clone(),
        base_repository_view_id: candidate.expected_view_id.clone(),
        policy_set_id: candidate.expected_policy_set_id.clone(),
        authorization_epoch: candidate.expected_authorization_epoch,
        changed_paths: candidate.changed_paths.clone(),
        title,
        lifecycle: row.get("lifecycle"),
        created_at: row.get("created_at"),
    })
}

fn same_fingerprint(reader: Option<&str>, writer: Option<&str>) -> bool {
    matches!((reader, writer), (Some(a), Some(b)) if a == b)
}

fn scoped_text_visible(
    reader_restricted: bool,
    writer_restricted: bool,
    same_writer: bool,
    reader_fingerprint: Option<&str>,
    writer_fingerprint: Option<&str>,
) -> bool {
    !reader_restricted
        || writer_restricted
            && (same_writer || same_fingerprint(reader_fingerprint, writer_fingerprint))
}

fn action_from(row: &sqlx::postgres::PgRow, share_body: bool) -> V3StoreResult<ScopedReviewAction> {
    let action: ScopedReviewActionKind =
        serde_json::from_value(serde_json::Value::String(row.get("action_kind"))).map_err(bad)?;
    Ok(ScopedReviewAction {
        action_id: row.get("action_id"),
        review_id: row.get("review_id"),
        actor_principal_id: parse(row.get("actor_principal_id"))?,
        action,
        body: share_body.then(|| row.get("body")),
        body_redacted: !share_body,
        anchor_path: row
            .get::<Option<String>, _>("anchor_path")
            .map(parse)
            .transpose()?,
        reply_to: row.get("reply_to"),
        created_at: row.get("created_at"),
    })
}

async fn scoped_status(
    store: &PgCollaborationStore,
    tx: &mut Transaction<'_, Postgres>,
    authority: &SpaceAuthority,
    candidate: &ScopedPatchCandidate,
    review_id: &str,
) -> V3StoreResult<ScopedIntegrationStatus> {
    scoped_status_with_locked(store, tx, authority, candidate, review_id, None).await
}

async fn scoped_status_with_locked(
    store: &PgCollaborationStore,
    tx: &mut Transaction<'_, Postgres>,
    authority: &SpaceAuthority,
    candidate: &ScopedPatchCandidate,
    review_id: &str,
    locked_reviewers: Option<&BTreeSet<String>>,
) -> V3StoreResult<ScopedIntegrationStatus> {
    let raw: Option<String> = sqlx::query_scalar("SELECT canonical_document_jcs FROM layrs_collaboration.layer_protections WHERE space_id=$1 AND layer_id=$2")
        .bind(authority.space_id.as_str()).bind(candidate.layer_id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
    let protection: Option<LayerProtection> = raw
        .map(|value| serde_json::from_str(&value).map_err(bad))
        .transpose()?;
    let required = protection
        .as_ref()
        .map_or(1, |p| p.minimum_approvals.max(1));
    let allow_self = protection.as_ref().is_some_and(|p| p.allow_self_approval);
    let review_author: String = sqlx::query_scalar("SELECT author_principal_id FROM layrs_collaboration.scoped_reviews WHERE review_id=$1 AND space_id=$2")
        .bind(review_id).bind(authority.space_id.as_str()).fetch_one(&mut **tx).await.map_err(pg_error)?;
    let decisions = sqlx::query("SELECT DISTINCT ON (a.actor_principal_id) a.actor_principal_id,a.action_kind,p.actor_id,p.deployment_id,p.subject_digest FROM layrs_collaboration.scoped_review_actions a JOIN principals p ON p.principal_id=a.actor_principal_id JOIN spaces s ON s.space_id=$2 JOIN workspace_memberships m ON m.workspace_id=s.workspace_id AND m.principal_id=p.principal_id WHERE a.review_id=$1 AND a.action_kind IN ('approve','request_changes') AND p.status='active' AND p.principal_kind IN ('account','bootstrap_admin') AND m.state='active' ORDER BY a.actor_principal_id,a.sequence DESC LIMIT 257")
        .bind(review_id).bind(authority.space_id.as_str()).fetch_all(&mut **tx).await.map_err(pg_error)?;
    if decisions.len() > 256 {
        return Err(V3StoreError::LimitExceeded {
            resource: "scoped_review_decisions",
            limit: 256,
        });
    }
    let mut approvals = 0;
    let mut blocking = 0;
    for row in decisions {
        let actor_id: String = row.get("actor_principal_id");
        if locked_reviewers.is_some_and(|reviewers| !reviewers.contains(&actor_id)) {
            continue;
        }
        if !allow_self
            && (actor_id == candidate.author_principal_id.as_str() || actor_id == review_author)
        {
            continue;
        }
        let actor = PrincipalContext::new(
            parse(actor_id)?,
            parse(row.get("actor_id"))?,
            row.get::<String, _>("deployment_id"),
            row.get::<String, _>("subject_digest"),
        )?;
        if store
            .scoped_authority(&actor, &authority.space_id, V3Action::ScopedReviewApprove)
            .await
            .is_err()
            || store
                .scoped_paths(&actor, authority, candidate, false)
                .await
                .is_err()
        {
            continue;
        }
        if row.get::<String, _>("action_kind") == "approve" {
            approvals += 1;
        } else {
            blocking += 1;
        }
    }
    let integrated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.scoped_review_integrations WHERE review_id=$1 AND space_id=$2 AND candidate_id=$3)")
        .bind(review_id).bind(authority.space_id.as_str()).bind(&candidate.candidate_id).fetch_one(&mut **tx).await.map_err(pg_error)?;
    let stale = candidate.expected_view_id != authority.repository_view_id
        || candidate.expected_policy_set_id != authority.policy_set_id
        || candidate.expected_authorization_epoch != authority.authorization_epoch;
    let proof_status = if let Some(protection) = &protection {
        if let Some(recipe) = &protection.recipe_object_id {
            if scoped_proof_passed(
                store,
                tx,
                authority,
                candidate,
                protection,
                recipe,
                locked_reviewers,
            )
            .await?
            {
                "passed"
            } else {
                "pending"
            }
        } else {
            "not_required"
        }
    } else {
        "not_required"
    };
    let state = if integrated {
        "integrated"
    } else if stale {
        "stale"
    } else if blocking > 0 {
        "changes_requested"
    } else if approvals >= required {
        "approved"
    } else {
        "pending"
    };
    Ok(ScopedIntegrationStatus {
        candidate_id: candidate.candidate_id.clone(),
        review_id: review_id.into(),
        state: state.into(),
        approvals,
        required_approvals: required,
        blocking_decisions: blocking,
        proof_status: proof_status.into(),
    })
}

async fn scoped_proof_passed(
    store: &PgCollaborationStore,
    tx: &mut Transaction<'_, Postgres>,
    authority: &SpaceAuthority,
    candidate: &ScopedPatchCandidate,
    protection: &LayerProtection,
    recipe: &ObjectId,
    locked_reviewers: Option<&BTreeSet<String>>,
) -> V3StoreResult<bool> {
    let runs = sqlx::query("SELECT DISTINCT ON (r.runner_principal_id) r.run_id,r.proof_object_id,r.completion_digest,r.runner_principal_id,r.outcome,r.completed_at>clock_timestamp()-($6::bigint*interval '1 second') AS fresh FROM layrs_collaboration.runs r JOIN principals p ON p.principal_id=r.runner_principal_id JOIN spaces s ON s.space_id=r.space_id JOIN workspace_memberships m ON m.workspace_id=s.workspace_id AND m.principal_id=p.principal_id WHERE r.space_id=$1 AND r.step_id=$2 AND r.recipe_object_id=$3 AND r.policy_set_id=$4 AND r.authorization_epoch=$5 AND r.completed_at IS NOT NULL AND p.status='active' AND p.principal_kind='service' AND m.state='active' ORDER BY r.runner_principal_id,r.completed_at DESC,r.run_id DESC LIMIT 257")
        .bind(authority.space_id.as_str()).bind(candidate.candidate_step_id.as_str())
        .bind(recipe.as_str()).bind(authority.policy_set_id.as_str())
        .bind(authority.authorization_epoch as i64).bind(protection.maximum_proof_age_seconds as i64)
        .fetch_all(&mut **tx).await.map_err(pg_error)?;
    if runs.len() > 256 {
        return Err(V3StoreError::LimitExceeded {
            resource: "scoped_proof_producers",
            limit: 256,
        });
    }
    for run in runs {
        let runner: String = run.get("runner_principal_id");
        if locked_reviewers.is_some_and(|reviewers| !reviewers.contains(&runner))
            || !protection
                .trusted_runner_principal_ids
                .iter()
                .any(|id| id.as_str() == runner)
            || run.get::<String, _>("outcome") != "passed"
            || !run.get::<bool, _>("fresh")
        {
            continue;
        }
        if super::proof_integrity::verify(
            tx,
            store.repository.cas().as_ref(),
            authority,
            &candidate.candidate_step_id,
            recipe,
            &run,
        )
        .await
        .is_ok()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_visibility_requires_captured_equivalent_read_scope() {
        assert!(scoped_text_visible(
            true,
            true,
            false,
            Some("same"),
            Some("same")
        ));
        assert!(!scoped_text_visible(
            true,
            true,
            false,
            Some("narrow"),
            Some("broad")
        ));
        assert!(!scoped_text_visible(true, true, false, None, None));
        assert!(scoped_text_visible(true, true, true, None, None));
        assert!(!scoped_text_visible(
            true,
            false,
            true,
            Some("same"),
            Some("same")
        ));
        assert!(scoped_text_visible(false, false, false, None, None));
    }
}
