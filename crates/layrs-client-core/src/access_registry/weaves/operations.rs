use super::weave_apply::{apply_completed_weave, unique_weave_id};
use super::weave_reconcile::{
    apply_conflict_marker_to_state, reconcile_path_with_lens_request,
    resolve_file_conflict_with_lens, PathReconcileOutcome, PathReconcileRequest,
};
use super::weave_resolution::{
    assemble_text_conflict_resolution, parse_block_resolution, resolve_text_conflict_block,
    validate_block_resolution_method, validate_file_resolution_method,
};
use super::weave_session::{
    active_weave_id, conflict_dir, ensure_local_weave_target_is_current,
    ensure_sync_weave_plan_is_current, proposed_state_path, read_weave_blob, read_weave_session,
    summarize_weave_session, write_active_weave_marker, write_weave_blob, write_weave_session,
};
use super::weave_state::{
    apply_file_choice, changed_paths_between_states, common_weave_base_state, ensure_layer_known,
    is_safe_take_theirs, latest_state_for_layer, sorted_steps, state_from_tree, target_has_origin,
};
use super::*;
use layrs_lens_sdk::ResolutionMethod;

pub fn weave_layers(
    local_space: String,
    source_layer_id: String,
    target_layer_id: String,
    preview: bool,
) -> Result<WeaveOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let source_layer_id = source_layer_id.trim().to_string();
    let target_layer_id = target_layer_id.trim().to_string();
    ensure_layer_known(&handle, &source_layer_id)?;
    ensure_layer_known(&handle, &target_layer_id)?;
    if source_layer_id == target_layer_id {
        return Err("Choose two different Layers to Weave.".to_string());
    }
    if active_weave_id(&handle.store, &handle.layrs_dir)?.is_some() {
        return Err(
            "A Weave is already active. Continue or abort it before starting another Weave."
                .to_string(),
        );
    }

    let source_state = latest_state_for_layer(&handle.store, &handle.layrs_dir, &source_layer_id)?;
    let target_state = latest_state_for_layer(&handle.store, &handle.layrs_dir, &target_layer_id)?;
    let source_steps = sorted_steps(&handle.store, &handle.layrs_dir, &source_layer_id)?;
    let target_steps = sorted_steps(&handle.store, &handle.layrs_dir, &target_layer_id)?;
    let planned_steps = source_steps
        .iter()
        .filter(|source_step| !target_has_origin(&target_steps, source_step))
        .map(|step| step.step_id.clone())
        .collect::<Vec<_>>();

    let weave_id = unique_weave_id(
        &handle.store,
        &handle.layrs_dir,
        &source_layer_id,
        &target_layer_id,
    )?;
    let now = unix_now();
    let pre_step_id = target_steps.last().map(|step| step.step_id.clone());
    let mut session = WeaveSessionFile {
        schema: WEAVE_SESSION_SCHEMA.to_string(),
        weave_id: weave_id.clone(),
        source_layer_id: source_layer_id.clone(),
        target_layer_id: target_layer_id.clone(),
        status: if preview { "preview" } else { "applying" }.to_string(),
        pre_weave_target_tree_id: target_state.root_tree_id.clone(),
        pre_weave_target_step_id: pre_step_id,
        planned_steps,
        applied_steps: Vec::new(),
        conflicts: Vec::new(),
        created_at_unix: now,
        updated_at_unix: now,
    };

    let mut proposed_state = target_state.clone();
    proposed_state.layer_id = target_layer_id.clone();
    let target_files = file_entries(&target_state.files);
    let source_files = file_entries(&source_state.files);
    let mut conflicts = Vec::new();
    let paths = changed_paths_between_states(&target_state, &source_state);
    let base_state = common_weave_base_state(&handle, &source_layer_id, &target_layer_id)
        .unwrap_or_else(|| target_state.clone());
    let base_files = file_entries(&base_state.files);

    for path in paths {
        let base = base_files.get(&path);
        let ours = target_files.get(&path);
        let theirs = source_files.get(&path);
        if is_safe_take_theirs(base, ours, theirs) {
            apply_file_choice(&mut proposed_state, &path, theirs.cloned());
            continue;
        }
        match reconcile_path_with_lens_request(
            &mut handle,
            PathReconcileRequest {
                weave_id: &weave_id,
                target_layer_id: &target_layer_id,
                source_layer_id: &source_layer_id,
                path: &path,
                base,
                ours,
                theirs,
            },
        )? {
            PathReconcileOutcome::AutoResolved(entry) => {
                apply_file_choice(&mut proposed_state, &path, entry);
            }
            PathReconcileOutcome::Conflicted(conflict) => {
                apply_conflict_marker_to_state(
                    &handle,
                    &mut proposed_state,
                    &weave_id,
                    &conflict,
                    ours,
                )?;
                conflicts.push(conflict);
            }
        }
    }

    proposed_state.root_tree_id =
        Some(write_tree_object(&handle.layrs_dir, &proposed_state.files)?);
    session.conflicts = conflicts;
    session.status = if preview {
        "preview".to_string()
    } else if session.conflicts.is_empty() {
        "applied".to_string()
    } else {
        "conflicted".to_string()
    };
    session.updated_at_unix = unix_now();

    if preview {
        return Ok(WeaveOperationResult {
            local_space: summary_from_handle(&handle),
            session: summarize_weave_session(&session),
            message: format!(
                "Weave preview found {} conflict(s).",
                session.conflicts.len()
            ),
        });
    }

    if handle.active.layer_id == target_layer_id {
        let current_state =
            capture_working_state(&mut handle.store, &handle.root, &target_layer_id)?;
        let changed_files = changed_file_count(Some(&target_state), &current_state);
        if changed_files > 0 {
            let step_id = write_step(
                &mut handle.store,
                &handle.layrs_dir,
                &target_layer_id,
                &current_state,
            )?;
            let step =
                read_step_file(&handle.store, &handle.layrs_dir, &target_layer_id, &step_id)?;
            write_pending_publish(&mut handle.store, &handle.layrs_dir, &step)?;
        }
    }

    write_json(
        &mut handle.store,
        &proposed_state_path(&handle.layrs_dir, &weave_id),
        &storage_state(&handle.layrs_dir, &proposed_state)?,
    )?;
    write_weave_session(&mut handle.store, &handle.layrs_dir, &session)?;

    if session.conflicts.is_empty() {
        apply_completed_weave(&mut handle, &session, &proposed_state, &source_steps)?;
        write_active_weave_marker(&mut handle.store, &handle.layrs_dir, None)?;
        let mut applied = read_weave_session(&handle.store, &handle.layrs_dir, &weave_id)?;
        applied.status = "applied".to_string();
        applied.updated_at_unix = unix_now();
        write_weave_session(&mut handle.store, &handle.layrs_dir, &applied)?;
        Ok(WeaveOperationResult {
            local_space: summary_from_handle(&handle),
            session: summarize_weave_session(&applied),
            message: "Weave applied.".to_string(),
        })
    } else {
        if handle.active.layer_id == target_layer_id {
            materialize_state(&mut handle.store, &handle.root, &proposed_state)?;
        }
        write_active_weave_marker(&mut handle.store, &handle.layrs_dir, Some(&weave_id))?;
        Ok(WeaveOperationResult {
            local_space: summary_from_handle(&handle),
            session: summarize_weave_session(&session),
            message: format!(
                "Weave paused with {} conflict(s). Resolve them, then continue or abort.",
                session.conflicts.len()
            ),
        })
    }
}

