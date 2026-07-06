pub fn scan_working_tree(local_space: String) -> Result<WorkingTreeScan, String> {
    let handle = open_local_space_handle(&local_space)?;
    let active_layer_id = handle.active.layer_id.clone();
    let current = capture_working_state(&handle.root, &active_layer_id, true)?;
    let previous = read_layer_state(&handle.layrs_dir, &active_layer_id).ok();
    let (added, modified, deleted) = diff_state(previous.as_ref(), &current);
    let diffs = lens_diff_entries(
        &handle,
        "workingTree",
        &active_layer_id,
        None,
        previous.as_ref(),
        &current,
        &added,
        &modified,
        &deleted,
    );
    let steps = local_step_summaries(&handle, &active_layer_id)?;
    let layer_activities = layer_step_activities(&handle.layrs_dir, &handle.meta.layers)?;
    let pending_publish_count =
        read_pending_publish_files(&handle.layrs_dir, &active_layer_id)?.len();

    Ok(WorkingTreeScan {
        root_path: handle.root.display().to_string(),
        active_layer_id,
        changed: !(added.is_empty() && modified.is_empty() && deleted.is_empty()),
        added,
        modified,
        deleted,
        diffs,
        steps,
        layer_activities,
        pending_publish_count,
        files: current.files,
    })
}

pub fn load_diff_window(
    local_space: String,
    path: String,
    source: Option<String>,
    start: usize,
    limit: usize,
) -> Result<LensDiffEntry, String> {
    let handle = open_local_space_handle(&local_space)?;
    let source = source.unwrap_or_else(|| "workingTree".to_string());

    if source == "workingTree" {
        let layer_id = handle.active.layer_id.clone();
        let current = capture_working_state(&handle.root, &layer_id, true)?;
        let previous = read_layer_state(&handle.layrs_dir, &layer_id).ok();
        return diff_window_for_path(
            &handle,
            "workingTree",
            &layer_id,
            None,
            previous.as_ref(),
            &current,
            &path,
            start,
            limit,
        );
    }

    if let Some(step_id) = source
        .strip_prefix("localStep:")
        .or_else(|| source.strip_prefix("step:"))
    {
        return load_step_diff_window(&handle, &path, step_id, start, limit);
    }

    Err(format!(
        "Layrs Desktop cannot load diff window source {source}. Supported sources: workingTree, localStep:<stepId>."
    ))
}

fn receive_linked_space_state(
    handle: &mut LocalSpaceHandle,
    config: &DesktopConfig,
    materialize_active: bool,
) -> Result<PathBuf, String> {
    ensure_linked_space_ready(handle)?;
    let layer_id = handle.active.layer_id.clone();
    if !is_probably_server_layer_id(&layer_id) {
        return Err(unlinked_layer_message(&layer_id));
    }

    let receive_path = format!(
        "/v1/workspaces/{}/spaces/{}/sync/receive",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id)
    );
    let request = ReceiveLocalSpaceRequest {
        layer_id: layer_id.clone(),
        limit: 200,
    };
    let token = desktop_token(config)?;
    let response: ReceiveLocalSpaceResponse = post_json(
        &config.server_endpoint,
        &receive_path,
        Some(&token),
        &request,
    )?;
    apply_receive_response(
        handle,
        response,
        materialize_active,
        Some(config.server_endpoint.as_str()),
        Some(token.as_str()),
    )
}

