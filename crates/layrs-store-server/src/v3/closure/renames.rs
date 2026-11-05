//! Reproduce exact rename policy from authorized immutable source Trees.
//! Shape/hash validation alone never permits a coupled placement reference.
use super::*;
use layrs_core::scm::{Step, TreeId};
use layrs_weave::graph_merge::{GraphMergeLimits, MergeObjects, validate_conflict_layout};

pub(super) fn matches_receipt(
    conflict: &layrs_weave::graph_merge::PlannedConflict,
    record: &layrs_sync::v3::WeaveReceiptRecord,
) -> bool {
    use layrs_sync::v3::MergeBase;
    let origins = |role: &str| {
        conflict.document()[role]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["provenance"]["stepId"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
    };
    let sides = record
        .source_heads
        .iter()
        .chain(&record.target_heads_before)
        .map(StepId::as_str)
        .collect::<BTreeSet<_>>();
    let bases = record
        .merge_bases
        .iter()
        .filter_map(|b| match b {
            MergeBase::Step { step_id, .. } => Some(step_id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    origins("sides") == sides
        && origins("bases") == bases
        && bases.len() == record.merge_bases.len()
}

pub(super) async fn validate(
    repo: &PgV3Repository,
    space: &SpaceId,
    closure: &mut VerifiedClosure,
    steps: &BTreeMap<StepId, Step>,
    roots: &BTreeSet<TreeId>,
    byte_budget: &mut u64,
) -> V3StoreResult<()> {
    if !closure
        .conflict_documents
        .values()
        .any(|c| c.is_multi_placement())
    {
        return Ok(());
    }
    let limits = GraphMergeLimits::default();
    if steps.len() > limits.max_steps {
        return Err(V3StoreError::InvalidDescriptor(
            "rename source Step budget".into(),
        ));
    }
    let mut objects = MergeObjects {
        steps: steps.clone(),
        trees: closure
            .trees
            .iter()
            .map(|t| (t.id().clone(), t.clone()))
            .collect(),
        conflicts: closure.conflict_documents.clone(),
        ..MergeObjects::default()
    };
    let mut pending = Vec::new();
    for conflict in objects
        .conflicts
        .values()
        .filter(|c| c.is_multi_placement())
    {
        for id in conflict.document()["originStepIds"].as_array().unwrap() {
            let id =
                StepId::parse(id.as_str().unwrap()).map_err(|_| V3StoreError::IdentityMismatch)?;
            pending.push((
                steps
                    .get(&id)
                    .ok_or(V3StoreError::NotFound)?
                    .root_tree_id()
                    .clone(),
                0usize,
            ));
        }
    }
    let mut seen = BTreeSet::new();
    let mut paths = 0usize;
    while let Some((id, depth)) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if depth > limits.max_depth || seen.len() > limits.max_paths {
            return Err(V3StoreError::InvalidDescriptor(
                "rename source Tree budget".into(),
            ));
        }
        if !objects.trees.contains_key(&id) {
            let object = ObjectId::parse(id.as_str()).expect("typed Tree");
            let descriptor = load_space_descriptor(repo.pool(), space, &object).await?;
            if descriptor.kind != ObjectKind::Tree {
                return Err(V3StoreError::IdentityMismatch);
            }
            charge_closure(byte_budget, descriptor.canonical_byte_len)?;
            let bytes =
                read_canonical_object(repo.cas().as_ref(), &descriptor, MAX_OBJECT_BYTES).await?;
            let tree: Tree = serde_json::from_slice(&bytes)
                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
            if tree.id() != &id {
                return Err(V3StoreError::IdentityMismatch);
            }
            objects.trees.insert(id.clone(), tree);
        }
        let tree = &objects.trees[&id];
        paths = paths.saturating_add(tree.entries().len());
        if paths > limits.max_paths {
            return Err(V3StoreError::InvalidDescriptor(
                "rename source path budget".into(),
            ));
        }
        for entry in tree.entries() {
            if let Some(child) = entry.target().tree_id() {
                pending.push((child.clone(), depth + 1));
            }
        }
    }
    // Origin snapshots are proof input, not additional live result roots.
    // Their old Conflict objects must not be added to the Weave conflict set.
    for conflict in objects
        .conflicts
        .values()
        .filter(|c| c.is_multi_placement())
    {
        conflict
            .validate_rename_context(&objects, limits)
            .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
    }
    validate_conflict_layout(&objects, &roots.iter().cloned().collect::<Vec<_>>(), limits)
        .map_err(|e| V3StoreError::CorruptObject(e.to_string()))
}