pub fn weave_status(local_space: String) -> Result<Option<WeaveSessionSummary>, String> {
    let handle = open_local_space_handle(&local_space)?;
    let Some(weave_id) = active_weave_id(&handle.store, &handle.layrs_dir)? else {
        return Ok(None);
    };
    Ok(Some(summarize_weave_session(&read_weave_session(
        &handle.store,
        &handle.layrs_dir,
        &weave_id,
    )?)))
}

pub fn weave_conflicts(local_space: String) -> Result<Vec<WeaveConflictSummary>, String> {
    Ok(weave_status(local_space)?
        .map(|session| session.conflicts)
        .unwrap_or_default())
}

pub fn abort_weave(local_space: String) -> Result<WeaveOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let weave_id = active_weave_id(&handle.store, &handle.layrs_dir)?
        .ok_or_else(|| "No active Weave to abort.".to_string())?;
    let mut session = read_weave_session(&handle.store, &handle.layrs_dir, &weave_id)?;
    let target_state = state_from_tree(
        &handle.layrs_dir,
        &session.target_layer_id,
        session.pre_weave_target_tree_id.clone(),
    )?;
    if handle.active.layer_id == session.target_layer_id {
        materialize_state(&mut handle.store, &handle.root, &target_state)?;
    }
    session.status = "aborted".to_string();
    session.updated_at_unix = unix_now();
    write_weave_session(&mut handle.store, &handle.layrs_dir, &session)?;
    write_active_weave_marker(&mut handle.store, &handle.layrs_dir, None)?;
    Ok(WeaveOperationResult {
        local_space: summary_from_handle(&handle),
        session: summarize_weave_session(&session),
        message: "Weave aborted. Pre-Weave working tree restored.".to_string(),
    })
}