pub fn receive_local_space(local_space: String) -> Result<SyncOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    if handle.meta.state == LOCAL_SPACE_STATE_DRAFT {
        return Err("Draft Local Spaces must be sent to Studio before receiving.".to_string());
    }
    ensure_linked_space_ready(&handle)?;

    let config = DesktopConfig::load_or_create()?;
    let layer_id = handle.active.layer_id.clone();
    if !is_probably_server_layer_id(&layer_id) {
        let path = write_sync_state(&handle, "receive", "blocked-unlinked-layer", 0, None)?;
        return Err(format!(
            "{} Sync state: {}",
            unlinked_layer_message(&layer_id),
            path.display()
        ));
    }

    let base_state = read_layer_index(&handle.layrs_dir, &layer_id).ok();
    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let latest_pending_root = latest_pending_publish_step(&handle.layrs_dir, &layer_id)?
        .and_then(|step| step.root_tree_id);
    let saved_step = if changed_files > 0 && latest_pending_root != current_state.root_tree_id {
        let step_id = write_step_and_pending_publish(&handle.layrs_dir, &layer_id, &current_state)?;
        propagate_step_to_linked_children(&mut handle, &layer_id, &step_id)?;
        Some(step_id)
    } else {
        None
    };
    write_working_state(&handle.layrs_dir, &layer_id, &current_state)?;
    let pending_steps = pending_publish_steps(&handle.layrs_dir, &layer_id)?;

    let receive_path = format!(
        "/v1/workspaces/{}/spaces/{}/sync/receive",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id)
    );
    let request = ReceiveLocalSpaceRequest {
        layer_id: layer_id.clone(),
        limit: 200,
    };
    let token = desktop_token(&config)?;
    let response: ReceiveLocalSpaceResponse = post_json(
        &config.server_endpoint,
        &receive_path,
        Some(&token),
        &request,
    )?;
    let sync_path = apply_receive_response(
        &mut handle,
        response,
        true,
        Some(config.server_endpoint.as_str()),
        Some(token.as_str()),
    )?;
    let received_state = read_layer_index(&handle.layrs_dir, &layer_id)?;
    let base_root = base_state.as_ref().and_then(|state| state.root_tree_id.clone());
    let received_root = received_state.root_tree_id.clone();
    let mut replayed_local = false;
    if !pending_steps.is_empty() {
        if received_root != base_root {
            if let Some(conflicted) = weave_local_state_over_received_sync(
                &mut handle,
                base_state.as_ref(),
                &current_state,
                &received_state,
                &pending_steps,
                &sync_path,
                "receive",
            )? {
                return Ok(conflicted);
            }
            replayed_local = true;
        } else {
            write_working_state(&handle.layrs_dir, &layer_id, &current_state)?;
            materialize_state(&handle.root, &current_state)?;
        }
    }
    let message = if replayed_local {
        "Received Studio state, then replayed local pending Step(s) above it.".to_string()
    } else if let Some(step_id) = saved_step {
        format!("Received Studio state. Local changes were preserved in Step {step_id}.")
    } else {
        "Received Studio state.".to_string()
    };

    Ok(SyncOperationResult {
        local_space: summary_from_handle(&handle),
        status: "received".to_string(),
        message,
        sync_state_path: sync_path.display().to_string(),
    })
}

