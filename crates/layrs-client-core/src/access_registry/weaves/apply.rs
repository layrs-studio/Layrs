use super::*;
use super::weave_session::{read_weave_session, weave_dir, write_weave_session};
use super::weave_state::{apply_file_choice, sorted_steps};

pub(super) fn apply_completed_weave(
    handle: &mut LocalSpaceHandle,
    session: &WeaveSessionFile,
    proposed_state: &WorkingStateFile,
    source_steps: &[StepFile],
) -> Result<(), String> {
    let target_layer_id = session.target_layer_id.as_str();
    write_layer_state(&handle.layrs_dir, target_layer_id, proposed_state)?;
    if handle.active.layer_id == target_layer_id {
        materialize_state(&handle.root, proposed_state)?;
    }
    let copied_steps = if layers_are_linked(
        &handle.meta,
        &session.source_layer_id,
        &session.target_layer_id,
    ) {
        weave_steps_into_linked_target(
            &handle.layrs_dir,
            source_steps,
            target_layer_id,
            &session.planned_steps,
            "woven",
        )?
    } else {
        copy_weave_steps_to_target(
            &handle.layrs_dir,
            source_steps,
            target_layer_id,
            &session.planned_steps,
            "woven",
        )?
    };
    for step_id in &copied_steps {
        let step = read_step_file(&handle.layrs_dir, target_layer_id, step_id)?;
        write_pending_publish(&handle.layrs_dir, &step)?;
    }
    let mut saved = read_weave_session(&handle.layrs_dir, &session.weave_id).unwrap_or_else(|_| {
        let mut session = session.clone();
        session.applied_steps = copied_steps.clone();
        session
    });
    saved.applied_steps = copied_steps;
    saved.status = "applied".to_string();
    saved.updated_at_unix = unix_now();
    write_weave_session(&handle.layrs_dir, &saved)?;
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    remember_local_space(&handle.meta, Some(target_layer_id.to_string()))?;
    Ok(())
}

fn layers_are_linked(meta: &LocalSpaceFile, source_layer_id: &str, target_layer_id: &str) -> bool {
    let source_parent = meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == source_layer_id)
        .and_then(|layer| layer.parent_layer_id.as_deref());
    let target_parent = meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == target_layer_id)
        .and_then(|layer| layer.parent_layer_id.as_deref());
    source_parent == Some(target_layer_id) || target_parent == Some(source_layer_id)
}

fn weave_steps_into_linked_target(
    layrs_dir: &Path,
    source_steps: &[StepFile],
    target_layer_id: &str,
    planned_steps: &[String],
    step_kind: &str,
) -> Result<Vec<String>, String> {
    let planned = planned_steps.iter().collect::<BTreeSet<_>>();
    let target_steps = sorted_steps(layrs_dir, target_layer_id)?;
    let mut target_by_origin = BTreeMap::new();
    for target_step in target_steps {
        target_by_origin.insert(step_origin_key(&target_step), target_step);
    }

    let mut copied = Vec::new();
    let mut ordered_steps = Vec::new();
    for source in source_steps {
        let key = step_origin_key(source);
        if let Some(target_step) = target_by_origin.remove(&key) {
            ordered_steps.push(target_step);
        } else if planned.contains(&source.step_id) {
            let step_id = unique_step_id(layrs_dir, target_layer_id);
            copied.push(step_id.clone());
            ordered_steps.push(StepFile {
                schema: STEP_SCHEMA.to_string(),
                step_id,
                layer_id: target_layer_id.to_string(),
                parent_step_id: None,
                base_layer_id: Some(target_layer_id.to_string()),
                base_tree_id: source.base_tree_id.clone(),
                root_tree_id: source.root_tree_id.clone(),
                changed_paths: source.changed_paths.clone(),
                timeline_position: None,
                origin_layer_id: Some(
                    source
                        .origin_layer_id
                        .clone()
                        .unwrap_or_else(|| source.layer_id.clone()),
                ),
                origin_layer_name: source
                    .origin_layer_name
                    .clone()
                    .or_else(|| step_layer_display_name(layrs_dir, &source.layer_id)),
                origin_step_id: Some(
                    source
                        .origin_step_id
                        .clone()
                        .unwrap_or_else(|| source.step_id.clone()),
                ),
                step_kind: Some(step_kind.to_string()),
                captured_at_unix: unix_now(),
                files: Vec::new(),
            });
        }
    }

    let mut remaining_target_steps = target_by_origin.into_values().collect::<Vec<_>>();
    remaining_target_steps.sort_by(compare_steps_by_timeline);
    ordered_steps.extend(remaining_target_steps);

    rewrite_linked_target_step_order(layrs_dir, target_layer_id, ordered_steps)?;
    Ok(copied)
}

