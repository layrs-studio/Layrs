//! Immutable-root DAG pages. Cursor state never grants access to an object.
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use layrs_core::scm::{ObjectId, RepoPath, SpaceId, Step, StepId};
use layrs_sync::v3::ObjectKind;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepHistoryRequest {
    pub anchor: StepId,
    pub cursor: Option<String>,
    pub limit: u16,
    #[serde(default)]
    pub path: Option<RepoPath>,
    #[serde(default)]
    pub filter: PublishedObservationFilter,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepHistoryResponse {
    pub anchor: StepId,
    pub steps: Vec<Step>,
    pub next: Option<String>,
    pub path: Option<RepoPath>,
    pub filter: PublishedObservationFilter,
    pub observation_snapshot: PublishedObservationSnapshot,
    pub observations: BTreeMap<StepId, Vec<PublishedStepObservation>>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    version: u8,
    space: SpaceId,
    principal: layrs_sync::v3::PrincipalId,
    epoch: u64,
    anchor: StepId,
    offset: usize,
    #[serde(default)]
    path: Option<RepoPath>,
    filter: PublishedObservationFilter,
    observation_snapshot: PublishedObservationSnapshot,
}
fn invalid() -> V3StoreError {
    V3StoreError::InvalidDescriptor("invalid history page or cursor".into())
}
impl PgV3Repository {
    /// Parents retain canonical order. New publications cannot change a page
    /// anchored to this immutable Step. Local observations are not fabricated.
    pub async fn step_history(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        request: StepHistoryRequest,
    ) -> V3StoreResult<StepHistoryResponse> {
        if !(1..=100).contains(&request.limit)
            || request
                .path
                .as_ref()
                .is_some_and(|path| path.as_str().len() > 256)
        {
            return Err(invalid());
        }
        request.filter.validate()?;
        self.history_allowed(
            principal,
            authority,
            AuthorizationTarget::Space(authority.space_id.clone()),
        )
        .await?;
        if let Some(path) = &request.path {
            self.history_allowed(
                principal,
                authority,
                AuthorizationTarget::Path {
                    space_id: authority.space_id.clone(),
                    repo_path: path.clone(),
                },
            )
            .await?;
        }
        let (offset, pinned) = if let Some(encoded) = &request.cursor {
            if encoded.len() > 4096 {
                return Err(invalid());
            }
            let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
            let cursor: Cursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if cursor.version != 2
                || cursor.space != authority.space_id
                || cursor.principal != principal.principal_id
                || cursor.anchor != request.anchor
                || cursor.path != request.path
                || cursor.filter != request.filter
                || cursor.offset > 100_000
            {
                return Err(invalid());
            }
            if cursor.epoch != authority.authorization_epoch {
                return Err(V3StoreError::AuthorizationEpochChanged);
            }
            (cursor.offset, Some(cursor.observation_snapshot))
        } else {
            (0, None)
        };
        let projection = self
            .published_observations(principal, authority, pinned.as_ref())
            .await?;
        let mut observations = BTreeMap::new();
        let mut pending = vec![request.anchor.clone()];
        let mut visited = BTreeSet::new();
        let mut steps = Vec::new();
        let mut has_next = false;
        let mut total_bytes = 0u64;
        let mut output_bytes = 0u64;
        let mut matched = 0usize;
        let mut path_reader = request
            .path
            .as_ref()
            .map(|path| super::history_path::PathHistory::new(self, principal, authority, path));
        while let Some(id) = pending.pop() {
            if !visited.insert(id.clone()) {
                continue;
            }
            if visited.len() > 100_000 {
                return Err(V3StoreError::LimitExceeded {
                    resource: "history traversal",
                    limit: 100_000,
                });
            }
            self.history_allowed(
                principal,
                authority,
                AuthorizationTarget::Object {
                    space_id: authority.space_id.clone(),
                    object_id: ObjectId::parse(id.as_str()).map_err(|_| invalid())?,
                },
            )
            .await?;
            let admitted: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM space_steps WHERE space_id=$1 AND step_id=$2)",
            )
            .bind(authority.space_id.as_str())
            .bind(id.as_str())
            .fetch_one(self.pool())
            .await
            .map_err(pg_error)?;
            if !admitted {
                return Err(V3StoreError::NotFound);
            }
            let descriptor = load_space_descriptor(
                self.pool(),
                &authority.space_id,
                &ObjectId::parse(id.as_str()).map_err(|_| invalid())?,
            )
            .await?;
            if descriptor.kind != ObjectKind::Step {
                return Err(V3StoreError::IdentityMismatch);
            }
            total_bytes = total_bytes
                .checked_add(descriptor.canonical_byte_len)
                .ok_or_else(invalid)?;
            if total_bytes > 64 * 1024 * 1024 {
                return Err(V3StoreError::LimitExceeded {
                    resource: "history bytes",
                    limit: 64 * 1024 * 1024,
                });
            }
            let bytes =
                read_canonical_object(self.cas().as_ref(), &descriptor, 1024 * 1024).await?;
            let step: Step =
                serde_json::from_slice(&bytes).map_err(|_| V3StoreError::IdentityMismatch)?;
            if step.id() != &id {
                return Err(V3StoreError::IdentityMismatch);
            }
            pending.extend(step.parent_step_ids().iter().rev().cloned());
            if !projection.matches(&id, &request.filter)? {
                continue;
            }
            if let Some(reader) = path_reader.as_mut() {
                let changed = reader.changed(&step).await?;
                if total_bytes + reader.byte_count() > 64 * 1024 * 1024 {
                    return Err(V3StoreError::LimitExceeded {
                        resource: "history bytes",
                        limit: 64 * 1024 * 1024,
                    });
                }
                if !changed {
                    continue;
                }
            }
            matched += 1;
            if matched <= offset {
                continue;
            }
            if steps.len() == usize::from(request.limit) {
                has_next = true;
                break;
            }
            let step_observations = projection.observations(&id).to_vec();
            let entry_bytes = descriptor.canonical_byte_len
                + id.as_str().len() as u64
                + 64
                + serde_json::to_vec(&step_observations)
                    .map_err(|_| invalid())?
                    .len() as u64;
            // Reserve envelope/cursor bytes and split only between complete
            // Steps. Cursor offset counts returned matches, never the deferred
            // match, so a byte-limited page cannot skip any observations.
            if output_bytes + entry_bytes > 2 * 1024 * 1024 - 16 * 1024 {
                if !steps.is_empty() {
                    has_next = true;
                    break;
                }
                return Err(V3StoreError::LimitExceeded {
                    resource: "history page bytes",
                    limit: 2 * 1024 * 1024,
                });
            }
            output_bytes += entry_bytes;
            observations.insert(id, step_observations);
            steps.push(step);
        }
        // Revocation cannot race a successful response under the writer lock order.
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        super::transaction_store::lock_identity(&mut tx, principal, &authority.space_id).await?;
        let row = sqlx::query("SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 FOR SHARE")
            .bind(authority.space_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        self.history_allowed(
            principal,
            authority,
            AuthorizationTarget::Space(authority.space_id.clone()),
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        let next = if has_next {
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&Cursor {
                        version: 2,
                        space: authority.space_id.clone(),
                        principal: principal.principal_id.clone(),
                        epoch: authority.authorization_epoch,
                        anchor: request.anchor.clone(),
                        offset: offset + steps.len(),
                        path: request.path.clone(),
                        filter: request.filter.clone(),
                        observation_snapshot: projection.snapshot().clone(),
                    })
                    .map_err(|_| invalid())?,
                ),
            )
        } else {
            None
        };
        Ok(StepHistoryResponse {
            anchor: request.anchor,
            steps,
            next,
            path: request.path,
            filter: request.filter,
            observation_snapshot: projection.snapshot().clone(),
            observations,
        })
    }
    pub(super) async fn history_allowed(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        target: AuthorizationTarget,
    ) -> V3StoreResult<()> {
        require_allowed(
            self.authorizer()
                .authorize(&AuthorizationCheck {
                    principal: principal.clone(),
                    action: V3Action::FetchObject,
                    target,
                    policy_set_id: authority.policy_set_id.clone(),
                    authorization_epoch: authority.authorization_epoch,
                })
                .await?,
        )
    }
}
