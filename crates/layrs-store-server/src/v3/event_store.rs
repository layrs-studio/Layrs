//! Space-scoped append-only event paging.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use layrs_core::scm::{ObjectId, SpaceId};
use layrs_sync::v3::{
    ActorKind, Cursor, EventActor, EventPageResponseType, PrincipalId, SpaceEvent, SpaceEventPage,
    SyncV3Protocol,
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::Row;

use super::{
    AuthorizationCheck, AuthorizationTarget, PgV3Repository, PrincipalContext, V3Action,
    V3StoreError, V3StoreResult, pg_error, require_allowed,
};

type HmacSha256 = Hmac<Sha256>;

pub trait BootstrapCursorIssuer: Clone + Send + Sync + 'static {
    fn issue_bootstrap_cursor(
        &self,
        space_id: &SpaceId,
        principal_id: &PrincipalId,
        authorization_epoch: u64,
        sequence: u64,
    ) -> V3StoreResult<Cursor>;
}

#[derive(Debug, Clone)]
pub struct EventCursorCodec {
    signing_key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventCursorClaims {
    version: u8,
    space_id: SpaceId,
    principal_id: PrincipalId,
    authorization_epoch: u64,
    sequence: u64,
}

impl EventCursorCodec {
    pub fn new(signing_key: impl AsRef<[u8]>) -> V3StoreResult<Self> {
        if signing_key.as_ref().len() < 32 {
            return Err(V3StoreError::InvalidCursor);
        }
        Ok(Self {
            signing_key: blake3::derive_key("layrs.sync.v3/event-cursor-key", signing_key.as_ref()),
        })
    }

    pub fn issue(
        &self,
        space_id: &SpaceId,
        principal_id: &PrincipalId,
        authorization_epoch: u64,
        sequence: u64,
    ) -> V3StoreResult<Cursor> {
        let body = serde_jcs::to_vec(&EventCursorClaims {
            version: 1,
            space_id: space_id.clone(),
            principal_id: principal_id.clone(),
            authorization_epoch,
            sequence,
        })
        .map_err(|error| V3StoreError::Backend(error.to_string()))?;
        let mut mac = HmacSha256::new_from_slice(&self.signing_key)
            .map_err(|_| V3StoreError::InvalidCursor)?;
        mac.update(&body);
        let signature = mac.finalize().into_bytes();
        Cursor::parse(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(body),
            URL_SAFE_NO_PAD.encode(signature)
        ))
        .map_err(|_| V3StoreError::InvalidCursor)
    }

    pub(crate) fn verify(
        &self,
        cursor: &Cursor,
        space_id: &SpaceId,
        principal_id: &PrincipalId,
        current_authorization_epoch: u64,
    ) -> V3StoreResult<u64> {
        let (body, signature) = cursor
            .as_str()
            .split_once('.')
            .ok_or(V3StoreError::InvalidCursor)?;
        let body = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| V3StoreError::InvalidCursor)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| V3StoreError::InvalidCursor)?;
        let mut mac = HmacSha256::new_from_slice(&self.signing_key)
            .map_err(|_| V3StoreError::InvalidCursor)?;
        mac.update(&body);
        mac.verify_slice(&signature)
            .map_err(|_| V3StoreError::InvalidCursor)?;
        let claims: EventCursorClaims =
            serde_json::from_slice(&body).map_err(|_| V3StoreError::InvalidCursor)?;
        if claims.version != 1
            || claims.space_id != *space_id
            || claims.principal_id != *principal_id
        {
            return Err(V3StoreError::InvalidCursor);
        }
        if claims.authorization_epoch != current_authorization_epoch {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        Ok(claims.sequence)
    }
}

impl BootstrapCursorIssuer for EventCursorCodec {
    fn issue_bootstrap_cursor(
        &self,
        space_id: &SpaceId,
        principal_id: &PrincipalId,
        authorization_epoch: u64,
        sequence: u64,
    ) -> V3StoreResult<Cursor> {
        self.issue(space_id, principal_id, authorization_epoch, sequence)
    }
}

#[derive(Clone)]
pub struct PgEventStore {
    repository: PgV3Repository,
    cursors: EventCursorCodec,
    max_scan_per_page: usize,
}

impl PgEventStore {
    pub fn new(repository: PgV3Repository, cursors: EventCursorCodec) -> Self {
        Self {
            repository,
            cursors,
            max_scan_per_page: 40_000,
        }
    }

