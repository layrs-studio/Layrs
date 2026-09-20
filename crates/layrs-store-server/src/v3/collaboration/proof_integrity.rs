//! A SQL verdict is a projection, never sufficient evidence for a passing Gate.
use super::*;
use layrs_core::scm::StepId;
use layrs_sync::v3::{
    ImmutableSubject, PrincipalId, ProductEnvelope, ProductLogicalId, ValidateV3,
};
use serde::Deserialize;
use sqlx::postgres::PgRow;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Verdict {
    schema_version: String,
    run_id: String,
    step_id: StepId,
    recipe_object_id: ObjectId,
    runner_principal_id: PrincipalId,
    policy_set_id: ObjectId,
    authorization_epoch: u64,
    outcome: RunOutcome,
    log: String,
    completed_at: Timestamp,
}

async fn read(
    tx: &mut Transaction<'_, Postgres>,
    cas: &dyn CatalogedCas,
    space: &SpaceId,
    id: &ObjectId,
    kind: ObjectKind,
    limit: u64,
) -> V3StoreResult<Vec<u8>> {
    let row = sqlx::query("SELECT o.object_id,o.object_kind,o.canonical_byte_length,o.encoded_byte_length,o.compression FROM canonical_objects o JOIN space_object_availability a ON a.object_id=o.object_id WHERE a.space_id=$1 AND a.object_id=$2 AND a.availability_state='available' AND o.verification_state='verified'")
        .bind(space.as_str()).bind(id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
    let descriptor = super::super::plan_store::descriptor_from_row(&row)?;
    if descriptor.kind != kind {
        return Err(types::invalid("proof_reference_kind_mismatch"));
    }
    read_canonical_object(cas, &descriptor, limit).await
}

pub(super) async fn verify(
    tx: &mut Transaction<'_, Postgres>,
    cas: &dyn CatalogedCas,
    authority: &SpaceAuthority,
    step: &StepId,
    recipe: &ObjectId,
    run: &PgRow,
) -> V3StoreResult<()> {
    let fail = || types::invalid("proof_evidence_binding_mismatch");
    let proof_id: ObjectId = parse(run.get("proof_object_id"))?;
    let bytes = read(
        tx,
        cas,
        &authority.space_id,
        &proof_id,
        ObjectKind::Product,
        1024 * 1024,
    )
    .await?;
    let product: ProductEnvelope = serde_json::from_slice(&bytes).map_err(bad)?;
    product.validate_v3().map_err(bad)?;
    if product.object_id() != &proof_id || product.canonical_bytes().map_err(bad)? != bytes {
        return Err(fail());
    }
    let ProductEnvelope::Proof {
        product_id,
        depends_on_step_ids,
        depends_on_product_ids,
        record,
        ..
    } = &product
    else {
        return Err(fail());
    };
    if record.space_id != authority.space_id
        || record.subject
            != (ImmutableSubject::Step {
                object_id: parse(step.as_str().to_owned())?,
            })
        || &record.recipe_object_id != recipe
        || depends_on_step_ids.as_slice() != std::slice::from_ref(step)
        || record.evidence_object_ids.len() != 1
    {
        return Err(fail());
    }
    let policies = sqlx::query("SELECT e.policy_product_object_id,p.logical_id FROM policy_set_entries e JOIN products p ON p.product_object_id=e.policy_product_object_id WHERE e.policy_set_id=$1 ORDER BY p.logical_id LIMIT 1025")
        .bind(authority.policy_set_id.as_str()).fetch_all(&mut **tx).await.map_err(pg_error)?;
    if policies.len() > 1024 {
        return Err(fail());
    }
    let policy_objects: Vec<ObjectId> = policies
        .iter()
        .map(|row| parse(row.get("policy_product_object_id")))
        .collect::<V3StoreResult<_>>()?;
    let policy_ids: Vec<ProductLogicalId> = policies
        .iter()
        .map(|row| parse(row.get("logical_id")))
        .collect::<V3StoreResult<_>>()?;
    if record.policy_version_object_ids != policy_objects || depends_on_product_ids != &policy_ids {
        return Err(fail());
    }
    // Escaping a bounded 256 KiB log can expand its canonical JSON to 1.5 MiB.
    let bytes = read(
        tx,
        cas,
        &authority.space_id,
        &record.evidence_object_ids[0],
        ObjectKind::Evidence,
        2 * 1024 * 1024,
    )
    .await?;
    let verdict: Verdict = serde_json::from_slice(&bytes).map_err(bad)?;
    if serde_jcs::to_vec(&verdict).map_err(bad)? != bytes
        || verdict.schema_version != "layrs.run-verdict.v1"
        || verdict.run_id != run.get::<String, _>("run_id")
        || &verdict.step_id != step
        || &verdict.recipe_object_id != recipe
        || verdict.runner_principal_id.as_str() != run.get::<String, _>("runner_principal_id")
        || verdict.policy_set_id.as_str() != authority.policy_set_id.as_str()
        || verdict.authorization_epoch != authority.authorization_epoch
        || verdict.outcome != RunOutcome::Passed
        || verdict.log.len() > 256 * 1024
        || verdict.completed_at != record.created_at
    {
        return Err(fail());
    }
    let completion_digest = digest(&CompleteRunRequest {
        outcome: verdict.outcome,
        log: verdict.log,
    })?;
    if completion_digest != run.get::<String, _>("completion_digest")
        || product_id.as_str()
            != format!("proof:{}", digest(&(&verdict.run_id, &completion_digest))?)
    {
        return Err(fail());
    }
    let recipe_bytes = read(
        tx,
        cas,
        &authority.space_id,
        recipe,
        ObjectKind::Evidence,
        1024 * 1024,
    )
    .await?;
    let recipe_value: serde_json::Value = serde_json::from_slice(&recipe_bytes).map_err(bad)?;
    if serde_jcs::to_vec(&recipe_value).map_err(bad)? != recipe_bytes {
        return Err(fail());
    }
    Ok(())
}
