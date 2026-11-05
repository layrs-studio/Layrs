//! Remote canonical hashes do not establish the truth of rename provenance.
use super::*;
use layrs_weave::graph_merge::{
    validate_conflict_layout, GraphMergeLimits, MergeObjects, PlannedConflict,
};

pub(super) fn validate(graph: &GraphObjects) -> RemoteResult<()> {
    let mut objects = MergeObjects {
        steps: graph.steps.clone(),
        ..MergeObjects::default()
    };
    for object in graph.objects.values() {
        match object {
            CanonicalObject::Tree(tree) => {
                objects.trees.insert(tree.id().clone(), tree.clone());
            }
            CanonicalObject::Conflict(conflict) => {
                let parsed = PlannedConflict::from_canonical_bytes(conflict.canonical_json())
                    .map_err(|_| NativeRemoteError::Invalid("invalid remote Conflict"))?;
                objects.conflicts.insert(parsed.id(), parsed);
            }
            _ => {}
        }
    }
    if !objects.conflicts.values().any(|c| c.is_multi_placement()) {
        return Ok(());
    }
    let limits = GraphMergeLimits::default();
    if objects.steps.len() > limits.max_steps {
        return Err(NativeRemoteError::Invalid("rename source Step budget"));
    }
    for conflict in objects
        .conflicts
        .values()
        .filter(|c| c.is_multi_placement())
    {
        conflict
            .validate_rename_context(&objects, limits)
            .map_err(|_| {
                NativeRemoteError::Invalid("remote rename differs from original Trees or ancestry")
            })?;
    }
    let roots = objects
        .steps
        .values()
        .map(|s| s.root_tree_id().clone())
        .collect::<BTreeSet<_>>();
    validate_conflict_layout(&objects, &roots.into_iter().collect::<Vec<_>>(), limits).map_err(
        |_| {
            NativeRemoteError::Invalid(
                "remote rename anchor occurrences are incomplete or misplaced",
            )
        },
    )
}
