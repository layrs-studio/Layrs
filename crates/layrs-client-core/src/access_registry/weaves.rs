use layrs_lens_sdk::{LensFileResolutionInput, ResolutionMethod};

#[path = "weaves/resolution.rs"]
mod weave_resolution;

use self::weave_resolution::{
    assemble_text_conflict_resolution, conflict_block_method_labels, conflict_file_method_labels,
    labels_to_methods, parse_block_resolution, resolution_method_labels_for_storage,
    resolve_text_conflict_block, validate_block_resolution_method, validate_file_resolution_method,
};

fn weave_dir(layrs_dir: &Path, weave_id: &str) -> PathBuf {
    layrs_dir.join("weaves").join(safe_id_fragment(weave_id))
}

fn active_weave_marker_path(layrs_dir: &Path) -> PathBuf {
    layrs_dir.join("weaves").join("active.json")
}

fn session_path(layrs_dir: &Path, weave_id: &str) -> PathBuf {
    weave_dir(layrs_dir, weave_id).join("session.json")
}

fn proposed_state_path(layrs_dir: &Path, weave_id: &str) -> PathBuf {
    weave_dir(layrs_dir, weave_id).join("proposed-state.json")
}

fn conflict_dir(layrs_dir: &Path, weave_id: &str, conflict_id: &str) -> PathBuf {
    weave_dir(layrs_dir, weave_id)
        .join("conflicts")
        .join(safe_id_fragment(conflict_id))
}

