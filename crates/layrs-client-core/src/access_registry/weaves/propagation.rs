use super::*;
use super::weave_apply::{copy_weave_steps_to_target, unique_weave_id};
use super::weave_reconcile::{
    apply_conflict_marker_to_state, reconcile_path_with_lens_request, PathReconcileOutcome,
    PathReconcileRequest,
};
use super::weave_session::{
    proposed_state_path, write_active_weave_marker, write_weave_session,
};
use super::weave_state::{
    apply_file_choice, is_safe_take_theirs, latest_state_for_layer, sorted_steps,
    target_has_origin,
};

pub(super) fn inherit_parent_steps(
    layrs_dir: &Path,
    parent_layer_id: &str,
    child_layer_id: &str,
) -> Result<(), String> {
    let parent_steps = sorted_steps(layrs_dir, parent_layer_id)?;
    let planned = parent_steps
        .iter()
        .map(|step| step.step_id.clone())
        .collect::<Vec<_>>();
    let _ = copy_weave_steps_to_target(
        layrs_dir,
        &parent_steps,
        child_layer_id,
        &planned,
        "inherited",
    )?;
    Ok(())
}

pub(super) fn propagate_step_to_linked_children(
    handle: &mut LocalSpaceHandle,
    parent_layer_id: &str,
    source_step_id: &str,
) -> Result<(), String> {
    let source_step = read_step_file(&handle.layrs_dir, parent_layer_id, source_step_id)?;
    let children = handle
        .meta
        .layers
        .iter()
        .filter(|layer| {
            layer.parent_layer_id.as_deref() == Some(parent_layer_id)
                && layer.lineage_status == "linked"
        })
        .map(|layer| layer.layer_id.clone())
        .collect::<Vec<_>>();
    for child_layer_id in children {
        if let Some(propagated_step_id) =
            propagate_step_to_child(handle, &source_step, &child_layer_id)?
        {
            propagate_step_to_linked_children(handle, &child_layer_id, &propagated_step_id)?;
        }
    }
    Ok(())
}