/// Read a stored variant through the legacy metadata authority, not loose JSON.
pub fn read_weave_conflict_variant(
    local_space: String,
    path: String,
    side: &str,
) -> Result<Vec<u8>, String> {
    if !matches!(side, "base" | "ours" | "theirs" | "resolved") {
        return Err("Unknown conflict variant".into());
    }
    let handle = open_local_space_handle(&local_space)?;
    let id = active_weave_id(&handle.store, &handle.layrs_dir)?.ok_or("No active Weave")?;
    let session = read_weave_session(&handle.store, &handle.layrs_dir, &id)?;
    let conflict = session
        .conflicts
        .iter()
        .find(|c| c.path == path || c.conflict_id == path)
        .ok_or("No matching conflict")?;
    read_weave_blob(
        &handle.store,
        &conflict_dir(&handle.layrs_dir, &id, &conflict.conflict_id).join(side),
    )
}

/// Explicit whole-file replacement; it does not claim Lens reconciliation.
pub fn resolve_weave_conflict_bytes(
    local_space: String,
    path: String,
    bytes: &[u8],
    resolution: &str,
) -> Result<WeaveOperationResult, String> {
    if bytes.len() > 64 * 1024 * 1024 || !matches!(resolution, "file" | "base") {
        return Err("Invalid bounded replacement".into());
    }
    let mut handle = open_local_space_handle(&local_space)?;
    let id = active_weave_id(&handle.store, &handle.layrs_dir)?.ok_or("No active Weave")?;
    let mut session = read_weave_session(&handle.store, &handle.layrs_dir, &id)?;
    let conflict = session
        .conflicts
        .iter_mut()
        .find(|c| c.path == path || c.conflict_id == path)
        .ok_or("No matching conflict")?;
    let dir = conflict_dir(&handle.layrs_dir, &id, &conflict.conflict_id);
    let base;
    let bytes = if resolution == "base" {
        base = read_weave_blob(&handle.store, &dir.join("base"))?;
        &base
    } else {
        bytes
    };
    write_weave_blob(&mut handle.store, &dir.join("resolved"), bytes)?;
    for block in &mut conflict.blocks {
        block.status = "resolved".into();
        block.resolution = Some(resolution.into());
    }
    conflict.status = "resolved".into();
    conflict.resolution = Some(resolution.into());
    if session.conflicts.iter().all(|c| c.status == "resolved") {
        session.status = "resolved".into();
    }
    session.updated_at_unix = unix_now();
    write_weave_session(&mut handle.store, &handle.layrs_dir, &session)?;
    Ok(WeaveOperationResult {
        local_space: summary_from_handle(&handle),
        session: summarize_weave_session(&session),
        message: "Conflict resolved by explicit replacement.".into(),
    })
}

pub fn resolve_weave_conflict(
    local_space: String,
    path: String,
    resolution: String,
    replacement_file: Option<String>,
    manual_text: Option<String>,
) -> Result<WeaveOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let weave_id = active_weave_id(&handle.store, &handle.layrs_dir)?
        .ok_or_else(|| "No active Weave to resolve.".to_string())?;
    let mut session = read_weave_session(&handle.store, &handle.layrs_dir, &weave_id)?;
    let conflict = session
        .conflicts
        .iter_mut()
        .find(|conflict| conflict.path == path || conflict.conflict_id == path)
        .ok_or_else(|| format!("No active Weave conflict matches {path}."))?;
    let dir = conflict_dir(&handle.layrs_dir, &weave_id, &conflict.conflict_id);
    if let Some((block_id, block_resolution)) = parse_block_resolution(&resolution) {
        let method = validate_block_resolution_method(conflict, &block_id, &block_resolution)?;
        if method == ResolutionMethod::Manual && manual_text.is_none() {
            return Err("Manual text block resolution requires manual text.".to_string());
        }
        resolve_text_conflict_block(conflict, &block_id, method, manual_text.as_deref())?;
        if conflict
            .blocks
            .iter()
            .all(|block| block.status == "resolved")
        {
            let bytes = assemble_text_conflict_resolution(conflict)?;
            write_weave_blob(&mut handle.store, &dir.join("resolved"), &bytes)?;
            conflict.status = "resolved".to_string();
            conflict.resolution = Some("blocks".to_string());
        }
    } else {
        let method = validate_file_resolution_method(conflict, resolution.as_str())?;
        if replacement_file.is_some() {
            return Err(format!(
                "Lens {} does not declare replacement-file resolution for {}.",
                conflict.lens_id, conflict.path
            ));
        }
        let bytes = resolve_file_conflict_with_lens(&handle, &weave_id, conflict, method)?;
        write_weave_blob(&mut handle.store, &dir.join("resolved"), &bytes)?;
        for block in &mut conflict.blocks {
            block.status = "resolved".to_string();
            block.resolution = Some(method.as_str().to_string());
        }
        conflict.status = "resolved".to_string();
        conflict.resolution = Some(method.as_str().to_string());
    }
    let conflict_resolved = conflict.status == "resolved";
    if session
        .conflicts
        .iter()
        .all(|conflict| conflict.status == "resolved")
    {
        session.status = "resolved".to_string();
    }
    session.updated_at_unix = unix_now();
    write_weave_session(&mut handle.store, &handle.layrs_dir, &session)?;
    Ok(WeaveOperationResult {
        local_space: summary_from_handle(&handle),
        session: summarize_weave_session(&session),
        message: if conflict_resolved {
            "Conflict resolved.".to_string()
        } else {
            "Conflict block resolved.".to_string()
        },
    })
}