pub fn sync_local_space(local_space: String) -> Result<SyncOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    if handle.meta.state == LOCAL_SPACE_STATE_DRAFT {
        return Err("Draft Local Spaces must be sent to Studio before syncing.".to_string());
    }
    ensure_linked_space_ready(&handle)?;

    if let Some(weave_id) = active_weave_id(&handle.layrs_dir)? {
        let session = read_weave_session(&handle.layrs_dir, &weave_id)?;
        return Err(format!(
            "A Weave is already {}. Resolve/continue or abort it before syncing.",
            session.status
        ));
    }

    let config = DesktopConfig::load_or_create()?;
    let synced_layer_deletions = flush_pending_layer_deletions(&handle, &config)?;
    let layer_deletion_message = if synced_layer_deletions > 0 {
        format!("Deleted {synced_layer_deletions} Layer(s) on Studio. ")
    } else {
        String::new()
    };
    let layer_id = handle.active.layer_id.clone();
    if !is_probably_server_layer_id(&layer_id) {
        let path = write_sync_state(&handle, "sync", "blocked-unlinked-layer", 0, None)?;
        return Err(format!(
            "{} Sync state: {}",
            unlinked_layer_message(&layer_id),
            path.display()
        ));
    }

    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    if changed_files > 0 {
        let step_id = write_step(&handle.layrs_dir, &layer_id, &current_state)?;
        let step = read_step_file(&handle.layrs_dir, &layer_id, &step_id)?;
        write_pending_publish(&handle.layrs_dir, &step)?;
        propagate_step_to_linked_children(&mut handle, &layer_id, &step_id)?;
        write_working_state(&handle.layrs_dir, &layer_id, &current_state)?;
    }

    let pending_count = read_pending_publish_files(&handle.layrs_dir, &layer_id)?.len();
    let needs_initial_publish = layer_needs_initial_publish(&handle, &layer_id)?;
    if needs_initial_publish {
        let published = publish_local_space(handle.meta.local_space_id.clone())?;
        return Ok(SyncOperationResult {
            local_space: published.local_space,
            status: "synced".to_string(),
            message: format!("Sync complete. {layer_deletion_message}{}", published.message),
            sync_state_path: published.sync_state_path,
        });
    }
    if pending_count == 0 {
        let received = receive_local_space(handle.meta.local_space_id.clone())?;
        return Ok(SyncOperationResult {
            local_space: received.local_space,
            status: "synced".to_string(),
            message: format!("Sync complete. {layer_deletion_message}{}", received.message),
            sync_state_path: received.sync_state_path,
        });
    }

    let base_state = read_layer_index(&handle.layrs_dir, &layer_id).ok();
    let local_state = capture_working_state(&handle.root, &layer_id, true)?;
    let pending_steps = pending_publish_steps(&handle.layrs_dir, &layer_id)?;
    let sync_path = receive_linked_space_state(&mut handle, &config, true)?;
    let studio_state = read_layer_index(&handle.layrs_dir, &layer_id)?;
    let studio_root = studio_state.root_tree_id.clone();
    let base_root = base_state.as_ref().and_then(|state| state.root_tree_id.clone());
    if studio_root != base_root {
        if let Some(conflicted) =
            weave_local_state_over_received_sync(
                &mut handle,
                base_state.as_ref(),
                &local_state,
                &studio_state,
                &pending_steps,
                &sync_path,
                "sync",
            )?
        {
            return Ok(conflicted);
        }
    } else {
        write_working_state(&handle.layrs_dir, &layer_id, &local_state)?;
        materialize_state(&handle.root, &local_state)?;
    }

    let published = publish_local_space(handle.meta.local_space_id.clone())?;
    Ok(SyncOperationResult {
        local_space: published.local_space,
        status: "synced".to_string(),
        message: format!(
            "Sync complete. {layer_deletion_message}Retrieved Studio state, then {}.",
            published.message
        ),
        sync_state_path: published.sync_state_path,
    })
}

