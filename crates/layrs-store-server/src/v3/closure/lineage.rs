//! A new pinned Conflict comes from a validated Weave or a monotonic resolution
//! of the exact Conflict at the same path in one of its direct parent Steps.
use super::*;
use layrs_core::scm::Step;
use layrs_sync::v3::ProductEnvelope;
use serde_json::Value;

pub(crate) async fn validate(
    repo: &PgV3Repository,
    space: &SpaceId,
    new_steps: &[Step],
    steps: &BTreeMap<StepId, Step>,
    products: &[ProductEnvelope],
    closure: &mut VerifiedClosure,
) -> V3StoreResult<()> {
    if closure
        .conflict_documents
        .values()
        .all(|c| c.document()["lensPin"].is_null() && !c.is_multi_placement())
    {
        return Ok(());
    }
    let mut visited = 0usize;
    let mut byte_budget = 0;
    for descriptor in closure.objects.values() {
        charge_closure(&mut byte_budget, descriptor.canonical_byte_len)?;
    }
    for step in new_steps {
        let mut pending = vec![(String::new(), step.root_tree_id().clone(), 0usize)];
        while let Some((prefix, tree_id, depth)) = pending.pop() {
            visited += 1;
            if visited > 100_000 || depth > 128 {
                return Err(V3StoreError::InvalidDescriptor(
                    "Conflict lineage traversal limit".into(),
                ));
            }
            let tree = closure
                .trees
                .iter()
                .find(|t| t.id() == &tree_id)
                .ok_or(V3StoreError::NotFound)?;
            for entry in tree.entries() {
                visited += 1;
                if visited > 100_000 {
                    return Err(V3StoreError::InvalidDescriptor(
                        "Conflict lineage entry limit".into(),
                    ));
                }
                let path = if prefix.is_empty() {
                    entry.name().as_str().to_owned()
                } else {
                    format!("{prefix}/{}", entry.name().as_str())
                };
                if let TreeEntryTarget::Tree { tree_id } = entry.target() {
                    pending.push((path, tree_id.clone(), depth + 1));
                    continue;
                }
                let TreeEntryTarget::Conflict { conflict_id } = entry.target() else {
                    continue;
                };
                let conflict = closure
                    .conflict_documents
                    .get(conflict_id)
                    .ok_or(V3StoreError::NotFound)?;
                let document = conflict.document();
                if document["lensPin"].is_null() && !conflict.is_multi_placement() {
                    continue;
                }
                if !conflict
                    .anchor_paths()
                    .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?
                    .iter()
                    .any(|p| p.as_str() == path)
                {
                    return Err(V3StoreError::IdentityMismatch);
                }
                let generated = products.iter().any(|p| matches!(p, ProductEnvelope::Weave { record, .. }
                    if record.result_step_id == *step.id() && record.conflict_ids.contains(conflict_id)
                        && (!conflict.is_multi_placement() || super::renames::matches_receipt(conflict,record))));
                if generated {
                    continue;
                }
                let mut inherited = false;
                for parent in step.parent_step_ids() {
                    let parent_step = steps.get(parent).ok_or(V3StoreError::NotFound)?;
                    let Some(entry) = conflicts::entry_at(repo, space, parent_step, &path).await?
                    else {
                        continue;
                    };
                    let TreeEntryTarget::Conflict {
                        conflict_id: old_id,
                    } = entry.target()
                    else {
                        continue;
                    };
                    let id = ObjectId::parse(old_id.as_str()).expect("typed Conflict");
                    let descriptor = load_space_descriptor(repo.pool(), space, &id).await?;
                    if descriptor.kind != ObjectKind::Conflict {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                    charge_closure(&mut byte_budget, descriptor.canonical_byte_len)?;
                    let bytes =
                        read_canonical_object(repo.cas().as_ref(), &descriptor, MAX_OBJECT_BYTES)
                            .await?;
                    let old =
                        layrs_weave::graph_merge::PlannedConflict::from_canonical_bytes(&bytes)
                            .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
                    if monotonic(old.document(), document) {
                        if closure.objects.len() >= 100_000 && !closure.objects.contains_key(&id) {
                            return Err(V3StoreError::LimitExceeded {
                                resource: "Conflict lineage closure",
                                limit: 100_000,
                            });
                        }
                        closure.objects.insert(id, descriptor);
                        inherited = true;
                        break;
                    }
                }
                if !inherited {
                    return Err(V3StoreError::InvalidDescriptor(
                        "pinned Conflict needs a matching Weave or parent resolution".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn monotonic(old: &Value, new: &Value) -> bool {
    if old == new {
        return true;
    }
    let mut before = old.clone();
    let mut after = new.clone();
    for key in ["conflictId", "resolutionState"] {
        before.as_object_mut().unwrap().remove(key);
        after.as_object_mut().unwrap().remove(key);
    }
    let old_blocks = before["blocks"].as_array_mut().unwrap();
    let new_blocks = after["blocks"].as_array_mut().unwrap();
    if old_blocks.len() != new_blocks.len() || old_blocks.is_empty() {
        return false;
    }
    for (old, new) in old_blocks.iter_mut().zip(new_blocks) {
        if !old["resolution"].is_null() && old["resolution"] != new["resolution"] {
            return false;
        }
        old["resolution"] = Value::Null;
        new["resolution"] = Value::Null;
    }
    before == after
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_resolution_retains_origins_and_never_rewrites_a_decision() {
        let old = serde_json::json!({"conflictId":"a","resolutionState":"unresolved","originStepIds":["origin"],"blocks":[{"blockId":"one","resolution":null},{"blockId":"two","resolution":null}]});
        let mut new = old.clone();
        new["conflictId"] = "b".into();
        new["resolutionState"] = "partial".into();
        new["blocks"][0]["resolution"] = serde_json::json!({"termId":"base"});
        assert!(monotonic(&old, &new));
        let mut forged = new.clone();
        forged["originStepIds"][0] = "other".into();
        assert!(!monotonic(&old, &forged));
        forged = new.clone();
        forged["blocks"][0]["resolution"]["termId"] = "other".into();
        assert!(!monotonic(&new, &forged));
    }
}