pub fn continue_weave(local_space: String) -> Result<WeaveOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let weave_id = active_weave_id(&handle.store, &handle.layrs_dir)?
        .ok_or_else(|| "No active Weave to continue.".to_string())?;
    let mut session = read_weave_session(&handle.store, &handle.layrs_dir, &weave_id)?;
    if session
        .conflicts
        .iter()
        .any(|conflict| conflict.status != "resolved")
    {
        return Err("Resolve all Weave conflicts before continuing.".to_string());
    }
    let mut proposed_state = read_state_file(
        &handle.store,
        &handle.layrs_dir,
        &proposed_state_path(&handle.layrs_dir, &weave_id),
    )?;
    if is_sync_weave_session(&session) {
        ensure_sync_weave_plan_is_current(&mut handle, &session, &proposed_state)?;
    } else {
        ensure_local_weave_target_is_current(&mut handle, &session, &proposed_state)?;
    }
    for conflict in &session.conflicts {
        let dir = conflict_dir(&handle.layrs_dir, &weave_id, &conflict.conflict_id);
        let bytes = read_weave_blob(&handle.store, &dir.join("resolved"))?;
        let entry = write_file_object(&handle.layrs_dir, &conflict.path, &bytes, true)?;
        apply_file_choice(&mut proposed_state, &conflict.path, Some(entry));
    }
    proposed_state.root_tree_id =
        Some(write_tree_object(&handle.layrs_dir, &proposed_state.files)?);
    if is_sync_weave_session(&session) {
        let step_id = apply_completed_sync_weave(&mut handle, &session, &proposed_state)?;
        session.applied_steps = vec![step_id];
        session.status = "applied".to_string();
        session.updated_at_unix = unix_now();
        write_weave_session(&mut handle.store, &handle.layrs_dir, &session)?;
        write_active_weave_marker(&mut handle.store, &handle.layrs_dir, None)?;
        return Ok(WeaveOperationResult {
            local_space: summary_from_handle(&handle),
            session: summarize_weave_session(&session),
            message: "Sync Weave applied after conflict resolution. Run Sync again to publish."
                .to_string(),
        });
    }
    let source_steps = sorted_steps(&handle.store, &handle.layrs_dir, &session.source_layer_id)?;
    apply_completed_weave(&mut handle, &session, &proposed_state, &source_steps)?;
    session.status = "applied".to_string();
    session.updated_at_unix = unix_now();
    write_weave_session(&mut handle.store, &handle.layrs_dir, &session)?;
    write_active_weave_marker(&mut handle.store, &handle.layrs_dir, None)?;
    Ok(WeaveOperationResult {
        local_space: summary_from_handle(&handle),
        session: summarize_weave_session(&session),
        message: "Weave applied after conflict resolution.".to_string(),
    })
}

pub(super) fn apply_completed_sync_weave(
    handle: &mut LocalSpaceHandle,
    session: &WeaveSessionFile,
    proposed_state: &WorkingStateFile,
) -> Result<String, String> {
    let target_layer_id = session.target_layer_id.as_str();
    let base_state = read_layer_index(&handle.store, &handle.layrs_dir, target_layer_id)
        .unwrap_or_else(|_| WorkingStateFile {
            schema: WORKING_STATE_SCHEMA.to_string(),
            layer_id: target_layer_id.to_string(),
            captured_at_unix: unix_now(),
            root_tree_id: None,
            files: Vec::new(),
        });
    archive_planned_sync_steps(
        &mut handle.store,
        &handle.layrs_dir,
        target_layer_id,
        &session.planned_steps,
        &session.weave_id,
    )?;
    write_working_state(
        &mut handle.store,
        &handle.layrs_dir,
        target_layer_id,
        proposed_state,
    )?;
    if handle.active.layer_id == target_layer_id {
        materialize_state(&mut handle.store, &handle.root, proposed_state)?;
    }
    let step_id = write_sync_woven_step(
        &mut handle.store,
        &handle.layrs_dir,
        target_layer_id,
        &base_state,
        proposed_state,
        &session.planned_steps,
    )?;
    let step = read_step_file(&handle.store, &handle.layrs_dir, target_layer_id, &step_id)?;
    write_pending_publish(&mut handle.store, &handle.layrs_dir, &step)?;
    handle.meta.updated_at_unix = unix_now();
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )?;
    remember_local_space(&handle.meta, Some(target_layer_id.to_string()))?;
    Ok(step_id)
}

