//! Authenticated collaboration over immutable native objects.
pub(crate) mod gates;
mod invitations;
mod members;
mod teams;
pub use invitations::{
    InvitationAction, InvitationActionRequest, InvitationListQuery, InvitationRole,
    InviteMemberRequest,
};
pub use teams::{ArchiveTeamRequest, TeamHistoryQuery, TeamRecord, TeamRequest, TeamRole};
mod comment_corrections;
mod notifications;
pub use comment_corrections::CorrectCommentRequest;
mod proof_integrity;
mod proofs;
mod run_execution;
mod run_listing;
pub use run_listing::RunListQuery;
mod run_cancellation;
pub use run_cancellation::CancelRunRequest;
mod review_integrations;
mod review_listing;
mod reviews;
mod scoped_reviews;
pub use review_listing::ReviewListQuery;
pub use scoped_reviews::ScopedIntegrationTarget;
mod releases;
mod types;
mod webhooks;
use super::*;
use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{ObjectId, SpaceId, Timestamp};
use layrs_sync::v3::{Compression, ObjectDescriptor, ObjectKind};
pub use releases::*;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Postgres, Row, Transaction};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
pub use types::*;

#[derive(Clone)]
pub struct PgCollaborationStore {
    pub(super) repository: PgV3Repository,
}
impl PgCollaborationStore {
    pub fn new(repository: PgV3Repository) -> Self {
        Self { repository }
    }
    pub async fn policy_management(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<PolicyManagementState> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        if !super::transaction_choose::administrator(&mut tx, principal, space).await? {
            return Err(V3StoreError::InvalidCapability);
        }
        super::policy_management::state(&self.repository, &mut tx, space, &authority.policy_set_id)
            .await
    }
    /// Whole-Space collaboration guard: the action must be granted and the
    /// principal must not be content-restricted (see `space_access::full_access`).
    async fn authority(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        action: V3Action,
    ) -> V3StoreResult<SpaceAuthority> {
        let authority = self.current_authority(space).await?;
        let decision = self.decide(principal, &authority, action).await?;
        if !super::space_access::full_access(&decision) {
            return Err(V3StoreError::InvalidCapability);
        }
        Ok(authority)
    }
    /// Current authority tuple of a ready Space; no authorization is implied.
    async fn current_authority(&self, space: &SpaceId) -> V3StoreResult<SpaceAuthority> {
        let row=sqlx::query("SELECT current_repository_view_id,current_repository_view_generation,current_policy_set_id,authorization_epoch,next_event_sequence FROM spaces WHERE space_id=$1 AND lifecycle_state='ready'").bind(space.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        Ok(SpaceAuthority {
            space_id: space.clone(),
            repository_view_id: parse(row.get::<String, _>("current_repository_view_id"))?,
            repository_view_generation: row.get::<i64, _>("current_repository_view_generation")
                as u64,
            policy_set_id: parse(row.get::<String, _>("current_policy_set_id"))?,
            authorization_epoch: row.get::<i64, _>("authorization_epoch") as u64,
            watermark_sequence: row.get::<i64, _>("next_event_sequence") as u64 - 1,
        })
    }
    /// The injected policy authorizer's Space-level decision for `action`.
    async fn decide(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        action: V3Action,
    ) -> V3StoreResult<AuthorizationDecision> {
        self.repository
            .authorizer()
            .authorize(&AuthorizationCheck {
                principal: principal.clone(),
                action,
                target: AuthorizationTarget::Space(authority.space_id.clone()),
                policy_set_id: authority.policy_set_id.clone(),
                authorization_epoch: authority.authorization_epoch,
            })
            .await
    }
    async fn locked<'a>(
        &'a self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
    ) -> V3StoreResult<Transaction<'a, Postgres>> {
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        super::transaction_store::lock_identity(&mut tx, principal, &authority.space_id).await?;
        let row=sqlx::query("SELECT current_repository_view_id,current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 AND lifecycle_state='ready' FOR UPDATE").bind(authority.space_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        if row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
            || row.get::<String, _>("current_repository_view_id")
                != authority.repository_view_id.as_str()
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        Ok(tx)
    }
    async fn evidence<T: Serialize>(&self, value: &T) -> V3StoreResult<PreparedCasObject> {
        let bytes = serde_jcs::to_vec(value).map_err(bad)?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-evidence-v1\0");
        hash.update(&bytes);
        self.prepare(
            ObjectKind::Evidence,
            parse(format!("blake3:{}", hash.finalize().to_hex()))?,
            bytes,
        )
        .await
    }
    async fn prepare(
        &self,
        kind: ObjectKind,
        id: ObjectId,
        bytes: Vec<u8>,
    ) -> V3StoreResult<PreparedCasObject> {
        let descriptor = ObjectDescriptor {
            kind,
            object_id: id,
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        };
        self.repository
            .cas()
            .put_if_absent(
                &descriptor,
                Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) })),
            )
            .await?;
        self.repository.cas().verify_existing(&descriptor).await
    }
    async fn install(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        space: &SpaceId,
        object: &PreparedCasObject,
    ) -> V3StoreResult<()> {
        let catalog = PgCatalogStore::new(self.repository.clone());
        catalog.install_prepared(tx, object).await?;
        catalog
            .make_available(tx, space, &object.descriptor.object_id)
            .await?;
        Ok(())
    }
    async fn notify(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        space: &SpaceId,
        kind: &str,
        subject: &str,
        id: &ObjectId,
    ) -> V3StoreResult<()> {
        sqlx::query("INSERT INTO layrs_collaboration.notifications(space_id,actor_principal_id,event_kind,subject_id,evidence_object_id) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING").bind(space.as_str()).bind(principal.principal_id.as_str()).bind(kind).bind(subject).bind(id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        Ok(())
    }
}
fn bad(e: impl std::fmt::Display) -> V3StoreError {
    V3StoreError::InvalidDescriptor(e.to_string())
}
fn parse<T: DeserializeOwned>(value: String) -> V3StoreResult<T> {
    serde_json::from_value(serde_json::Value::String(value)).map_err(bad)
}
fn jcs<T: Serialize>(value: &T) -> V3StoreResult<String> {
    serde_jcs::to_string(value).map_err(bad)
}
fn digest<T: Serialize>(value: &T) -> V3StoreResult<String> {
    Ok(blake3::hash(jcs(value)?.as_bytes()).to_hex().to_string())
}
fn now() -> V3StoreResult<Timestamp> {
    Timestamp::parse(OffsetDateTime::now_utc().format(&Rfc3339).map_err(bad)?).map_err(bad)
}
fn key_id(prefix: &str, principal: &PrincipalContext, space: &SpaceId, key: &str) -> String {
    format!(
        "{prefix}:{}",
        blake3::hash(
            format!(
                "{}\0{}\0{key}",
                principal.principal_id.as_str(),
                space.as_str()
            )
            .as_bytes()
        )
        .to_hex()
    )
}