fn weave_local_state_over_received_sync(
    handle: &mut LocalSpaceHandle,
    base_state: Option<&WorkingStateFile>,
    local_state: &WorkingStateFile,
    received_state: &WorkingStateFile,
    pending_steps: &[StepFile],
    sync_path: &Path,
    operation: &str,
) -> Result<Option<SyncOperationResult>, String> {
    if pending_steps.is_empty() {
        return Ok(None);
    }

    let layer_id = handle.active.layer_id.clone();
    write_layer_state(&handle.layrs_dir, &layer_id, received_state)?;
    materialize_state(&handle.root, received_state)?;

    let weave_id = unique_weave_id(&handle.layrs_dir, "local-sync", &layer_id);
    let source_layer_id = format!("local-sync:{layer_id}");
    let now = unix_now();
    let mut session = WeaveSessionFile {
        schema: WEAVE_SESSION_SCHEMA.to_string(),
        weave_id: weave_id.clone(),
        source_layer_id: source_layer_id.clone(),
        target_layer_id: layer_id.clone(),
        status: "applying".to_string(),
        pre_weave_target_tree_id: local_state.root_tree_id.clone(),
        pre_weave_target_step_id: pending_steps.last().map(|step| step.step_id.clone()),
        planned_steps: pending_steps.iter().map(|step| step.step_id.clone()).collect(),
        applied_steps: Vec::new(),
        conflicts: Vec::new(),
        created_at_unix: now,
        updated_at_unix: now,
    };
    let mut proposed_state = received_state.clone();
    let base_files = base_state
        .map(|state| file_entries(&state.files))
        .unwrap_or_default();
    let local_files = file_entries(&local_state.files);
    let received_files = file_entries(&received_state.files);
    let mut paths = BTreeSet::new();
    let (local_added, local_modified, local_deleted) = diff_state(base_state, local_state);
    paths.extend(local_added);
    paths.extend(local_modified);
    paths.extend(local_deleted);

    let mut conflicts = Vec::new();
    for path in paths {
        let base = base_files.get(&path);
        let ours = received_files.get(&path);
        let theirs = local_files.get(&path);
        if is_safe_take_theirs(base, ours, theirs) {
            apply_file_choice(&mut proposed_state, &path, theirs.cloned());
            continue;
        }
        match reconcile_path_with_lens(
            handle,
            &weave_id,
            &layer_id,
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
                apply_conflict_marker_to_state(handle, &mut proposed_state, &weave_id, &conflict, ours)?;
                conflicts.push(conflict);
            }
        }
    }

    proposed_state.root_tree_id = Some(write_tree_object(&handle.layrs_dir, &proposed_state.files)?);
    if conflicts.is_empty() {
        let step_id = apply_completed_sync_weave(handle, &session, &proposed_state)?;
        session.applied_steps = vec![step_id];
        session.status = "applied".to_string();
        session.updated_at_unix = unix_now();
        write_weave_session(&handle.layrs_dir, &session)?;
        write_active_weave_marker(&handle.layrs_dir, None)?;
        return Ok(None);
    }

    session.conflicts = conflicts;
    session.status = "conflicted".to_string();
    session.updated_at_unix = unix_now();
    write_json(
        &proposed_state_path(&handle.layrs_dir, &weave_id),
        &storage_state(&handle.layrs_dir, &proposed_state)?,
    )?;
    write_weave_session(&handle.layrs_dir, &session)?;
    write_active_weave_marker(&handle.layrs_dir, Some(&weave_id))?;
    materialize_state(&handle.root, &proposed_state)?;
    let sync_state_path = write_sync_state(handle, operation, "conflicted", session.conflicts.len(), None)
        .unwrap_or_else(|_| sync_path.to_path_buf());
    Ok(Some(SyncOperationResult {
        local_space: summary_from_handle(handle),
        status: "conflicted".to_string(),
        message: format!(
            "{} retrieved Studio changes but paused with {} conflict(s). Resolve the Weave, then run Sync again.",
            if operation == "receive" { "Receive" } else { "Sync" },
            session.conflicts.len(),
        ),
        sync_state_path: sync_state_path.display().to_string(),
    }))
}

pub fn compact_local_space(local_space: String) -> Result<CompactLocalSpaceResult, String> {
    let handle = open_local_space_handle(&local_space)?;
    let compacted: CompactStoreResult = compact_loose_chunks(&handle.layrs_dir)?;
    Ok(CompactLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        packed_chunks: compacted.packed_chunks,
        loose_chunks_removed: compacted.loose_chunks_removed,
        raw_bytes: compacted.raw_bytes,
        stored_bytes: compacted.stored_bytes,
        pack_path: compacted.pack_path,
    })
}

