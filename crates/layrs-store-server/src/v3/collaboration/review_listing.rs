use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewListQuery {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewCursor {
    schema: String,
    space: SpaceId,
    epoch: u64,
    anchor: i64,
    before: i64,
}
impl PgCollaborationStore {
    /// Cursor controls ranges only, never authorization. The first immutable
    /// proposal notification pins membership/order; latest versions stay live.
    pub async fn list_reviews_page(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        query: ReviewListQuery,
    ) -> V3StoreResult<serde_json::Value> {
        let limit = query.limit.unwrap_or(100);
        if !(1..=100).contains(&limit) {
            return Err(types::invalid("invalid_review_page_limit"));
        }
        let cursor: Option<ReviewCursor> = query
            .cursor
            .map(|token| {
                if token.len() > 1024 {
                    return Err(types::invalid("invalid_review_cursor"));
                }
                let bytes = URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|_| types::invalid("invalid_review_cursor"))?;
                let cursor: ReviewCursor = serde_json::from_slice(&bytes)
                    .map_err(|_| types::invalid("invalid_review_cursor"))?;
                if cursor.schema != "layrs.review-list-cursor.v1"
                    || cursor.space != *space
                    || cursor.anchor < 0
                    || cursor.before <= 0
                    || cursor.before > cursor.anchor
                {
                    return Err(types::invalid("invalid_review_cursor"));
                }
                Ok(cursor)
            })
            .transpose()?;
        let authority = self
            .authority(principal, space, V3Action::ReadReviews)
            .await?;
        if cursor
            .as_ref()
            .is_some_and(|c| c.epoch != authority.authorization_epoch)
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let mut tx = self.locked(principal, &authority).await?;
        let anchor = match &cursor {
            Some(c)=>c.anchor,
            None=>sqlx::query_scalar("SELECT coalesce(max(sequence),0) FROM layrs_collaboration.notifications WHERE space_id=$1 AND event_kind='review.proposed'").bind(space.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?,
        };
        let before = cursor.as_ref().map(|c| c.before);
        let rows=sqlx::query("SELECT n.sequence,r.lifecycle,v.canonical_document_jcs FROM layrs_collaboration.reviews r JOIN layrs_collaboration.review_versions initial ON initial.review_id=r.review_id AND initial.version=1 JOIN layrs_collaboration.notifications n ON n.space_id=r.space_id AND n.subject_id=r.review_id AND n.event_kind='review.proposed' AND n.evidence_object_id=initial.proposal_object_id JOIN layrs_collaboration.review_versions v ON v.review_id=r.review_id AND v.version=r.latest_version WHERE r.space_id=$1 AND n.sequence<=$2 AND ($3::bigint IS NULL OR n.sequence<$3) ORDER BY n.sequence DESC LIMIT $4")
            .bind(space.as_str()).bind(anchor).bind(before).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(pg_error)?;
        let has_more = rows.len() > usize::from(limit);
        let mut reviews = vec![];
        let mut last = 0;
        let mut bytes = 0usize;
        for row in rows.into_iter().take(usize::from(limit)) {
            let raw: String = row.get("canonical_document_jcs");
            bytes += raw.len();
            if bytes > 4 * 1024 * 1024 {
                return Err(V3StoreError::LimitExceeded {
                    resource: "review_page_bytes",
                    limit: 4 * 1024 * 1024,
                });
            }
            let proposal: ReviewVersion = serde_json::from_str(&raw).map_err(bad)?;
            let decision = self
                .review_decision_state(&mut tx, &authority, &proposal)
                .await?;
            last = row.get("sequence");
            reviews.push(serde_json::json!({"lifecycle":row.get::<String,_>("lifecycle"),"proposal":proposal,"decisionState":decision}));
        }
        let next = if has_more {
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&ReviewCursor {
                        schema: "layrs.review-list-cursor.v1".into(),
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
        Ok(serde_json::json!({"reviews":reviews,"nextCursor":next,"hasMore":has_more}))
    }

    pub(super) async fn review_decision_state(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        authority: &SpaceAuthority,
        proposal: &ReviewVersion,
    ) -> V3StoreResult<serde_json::Value> {
        let raw:Option<String>=sqlx::query_scalar("SELECT canonical_document_jcs FROM layrs_collaboration.layer_protections WHERE space_id=$1 AND layer_id=$2").bind(authority.space_id.as_str()).bind(proposal.layer_id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
        let protection: Option<LayerProtection> = raw
            .map(|s| serde_json::from_str(&s).map_err(bad))
            .transpose()?;
        let required = protection
            .as_ref()
            .map_or(1, |p| p.minimum_approvals.max(1));
        let allow_self = protection.as_ref().is_some_and(|p| p.allow_self_approval);
        let stale = proposal.base_repository_view_id != authority.repository_view_id
            || proposal.policy_set_id != authority.policy_set_id
            || proposal.authorization_epoch != authority.authorization_epoch;
        let decisions=sqlx::query("SELECT DISTINCT ON (a.actor_principal_id) a.actor_principal_id,a.action_kind FROM layrs_collaboration.review_actions a JOIN principals p ON p.principal_id=a.actor_principal_id JOIN spaces s ON s.space_id=$3 JOIN workspace_memberships m ON m.workspace_id=s.workspace_id AND m.principal_id=p.principal_id WHERE a.review_id=$1 AND a.version=$2 AND a.action_kind IN ('approve','request_changes') AND p.status='active' AND p.principal_kind IN ('account','bootstrap_admin') AND m.state='active' ORDER BY a.actor_principal_id,a.sequence DESC LIMIT 257")
            .bind(&proposal.review_id).bind(proposal.version as i32).bind(authority.space_id.as_str()).fetch_all(&mut **tx).await.map_err(pg_error)?;
        if decisions.len() > 256 {
            return Err(V3StoreError::LimitExceeded {
                resource: "review_decision_principals",
                limit: 256,
            });
        }
        let mut approvals = 0;
        let mut blocking = 0;
        for row in decisions {
            let actor: String = row.get("actor_principal_id");
            if !allow_self && actor == proposal.author_principal_id.as_str() {
                continue;
            }
            if row.get::<String, _>("action_kind") == "approve" {
                approvals += 1;
            } else {
                blocking += 1;
            }
        }
        let state = if stale {
            "stale"
        } else if blocking > 0 {
            "changes_requested"
        } else if approvals >= required {
            "approved"
        } else {
            "pending"
        };
        Ok(
            serde_json::json!({"state":state,"approvals":approvals,"blockingDecisions":blocking,"requiredApprovals":required,"stale":stale,"allowsIntegration":false}),
        )
    }
}