fn propagate_step_to_child(
    handle: &mut LocalSpaceHandle,
    source_step: &StepFile,
    child_layer_id: &str,
) -> Result<Option<String>, String> {
    let child_steps = sorted_steps(&handle.layrs_dir, child_layer_id)?;
    if target_has_origin(&child_steps, source_step) {
        return Ok(child_steps
            .iter()
            .find(|step| {
                step.origin_layer_id
                    .as_deref()
                    .unwrap_or(step.layer_id.as_str())
                    == source_step
                        .origin_layer_id
                        .as_deref()
                        .unwrap_or(source_step.layer_id.as_str())
                    && step
                        .origin_step_id
                        .as_deref()
                        .unwrap_or(step.step_id.as_str())
                        == source_step
                            .origin_step_id
                            .as_deref()
                            .unwrap_or(source_step.step_id.as_str())
            })
            .map(|step| step.step_id.clone()));
    }
    let base_state =
        recorded_base_state_for_step(&handle.layrs_dir, source_step).unwrap_or_else(|| {
            WorkingStateFile {
                schema: WORKING_STATE_SCHEMA.to_string(),
                layer_id: source_step
                    .base_layer_id
                    .clone()
                    .unwrap_or_else(|| source_step.layer_id.clone()),
                captured_at_unix: source_step.captured_at_unix,
                root_tree_id: source_step.base_tree_id.clone(),
                files: Vec::new(),
            }
        });
    let mut base_state = base_state;
    hydrate_state_files(&handle.layrs_dir, &mut base_state)?;
    let source_state = state_from_step(&handle.layrs_dir, source_step)?;
    let mut child_state = latest_state_for_layer(&handle.layrs_dir, child_layer_id)?;
    child_state.layer_id = child_layer_id.to_string();
    let base_files = file_entries(&base_state.files);
    let source_files = file_entries(&source_state.files);
    let child_files = file_entries(&child_state.files);
    let mut conflicts = Vec::new();

    for path in &source_step.changed_paths {
        let base = base_files.get(path);
        let ours = child_files.get(path);
        let theirs = source_files.get(path);
        if is_safe_take_theirs(base, ours, theirs) {
            apply_file_choice(&mut child_state, path, theirs.cloned());
        } else {
            let weave_id =
                unique_weave_id(&handle.layrs_dir, &source_step.layer_id, child_layer_id);
            let pre_weave_target_tree_id = child_state.root_tree_id.clone();
            let outcome = reconcile_path_with_lens_request(
                handle,
                PathReconcileRequest {
                    weave_id: &weave_id,
                    target_layer_id: child_layer_id,
                    source_layer_id: &source_step.layer_id,
                    path,
                    base,
                    ours,
                    theirs,
                },
            )?;
            let conflict = match outcome {
                PathReconcileOutcome::AutoResolved(entry) => {
                    apply_file_choice(&mut child_state, path, entry);
                    continue;
                }
                PathReconcileOutcome::Conflicted(conflict) => conflict,
            };
            apply_conflict_marker_to_state(handle, &mut child_state, &weave_id, &conflict, ours)?;
            child_state.root_tree_id =
                Some(write_tree_object(&handle.layrs_dir, &child_state.files)?);
            write_json(
                &proposed_state_path(&handle.layrs_dir, &weave_id),
                &storage_state(&handle.layrs_dir, &child_state)?,
            )?;
            conflicts.push(conflict);
            let session = WeaveSessionFile {
                schema: WEAVE_SESSION_SCHEMA.to_string(),
                weave_id: weave_id.clone(),
                source_layer_id: source_step.layer_id.clone(),
                target_layer_id: child_layer_id.to_string(),
                status: "conflicted".to_string(),
                pre_weave_target_tree_id,
                pre_weave_target_step_id: child_steps.last().map(|step| step.step_id.clone()),
                planned_steps: vec![source_step.step_id.clone()],
                applied_steps: Vec::new(),
                conflicts,
                created_at_unix: unix_now(),
                updated_at_unix: unix_now(),
            };
            write_weave_session(&handle.layrs_dir, &session)?;
            write_active_weave_marker(&handle.layrs_dir, Some(&weave_id))?;
            return Ok(None);
        }
    }

    child_state.root_tree_id = Some(write_tree_object(&handle.layrs_dir, &child_state.files)?);
    write_layer_state(&handle.layrs_dir, child_layer_id, &child_state)?;
    let step_id = unique_step_id(&handle.layrs_dir, child_layer_id);
    let step = StepFile {
        schema: STEP_SCHEMA.to_string(),
        step_id: step_id.clone(),
        layer_id: child_layer_id.to_string(),
        parent_step_id: child_steps.last().map(|step| step.step_id.clone()),
        base_layer_id: Some(child_layer_id.to_string()),
        base_tree_id: child_steps
            .last()
            .and_then(|step| step.root_tree_id.clone())
            .or_else(|| {
                read_layer_index(&handle.layrs_dir, child_layer_id)
                    .ok()
                    .and_then(|state| state.root_tree_id)
            }),
        root_tree_id: child_state.root_tree_id.clone(),
        changed_paths: source_step.changed_paths.clone(),
        timeline_position: Some(next_timeline_position(&handle.layrs_dir, child_layer_id)?),
        origin_layer_id: Some(
            source_step
                .origin_layer_id
                .clone()
                .unwrap_or_else(|| source_step.layer_id.clone()),
        ),
        origin_layer_name: source_step
            .origin_layer_name
            .clone()
            .or_else(|| step_layer_display_name(&handle.layrs_dir, &source_step.layer_id)),
        origin_step_id: Some(
            source_step
                .origin_step_id
                .clone()
                .unwrap_or_else(|| source_step.step_id.clone()),
        ),
        step_kind: Some("inherited".to_string()),
        captured_at_unix: unix_now(),
        files: Vec::new(),
    };
    write_json(
        &layer_dir(&handle.layrs_dir, child_layer_id)
            .join("steps")
            .join(format!("{step_id}.json")),
        &step,
    )?;
    Ok(Some(step_id))
}