pub fn save_local_step(local_space: String) -> Result<SaveLocalStepResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let layer_id = handle.active.layer_id.clone();
    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &layer_id).ok();
    let (added, modified, deleted) = diff_state(previous_state.as_ref(), &current_state);
    let changed_files = added.len() + modified.len() + deleted.len();

    if changed_files == 0 {
        return Ok(SaveLocalStepResult {
            local_space: summary_from_handle(&handle),
            status: "clean".to_string(),
            message: "Nothing to save.".to_string(),
            step_id: None,
            changed_files: 0,
            diff_stats: DiffStats::default(),
            pending_publish_count: read_pending_publish_files(&handle.layrs_dir, &layer_id)?.len(),
        });
    }

    let diffs = lens_diff_entries(
        &handle,
        "workingTree",
        &layer_id,
        None,
        previous_state.as_ref(),
        &current_state,
        &added,
        &modified,
        &deleted,
    );
    let stats = diff_stats(&diffs);
    let step_id = write_step(&handle.layrs_dir, &layer_id, &current_state)?;
    let step = read_step_file(&handle.layrs_dir, &layer_id, &step_id)?;
    write_pending_publish(&handle.layrs_dir, &step)?;
    propagate_step_to_linked_children(&mut handle, &layer_id, &step_id)?;
    write_working_state(&handle.layrs_dir, &layer_id, &current_state)?;
    let pending_publish_count = read_pending_publish_files(&handle.layrs_dir, &layer_id)?.len();

    Ok(SaveLocalStepResult {
        local_space: summary_from_handle(&handle),
        status: "saved".to_string(),
        message: format!("Step saved with {changed_files} changed file(s)."),
        step_id: Some(step_id),
        changed_files,
        diff_stats: stats,
        pending_publish_count,
    })
}

pub fn publish_local_space(local_space: String) -> Result<SyncOperationResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    if handle.meta.state == LOCAL_SPACE_STATE_DRAFT {
        return Err("Draft Local Spaces must be sent to Studio before publishing.".to_string());
    }
    ensure_linked_space_ready(&handle)?;
    if let Some(weave_id) = active_weave_id(&handle.layrs_dir)? {
        let session = read_weave_session(&handle.layrs_dir, &weave_id)?;
        return Err(format!(
            "A Weave is already {}. Resolve/continue or abort it before publishing.",
            session.status
        ));
    }

    let config = DesktopConfig::load_or_create()?;
    let mut layer_id = handle.active.layer_id.clone();
    if !handle
        .meta
        .layers
        .iter()
        .any(|layer| layer.layer_id == layer_id)
    {
        return Err(format!(
            "Layrs Desktop does not know active Layer {layer_id}. Switch to a known Layer before publishing."
        ));
    }
    let mut force_full_publish = false;
    if !is_probably_server_layer_id(&layer_id) {
        let linked_layer_id = link_active_local_layer_for_sync(&mut handle, "publish")?;
        layer_id = linked_layer_id;
        force_full_publish = true;
    }
    let needs_initial_publish = layer_needs_initial_publish(&handle, &layer_id)?;
    if needs_initial_publish {
        force_full_publish = true;
    }
    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let previous_state = if force_full_publish {
        None
    } else {
        read_layer_index(&handle.layrs_dir, &layer_id).ok()
    };
    let (added, modified, deleted) = diff_state(previous_state.as_ref(), &current_state);
    let changed_files = added.len() + modified.len() + deleted.len();
    if changed_files == 0 && !needs_initial_publish {
        clear_pending_publish(&handle.layrs_dir, &layer_id)?;
        let path = write_sync_state(&handle, "publish", "clean", 0, None)?;
        return Ok(SyncOperationResult {
            local_space: summary_from_handle(&handle),
            status: "clean".to_string(),
            message: "No local changes to publish.".to_string(),
            sync_state_path: path.display().to_string(),
        });
    }
    let latest_pending_step = latest_pending_publish_step(&handle.layrs_dir, &layer_id)?;
    match latest_pending_step {
        Some(step) if step.root_tree_id == current_state.root_tree_id => {}
        _ if config.auto_local_steps => {
            let step_id = write_step(&handle.layrs_dir, &layer_id, &current_state)?;
            let step = read_step_file(&handle.layrs_dir, &layer_id, &step_id)?;
            write_pending_publish(&handle.layrs_dir, &step)?;
        }
        _ => {}
    }
    let publish_steps =
        publish_steps_for_layer(&handle.layrs_dir, &layer_id, needs_initial_publish)?;

    let publish_paths = added
        .iter()
        .chain(modified.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    let publish_path = format!(
        "/v1/workspaces/{}/spaces/{}/sync/publish",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id)
    );
    let body = build_publish_v2_request(
        &handle,
        &config,
        &layer_id,
        previous_state
            .as_ref()
            .and_then(|state| state.root_tree_id.clone()),
        &current_state,
        publish_paths
            .iter()
            .chain(deleted.iter())
            .cloned()
            .collect(),
        deleted.clone(),
        &publish_steps,
    )?;
    let token = desktop_token(&config)?;
    upload_publish_chunks(
        &handle,
        &config.server_endpoint,
        Some(&token),
        &handle.meta.workspace_id,
        &handle.meta.space_id,
        &body.store_objects,
    )?;
    let response: Value =
        match post_json(&config.server_endpoint, &publish_path, Some(&token), &body) {
            Ok(response) => response,
            Err(error) if is_layer_not_found_error(&error) => {
                let path = write_sync_state(
                    &handle,
                    "publish",
                    "blocked-unlinked-layer",
                    changed_files,
                    None,
                )?;
                return Err(format!(
                    "{} Sync state: {}",
                    unlinked_layer_message(&layer_id),
                    path.display()
                ));
            }
            Err(error) => return Err(error),
        };
    write_layer_state(&handle.layrs_dir, &layer_id, &current_state)?;
    clear_pending_publish(&handle.layrs_dir, &layer_id)?;
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    remember_local_space(&handle.meta, Some(layer_id.clone()))?;

    let server_cursor = response
        .get("serverCursor")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let sync_path = write_sync_state(
        &handle,
        "publish",
        "published",
        changed_files,
        server_cursor,
    )?;

    Ok(SyncOperationResult {
        local_space: summary_from_handle(&handle),
        status: "published".to_string(),
        message: format!(
            "Published {changed_files} local change(s) and {} pending step(s) to Studio.",
            publish_steps.len()
        ),
        sync_state_path: sync_path.display().to_string(),
    })
}