    pub fn with_max_scan_per_page(mut self, maximum: usize) -> V3StoreResult<Self> {
        if maximum == 0 || maximum > 1_000_000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "event scan page",
                limit: 1_000_000,
            });
        }
        self.max_scan_per_page = maximum;
        Ok(self)
    }

    pub async fn page(
        &self,
        principal: &PrincipalContext,
        space_id: &SpaceId,
        cursor: &Cursor,
        max_events: u16,
    ) -> V3StoreResult<SpaceEventPage> {
        if max_events == 0 || max_events > 10_000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "event page",
                limit: 10_000,
            });
        }
        let authority = load_event_authority(self.repository.pool(), space_id).await?;
        let start = self.cursors.verify(
            cursor,
            space_id,
            &principal.principal_id,
            authority.authorization_epoch,
        )?;
        if start > authority.watermark_sequence {
            return Err(V3StoreError::InvalidCursor);
        }
        require_allowed(
            self.repository
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action: V3Action::ReadEvents,
                    target: AuthorizationTarget::Space(space_id.clone()),
                    policy_set_id: authority.policy_set_id.clone(),
                    authorization_epoch: authority.authorization_epoch,
                })
                .await?,
        )?;

        let mut visible = Vec::with_capacity(usize::from(max_events));
        let mut scanned = start;
        let mut remaining_scan = self.max_scan_per_page;
        while visible.len() < usize::from(max_events)
            && scanned < authority.watermark_sequence
            && remaining_scan > 0
        {
            let batch = remaining_scan.min(usize::from(max_events).saturating_mul(4).max(64));
            let rows = sqlx::query(
                r#"
                SELECT sequence, event_kind, actor_kind, actor_id, device_id,
                       payload_canonical_jcs
                FROM space_events
                WHERE space_id=$1 AND sequence>$2 AND sequence<=$3
                ORDER BY sequence
                LIMIT $4
                "#,
            )
            .bind(space_id.as_str())
            .bind(i64::try_from(scanned).map_err(|_| V3StoreError::InvalidCursor)?)
            .bind(
                i64::try_from(authority.watermark_sequence)
                    .map_err(|_| V3StoreError::InvalidCursor)?,
            )
            .bind(i64::try_from(batch).map_err(|_| V3StoreError::InvalidCursor)?)
            .fetch_all(self.repository.pool())
            .await
            .map_err(pg_error)?;
            if rows.is_empty() {
                // Sequence allocation is authoritative and append-only; a gap
                // up to the watermark indicates corruption, never EOF.
                return Err(V3StoreError::CorruptObject(
                    "event sequence gap before watermark".into(),
                ));
            }
            remaining_scan -= rows.len();
            for row in rows {
                let sequence = u64::try_from(row.get::<i64, _>("sequence"))
                    .map_err(|_| V3StoreError::CorruptObject("invalid event sequence".into()))?;
                if sequence != scanned + 1 {
                    return Err(V3StoreError::CorruptObject(
                        "non-contiguous event sequence".into(),
                    ));
                }
                scanned = sequence;
                let stored_kind: String = row.get("event_kind");
                let stored_jcs: String = row.get("payload_canonical_jcs");
                let event: SpaceEvent = serde_json::from_str(&stored_jcs).map_err(|error| {
                    V3StoreError::CorruptObject(format!("invalid stored SpaceEvent: {error}"))
                })?;
                if event.sequence() != sequence || event_kind(&event) != stored_kind {
                    return Err(V3StoreError::CorruptObject(
                        "stored SpaceEvent metadata mismatch".into(),
                    ));
                }
                let actor = event_actor(&event);
                let stored_actor_kind: String = row.get("actor_kind");
                let stored_actor_id: String = row.get("actor_id");
                let stored_device_id: Option<String> = row.get("device_id");
                if actor_kind(actor.kind) != stored_actor_kind
                    || actor.actor_id.as_str() != stored_actor_id
                    || actor.device_id.as_ref().map(|id| id.as_str()) != stored_device_id.as_deref()
                {
                    return Err(V3StoreError::CorruptObject(
                        "stored SpaceEvent actor mismatch".into(),
                    ));
                }
                let canonical = serde_jcs::to_string(&event)
                    .map_err(|error| V3StoreError::Backend(error.to_string()))?;
                if canonical != stored_jcs {
                    return Err(V3StoreError::CorruptObject(
                        "stored SpaceEvent is not canonical JCS".into(),
                    ));
                }
                let decision = self
                    .repository
                    .authorizer()
                    .authorize(&AuthorizationCheck {
                        principal: principal.clone(),
                        action: V3Action::ReadEvents,
                        target: AuthorizationTarget::Event {
                            space_id: space_id.clone(),
                            event_kind: stored_kind,
                        },
                        policy_set_id: authority.policy_set_id.clone(),
                        authorization_epoch: authority.authorization_epoch,
                    })
                    .await?;
                if decision.allowed {
                    visible.push(event);
                    if visible.len() == usize::from(max_events) {
                        break;
                    }
                }
            }
        }

        let final_authority = load_event_authority(self.repository.pool(), space_id).await?;
        if final_authority.authorization_epoch != authority.authorization_epoch
            || final_authority.policy_set_id != authority.policy_set_id
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let has_more = scanned < final_authority.watermark_sequence;
        let next_cursor = self.cursors.issue(
            space_id,
            &principal.principal_id,
            authority.authorization_epoch,
            scanned,
        )?;
        Ok(SpaceEventPage {
            protocol: SyncV3Protocol::V3,
            message_type: EventPageResponseType::Response,
            principal_id: principal.principal_id.clone(),
            authorization_epoch: authority.authorization_epoch,
            current_policy_set_id: authority.policy_set_id,
            space_id: space_id.clone(),
            events: visible,
            scanned_through_sequence: scanned,
            next_cursor,
            has_more,
        })
    }
}

