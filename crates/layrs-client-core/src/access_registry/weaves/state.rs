use super::*;
use super::weave_apply::step_origin_key;

pub(super) fn ensure_layer_known(
    handle: &LocalSpaceHandle,
    layer_id: &str,
) -> Result<(), String> {
    if handle
        .meta
        .layers
        .iter()
        .any(|layer| layer.layer_id == layer_id)
    {
        Ok(())
    } else {
        Err(format!("Layrs does not know Layer {layer_id}."))
    }
}

pub(super) fn sorted_steps(
    store: &LocalStoreSession,
    layrs_dir: &Path,
    layer_id: &str,
) -> Result<Vec<StepFile>, String> {
    let mut steps = read_step_files(store, layrs_dir, layer_id)?;
    steps.sort_by(compare_steps_by_timeline);
    Ok(steps)
}

pub(super) fn latest_state_for_layer(
    store: &LocalStoreSession,
    layrs_dir: &Path,
    layer_id: &str,
) -> Result<WorkingStateFile, String> {
    let steps = sorted_steps(store, layrs_dir, layer_id)?;
    if let Some(step) = steps.last() {
        return state_from_step(layrs_dir, step);
    }
    read_layer_state(store, layrs_dir, layer_id)
        .or_else(|_| read_layer_index(store, layrs_dir, layer_id))
}

pub(super) fn state_from_tree(
    layrs_dir: &Path,
    layer_id: &str,
    tree_id: Option<String>,
) -> Result<WorkingStateFile, String> {
    let Some(tree_id) = tree_id else {
        return Ok(WorkingStateFile {
            schema: WORKING_STATE_SCHEMA.to_string(),
            layer_id: layer_id.to_string(),
            captured_at_unix: unix_now(),
            root_tree_id: None,
            files: Vec::new(),
        });
    };
    let mut state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: layer_id.to_string(),
        captured_at_unix: unix_now(),
        root_tree_id: Some(tree_id),
        files: Vec::new(),
    };
    hydrate_state_files(layrs_dir, &mut state)?;
    Ok(state)
}

pub(super) fn target_has_origin(target_steps: &[StepFile], source_step: &StepFile) -> bool {
    let origin_layer_id = source_step
        .origin_layer_id
        .as_deref()
        .unwrap_or(source_step.layer_id.as_str());
    let origin_step_id = source_step
        .origin_step_id
        .as_deref()
        .unwrap_or(source_step.step_id.as_str());
    target_steps.iter().any(|target_step| {
        target_step
            .origin_layer_id
            .as_deref()
            .unwrap_or(target_step.layer_id.as_str())
            == origin_layer_id
            && target_step
                .origin_step_id
                .as_deref()
                .unwrap_or(target_step.step_id.as_str())
                == origin_step_id
    })
}

pub(super) fn changed_paths_between_states(
    left: &WorkingStateFile,
    right: &WorkingStateFile,
) -> Vec<String> {
    let (added, modified, deleted) = diff_state(Some(left), right);
    added
        .into_iter()
        .chain(modified)
        .chain(deleted)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn common_weave_base_state(
    handle: &LocalSpaceHandle,
    source_layer_id: &str,
    target_layer_id: &str,
) -> Option<WorkingStateFile> {
    let source_steps = sorted_steps(&handle.store, &handle.layrs_dir, source_layer_id).ok()?;
    let target_steps = sorted_steps(&handle.store, &handle.layrs_dir, target_layer_id).ok()?;
    let mut latest_common: Option<StepFile> = None;
    for source_step in &source_steps {
        if let Some(target_step) = target_steps
            .iter()
            .find(|target_step| step_origin_key(target_step) == step_origin_key(source_step))
        {
            latest_common = Some(target_step.clone());
        }
    }
    if let Some(step) = latest_common {
        return state_from_step(&handle.layrs_dir, &step).ok();
    }

    let source_layer = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == source_layer_id)?;
    if source_layer.parent_layer_id.as_deref() == Some(target_layer_id) {
        return read_layer_index(&handle.store, &handle.layrs_dir, target_layer_id).ok();
    }
    let target_layer = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == target_layer_id)?;
    if target_layer.parent_layer_id.as_deref() == Some(source_layer_id) {
        return read_layer_index(&handle.store, &handle.layrs_dir, source_layer_id).ok();
    }
    None
}

pub(super) fn is_safe_take_theirs(
    base: Option<&FileSnapshotEntry>,
    ours: Option<&FileSnapshotEntry>,
    theirs: Option<&FileSnapshotEntry>,
) -> bool {
    match (base, ours, theirs) {
        (_, ours, theirs) if file_entry_hash(ours) == file_entry_hash(theirs) => true,
        (base, ours, _) if file_entry_hash(base) == file_entry_hash(ours) => true,
        (base, _, theirs) if file_entry_hash(base) == file_entry_hash(theirs) => true,
        _ => false,
    }
}

pub(super) fn file_entry_hash(entry: Option<&FileSnapshotEntry>) -> Option<&str> {
    entry.map(|entry| entry.hash.as_str())
}

pub(super) fn apply_file_choice(
    state: &mut WorkingStateFile,
    path: &str,
    entry: Option<FileSnapshotEntry>,
) {
    state.files.retain(|file| file.path != path);
    if let Some(entry) = entry {
        state.files.push(entry);
        state
            .files
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
}