fn layer_needs_initial_publish(
    handle: &LocalSpaceHandle,
    layer_id: &str,
) -> Result<bool, String> {
    let path = layer_dir(&handle.layrs_dir, layer_id).join("sync-state.json");
    if !path.exists() {
        return Ok(false);
    }
    let value = read_json::<Value>(&path)?;
    Ok(value
        .get("needsInitialPublish")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

fn publish_steps_for_layer(
    layrs_dir: &Path,
    layer_id: &str,
    needs_initial_publish: bool,
) -> Result<Vec<StepFile>, String> {
    if needs_initial_publish {
        let steps = sorted_steps(layrs_dir, layer_id)?;
        if !steps.is_empty() {
            return Ok(steps);
        }
    }
    pending_publish_steps(layrs_dir, layer_id)
}

pub fn load_desktop_settings() -> Result<DesktopSettings, String> {
    Ok(DesktopConfig::load_or_create()?.settings())
}

pub fn save_desktop_settings(settings: DesktopSettings) -> Result<DesktopSettings, String> {
    if !settings.server_endpoint.trim().starts_with("http://") {
        return Err(
            "Layrs Desktop currently accepts only http:// server endpoints for local development."
                .to_string(),
        );
    }
    if settings.shortcuts.enabled {
        let save_step = settings.shortcuts.save_step.trim();
        let publish = settings.shortcuts.publish.trim();
        if save_step.is_empty() || publish.is_empty() {
            return Err("Shortcut fields cannot be empty while shortcuts are enabled.".to_string());
        }
        if save_step.eq_ignore_ascii_case(publish) {
            return Err("Save Step and Publish shortcuts must be different.".to_string());
        }
    }

    let mut config = DesktopConfig::load_or_create()?;
    config.apply_settings(settings);
    config.save()?;
    Ok(config.settings())
}
