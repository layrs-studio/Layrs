use super::*;
use layrs_sync::v3::{LayerMutationResult, Rejection, RejectionCode};
use std::collections::BTreeSet;

/// Lock all bounded workspace identities/memberships before the Space lock. A
/// new member not in this captured set cannot contribute a counted decision.
pub(crate) async fn lock_reviewers(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
) -> V3StoreResult<BTreeSet<String>> {
    let rows=sqlx::query("SELECT p.principal_id FROM spaces s JOIN workspace_memberships m USING(workspace_id) JOIN principals p USING(principal_id) WHERE s.space_id=$1 ORDER BY p.principal_id LIMIT 257 FOR SHARE OF p,m").bind(space.as_str()).fetch_all(&mut **tx).await.map_err(pg_error)?;
    if rows.len() > 256 {
        return Err(V3StoreError::LimitExceeded {
            resource: "workspace_gate_principals",
            limit: 256,
        });
    }
    Ok(rows.into_iter().map(|r| r.get("principal_id")).collect())
}

pub(crate) async fn enforce(
    cas: &dyn CatalogedCas,
    tx: &mut Transaction<'_, Postgres>,
    authority: &SpaceAuthority,
    layers: &[LayerMutationResult],
    locked: &BTreeSet<String>,
) -> V3StoreResult<Option<Rejection>> {
    let reject = |message: &str| {
        Some(super::super::transaction_plan::reject(
            RejectionCode::GateRejected,
            message,
        ))
    };
    for layer in layers {
        if layer.resulting_heads == layer.previous_heads {
            continue;
        }
        let Some(raw)=sqlx::query_scalar::<_,String>("SELECT canonical_document_jcs FROM layrs_collaboration.layer_protections WHERE space_id=$1 AND layer_id=$2").bind(authority.space_id.as_str()).bind(layer.layer_id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)? else{continue};
        let protection: LayerProtection = serde_json::from_str(&raw).map_err(bad)?;
        if layer.resulting_heads.len() != 1 {
            return Ok(reject("protected Layer requires exactly one reviewed head"));
        }
        let step = &layer.resulting_heads[0];
        if protection.reject_conflicts {
            let raw: Option<String> =
                sqlx::query_scalar("SELECT canonical_record_jcs FROM steps WHERE step_id=$1")
                    .bind(step.as_str())
                    .fetch_optional(&mut **tx)
                    .await
                    .map_err(pg_error)?;
            let Some(raw) = raw else {
                return Ok(reject("protected Step must be admitted before evaluation"));
            };
            let record: layrs_core::scm::Step = serde_json::from_str(&raw).map_err(bad)?;
            let conflicts:bool=sqlx::query_scalar("WITH RECURSIVE nodes(id) AS (SELECT $1::text UNION SELECT e.target_object_id FROM canonical_object_edges e JOIN nodes n ON n.id=e.source_object_id WHERE e.edge_kind='tree_entry') SELECT EXISTS(SELECT 1 FROM nodes n JOIN canonical_objects o ON o.object_id=n.id WHERE o.object_kind='conflict')").bind(record.root_tree_id().as_str()).fetch_one(&mut **tx).await.map_err(pg_error)?;
            if conflicts {
                return Ok(reject("protected Step contains unresolved conflicts"));
            }
        }
        if protection.minimum_approvals > 0 {
            let row=sqlx::query("SELECT r.review_id,r.author_principal_id,v.version FROM layrs_collaboration.reviews r JOIN layrs_collaboration.review_versions v ON v.review_id=r.review_id AND v.version=r.latest_version WHERE r.space_id=$1 AND r.layer_id=$2 AND r.lifecycle='open' AND v.proposed_step_id=$3 AND v.base_repository_view_id=$4 AND v.policy_set_id=$5 AND v.authorization_epoch=$6 ORDER BY r.created_at DESC,r.review_id LIMIT 1")
                .bind(authority.space_id.as_str()).bind(layer.layer_id.as_str()).bind(step.as_str()).bind(authority.repository_view_id.as_str()).bind(authority.policy_set_id.as_str()).bind(authority.authorization_epoch as i64).fetch_optional(&mut **tx).await.map_err(pg_error)?;
            let Some(row) = row else {
                return Ok(reject(
                    "current Step, target View and policy require a review",
                ));
            };
            let author: String = row.get("author_principal_id");
            let decisions=sqlx::query("SELECT DISTINCT ON (a.actor_principal_id) a.actor_principal_id,a.action_kind FROM layrs_collaboration.review_actions a JOIN principals p ON p.principal_id=a.actor_principal_id JOIN spaces s ON s.space_id=$3 JOIN workspace_memberships m ON m.workspace_id=s.workspace_id AND m.principal_id=p.principal_id WHERE a.review_id=$1 AND a.version=$2 AND a.action_kind IN ('approve','request_changes') AND p.status='active' AND p.principal_kind IN ('account','bootstrap_admin') AND m.state='active' ORDER BY a.actor_principal_id,a.sequence DESC")
                .bind(row.get::<String,_>("review_id")).bind(row.get::<i32,_>("version")).bind(authority.space_id.as_str()).fetch_all(&mut **tx).await.map_err(pg_error)?;
            let mut approvals = 0;
            for decision in decisions {
                let actor: String = decision.get("actor_principal_id");
                if !locked.contains(&actor) || !protection.allow_self_approval && actor == author {
                    continue;
                }
                if decision.get::<String, _>("action_kind") == "request_changes" {
                    return Ok(reject("review has an outstanding change request"));
                }
                approvals += 1;
            }
            if approvals < protection.minimum_approvals {
                return Ok(reject("review has insufficient current approvals"));
            }
        }
        if let Some(recipe) = &protection.recipe_object_id {
            let rows=sqlx::query("SELECT DISTINCT ON (r.runner_principal_id) r.run_id,r.proof_object_id,r.completion_digest,r.runner_principal_id,r.outcome,r.completed_at>clock_timestamp()-($6::bigint*interval '1 second') AS fresh FROM layrs_collaboration.runs r JOIN principals p ON p.principal_id=r.runner_principal_id JOIN spaces s ON s.space_id=r.space_id JOIN workspace_memberships m ON m.workspace_id=s.workspace_id AND m.principal_id=p.principal_id WHERE r.space_id=$1 AND r.step_id=$2 AND r.recipe_object_id=$3 AND r.policy_set_id=$4 AND r.authorization_epoch=$5 AND r.completed_at IS NOT NULL AND p.status='active' AND p.principal_kind='service' AND m.state='active' ORDER BY r.runner_principal_id,r.completed_at DESC,r.run_id DESC LIMIT 257")
                .bind(authority.space_id.as_str()).bind(step.as_str()).bind(recipe.as_str()).bind(authority.policy_set_id.as_str()).bind(authority.authorization_epoch as i64).bind(protection.maximum_proof_age_seconds as i64).fetch_all(&mut **tx).await.map_err(pg_error)?;
            if rows.len() > 256 {
                return Ok(reject("too many proof producers"));
            }
            let mut passed = false;
            for row in &rows {
                let actor: String = row.get("runner_principal_id");
                let eligible = locked.contains(&actor)
                    && protection
                        .trusted_runner_principal_ids
                        .iter()
                        .any(|p| p.as_str() == actor)
                    && row.get::<String, _>("outcome") == "passed"
                    && row.get::<bool, _>("fresh");
                if eligible {
                    if super::proof_integrity::verify(tx, cas, authority, step, recipe, row)
                        .await
                        .is_err()
                    {
                        return Ok(reject(
                            "passing Proof or its Evidence is unavailable or invalid",
                        ));
                    }
                    passed = true;
                }
            }
            if !passed {
                return Ok(reject(
                    "missing fresh passing Proof for exact Step, recipe and policy",
                ));
            }
        }
    }
    Ok(None)
}

/// Releases carry no Layer ID. Conservatively apply every protected Layer in
/// their Space to the exact release target; no unscoped publication bypass.
pub(crate) async fn enforce_release(
    cas: &dyn CatalogedCas,
    tx: &mut Transaction<'_, Postgres>,
    authority: &SpaceAuthority,
    step: &layrs_core::scm::StepId,
    locked: &BTreeSet<String>,
) -> V3StoreResult<Option<Rejection>> {
    let ids:Vec<String>=sqlx::query_scalar("SELECT layer_id FROM layrs_collaboration.layer_protections WHERE space_id=$1 ORDER BY layer_id").bind(authority.space_id.as_str()).fetch_all(&mut **tx).await.map_err(pg_error)?;
    let mut layers = vec![];
    for id in ids {
        layers.push(LayerMutationResult {
            layer_id: parse(id)?,
            mode: layrs_sync::v3::LayerMutationMode::Advance,
            previous_heads: vec![],
            proposed_heads: vec![step.clone()],
            resulting_heads: vec![step.clone()],
            divergence_created: false,
            decision_product_object_id: layrs_sync::v3::Nullable::null(),
        });
    }
    enforce(cas, tx, authority, &layers, locked).await
}
