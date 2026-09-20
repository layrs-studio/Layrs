//! Validate an authenticated publisher's receipt contract, not remote execution
//! attestation. Policy digests are opaque commitments: their preimage includes
//! the local worker binary and is deliberately not guessed by the server.
use super::*;
use crate::v3::closure::{VerifiedClosure, conflicts::entry_at};
use layrs_core::scm::{TreeEntry, TreeEntryTarget};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn invalid() -> V3StoreError {
    V3StoreError::InvalidDescriptor("unsupported or inconsistent native Weave contract".into())
}

pub(super) async fn validate(
    repo: &PgV3Repository,
    record: &WeaveReceiptRecord,
    closure: &VerifiedClosure,
    steps: &BTreeMap<StepId, Step>,
) -> V3StoreResult<()> {
    let profile = &record.determinism_profile;
    let ambient = &profile.host_policy.ambient_capabilities;
    let resolution = &profile.resolution_policy;
    let limits = &profile.limits;
    if profile.content_hash.version != "1"
        || profile.merge_algorithm.id != "layrs.native-tree-merge"
        || profile.merge_algorithm.version != "1"
        || profile.base_algorithm.id != "layrs.step-dag-bases"
        || profile.base_algorithm.version != "1"
        || profile.host_policy.version != "1"
        || ambient.network
        || ambient.wall_clock
        || ambient.randomness
        || ambient.locale
        || ambient.host_filesystem
        || ambient.environment
        || resolution.id != "layrs.structural"
        || resolution.version != "1"
        || resolution.digest != profile.host_policy.digest
        || resolution.allowed_methods != [ResolutionMethod::SelectTerm]
        || resolution.automatic_side_selection
        || resolution.combine_order != CombineOrder::TermIdAscii
        || resolution.manual_input != PermissionToggle::Forbidden
        || resolution.rename_resolution != PermissionToggle::Forbidden
        || [
            resolution.on_unavailable_lens,
            resolution.on_timeout,
            resolution.on_quota_exceeded,
            resolution.on_trap,
            resolution.on_invalid_output,
            resolution.on_invalid_signature,
            resolution.on_digest_mismatch,
        ]
        .iter()
        .any(|action| *action != ResolutionFailureAction::Reject)
        || !(1..=256 * 1024 * 1024).contains(&limits.max_input_bytes)
        || !(1..=256 * 1024 * 1024).contains(&limits.max_output_bytes)
        || !(1..=100_000).contains(&limits.max_paths)
        || !(1..=128).contains(&limits.max_recursion_depth)
        || !(1..=100_000).contains(&limits.max_blocks)
        || !(1..=200_000).contains(&limits.max_segments)
    {
        return Err(invalid());
    }
    let pinned: BTreeSet<_> = closure
        .conflict_documents
        .iter()
        .filter(|(_, c)| !c.document()["lensPin"].is_null())
        .map(|(id, _)| id.clone())
        .collect();
    if profile.host_policy.id == "layrs.no-lens-execution" {
        if !record.lens_executions.is_empty()
            || !pinned.is_empty()
            || limits.max_memory_bytes != 1
            || limits.max_execution_ms != 1
            || limits.max_fuel.is_some()
        {
            return Err(invalid());
        }
        return Ok(());
    }
    layrs_weave::wasm_host::validate_receipt_host(record).map_err(|_| invalid())?;
    let heads: BTreeSet<_> = record
        .source_heads
        .iter()
        .chain(&record.target_heads_before)
        .cloned()
        .collect();
    let bases: Vec<_> = record
        .merge_bases
        .iter()
        .map(|base| match base {
            MergeBase::Step { step_id, .. } => Ok(step_id.clone()),
            _ => Err(invalid()),
        })
        .collect::<V3StoreResult<_>>()?;
    // The supported text/code executor operates on exactly three real terms.
    if !record.lens_executions.is_empty() && (heads.len() != 2 || bases.len() != 1) {
        return Err(invalid());
    }
    let mut paths = BTreeSet::new();
    let mut generated = BTreeSet::new();
    for execution in &record.lens_executions {
        if !paths.insert(execution.path.clone()) || paths.len() as u64 > limits.max_paths {
            return Err(invalid());
        }
        let mut expected_roles = Vec::new();
        for (kind, origins) in [
            ("base", bases.clone()),
            ("side", heads.iter().cloned().collect()),
        ] {
            for (index, id) in origins.iter().enumerate() {
                let step = steps.get(id).ok_or(V3StoreError::NotFound)?;
                let entry = entry_at(repo, &record.space_id, step, execution.path.as_str()).await?;
                let term_id = term_id(kind, id, execution.path.as_str(), entry.as_ref())?;
                expected_roles.push(TermRole {
                    term_id,
                    role: if kind == "base" {
                        TermRoleKind::Base
                    } else if index == 0 {
                        TermRoleKind::Existing
                    } else {
                        TermRoleKind::Incoming
                    },
                });
            }
        }
        expected_roles.sort_by(|a, b| (&a.term_id, a.role).cmp(&(&b.term_id, b.role)));
        if expected_roles != execution.term_roles {
            return Err(invalid());
        }
        let result = steps
            .get(&record.result_step_id)
            .ok_or(V3StoreError::NotFound)?;
        let result_entry =
            entry_at(repo, &record.space_id, result, execution.path.as_str()).await?;
        if let Some(id) = execution.conflict_id.as_ref() {
            if !generated.insert(id.clone())
                || !pinned.contains(id)
                || !matches!(
                    execution.result,
                    LensExecutionResult::Conflicted | LensExecutionResult::StructuralFallback
                )
                || result_entry.as_ref().and_then(|e| e.target().conflict_id()) != Some(id)
            {
                return Err(invalid());
            }
            let doc = closure
                .conflict_documents
                .get(id)
                .ok_or_else(invalid)?
                .document();
            let terms: BTreeSet<_> = ["bases", "sides"]
                .into_iter()
                .flat_map(|role| {
                    doc[role]
                        .as_array()
                        .expect("validated terms")
                        .iter()
                        .map(|term| term["termId"].as_str().unwrap())
                })
                .collect();
            if doc["lensPin"] != serde_json::to_value(&execution.pin).map_err(|_| invalid())?
                || doc["lensImplementationDigest"] != execution.implementation_digest.as_str()
                || doc["placements"][0]["path"] != execution.path.as_str()
                || doc["resolutionState"] != "unresolved"
                || terms
                    != expected_roles
                        .iter()
                        .map(|role| role.term_id.as_str())
                        .collect()
                || doc["blocks"].as_array().unwrap().len() as u64 > limits.max_blocks
                || doc["segments"].as_array().unwrap().len() as u64 > limits.max_segments
            {
                return Err(invalid());
            }
        } else if execution.result != LensExecutionResult::AutoResolved
            || result_entry
                .as_ref()
                .and_then(|e| e.target().file_id())
                .is_none()
        {
            return Err(invalid());
        }
    }
    if generated != pinned {
        return Err(invalid());
    }
    Ok(())
}

fn digest(domain: &str, value: &Value) -> V3StoreResult<ObjectId> {
    let bytes = serde_jcs::to_vec(value).map_err(|_| invalid())?;
    let mut hash = blake3::Hasher::new();
    hash.update(domain.as_bytes());
    hash.update(&[0]);
    hash.update(&bytes);
    ObjectId::parse(format!("blake3:{}", hash.finalize())).map_err(|_| invalid())
}
fn term_id(
    kind: &str,
    step: &StepId,
    path: &str,
    entry: Option<&TreeEntry>,
) -> V3StoreResult<ObjectId> {
    let placement = digest(
        "layrs-conflict-placement-v1",
        &json!({"path":path,"role":"primary"}),
    )?;
    let value = match entry {
        None => json!({"kind":"absent"}),
        Some(entry) => match entry.target() {
            TreeEntryTarget::File { file_id } => {
                json!({"kind":"file","objectId":file_id,"executable":entry.executable()})
            }
            _ => return Err(invalid()),
        },
    };
    digest(
        "layrs-conflict-term-v1",
        &json!({"termKind":kind,"provenance":{"kind":"step","stepId":step},"values":[{"placementId":placement,"value":value}]}),
    )
}
