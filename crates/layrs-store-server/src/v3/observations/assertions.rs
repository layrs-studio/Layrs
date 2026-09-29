//! Client assertions are append-only statements by the authenticated account.
use super::*;
use layrs_sync::v3::{Rejection, RejectionCode, SyncTransactionRequest, client_observation_id};
use std::collections::BTreeSet;

const MAX_PRINCIPAL: i64 = 10_000;
const MAX_SPACE: i64 = 100_000;
const MAX_PER_STEP: i64 = 256;
const MAX_ASSERTION_BYTES: i64 = 64 * 1024 * 1024;

pub(crate) struct PreparedAssertion {
    observation: StepObservation,
    canonical: String,
    existing: bool,
}
fn reject(code: RejectionCode, message: &str) -> Rejection {
    super::super::transaction_plan::reject(code, message)
}
pub(crate) fn prepare(
    principal: &PrincipalContext,
    request: &SyncTransactionRequest,
) -> Result<Vec<PreparedAssertion>, Rejection> {
    request.observations.iter().map(|observation| {
        if observation.space_id() != Some(&request.space_id)
            || observation.actor_id() != Some(&principal.actor_id)
            || observation.operation_id().is_some()
            || observation.worktree_id().is_some()
            || observation.device_id().is_some()
            || !observation.metadata().entries().is_empty()
        { return Err(reject(RejectionCode::PermissionDenied,"client observation requires exact authenticated actor and Space; local authorities and metadata are forbidden")); }
        let id = client_observation_id(&request.space_id,&principal.principal_id,observation.step_id(),observation.observed_at(),observation.capture_reason())
            .map_err(|_| reject(RejectionCode::InvalidRequest,"invalid client observation identity"))?;
        if &id != observation.id() { return Err(reject(RejectionCode::ImmutableObjectMismatch,"client observation identity differs from its bound assertion")); }
        let canonical=serde_jcs::to_string(observation).map_err(|_|reject(RejectionCode::InvalidRequest,"invalid client observation"))?;
        if canonical.len()>MAX_RECORD { return Err(reject(RejectionCode::InvalidRequest,"client observation record exceeds 4096 bytes")); }
        Ok(PreparedAssertion{observation:observation.clone(),canonical,existing:false})
    }).collect()
}
pub(crate) fn validate_steps(
    assertions: &[PreparedAssertion],
    known: &BTreeSet<StepId>,
) -> Option<Rejection> {
    assertions
        .iter()
        .any(|entry| !known.contains(entry.observation.step_id()))
        .then(|| {
            reject(
                RejectionCode::MissingObject,
                "observation Step is not admitted in this Space or batch",
            )
        })
}

