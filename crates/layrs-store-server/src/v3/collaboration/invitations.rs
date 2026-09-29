//! Invitations convey intent only. Acceptance installs the existing explicit
//! Principal policy, atomically with a one-way invitation decision.
use super::*;
use layrs_sync::v3::PrincipalId;
use serde::Deserialize;
use serde_json::{Value, json};
use types::{invalid, validate_key};
mod actions;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InviteMemberRequest {
    pub idempotency_key: String,
    pub recipient_principal_id: PrincipalId,
    pub role: InvitationRole,
    pub expires_in_seconds: u32,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationRole {
    Member,
    Auditor,
}
impl InvitationRole {
    fn member(self) -> MemberRole {
        match self {
            Self::Member => MemberRole::Member,
            Self::Auditor => MemberRole::Auditor,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Auditor => "auditor",
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvitationListQuery {
    pub after: Option<u64>,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvitationAction {
    Accept,
    Reject,
    Revoke,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvitationActionRequest {
    pub action: InvitationAction,
}

const COLUMNS: &str = "i.*,i.created_at::text AS created_text,i.expires_at::text AS expires_text,i.decided_at::text AS decided_text,i.expires_at<=clock_timestamp() AS expired,s.authorization_epoch<>i.authorization_epoch AS stale";
fn document(row: &sqlx::postgres::PgRow) -> Value {
    json!({"invitationId":row.get::<String,_>("invitation_id"),"sequence":row.get::<i64,_>("sequence"),"spaceId":row.get::<String,_>("space_id"),"inviterPrincipalId":row.get::<String,_>("inviter_principal_id"),"recipientPrincipalId":row.get::<String,_>("recipient_principal_id"),"role":row.get::<String,_>("member_role"),"state":row.get::<String,_>("state"),"createdAt":row.get::<String,_>("created_text"),"expiresAt":row.get::<String,_>("expires_text"),"decidedAt":row.get::<Option<String>,_>("decided_text"),"expired":row.get::<bool,_>("expired"),"stale":row.get::<bool,_>("stale"),"evidenceObjectId":row.get::<String,_>("proposal_object_id"),"decisionObjectId":row.get::<Option<String>,_>("decision_object_id")})
}
fn valid_id(id: &str) -> V3StoreResult<()> {
    if id.len() != 75
        || !id.starts_with("invitation:")
        || !id[11..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("invalid_invitation_id"));
    }
    Ok(())
}
impl PgCollaborationStore {
    async fn lock_invitation_principal(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
    ) -> V3StoreResult<()> {
        let active:Option<String>=sqlx::query_scalar("SELECT principal_id FROM principals WHERE principal_id=$1 AND deployment_id=$2 AND status='active' AND subject_digest=$3 AND principal_kind IN ('account','bootstrap_admin') FOR SHARE").bind(principal.principal_id.as_str()).bind(&principal.deployment_id).bind(&principal.subject_digest).fetch_optional(&mut **tx).await.map_err(pg_error)?;
        active.ok_or(V3StoreError::InvalidCapability)?;
        Ok(())
    }
    async fn invitation_row(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        locked: bool,
    ) -> V3StoreResult<sqlx::postgres::PgRow> {
        sqlx::query(&format!("SELECT {COLUMNS} FROM layrs_collaboration.space_invitations i JOIN spaces s USING(space_id) WHERE invitation_id=$1 {}",if locked {"FOR UPDATE OF i"} else {""})).bind(id).fetch_optional(&mut **tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)
    }
    pub async fn list_invitations(
        &self,
        principal: &PrincipalContext,
        space: Option<&SpaceId>,
        query: InvitationListQuery,
    ) -> V3StoreResult<Value> {
        let after = i64::try_from(query.after.unwrap_or(0)).map_err(bad)?;
        let mut tx = if let Some(space) = space {
            let auth = self
                .authority(principal, space, V3Action::AdministerPolicy)
                .await?;
            self.locked(principal, &auth).await?
        } else {
            self.repository.pool().begin().await.map_err(pg_error)?
        };
        self.lock_invitation_principal(&mut tx, principal).await?;
        let rows=sqlx::query(&format!("SELECT {COLUMNS} FROM layrs_collaboration.space_invitations i JOIN spaces s USING(space_id) WHERE (($1::text IS NULL AND i.recipient_principal_id=$2) OR i.space_id=$1) AND i.sequence>$3 ORDER BY i.sequence LIMIT 101")).bind(space.map(SpaceId::as_str)).bind(principal.principal_id.as_str()).bind(after).fetch_all(&mut *tx).await.map_err(pg_error)?;
        let more = rows.len() > 100;
        let visible = &rows[..rows.len().min(100)];
        Ok(
            json!({"invitations":visible.iter().map(document).collect::<Vec<_>>(),"nextAfter":if more {visible.last().map(|r|r.get::<i64,_>("sequence"))} else {None}}),
        )
    }
    pub async fn invite_member(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        request: InviteMemberRequest,
    ) -> V3StoreResult<Value> {
        validate_key(&request.idempotency_key)?;
        if !(60..=604800).contains(&request.expires_in_seconds)
            || request.recipient_principal_id == principal.principal_id
        {
            return Err(invalid("invalid_invitation"));
        }
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let id = key_id("invitation", principal, space, &request.idempotency_key);
        let request_digest = digest(&request)?;
        let created = OffsetDateTime::now_utc();
        let expires = (created + time::Duration::seconds(i64::from(request.expires_in_seconds)))
            .format(&Rfc3339)
            .map_err(bad)?;
        let evidence=self.evidence(&json!({"schemaVersion":"layrs.space-invitation.v1","invitationId":id,"spaceId":space,"inviterPrincipalId":principal.principal_id,"recipientPrincipalId":request.recipient_principal_id,"role":request.role,"expiresAt":expires,"policySetId":authority.policy_set_id,"authorizationEpoch":authority.authorization_epoch})).await?;
        let mut tx = self.locked(principal, &authority).await?;
        self.lock_invitation_principal(&mut tx, principal).await?;
        let administrator:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspace_memberships m JOIN spaces s USING(workspace_id) WHERE s.space_id=$1 AND m.principal_id=$2 AND m.state='active' AND m.role IN ('owner','administrator'))").bind(space.as_str()).bind(principal.principal_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if !administrator {
            return Err(V3StoreError::InvalidCapability);
        }
        let old:Option<String>=sqlx::query_scalar("SELECT request_digest FROM layrs_collaboration.space_invitations WHERE invitation_id=$1").bind(&id).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        if let Some(old) = old {
            if old != request_digest {
                return Err(invalid("invitation_idempotency_mismatch"));
            }
            return Ok(document(&self.invitation_row(&mut tx, &id, false).await?));
        }
        super::super::policy_management::require_managed(
            &self.repository,
            &mut tx,
            space,
            &authority.policy_set_id,
        )
        .await?;
        let recipient:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM principals WHERE principal_id=$1 AND deployment_id=$2 AND status='active' AND principal_kind IN ('account','bootstrap_admin'))").bind(request.recipient_principal_id.as_str()).bind(&principal.deployment_id).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if !recipient {
            return Err(V3StoreError::NotFound);
        }
        let pending:i64=sqlx::query_scalar("SELECT count(*) FROM layrs_collaboration.space_invitations WHERE space_id=$1 AND state='pending' AND expires_at>clock_timestamp()").bind(space.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if pending >= 256 {
            return Err(invalid("pending_invitation_limit"));
        }
        self.install(&mut tx, space, &evidence).await?;
        sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,0,'evidence',$2)")
            .bind(evidence.descriptor.object_id.as_str()).bind(authority.policy_set_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO layrs_collaboration.space_invitations(invitation_id,space_id,inviter_principal_id,recipient_principal_id,member_role,request_digest,policy_set_id,authorization_epoch,expires_at,proposal_object_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9::text::timestamptz,$10)").bind(&id).bind(space.as_str()).bind(principal.principal_id.as_str()).bind(request.recipient_principal_id.as_str()).bind(request.role.name()).bind(request_digest).bind(authority.policy_set_id.as_str()).bind(authority.authorization_epoch as i64).bind(expires).bind(evidence.descriptor.object_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        self.notify(
            &mut tx,
            principal,
            space,
            "invitation.created",
            &id,
            &evidence.descriptor.object_id,
        )
        .await?;
        let result = document(&self.invitation_row(&mut tx, &id, false).await?);
        tx.commit().await.map_err(pg_error)?;
        Ok(result)
    }
}