fn is_sync_weave_session(session: &WeaveSessionFile) -> bool {
    session.source_layer_id.starts_with("studio-sync:")
        || session.source_layer_id.starts_with("local-sync:")
}

fn archive_planned_sync_steps(
    store: &mut LocalStoreSession,
    layrs_dir: &Path,
    layer_id: &str,
    planned_steps: &[String],
    weave_id: &str,
) -> Result<(), String> {
    if planned_steps.is_empty() {
        return Ok(());
    }

    let archive_dir = layrs_dir
        .join("sync")
        .join("local-replay")
        .join(safe_id_fragment(weave_id));
    let archive_steps_dir = archive_dir.join("steps");
    let archive_pending_dir = archive_dir.join("pending-publish");
    let steps_dir = layer_dir(layrs_dir, layer_id).join("steps");
    let pending_dir = pending_publish_dir(layrs_dir, layer_id);
    for step_id in planned_steps {
        let step_path = steps_dir.join(format!("{step_id}.json"));
        if store.exists(&step_path)? {
            let step = store.read_value(&step_path)?;
            store.write(&archive_steps_dir.join(format!("{step_id}.json")), &step)?;
        }
        let pending_path = pending_dir.join(format!("{step_id}.json"));
        if store.exists(&pending_path)? {
            let pending = store.read_value(&pending_path)?;
            store.write(
                &archive_pending_dir.join(format!("{step_id}.json")),
                &pending,
            )?;
        }
    }
    for step_id in planned_steps {
        // Drop the pending reference before its Step: each write rebuilds the
        // constrained SQLite projection. Both documents are already archived.
        let pending_path = pending_dir.join(format!("{step_id}.json"));
        store.remove(&pending_path)?;
        let step_path = steps_dir.join(format!("{step_id}.json"));
        store.remove(&step_path)?;
    }
    Ok(())
}

fn write_sync_woven_step(
    store: &mut LocalStoreSession,
    layrs_dir: &Path,
    layer_id: &str,
    base_state: &WorkingStateFile,
    state: &WorkingStateFile,
    planned_steps: &[String],
) -> Result<String, String> {
    let step_id = unique_step_id(store, layrs_dir, layer_id)?;
    let root_tree_id = state
        .root_tree_id
        .clone()
        .map(Ok)
        .unwrap_or_else(|| write_tree_object(layrs_dir, &state.files))?;
    let (added, modified, deleted) = diff_state(Some(base_state), state);
    let changed_paths = added
        .iter()
        .chain(modified.iter())
        .chain(deleted.iter())
        .cloned()
        .collect::<Vec<_>>();
    let parent_step_id = sorted_steps(store, layrs_dir, layer_id)?
        .last()
        .map(|step| step.step_id.clone());
    let origin_step_id = planned_steps
        .last()
        .cloned()
        .unwrap_or_else(|| step_id.clone());
    let step = StepFile {
        schema: STEP_SCHEMA.to_string(),
        step_id: step_id.clone(),
        layer_id: layer_id.to_string(),
        parent_step_id,
        base_layer_id: Some(layer_id.to_string()),
        base_tree_id: base_state.root_tree_id.clone(),
        root_tree_id: Some(root_tree_id),
        changed_paths,
        timeline_position: Some(next_timeline_position(store, layrs_dir, layer_id)?),
        origin_layer_id: Some(layer_id.to_string()),
        origin_layer_name: step_layer_display_name(store, layrs_dir, layer_id),
        origin_step_id: Some(origin_step_id),
        step_kind: Some("woven".to_string()),
        captured_at_unix: state.captured_at_unix,
        files: Vec::new(),
    };
    write_json(
        store,
        &layer_dir(layrs_dir, layer_id)
            .join("steps")
            .join(format!("{step_id}.json")),
        &step,
    )?;
    Ok(step_id)
}