/// Called under identity/Space locks before any accepted effects. Duplicate
/// records retain their first immutable provenance and consume no new budget.
pub(crate) async fn recheck(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    space: &SpaceId,
    assertions: &mut [PreparedAssertion],
) -> V3StoreResult<Option<Rejection>> {
    if assertions.is_empty() {
        return Ok(None);
    }
    let ids: Vec<_> = assertions
        .iter()
        .map(|a| a.observation.id().as_str())
        .collect();
    let rows=sqlx::query("SELECT o.observation_id,o.space_id,CASE WHEN octet_length(o.canonical_record_jcs)<=4096 THEN o.canonical_record_jcs ELSE NULL END AS canonical_record_jcs,p.principal_id,p.provenance,p.record_digest FROM step_observations o LEFT JOIN layrs_collaboration.published_step_observations p ON p.observation_id=o.observation_id WHERE o.observation_id=ANY($1)")
        .bind(&ids).fetch_all(&mut **tx).await.map_err(pg_error)?;
    let existing: BTreeMap<String, _> = rows
        .into_iter()
        .map(|r| (r.get("observation_id"), r))
        .collect();
    let mut count = 0i64;
    let mut bytes = 0i64;
    let mut added_by_step: BTreeMap<StepId, i64> = BTreeMap::new();
    for entry in assertions.iter_mut() {
        if let Some(row) = existing.get(entry.observation.id().as_str()) {
            if row.get::<String, _>("space_id") != space.as_str()
                || row
                    .get::<Option<String>, _>("canonical_record_jcs")
                    .as_deref()
                    != Some(entry.canonical.as_str())
                || row.get::<Option<String>, _>("principal_id").as_deref()
                    != Some(principal.principal_id.as_str())
                || row.get::<Option<String>, _>("provenance").as_deref() != Some("client_assertion")
                || row.get::<Option<String>, _>("record_digest").as_deref()
                    != Some(digest(entry.canonical.as_bytes()).as_str())
            {
                return Ok(Some(reject(
                    RejectionCode::ImmutableObjectMismatch,
                    "observation identity already has different bytes or provenance",
                )));
            }
            entry.existing = true;
        } else {
            entry.existing = false;
            count += 1;
            bytes += entry.canonical.len() as i64;
            *added_by_step
                .entry(entry.observation.step_id().clone())
                .or_default() += 1;
        }
    }
    let step_ids: Vec<_> = added_by_step.keys().map(StepId::as_str).collect();
    let rows=sqlx::query("SELECT step_id,count(*) AS n FROM layrs_collaboration.published_step_observations WHERE space_id=$1 AND provenance='client_assertion' AND step_id=ANY($2) GROUP BY step_id")
        .bind(space.as_str()).bind(&step_ids).fetch_all(&mut **tx).await.map_err(pg_error)?;
    for row in rows {
        let step = StepId::parse(row.get::<String, _>("step_id")).map_err(|_| corrupt())?;
        let total = added_by_step.entry(step).or_default();
        *total = total
            .checked_add(row.get::<i64, _>("n"))
            .ok_or_else(corrupt)?;
    }
    if added_by_step.values().any(|&n| n > MAX_PER_STEP) {
        return Ok(Some(reject(
            RejectionCode::InvalidRequest,
            "client observation per-Step quota exceeded",
        )));
    }
    let budget=sqlx::query("SELECT count(*) AS total,count(*) FILTER (WHERE p.principal_id=$2) AS actor,COALESCE(sum(octet_length(o.canonical_record_jcs)),0)::bigint AS bytes FROM layrs_collaboration.published_step_observations p JOIN step_observations o ON o.observation_id=p.observation_id WHERE p.space_id=$1 AND p.provenance='client_assertion'")
        .bind(space.as_str()).bind(principal.principal_id.as_str()).fetch_one(&mut **tx).await.map_err(pg_error)?;
    if budget
        .get::<i64, _>("total")
        .checked_add(count)
        .is_none_or(|v| v > MAX_SPACE)
        || budget
            .get::<i64, _>("actor")
            .checked_add(count)
            .is_none_or(|v| v > MAX_PRINCIPAL)
        || budget
            .get::<i64, _>("bytes")
            .checked_add(bytes)
            .is_none_or(|v| v > MAX_ASSERTION_BYTES)
    {
        return Ok(Some(reject(
            RejectionCode::InvalidRequest,
            "client observation quota exceeded",
        )));
    }
    Ok(None)
}

pub(crate) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    space: &SpaceId,
    request_digest: &ObjectId,
    assertions: &[PreparedAssertion],
    received_at: &Timestamp,
) -> V3StoreResult<(Vec<ObservationId>, Vec<ObservationId>)> {
    if assertions.is_empty() {
        return Ok((vec![], vec![]));
    }
    let received_at = server_timestamp(received_at)?;
    let mut ordinal:i64=sqlx::query_scalar("SELECT COALESCE(max(ordinal),0) FROM layrs_collaboration.published_step_observations WHERE space_id=$1")
        .bind(space.as_str()).fetch_one(&mut **tx).await.map_err(pg_error)?;
    let mut accepted = Vec::with_capacity(assertions.len());
    let mut inserted = Vec::new();
    for entry in assertions {
        let observation = &entry.observation;
        accepted.push(observation.id().clone());
        if entry.existing {
            continue;
        }
        ordinal = ordinal.checked_add(1).ok_or_else(corrupt)?;
        sqlx::query("INSERT INTO step_observations(observation_id,space_id,step_id,canonical_record_jcs,observed_at) VALUES($1,$2,$3,$4,$5::timestamptz)")
            .bind(observation.id().as_str()).bind(space.as_str()).bind(observation.step_id().as_str()).bind(&entry.canonical).bind(observation.observed_at().as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO layrs_collaboration.published_step_observations(observation_id,space_id,step_id,principal_id,request_digest,record_digest,received_at,ordinal,provenance) VALUES($1,$2,$3,$4,$5,$6,$7::timestamptz,$8,'client_assertion')")
            .bind(observation.id().as_str()).bind(space.as_str()).bind(observation.step_id().as_str()).bind(principal.principal_id.as_str()).bind(request_digest.as_str()).bind(digest(entry.canonical.as_bytes()).as_str()).bind(received_at.as_str()).bind(ordinal).execute(&mut **tx).await.map_err(pg_error)?;
        inserted.push(observation.id().clone());
    }
    accepted.sort();
    inserted.sort();
    Ok((accepted, inserted))
}
