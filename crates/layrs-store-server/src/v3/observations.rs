//! Authenticated observation provenance, separate from local capture authority.
use super::*;
use layrs_core::scm::{
    ActorId, CaptureReason, Metadata, MetadataEntry, ObjectId, ObservationId, SpaceId, StepId,
    StepObservation, Timestamp,
};
use layrs_sync::v3::PrincipalId;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) mod assertions;

const MAX_ROWS: u64 = 100_000;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECORD: usize = 4096;
fn invalid() -> V3StoreError {
    V3StoreError::InvalidDescriptor("invalid published observation snapshot or filter".into())
}
fn corrupt() -> V3StoreError {
    V3StoreError::CorruptObject("published observation provenance mismatch".into())
}
fn instant(value: &Timestamp) -> V3StoreResult<OffsetDateTime> {
    OffsetDateTime::parse(value.as_str(), &Rfc3339).map_err(|_| invalid())
}
fn digest(bytes: &[u8]) -> ObjectId {
    ObjectId::parse(format!("blake3:{}", blake3::hash(bytes))).expect("digest")
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedObservationFilter {
    pub actor_id: Option<ActorId>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
}
impl PublishedObservationFilter {
    pub fn validate(&self) -> V3StoreResult<()> {
        if let (Some(since), Some(until)) = (&self.since, &self.until) {
            if instant(since)? > instant(until)? {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedObservationSnapshot {
    pub version: u32,
    pub space_id: SpaceId,
    pub max_ordinal: u64,
    pub count: u64,
    pub digest: ObjectId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishedObservationProvenance {
    ServerAdmission,
    ClientAssertion,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublishedStepObservation {
    pub observation: StepObservation,
    pub provenance: PublishedObservationProvenance,
    pub principal_id: PrincipalId,
    pub received_at: Timestamp,
    pub request_digest: ObjectId,
}
pub(crate) struct PublishedObservationIndex {
    snapshot: PublishedObservationSnapshot,
    by_step: BTreeMap<StepId, Vec<PublishedStepObservation>>,
}
impl PublishedObservationIndex {
    pub(crate) fn snapshot(&self) -> &PublishedObservationSnapshot {
        &self.snapshot
    }
    pub(crate) fn observations(&self, step: &StepId) -> &[PublishedStepObservation] {
        self.by_step.get(step).map(Vec::as_slice).unwrap_or(&[])
    }
    pub(crate) fn matches(
        &self,
        step: &StepId,
        filter: &PublishedObservationFilter,
    ) -> V3StoreResult<bool> {
        filter.validate()?;
        if filter.actor_id.is_none() && filter.since.is_none() && filter.until.is_none() {
            return Ok(true);
        }
        let since = filter.since.as_ref().map(instant).transpose()?;
        let until = filter.until.as_ref().map(instant).transpose()?;
        for value in self.observations(step) {
            let when = instant(&value.received_at)?;
            if filter
                .actor_id
                .as_ref()
                .is_none_or(|actor| value.observation.actor_id() == Some(actor))
                && since.is_none_or(|start| when >= start)
                && until.is_none_or(|end| when <= end)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

/// Called after Step admission under the same identity and Space locks. New
/// Steps only: republishing an old graph must not claim authorship or append noise.
pub(crate) async fn record_admissions(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    space: &SpaceId,
    request_digest: &ObjectId,
    steps: &[StepId],
    received_at: &Timestamp,
) -> V3StoreResult<Vec<ObservationId>> {
    let received_at = server_timestamp(received_at)?;
    let mut ordinal:i64=sqlx::query_scalar("SELECT COALESCE(max(ordinal),0) FROM layrs_collaboration.published_step_observations WHERE space_id=$1")
        .bind(space.as_str()).fetch_one(&mut **tx).await.map_err(pg_error)?;
    let mut ids = Vec::with_capacity(steps.len());
    for step in steps {
        ordinal = ordinal.checked_add(1).ok_or_else(corrupt)?;
        let identity = serde_jcs::to_vec(&(
            "layrs.server-admission-observation.v1",
            space,
            step,
            request_digest,
        ))
        .map_err(|_| corrupt())?;
        let id = ObservationId::parse(format!("observation:{}", blake3::hash(&identity)))
            .map_err(|_| corrupt())?;
        let metadata = Metadata::new(vec![
            MetadataEntry::new("provenance", "server_admission").map_err(|_| corrupt())?,
            MetadataEntry::new("principalId", principal.principal_id.as_str())
                .map_err(|_| corrupt())?,
            MetadataEntry::new("requestDigest", request_digest.as_str()).map_err(|_| corrupt())?,
        ])
        .map_err(|_| corrupt())?;
        let observation = StepObservation::new(
            id.clone(),
            step.clone(),
            Some(space.clone()),
            None,
            None,
            Some(principal.actor_id.clone()),
            None,
            received_at.clone(),
            CaptureReason::Received,
            metadata,
        );
        let canonical = serde_jcs::to_string(&observation).map_err(|_| corrupt())?;
        sqlx::query("INSERT INTO step_observations(observation_id,space_id,step_id,canonical_record_jcs,observed_at) VALUES($1,$2,$3,$4,$5::timestamptz)")
            .bind(id.as_str()).bind(space.as_str()).bind(step.as_str()).bind(&canonical).bind(received_at.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO layrs_collaboration.published_step_observations(observation_id,space_id,step_id,principal_id,request_digest,record_digest,received_at,ordinal) VALUES($1,$2,$3,$4,$5,$6,$7::timestamptz,$8)")
            .bind(id.as_str()).bind(space.as_str()).bind(step.as_str()).bind(principal.principal_id.as_str()).bind(request_digest.as_str()).bind(digest(canonical.as_bytes()).as_str()).bind(received_at.as_str()).bind(ordinal).execute(&mut **tx).await.map_err(pg_error)?;
        ids.push(id);
    }
    ids.sort();
    Ok(ids)
}

fn server_timestamp(value: &Timestamp) -> V3StoreResult<Timestamp> {
    let when = instant(value)?;
    let when = when
        .replace_nanosecond(when.nanosecond() / 1000 * 1000)
        .map_err(|_| corrupt())?;
    Timestamp::parse(when.format(&Rfc3339).map_err(|_| corrupt())?).map_err(|_| corrupt())
}

impl PgV3Repository {
    /// Internal index only: history must still authorize each returned Step and
    /// revalidate the captured identity/epoch before exposing metadata.
    pub(crate) async fn published_observations(
        &self,
        principal: &PrincipalContext,
        authority: &SpaceAuthority,
        pinned: Option<&PublishedObservationSnapshot>,
    ) -> V3StoreResult<PublishedObservationIndex> {
        self.history_allowed(
            principal,
            authority,
            AuthorizationTarget::Space(authority.space_id.clone()),
        )
        .await?;
        if pinned.is_some_and(|s| {
            s.version != 2
                || s.space_id != authority.space_id
                || s.count > MAX_ROWS
                || s.max_ordinal > i64::MAX as u64
        }) {
            return Err(invalid());
        }
        let mut read = self.pool().begin().await.map_err(pg_error)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *read)
            .await
            .map_err(pg_error)?;
        let current:i64=sqlx::query_scalar("SELECT COALESCE(max(ordinal),0) FROM layrs_collaboration.published_step_observations WHERE space_id=$1")
            .bind(authority.space_id.as_str()).fetch_one(&mut *read).await.map_err(pg_error)?;
        let max = pinned.map_or(current as u64, |s| s.max_ordinal);
        if max > current as u64 {
            return Err(invalid());
        }
        // Bound both row allocation and aggregate records before fetching payloads.
        let population=sqlx::query("SELECT count(*) AS n,COALESCE(sum(octet_length(o.canonical_record_jcs)),0)::bigint AS bytes FROM layrs_collaboration.published_step_observations p LEFT JOIN step_observations o ON o.observation_id=p.observation_id WHERE p.space_id=$1 AND p.ordinal<=$2")
            .bind(authority.space_id.as_str()).bind(max as i64).fetch_one(&mut *read).await.map_err(pg_error)?;
        if population.get::<i64, _>("n") as u64 > MAX_ROWS
            || population.get::<i64, _>("bytes") as u64 > MAX_BYTES
        {
            return Err(V3StoreError::LimitExceeded {
                resource: "published observations",
                limit: MAX_ROWS,
            });
        }
        let rows=sqlx::query("SELECT p.ordinal,p.observation_id,p.step_id,p.principal_id,p.request_digest,p.record_digest,p.provenance,to_char(p.received_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS received_at,o.canonical_record_jcs,o.space_id AS observation_space,a.actor_id FROM layrs_collaboration.published_step_observations p LEFT JOIN step_observations o ON o.observation_id=p.observation_id JOIN principals a ON a.principal_id=p.principal_id WHERE p.space_id=$1 AND p.ordinal<=$2 ORDER BY p.ordinal LIMIT 100001")
            .bind(authority.space_id.as_str()).bind(max as i64).fetch_all(&mut *read).await.map_err(pg_error)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs.published-observation-snapshot.v2\0");
        let mut by_step: BTreeMap<StepId, Vec<PublishedStepObservation>> = BTreeMap::new();
        let mut total = 0usize;
        for row in &rows {
            let canonical: Option<String> = row.get("canonical_record_jcs");
            let canonical = canonical.ok_or_else(corrupt)?;
            total += canonical.len();
            if canonical.len() > MAX_RECORD
                || total > MAX_BYTES as usize
                || rows.len() > MAX_ROWS as usize
            {
                return Err(corrupt());
            }
            let observation: StepObservation =
                serde_json::from_str(&canonical).map_err(|_| corrupt())?;
            let principal_id =
                PrincipalId::parse(row.get::<String, _>("principal_id")).map_err(|_| corrupt())?;
            let request_digest =
                ObjectId::parse(row.get::<String, _>("request_digest")).map_err(|_| corrupt())?;
            let received =
                Timestamp::parse(row.get::<String, _>("received_at")).map_err(|_| corrupt())?;
            let source: String = row.get("provenance");
            let provenance = match source.as_str() {
                "server_admission" => PublishedObservationProvenance::ServerAdmission,
                "client_assertion" => PublishedObservationProvenance::ClientAssertion,
                _ => return Err(corrupt()),
            };
            let expected_metadata = Metadata::new(vec![
                MetadataEntry::new("provenance", "server_admission").map_err(|_| corrupt())?,
                MetadataEntry::new("principalId", principal_id.as_str()).map_err(|_| corrupt())?,
                MetadataEntry::new("requestDigest", request_digest.as_str())
                    .map_err(|_| corrupt())?,
            ])
            .map_err(|_| corrupt())?;
            if digest(canonical.as_bytes()).as_str() != row.get::<String, _>("record_digest")
                || serde_jcs::to_string(&observation).map_err(|_| corrupt())? != canonical
                || observation.id().as_str() != row.get::<String, _>("observation_id")
                || observation.space_id() != Some(&authority.space_id)
                || row.get::<Option<String>, _>("observation_space").as_deref()
                    != Some(authority.space_id.as_str())
                || observation.step_id().as_str() != row.get::<String, _>("step_id")
                || observation.actor_id().map(ActorId::as_str)
                    != Some(row.get::<String, _>("actor_id").as_str())
                || observation.operation_id().is_some()
                || observation.worktree_id().is_some()
                || observation.device_id().is_some()
            {
                return Err(corrupt());
            }
            match provenance {
                PublishedObservationProvenance::ServerAdmission => {
                    if observation.capture_reason() != CaptureReason::Received
                        || observation.metadata() != &expected_metadata
                        || instant(observation.observed_at())? != instant(&received)?
                    {
                        return Err(corrupt());
                    }
                }
                PublishedObservationProvenance::ClientAssertion => {
                    let expected = layrs_sync::v3::client_observation_id(
                        &authority.space_id,
                        &principal_id,
                        observation.step_id(),
                        observation.observed_at(),
                        observation.capture_reason(),
                    )
                    .map_err(|_| corrupt())?;
                    if observation.id() != &expected || !observation.metadata().entries().is_empty()
                    {
                        return Err(corrupt());
                    }
                }
            }
            let ordinal: i64 = row.get("ordinal");
            hasher.update(&ordinal.to_le_bytes());
            hasher.update(&(canonical.len() as u64).to_le_bytes());
            hasher.update(canonical.as_bytes());
            let received_at = server_timestamp(&received)?;
            let provenance_bytes =
                serde_jcs::to_vec(&(&provenance, &principal_id, &received_at, &request_digest))
                    .map_err(|_| corrupt())?;
            hasher.update(&(provenance_bytes.len() as u64).to_le_bytes());
            hasher.update(&provenance_bytes);
            by_step
                .entry(observation.step_id().clone())
                .or_default()
                .push(PublishedStepObservation {
                    observation,
                    provenance,
                    principal_id,
                    received_at,
                    request_digest,
                });
        }
        let snapshot = PublishedObservationSnapshot {
            version: 2,
            space_id: authority.space_id.clone(),
            max_ordinal: max,
            count: rows.len() as u64,
            digest: ObjectId::parse(format!("blake3:{}", hasher.finalize())).expect("digest"),
        };
        if pinned.is_some_and(|old| old != &snapshot) {
            return Err(invalid());
        }
        read.commit().await.map_err(pg_error)?;
        Ok(PublishedObservationIndex { snapshot, by_step })
    }
}
