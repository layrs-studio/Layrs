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

    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let pending_publish_count = read_pending_publish_files(&handle.layrs_dir, &layer_id)?.len();
    if pending_publish_count > 0 {
        return Err(
            "Publish pending local Step(s) before receiving Studio state for this Layer."
                .to_string(),
        );
    }
    let previous_state = read_layer_state(&handle.layrs_dir, &layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let saved_step = if changed_files > 0 && config.auto_local_steps {
        Some(write_step(&handle.layrs_dir, &layer_id, &current_state)?)
    } else {
        None
    };
    write_working_state(&handle.layrs_dir, &layer_id, &current_state)?;

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
    let message = if let Some(step_id) = saved_step {
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
    let handle = open_local_space_handle(&local_space)?;
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
    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let previous_state = if force_full_publish {
        None
    } else {
        read_layer_index(&handle.layrs_dir, &layer_id).ok()
    };
    let (added, modified, deleted) = diff_state(previous_state.as_ref(), &current_state);
    let changed_files = added.len() + modified.len() + deleted.len();
    if changed_files == 0 {
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
    let publish_steps = pending_publish_steps(&handle.layrs_dir, &layer_id)?;

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