fn rewrite_linked_target_step_order(
    layrs_dir: &Path,
    target_layer_id: &str,
    ordered_steps: Vec<StepFile>,
) -> Result<(), String> {
    let mut previous_state: Option<WorkingStateFile> = None;
    let mut previous_step_id: Option<String> = None;

    for (index, mut step) in ordered_steps.into_iter().enumerate() {
        let base_state = previous_state
            .clone()
            .or_else(|| recorded_base_state_for_step(layrs_dir, &step))
            .or_else(|| read_layer_index(layrs_dir, target_layer_id).ok())
            .unwrap_or_else(|| WorkingStateFile {
                schema: WORKING_STATE_SCHEMA.to_string(),
                layer_id: target_layer_id.to_string(),
                captured_at_unix: unix_now(),
                root_tree_id: None,
                files: Vec::new(),
            });
        let replayed_state = replay_step_on_state(layrs_dir, target_layer_id, &base_state, &step)?;
        step.layer_id = target_layer_id.to_string();
        step.parent_step_id = previous_step_id.clone();
        step.base_layer_id = Some(target_layer_id.to_string());
        step.base_tree_id = base_state.root_tree_id.clone();
        step.root_tree_id = replayed_state.root_tree_id.clone();
        step.timeline_position = Some((index + 1) as u64);
        write_json(
            &layer_dir(layrs_dir, target_layer_id)
                .join("steps")
                .join(format!("{}.json", step.step_id)),
            &step,
        )?;
        previous_step_id = Some(step.step_id);
        previous_state = Some(replayed_state);
    }

    Ok(())
}

fn replay_step_on_state(
    layrs_dir: &Path,
    layer_id: &str,
    base_state: &WorkingStateFile,
    step: &StepFile,
) -> Result<WorkingStateFile, String> {
    let source_state = state_from_step(layrs_dir, step)?;
    let source_files = file_entries(&source_state.files);
    let mut replayed = base_state.clone();
    replayed.layer_id = layer_id.to_string();
    replayed.captured_at_unix = unix_now();
    for path in &step.changed_paths {
        apply_file_choice(&mut replayed, path, source_files.get(path).cloned());
    }
    replayed.root_tree_id = Some(write_tree_object(layrs_dir, &replayed.files)?);
    Ok(replayed)
}

pub(super) fn step_origin_key(step: &StepFile) -> (String, String) {
    (
        step.origin_layer_id
            .clone()
            .unwrap_or_else(|| step.layer_id.clone()),
        step.origin_step_id
            .clone()
            .unwrap_or_else(|| step.step_id.clone()),
    )
}

pub(super) fn copy_weave_steps_to_target(
    layrs_dir: &Path,
    source_steps: &[StepFile],
    target_layer_id: &str,
    planned_steps: &[String],
    step_kind: &str,
) -> Result<Vec<String>, String> {
    let planned = planned_steps.iter().collect::<BTreeSet<_>>();
    let mut copied = Vec::new();
    let mut position = next_timeline_position(layrs_dir, target_layer_id)?;
    for source in source_steps {
        if !planned.contains(&source.step_id) {
            continue;
        }
        let step_id = unique_step_id(layrs_dir, target_layer_id);
        let target_step = StepFile {
            schema: STEP_SCHEMA.to_string(),
            step_id: step_id.clone(),
            layer_id: target_layer_id.to_string(),
            parent_step_id: None,
            base_layer_id: Some(target_layer_id.to_string()),
            base_tree_id: source.base_tree_id.clone(),
            root_tree_id: source.root_tree_id.clone(),
            changed_paths: source.changed_paths.clone(),
            timeline_position: Some(position),
            origin_layer_id: Some(
                source
                    .origin_layer_id
                    .clone()
                    .unwrap_or_else(|| source.layer_id.clone()),
            ),
            origin_layer_name: source
                .origin_layer_name
                .clone()
                .or_else(|| step_layer_display_name(layrs_dir, &source.layer_id)),
            origin_step_id: Some(
                source
                    .origin_step_id
                    .clone()
                    .unwrap_or_else(|| source.step_id.clone()),
            ),
            step_kind: Some(step_kind.to_string()),
            captured_at_unix: unix_now(),
            files: Vec::new(),
        };
        write_json(
            &layer_dir(layrs_dir, target_layer_id)
                .join("steps")
                .join(format!("{step_id}.json")),
            &target_step,
        )?;
        copied.push(step_id);
        position = position.saturating_add(1);
    }
    Ok(copied)
}

pub(super) fn unique_weave_id(
    layrs_dir: &Path,
    source_layer_id: &str,
    target_layer_id: &str,
) -> String {
    let base = format!(
        "weave-{}-{}-{}",
        safe_id_fragment(source_layer_id),
        safe_id_fragment(target_layer_id),
        unix_now()
    );
    let mut candidate = base.clone();
    let mut attempt = 2;
    while weave_dir(layrs_dir, &candidate).exists() {
        candidate = format!("{base}-{attempt}");
        attempt += 1;
    }
    candidate
}