#[derive(Debug)]
struct EventAuthority {
    policy_set_id: ObjectId,
    authorization_epoch: u64,
    watermark_sequence: u64,
}

async fn load_event_authority(
    pool: &sqlx::PgPool,
    space_id: &SpaceId,
) -> V3StoreResult<EventAuthority> {
    let row = sqlx::query(
        r#"
        SELECT current_policy_set_id, authorization_epoch,
               next_event_sequence - 1 AS watermark
        FROM spaces
        WHERE space_id=$1 AND lifecycle_state='ready'
        "#,
    )
    .bind(space_id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(pg_error)?
    .ok_or(V3StoreError::InvalidCapability)?;
    Ok(EventAuthority {
        policy_set_id: ObjectId::parse(row.get::<String, _>("current_policy_set_id"))
            .map_err(|_| V3StoreError::CorruptObject("invalid PolicySet ID".into()))?,
        authorization_epoch: u64::try_from(row.get::<i64, _>("authorization_epoch"))
            .map_err(|_| V3StoreError::CorruptObject("invalid epoch".into()))?,
        watermark_sequence: u64::try_from(row.get::<i64, _>("watermark"))
            .map_err(|_| V3StoreError::CorruptObject("invalid watermark".into()))?,
    })
}

fn event_kind(event: &SpaceEvent) -> &'static str {
    match event {
        SpaceEvent::StepsAccepted { .. } => "steps.accepted",
        SpaceEvent::ProductsAccepted { .. } => "products.accepted",
        SpaceEvent::LayerDefinitionChanged { .. } => "layer.definition_changed",
        SpaceEvent::RepositoryViewChanged { .. } => "repository_view.changed",
        SpaceEvent::PolicyRootChanged { .. } => "policy.root_changed",
    }
}

fn event_actor(event: &SpaceEvent) -> &EventActor {
    match event {
        SpaceEvent::StepsAccepted { actor, .. }
        | SpaceEvent::ProductsAccepted { actor, .. }
        | SpaceEvent::LayerDefinitionChanged { actor, .. }
        | SpaceEvent::RepositoryViewChanged { actor, .. }
        | SpaceEvent::PolicyRootChanged { actor, .. } => actor,
    }
}

fn actor_kind(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Account => "account",
        ActorKind::Device => "device",
        ActorKind::Automation => "automation",
        ActorKind::Server => "server",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_is_bound_to_principal_space_epoch_and_sequence() {
        let codec = EventCursorCodec::new([7_u8; 32]).unwrap();
        let space = SpaceId::parse("space:a").unwrap();
        let principal = PrincipalId::parse("principal:a").unwrap();
        let cursor = codec.issue(&space, &principal, 9, 42).unwrap();
        assert_eq!(codec.verify(&cursor, &space, &principal, 9).unwrap(), 42);
        assert!(matches!(
            codec.verify(&cursor, &space, &principal, 10),
            Err(V3StoreError::AuthorizationEpochChanged)
        ));
        assert!(matches!(
            codec.verify(
                &cursor,
                &space,
                &PrincipalId::parse("principal:b").unwrap(),
                9
            ),
            Err(V3StoreError::InvalidCursor)
        ));
    }
}
