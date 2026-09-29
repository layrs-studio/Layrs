//! Explicit delta application. The chosen source parent is the base even when
//! target history has a different merge base. No synthetic ancestry is created.
use super::*;

pub fn plan_replay(
    objects: &MergeObjects,
    source: &StepId,
    selected_parent: Option<&StepId>,
    target: &StepId,
    inverse: bool,
    file_policy: FileMergePolicy,
    limits: GraphMergeLimits,
) -> MergeResult<InstallationBundle> {
    if objects.steps.len() > limits.max_steps {
        return Err(GraphMergeError::Limit("replay steps"));
    }
    let source_step = objects
        .steps
        .get(source)
        .ok_or_else(|| GraphMergeError::Missing(source.to_string()))?;
    let parent = match (source_step.parent_step_ids(), selected_parent) {
        ([], None) if !inverse => None,
        ([parent], None) => Some(parent),
        (parents, Some(parent)) if parents.contains(parent) => Some(parent),
        _ => {
            return Err(invalid(
                "replay requires an explicit actual parent for a merge; root revert is unsupported",
            ));
        }
    };
    if !objects.steps.contains_key(target) {
        return Err(GraphMergeError::Missing(target.to_string()));
    }
    let (bases, incoming) = if inverse {
        (
            vec![source.clone()],
            parent
                .ok_or_else(|| invalid("root revert unsupported"))?
                .clone(),
        )
    } else {
        (
            parent.cloned().into_iter().collect::<Vec<_>>(),
            source.clone(),
        )
    };
    let mut sides = vec![incoming.clone(), target.clone()];
    sides.sort();
    sides.dedup();
    let mut input_bytes = 0usize;
    for (id, step) in &objects.steps {
        if id != step.id() {
            return Err(invalid("Step map key mismatch"));
        }
        input_bytes = input_bytes
            .checked_add(step.identity_bytes().len())
            .ok_or(GraphMergeError::Limit("replay bytes"))?;
    }
    if input_bytes > limits.max_input_bytes {
        return Err(GraphMergeError::Limit("replay bytes"));
    }
    StepDag::from_steps(objects.steps.values()).map_err(invalid)?;
    let mut validator = InputValidator {
        objects,
        limits,
        paths: 0,
        bytes: input_bytes,
        seen_files: BTreeSet::new(),
        seen_metadata: BTreeSet::new(),
        validated_conflicts: BTreeSet::new(),
    };
    for id in sides.iter().chain(&bases) {
        let step = objects
            .steps
            .get(id)
            .ok_or_else(|| GraphMergeError::Missing(id.to_string()))?;
        validator.tree(step.root_tree_id(), "", 0)?;
    }
    let placeholder = Step::new(
        vec![target.clone()],
        objects.steps[target].root_tree_id().clone(),
    )
    .map_err(invalid)?;
    let mut planner = Planner {
        objects,
        executor: None,
        plan: GraphMergePlan {
            source_heads: vec![incoming],
            target_heads: vec![target.clone()],
            merge_bases: bases.clone(),
            step: placeholder,
            trees: BTreeMap::new(),
            files: BTreeMap::new(),
            conflicts: BTreeMap::new(),
            executions: vec![],
            file_policy,
            limits,
            semantic_execution: true,
            enforced_host: None,
        },
        paths: 0,
        output_bytes: 0,
        retained_trees: BTreeSet::new(),
        rename_overrides: BTreeMap::new(),
    };
    let roots = |ids: &[StepId]| {
        ids.iter()
            .map(|id| (id.clone(), Some(objects.steps[id].root_tree_id().clone())))
            .collect::<Vec<_>>()
    };
    // A replay delta base need not be the sides' DAG merge base. Do not emit
    // a rename Conflict claiming Weave ancestry for that different operation.
    if bases.len() == 1
        && sides.len() == 2
        && !renames::analyze_renames(objects, &bases[0], &sides, limits)?
            .conflicts
            .is_empty()
    {
        return Err(GraphMergeError::Unsupported(
            "resolve coupled movements in a Weave before replay".into(),
        ));
    }
    let root = planner.tree("", &roots(&bases), &roots(&sides), 0)?;
    planner.plan.step = Step::new(vec![target.clone()], root.clone()).map_err(invalid)?;
    planner.retain_conflicts(&root, 0)?;
    let bundle = planner.plan.installation_bundle()?;
    if bundle.canonical_bytes()?.len() > limits.max_output_bytes {
        return Err(GraphMergeError::Limit("replay output bytes"));
    }
    Ok(bundle)
}