fn active_weave_id(layrs_dir: &Path) -> Result<Option<String>, String> {
    let path = active_weave_marker_path(layrs_dir);
    if !path.exists() {
        return Ok(None);
    }
    let value = read_json::<Value>(&path)?;
    Ok(value
        .get("weaveId")
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

fn write_active_weave_marker(layrs_dir: &Path, weave_id: Option<&str>) -> Result<(), String> {
    let path = active_weave_marker_path(layrs_dir);
    if let Some(weave_id) = weave_id {
        write_json(
            &path,
            &serde_json::json!({
                "schema": "layrs.active_weave.v1",
                "weaveId": weave_id,
                "updatedAtUnix": unix_now()
            }),
        )
    } else if path.exists() {
        fs::remove_file(&path).map_err(|error| {
            format!(
                "Layrs Desktop could not clear active Weave marker {}: {error}",
                path.display()
            )
        })
    } else {
        Ok(())
    }
}

fn read_weave_session(layrs_dir: &Path, weave_id: &str) -> Result<WeaveSessionFile, String> {
    read_json(&session_path(layrs_dir, weave_id))
}

fn write_weave_session(layrs_dir: &Path, session: &WeaveSessionFile) -> Result<(), String> {
    write_json(&session_path(layrs_dir, &session.weave_id), session)
}

fn summarize_weave_session(session: &WeaveSessionFile) -> WeaveSessionSummary {
    WeaveSessionSummary {
        weave_id: session.weave_id.clone(),
        source_layer_id: session.source_layer_id.clone(),
        target_layer_id: session.target_layer_id.clone(),
        status: session.status.clone(),
        pre_weave_target_tree_id: session.pre_weave_target_tree_id.clone(),
        pre_weave_target_step_id: session.pre_weave_target_step_id.clone(),
        planned_steps: session.planned_steps.clone(),
        applied_steps: session.applied_steps.clone(),
        conflicts: session
            .conflicts
            .iter()
            .map(|conflict| WeaveConflictSummary {
                conflict_id: conflict.conflict_id.clone(),
                path: conflict.path.clone(),
                lens_id: conflict.lens_id.clone(),
                status: conflict.status.clone(),
                message: conflict.message.clone(),
                methods: conflict_file_method_labels(conflict),
                resolution: conflict.resolution.clone(),
                blocks: conflict
                    .blocks
                    .iter()
                    .map(|block| WeaveConflictBlockSummary {
                        block_id: block.block_id.clone(),
                        status: block.status.clone(),
                        base: block.base.clone(),
                        existing: block.ours.clone(),
                        incoming: block.theirs.clone(),
                        ours: block.ours.clone(),
                        theirs: block.theirs.clone(),
                        methods: conflict_block_method_labels(conflict, block),
                        resolution: block.resolution.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

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
    if active_weave_id(&handle.layrs_dir)?.is_some() {
        return Err(
            "A Weave is already active. Continue or abort it before starting another Weave."
                .to_string(),
        );
    }

    let source_state = latest_state_for_layer(&handle.layrs_dir, &source_layer_id)?;
    let target_state = latest_state_for_layer(&handle.layrs_dir, &target_layer_id)?;
    let source_steps = sorted_steps(&handle.layrs_dir, &source_layer_id)?;
    let target_steps = sorted_steps(&handle.layrs_dir, &target_layer_id)?;
    let planned_steps = source_steps
        .iter()
        .filter(|source_step| !target_has_origin(&target_steps, source_step))
        .map(|step| step.step_id.clone())
        .collect::<Vec<_>>();

    let weave_id = unique_weave_id(&handle.layrs_dir, &source_layer_id, &target_layer_id);
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
        match reconcile_path_with_lens(
            &handle,
            &weave_id,
            &target_layer_id,
            &source_layer_id,
            &path,
            base,
            ours,
            theirs,
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
        let current_state = capture_working_state(&handle.root, &target_layer_id, true)?;
        let changed_files = changed_file_count(Some(&target_state), &current_state);
        if changed_files > 0 {
            let step_id = write_step(&handle.layrs_dir, &target_layer_id, &current_state)?;
            let step = read_step_file(&handle.layrs_dir, &target_layer_id, &step_id)?;
            write_pending_publish(&handle.layrs_dir, &step)?;
        }
    }

    write_json(
        &proposed_state_path(&handle.layrs_dir, &weave_id),
        &storage_state(&handle.layrs_dir, &proposed_state)?,
    )?;
    write_weave_session(&handle.layrs_dir, &session)?;

    if session.conflicts.is_empty() {
        apply_completed_weave(&mut handle, &session, &proposed_state, &source_steps)?;
        write_active_weave_marker(&handle.layrs_dir, None)?;
        let mut applied = read_weave_session(&handle.layrs_dir, &weave_id)?;
        applied.status = "applied".to_string();
        applied.updated_at_unix = unix_now();
        write_weave_session(&handle.layrs_dir, &applied)?;
        Ok(WeaveOperationResult {
            local_space: summary_from_handle(&handle),
            session: summarize_weave_session(&applied),
            message: "Weave applied.".to_string(),
        })
    } else {
        if handle.active.layer_id == target_layer_id {
            materialize_state(&handle.root, &proposed_state)?;
        }
        write_active_weave_marker(&handle.layrs_dir, Some(&weave_id))?;
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
    let Some(weave_id) = active_weave_id(&handle.layrs_dir)? else {
        return Ok(None);
    };
    Ok(Some(summarize_weave_session(&read_weave_session(
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
    let handle = open_local_space_handle(&local_space)?;
    let weave_id = active_weave_id(&handle.layrs_dir)?
        .ok_or_else(|| "No active Weave to abort.".to_string())?;
    let mut session = read_weave_session(&handle.layrs_dir, &weave_id)?;
    let target_state = state_from_tree(
        &handle.layrs_dir,
        &session.target_layer_id,
        session.pre_weave_target_tree_id.clone(),
    )?;
    if handle.active.layer_id == session.target_layer_id {
        materialize_state(&handle.root, &target_state)?;
    }
    session.status = "aborted".to_string();
    session.updated_at_unix = unix_now();
    write_weave_session(&handle.layrs_dir, &session)?;
    write_active_weave_marker(&handle.layrs_dir, None)?;
    Ok(WeaveOperationResult {
        local_space: summary_from_handle(&handle),
        session: summarize_weave_session(&session),
        message: "Weave aborted. Pre-Weave working tree restored.".to_string(),
    })
}

pub fn resolve_weave_conflict(
    local_space: String,
    path: String,
    resolution: String,
    replacement_file: Option<String>,
    manual_text: Option<String>,
) -> Result<WeaveOperationResult, String> {
    let handle = open_local_space_handle(&local_space)?;
    let weave_id = active_weave_id(&handle.layrs_dir)?
        .ok_or_else(|| "No active Weave to resolve.".to_string())?;
    let mut session = read_weave_session(&handle.layrs_dir, &weave_id)?;
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
            fs::write(dir.join("resolved"), bytes).map_err(|error| {
                format!(
                    "Layrs Desktop could not write resolved conflict {}: {error}",
                    conflict.path
                )
            })?;
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
        fs::write(dir.join("resolved"), bytes).map_err(|error| {
            format!(
                "Layrs Desktop could not write resolved conflict {}: {error}",
                conflict.path
            )
        })?;
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
    write_weave_session(&handle.layrs_dir, &session)?;
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
    let weave_id = active_weave_id(&handle.layrs_dir)?
        .ok_or_else(|| "No active Weave to continue.".to_string())?;
    let mut session = read_weave_session(&handle.layrs_dir, &weave_id)?;
    if session
        .conflicts
        .iter()
        .any(|conflict| conflict.status != "resolved")
    {
        return Err("Resolve all Weave conflicts before continuing.".to_string());
    }
    let mut proposed_state = read_state_file(
        &handle.layrs_dir,
        &proposed_state_path(&handle.layrs_dir, &weave_id),
    )?;
    for conflict in &session.conflicts {
        let dir = conflict_dir(&handle.layrs_dir, &weave_id, &conflict.conflict_id);
        let bytes = fs::read(dir.join("resolved")).map_err(|error| {
            format!(
                "Layrs Desktop could not read resolved conflict {}: {error}",
                conflict.path
            )
        })?;
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
        write_weave_session(&handle.layrs_dir, &session)?;
        write_active_weave_marker(&handle.layrs_dir, None)?;
        return Ok(WeaveOperationResult {
            local_space: summary_from_handle(&handle),
            session: summarize_weave_session(&session),
            message: "Sync Weave applied after conflict resolution. Run Sync again to publish."
                .to_string(),
        });
    }
    let source_steps = sorted_steps(&handle.layrs_dir, &session.source_layer_id)?;
    apply_completed_weave(&mut handle, &session, &proposed_state, &source_steps)?;
    session.status = "applied".to_string();
    session.updated_at_unix = unix_now();
    write_weave_session(&handle.layrs_dir, &session)?;
    write_active_weave_marker(&handle.layrs_dir, None)?;
    Ok(WeaveOperationResult {
        local_space: summary_from_handle(&handle),
        session: summarize_weave_session(&session),
        message: "Weave applied after conflict resolution.".to_string(),
    })
}

fn apply_completed_sync_weave(
    handle: &mut LocalSpaceHandle,
    session: &WeaveSessionFile,
    proposed_state: &WorkingStateFile,
) -> Result<String, String> {
    let target_layer_id = session.target_layer_id.as_str();
    let base_state = read_layer_index(&handle.layrs_dir, target_layer_id).unwrap_or_else(|_| {
        WorkingStateFile {
            schema: WORKING_STATE_SCHEMA.to_string(),
            layer_id: target_layer_id.to_string(),
            captured_at_unix: unix_now(),
            root_tree_id: None,
            files: Vec::new(),
        }
    });
    archive_planned_sync_steps(
        &handle.layrs_dir,
        target_layer_id,
        &session.planned_steps,
        &session.weave_id,
    )?;
    write_working_state(&handle.layrs_dir, target_layer_id, proposed_state)?;
    if handle.active.layer_id == target_layer_id {
        materialize_state(&handle.root, proposed_state)?;
    }
    let step_id = write_sync_woven_step(
        &handle.layrs_dir,
        target_layer_id,
        &base_state,
        proposed_state,
        &session.planned_steps,
    )?;
    let step = read_step_file(&handle.layrs_dir, target_layer_id, &step_id)?;
    write_pending_publish(&handle.layrs_dir, &step)?;
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    remember_local_space(&handle.meta, Some(target_layer_id.to_string()))?;
    Ok(step_id)
}

fn is_sync_weave_session(session: &WeaveSessionFile) -> bool {
    session.source_layer_id.starts_with("studio-sync:")
        || session.source_layer_id.starts_with("local-sync:")
}

fn archive_planned_sync_steps(
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
    fs::create_dir_all(&archive_steps_dir).map_err(|error| {
        format!(
            "Layrs Desktop could not create sync replay archive {}: {error}",
            archive_steps_dir.display()
        )
    })?;
    fs::create_dir_all(&archive_pending_dir).map_err(|error| {
        format!(
            "Layrs Desktop could not create sync replay archive {}: {error}",
            archive_pending_dir.display()
        )
    })?;

    let steps_dir = layer_dir(layrs_dir, layer_id).join("steps");
    let pending_dir = pending_publish_dir(layrs_dir, layer_id);
    for step_id in planned_steps {
        let step_path = steps_dir.join(format!("{step_id}.json"));
        if step_path.exists() {
            fs::copy(&step_path, archive_steps_dir.join(format!("{step_id}.json"))).map_err(
                |error| {
                    format!(
                        "Layrs Desktop could not archive pending Step {}: {error}",
                        step_path.display()
                    )
                },
            )?;
            fs::remove_file(&step_path).map_err(|error| {
                format!(
                    "Layrs Desktop could not replace pending Step {}: {error}",
                    step_path.display()
                )
            })?;
        }

        let pending_path = pending_dir.join(format!("{step_id}.json"));
        if pending_path.exists() {
            fs::copy(&pending_path, archive_pending_dir.join(format!("{step_id}.json"))).map_err(
                |error| {
                    format!(
                        "Layrs Desktop could not archive pending publish entry {}: {error}",
                        pending_path.display()
                    )
                },
            )?;
        }
    }
    clear_pending_publish(layrs_dir, layer_id)
}

fn write_sync_woven_step(
    layrs_dir: &Path,
    layer_id: &str,
    base_state: &WorkingStateFile,
    state: &WorkingStateFile,
    planned_steps: &[String],
) -> Result<String, String> {
    let step_id = unique_step_id(layrs_dir, layer_id);
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
    let parent_step_id = sorted_steps(layrs_dir, layer_id)?
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
        timeline_position: Some(next_timeline_position(layrs_dir, layer_id)?),
        origin_layer_id: Some(layer_id.to_string()),
        origin_layer_name: step_layer_display_name(layrs_dir, layer_id),
        origin_step_id: Some(origin_step_id),
        step_kind: Some("woven".to_string()),
        captured_at_unix: state.captured_at_unix,
        files: Vec::new(),
    };
    write_json(
        &layer_dir(layrs_dir, layer_id)
            .join("steps")
            .join(format!("{step_id}.json")),
        &step,
    )?;
    Ok(step_id)
}

fn ensure_layer_known(handle: &LocalSpaceHandle, layer_id: &str) -> Result<(), String> {
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

fn sorted_steps(layrs_dir: &Path, layer_id: &str) -> Result<Vec<StepFile>, String> {
    let mut steps = read_step_files(layrs_dir, layer_id)?;
    steps.sort_by(compare_steps_by_timeline);
    Ok(steps)
}

fn latest_state_for_layer(layrs_dir: &Path, layer_id: &str) -> Result<WorkingStateFile, String> {
    let steps = sorted_steps(layrs_dir, layer_id)?;
    if let Some(step) = steps.last() {
        return state_from_step(layrs_dir, step);
    }
    read_layer_state(layrs_dir, layer_id).or_else(|_| read_layer_index(layrs_dir, layer_id))
}

fn state_from_tree(
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

fn target_has_origin(target_steps: &[StepFile], source_step: &StepFile) -> bool {
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

fn changed_paths_between_states(left: &WorkingStateFile, right: &WorkingStateFile) -> Vec<String> {
    let (added, modified, deleted) = diff_state(Some(left), right);
    added
        .into_iter()
        .chain(modified)
        .chain(deleted)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn common_weave_base_state(
    handle: &LocalSpaceHandle,
    source_layer_id: &str,
    target_layer_id: &str,
) -> Option<WorkingStateFile> {
    let source_steps = sorted_steps(&handle.layrs_dir, source_layer_id).ok()?;
    let target_steps = sorted_steps(&handle.layrs_dir, target_layer_id).ok()?;
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
        return read_layer_index(&handle.layrs_dir, target_layer_id).ok();
    }
    let target_layer = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == target_layer_id)?;
    if target_layer.parent_layer_id.as_deref() == Some(source_layer_id) {
        return read_layer_index(&handle.layrs_dir, source_layer_id).ok();
    }
    None
}

fn is_safe_take_theirs(
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

fn file_entry_hash(entry: Option<&FileSnapshotEntry>) -> Option<&str> {
    entry.map(|entry| entry.hash.as_str())
}

fn apply_file_choice(state: &mut WorkingStateFile, path: &str, entry: Option<FileSnapshotEntry>) {
    state.files.retain(|file| file.path != path);
    if let Some(entry) = entry {
        state.files.push(entry);
        state
            .files
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
}

enum PathReconcileOutcome {
    AutoResolved(Option<FileSnapshotEntry>),
    Conflicted(WeaveConflictFile),
}

fn reconcile_path_with_lens(
    handle: &LocalSpaceHandle,
    weave_id: &str,
    target_layer_id: &str,
    source_layer_id: &str,
    path: &str,
    base: Option<&FileSnapshotEntry>,
    ours: Option<&FileSnapshotEntry>,
    theirs: Option<&FileSnapshotEntry>,
) -> Result<PathReconcileOutcome, String> {
    let conflict_id = format!("conflict-{}", safe_id_fragment(path));
    let base_bytes = read_optional_entry_bytes(&handle.layrs_dir, base)?;
    let ours_bytes = read_optional_entry_bytes(&handle.layrs_dir, ours)?;
    let theirs_bytes = read_optional_entry_bytes(&handle.layrs_dir, theirs)?;
    let lens_id = lens_id_for_path(path);
    let input = layrs_lens_sdk::LensReconcileInput {
        path: Some(Path::new(path)),
        media_type: None,
        base: reconcile_side(base, &base_bytes),
        ours: reconcile_side(ours, &ours_bytes),
        theirs: reconcile_side(theirs, &theirs_bytes),
        ours_label: target_layer_id,
        theirs_label: source_layer_id,
    };
    let result = match lens_id {
        "layrs.text" => layrs_lens_text::reconcile_text(input),
        "layrs.raw" | "layrs.image" | "layrs.code" => layrs_lens_raw::reconcile_raw(input),
        _ => layrs_lens_raw::reconcile_raw(input),
    };

    if result.status == layrs_lens_sdk::LensReconcileResultStatus::AutoResolved {
        let content = result
            .resolved
            .ok_or_else(|| format!("Lens {lens_id} auto-resolved {path} without content."))?;
        return Ok(PathReconcileOutcome::AutoResolved(
            write_reconciled_content(handle, path, content)?,
        ));
    }

    let dir = conflict_dir(&handle.layrs_dir, weave_id, &conflict_id);
    fs::create_dir_all(&dir).map_err(|error| {
        format!(
            "Layrs Desktop could not create Weave conflict directory {}: {error}",
            dir.display()
        )
    })?;
    fs::write(dir.join("base"), &base_bytes).map_err(|error| {
        format!("Layrs Desktop could not write base conflict bytes for {path}: {error}")
    })?;
    fs::write(dir.join("ours"), &ours_bytes).map_err(|error| {
        format!("Layrs Desktop could not write ours conflict bytes for {path}: {error}")
    })?;
    fs::write(dir.join("theirs"), &theirs_bytes).map_err(|error| {
        format!("Layrs Desktop could not write theirs conflict bytes for {path}: {error}")
    })?;
    if let Some(conflict_content) = result.conflict.as_ref() {
        if conflict_content.exists {
            fs::write(dir.join("marked"), &conflict_content.bytes).map_err(|error| {
                format!("Layrs Desktop could not write marked conflict bytes for {path}: {error}")
            })?;
        }
    }
    let blocks = result
        .blocks
        .into_iter()
        .map(|block| WeaveConflictBlockFile {
            block_id: block.block_id,
            status: "open".to_string(),
            base: block.base,
            ours: block.ours,
            theirs: block.theirs,
            methods: resolution_method_labels_for_storage(
                if block.methods.is_empty() {
                    labels_to_methods(&block.supported_resolutions)
                } else {
                    block.methods
                }
                .as_slice(),
            ),
            resolution: None,
            resolved_text: None,
        })
        .collect();
    let segments = result
        .segments
        .into_iter()
        .map(|segment| WeaveConflictSegmentFile {
            kind: match segment.kind {
                layrs_lens_sdk::LensConflictSegmentKind::Text => "text".to_string(),
                layrs_lens_sdk::LensConflictSegmentKind::Block => "block".to_string(),
            },
            text: segment.text,
            block_id: segment.block_id,
        })
        .collect();
    let message = if result.status == layrs_lens_sdk::LensReconcileResultStatus::Unsupported {
        result.summary
    } else {
        format!("{}.", result.summary)
    };
    Ok(PathReconcileOutcome::Conflicted(WeaveConflictFile {
        conflict_id,
        path: path.to_string(),
        lens_id: lens_id.to_string(),
        status: "open".to_string(),
        message,
        methods: resolution_method_labels_for_storage(&result.file_methods),
        resolution: None,
        blocks,
        segments,
    }))
}

fn write_reconciled_content(
    handle: &LocalSpaceHandle,
    path: &str,
    content: layrs_lens_sdk::LensReconcileContent,
) -> Result<Option<FileSnapshotEntry>, String> {
    if content.exists {
        write_file_object(&handle.layrs_dir, path, &content.bytes, true).map(Some)
    } else {
        Ok(None)
    }
}

fn reconcile_side<'a>(
    entry: Option<&'a FileSnapshotEntry>,
    bytes: &'a [u8],
) -> layrs_lens_sdk::LensReconcileSide<'a> {
    if let Some(entry) = entry {
        layrs_lens_sdk::LensReconcileSide {
            exists: true,
            bytes,
            content_hash: Some(entry.hash.as_str()),
            size: entry.size,
        }
    } else {
        layrs_lens_sdk::LensReconcileSide::absent()
    }
}

fn read_optional_entry_bytes(
    layrs_dir: &Path,
    entry: Option<&FileSnapshotEntry>,
) -> Result<Vec<u8>, String> {
    entry
        .map(|entry| read_snapshot_object_bytes(layrs_dir, entry))
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn apply_conflict_marker_to_state(
    handle: &LocalSpaceHandle,
    state: &mut WorkingStateFile,
    weave_id: &str,
    conflict: &WeaveConflictFile,
    ours: Option<&FileSnapshotEntry>,
) -> Result<(), String> {
    let dir = conflict_dir(&handle.layrs_dir, weave_id, &conflict.conflict_id);
    let marked = dir.join("marked");
    if marked.exists() {
        let bytes = fs::read(&marked).map_err(|error| {
            format!(
                "Layrs Desktop could not read marked conflict file {}: {error}",
                marked.display()
            )
        })?;
        let entry = write_file_object(&handle.layrs_dir, &conflict.path, &bytes, true)?;
        apply_file_choice(state, &conflict.path, Some(entry));
    } else {
        apply_file_choice(state, &conflict.path, ours.cloned());
    }
    Ok(())
}

fn resolve_file_conflict_with_lens(
    handle: &LocalSpaceHandle,
    weave_id: &str,
    conflict: &WeaveConflictFile,
    method: ResolutionMethod,
) -> Result<Vec<u8>, String> {
    let dir = conflict_dir(&handle.layrs_dir, weave_id, &conflict.conflict_id);
    let base_bytes = read_conflict_side(&dir, "base", conflict)?;
    let existing_bytes = read_conflict_side(&dir, "ours", conflict)?;
    let incoming_bytes = read_conflict_side(&dir, "theirs", conflict)?;
    let input = LensFileResolutionInput {
        method,
        base: side_from_bytes(&base_bytes),
        existing: side_from_bytes(&existing_bytes),
        incoming: side_from_bytes(&incoming_bytes),
    };
    let content = match conflict.lens_id.as_str() {
        "layrs.raw" | "layrs.image" | "layrs.code" => layrs_lens_raw::resolve_raw_conflict(input),
        other => {
            return Err(format!(
                "Lens {other} does not support file-level resolution for {}.",
                conflict.path
            ));
        }
    }
    .map_err(|error| error.to_string())?;
    if content.exists {
        Ok(content.bytes)
    } else {
        Ok(Vec::new())
    }
}

fn read_conflict_side(
    dir: &Path,
    name: &str,
    conflict: &WeaveConflictFile,
) -> Result<Vec<u8>, String> {
    fs::read(dir.join(name)).map_err(|error| {
        format!(
            "Layrs Desktop could not read resolution bytes for {}: {error}",
            conflict.path
        )
    })
}

fn side_from_bytes(bytes: &[u8]) -> layrs_lens_sdk::LensReconcileSide<'_> {
    layrs_lens_sdk::LensReconcileSide {
        exists: true,
        bytes,
        content_hash: None,
        size: bytes.len() as u64,
    }
}

fn apply_completed_weave(
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

fn step_origin_key(step: &StepFile) -> (String, String) {
    (
        step.origin_layer_id
            .clone()
            .unwrap_or_else(|| step.layer_id.clone()),
        step.origin_step_id
            .clone()
            .unwrap_or_else(|| step.step_id.clone()),
    )
}

fn copy_weave_steps_to_target(
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

fn unique_weave_id(layrs_dir: &Path, source_layer_id: &str, target_layer_id: &str) -> String {
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

fn inherit_parent_steps(
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

fn propagate_step_to_linked_children(
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
            let outcome = reconcile_path_with_lens(
                handle,
                &weave_id,
                child_layer_id,
                &source_step.layer_id,
                path,
                base,
                ours,
                theirs,
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
