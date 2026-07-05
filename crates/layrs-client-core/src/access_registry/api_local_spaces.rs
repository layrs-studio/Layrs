pub fn list_available_spaces() -> Result<Vec<AvailableSpaceView>, String> {
    let (bootstrap, freshness, message) = load_live_bootstrap_or_cache()?;
    let local_spaces = list_local_spaces()?;
    let mut result = Vec::with_capacity(bootstrap.spaces.len());

    for space in bootstrap.spaces {
        let layers: Vec<LayerSummary> = bootstrap
            .layers
            .iter()
            .filter(|layer| layer.space_id == space.id)
            .cloned()
            .collect();
        let matching_local_spaces = local_spaces
            .iter()
            .filter(|local| local.space_id == space.id)
            .cloned()
            .collect();

        result.push(AvailableSpaceView {
            space_id: space.id.clone(),
            workspace_id: space.workspace_id.clone(),
            name: space.name.clone(),
            current_layer_id: space.current_layer_id.clone(),
            layers: access_views(&layers, None)?,
            local_spaces: matching_local_spaces,
            freshness: freshness.clone(),
            message: message.clone(),
        });
    }

    Ok(result)
}

pub fn list_local_spaces() -> Result<Vec<LocalSpaceSummary>, String> {
    let mut config = DesktopConfig::load_or_create()?;
    let mut spaces = Vec::new();
    let mut retained_entries = Vec::new();

    for entry in config.local_spaces.iter().cloned() {
        if let Ok(handle) = open_local_space_at(PathBuf::from(&entry.root_path)) {
            spaces.push(summary_from_handle(&handle));
            retained_entries.push(entry);
        }
    }

    if retained_entries.len() != config.local_spaces.len() {
        config.local_spaces = retained_entries;
        config.save()?;
    }

    Ok(spaces)
}

pub fn create_local_space(
    space_id: String,
    target_folder: String,
    initial_layer_id: Option<String>,
) -> Result<CreateLocalSpaceResult, String> {
    create_local_space_internal(space_id, target_folder, initial_layer_id, true)
}

fn create_local_space_internal(
    space_id: String,
    target_folder: String,
    initial_layer_id: Option<String>,
    receive_from_server: bool,
) -> Result<CreateLocalSpaceResult, String> {
    let config = DesktopConfig::load_or_create()?;
    let bootstrap = load_cached_bootstrap()?.unwrap_or_default();
    let root = absolute_path(&PathBuf::from(target_folder.trim()))?;
    if root.exists() && !root.is_dir() {
        return Err(format!(
            "Layrs Desktop cannot create a Local Space in a file: {}",
            root.display()
        ));
    }

    fs::create_dir_all(&root).map_err(|error| {
        format!(
            "Layrs Desktop could not create Local Space folder {}: {error}",
            root.display()
        )
    })?;

    let (space, mut layers) = bootstrap_space(&bootstrap, &space_id, initial_layer_id.as_deref());
    let active_layer_id = initial_layer_id
        .filter(|value| !value.trim().is_empty())
        .or_else(|| space.current_layer_id.clone())
        .or_else(|| layers.first().map(|layer| layer.id.clone()))
        .unwrap_or_else(|| "main".to_string());

    if !layers.iter().any(|layer| layer.id == active_layer_id) {
        layers.push(ad_hoc_layer(&space, &active_layer_id, "Main"));
    }

    let layer_views = access_views(&layers, Some(&root))?;
    let active_access = layer_views
        .iter()
        .find(|layer| layer.layer_id == active_layer_id)
        .ok_or_else(|| "Layrs Desktop could not resolve the initial Layer.".to_string())?;
    if !active_access.can_open {
        return Err(format!(
            "Layrs Desktop cannot open initial Layer {}: {}",
            active_layer_id,
            active_access
                .reason
                .clone()
                .unwrap_or_else(|| "Layer access is blocked.".to_string())
        ));
    }

    let layrs_dir = root.join(LAYRS_DIR);
    let created = !layrs_dir.join("local-space.json").exists();
    create_local_space_directories(&layrs_dir)?;

    let now = unix_now();
    let local_space_id = format!("local-{}-{}", safe_id_fragment(&space.id), now);
    let existing_meta = read_json::<LocalSpaceFile>(&layrs_dir.join("local-space.json")).ok();
    let local_space_id = existing_meta
        .as_ref()
        .map(|meta| meta.local_space_id.clone())
        .unwrap_or(local_space_id);
    let created_at_unix = existing_meta
        .as_ref()
        .map(|meta| meta.created_at_unix)
        .unwrap_or(now);

    for view in &layer_views {
        scaffold_layer(&layrs_dir, &local_space_id, view)?;
    }

    let metadata_layers = layer_views
        .iter()
        .map(|view| LocalLayerMetadata {
            layer_id: view.layer_id.clone(),
            display_name: view.display_name.clone(),
            parent_layer_id: layers
                .iter()
                .find(|layer| layer.id == view.layer_id)
                .and_then(|layer| layer.parent_layer_id.clone()),
            lineage_status: default_layer_lineage_status(),
            access: view.access.clone(),
            can_open: view.can_open,
        })
        .collect::<Vec<_>>();
    let meta = LocalSpaceFile {
        schema: LOCAL_SPACE_SCHEMA.to_string(),
        state: LOCAL_SPACE_STATE_LINKED.to_string(),
        local_space_id: local_space_id.clone(),
        space_id: space.id.clone(),
        server_space_id: Some(space.id.clone()),
        workspace_id: space.workspace_id.clone(),
        name: space.name.clone(),
        root_path: root.display().to_string(),
        created_at_unix,
        updated_at_unix: now,
        layers: metadata_layers,
    };
    write_json(&layrs_dir.join("local-space.json"), &meta)?;

    let active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: active_layer_id.clone(),
        updated_at_unix: now,
    };
    write_json(&layrs_dir.join("active-layer.json"), &active)?;
    write_access_pointer(
        &layrs_dir,
        &local_space_id,
        Some(&active_layer_id),
        &layer_views,
    )?;

    remember_local_space(&meta, Some(active_layer_id.clone()))?;
    let mut handle = LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
    };

    if receive_from_server {
        if let Err(error) = receive_linked_space_state(&mut handle, &config, true) {
            if created {
                let _ = fs::remove_dir_all(&handle.layrs_dir);
            }
            let _ = remove_local_space_config_entry(&handle.meta.local_space_id, &handle.root);
            return Err(format!(
                "Layrs Desktop could not copy Space {} from Studio: {error}",
                space.name
            ));
        }
    } else {
        let state = capture_working_state(&handle.root, &active_layer_id, true)?;
        write_layer_state(&handle.layrs_dir, &active_layer_id, &state)?;
    }

    Ok(CreateLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        created,
    })
}

pub fn create_draft_local_space(
    name: String,
    target_folder: String,
) -> Result<CreateLocalSpaceResult, String> {
    let display_name = if name.trim().is_empty() {
        "Untitled Space".to_string()
    } else {
        name.trim().to_string()
    };
    let root = absolute_path(&PathBuf::from(target_folder.trim()))?;
    if root.exists() && !root.is_dir() {
        return Err(format!(
            "Layrs Desktop cannot create a Draft Local Space in a file: {}",
            root.display()
        ));
    }

    fs::create_dir_all(&root).map_err(|error| {
        format!(
            "Layrs Desktop could not create Draft Local Space folder {}: {error}",
            root.display()
        )
    })?;

    let layrs_dir = root.join(LAYRS_DIR);
    if layrs_dir.join("local-space.json").exists() {
        return Err(format!(
            "Layrs Desktop found an existing Local Space at {}.",
            root.display()
        ));
    }

    create_local_space_directories(&layrs_dir)?;
    let now = unix_now();
    let local_space_id = format!("local_space_{}", now);
    let layer_id = "local_layer_main".to_string();
    let layer_view = LayerAccessView {
        layer_id: layer_id.clone(),
        workspace_id: String::new(),
        space_id: local_space_id.clone(),
        display_name: "Main".to_string(),
        access: LayerAccessKind::Open,
        can_open: true,
        local_path: Some(layer_dir(&layrs_dir, &layer_id).display().to_string()),
        reason: None,
    };
    scaffold_layer(&layrs_dir, &local_space_id, &layer_view)?;

    let meta = LocalSpaceFile {
        schema: LOCAL_SPACE_SCHEMA.to_string(),
        state: LOCAL_SPACE_STATE_DRAFT.to_string(),
        local_space_id: local_space_id.clone(),
        space_id: local_space_id.clone(),
        server_space_id: None,
        workspace_id: String::new(),
        name: display_name,
        root_path: root.display().to_string(),
        created_at_unix: now,
        updated_at_unix: now,
        layers: vec![LocalLayerMetadata {
            layer_id: layer_id.clone(),
            display_name: "Main".to_string(),
            parent_layer_id: None,
            lineage_status: default_layer_lineage_status(),
            access: LayerAccessKind::Open,
            can_open: true,
        }],
    };
    write_json(&layrs_dir.join("local-space.json"), &meta)?;
    let active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        updated_at_unix: now,
    };
    write_json(&layrs_dir.join("active-layer.json"), &active)?;
    write_access_pointer(&layrs_dir, &local_space_id, Some(&layer_id), &[layer_view])?;

    let state = capture_working_state(&root, &layer_id, true)?;
    write_layer_state(&layrs_dir, &layer_id, &state)?;
    remember_local_space(&meta, Some(layer_id.clone()))?;

    let handle = LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
    };

    Ok(CreateLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        created: true,
    })
}

pub fn init_local_space(
    name: String,
    target_folder: String,
) -> Result<InitLocalSpaceResult, String> {
    let display_name = if name.trim().is_empty() {
        "Untitled Space".to_string()
    } else {
        name.trim().to_string()
    };
    let root = absolute_path(&PathBuf::from(target_folder.trim()))?;
    if root.exists() && !root.is_dir() {
        return Err(format!(
            "Layrs cannot initialize a Local Space in a file: {}",
            root.display()
        ));
    }

    fs::create_dir_all(&root).map_err(|error| {
        format!(
            "Layrs could not create Local Space folder {}: {error}",
            root.display()
        )
    })?;

    let layrs_dir = root.join(LAYRS_DIR);
    if layrs_dir.join("local-space.json").exists() {
        return Err(format!(
            "Layrs found an existing Local Space at {}.",
            root.display()
        ));
    }

    create_local_space_directories(&layrs_dir)?;
    let now = unix_now();
    let local_space_id = format!("local_space_{}", now);
    let layer_id = "local_layer_main".to_string();
    let layer_view = LayerAccessView {
        layer_id: layer_id.clone(),
        workspace_id: String::new(),
        space_id: local_space_id.clone(),
        display_name: "Main".to_string(),
        access: LayerAccessKind::Open,
        can_open: true,
        local_path: Some(layer_dir(&layrs_dir, &layer_id).display().to_string()),
        reason: None,
    };
    scaffold_layer(&layrs_dir, &local_space_id, &layer_view)?;

    let meta = LocalSpaceFile {
        schema: LOCAL_SPACE_SCHEMA.to_string(),
        state: LOCAL_SPACE_STATE_DRAFT.to_string(),
        local_space_id: local_space_id.clone(),
        space_id: local_space_id.clone(),
        server_space_id: None,
        workspace_id: String::new(),
        name: display_name,
        root_path: root.display().to_string(),
        created_at_unix: now,
        updated_at_unix: now,
        layers: vec![LocalLayerMetadata {
            layer_id: layer_id.clone(),
            display_name: "Main".to_string(),
            parent_layer_id: None,
            lineage_status: default_layer_lineage_status(),
            access: LayerAccessKind::Open,
            can_open: true,
        }],
    };
    write_json(&layrs_dir.join("local-space.json"), &meta)?;
    let active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        updated_at_unix: now,
    };
    write_json(&layrs_dir.join("active-layer.json"), &active)?;
    write_access_pointer(&layrs_dir, &local_space_id, Some(&layer_id), &[layer_view])?;

    let empty_state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        captured_at_unix: now,
        root_tree_id: None,
        files: Vec::new(),
    };
    write_layer_state(&layrs_dir, &layer_id, &empty_state)?;
    remember_local_space(&meta, Some(layer_id.clone()))?;

    let handle = LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
    };
    let current_state = capture_working_state(&handle.root, &layer_id, true)?;
    let scanned_files = current_state.files.len();
    let initial_step_id = if scanned_files > 0 {
        let step_id = write_step(&handle.layrs_dir, &layer_id, &current_state)?;
        let step = read_step_file(&handle.layrs_dir, &layer_id, &step_id)?;
        write_pending_publish(&handle.layrs_dir, &step)?;
        write_working_state(&handle.layrs_dir, &layer_id, &current_state)?;
        Some(step_id)
    } else {
        None
    };
    let pending_publish_count = read_pending_publish_files(&handle.layrs_dir, &layer_id)?.len();

    Ok(InitLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        created: true,
        initial_step_id,
        scanned_files,
        pending_publish_count,
    })
}

pub fn send_draft_local_space(
    local_space: String,
    workspace_id: String,
) -> Result<SendDraftLocalSpaceResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    if handle.meta.state != LOCAL_SPACE_STATE_DRAFT {
        return Err("Only Draft Local Spaces can be sent to Studio.".to_string());
    }
    let workspace_id = workspace_id.trim().to_string();
    if workspace_id.is_empty() {
        return Err("Choose a Workspace before sending this Draft Local Space.".to_string());
    }

    let config = DesktopConfig::load_or_create()?;
    let token = desktop_token(&config)?;
    let active_layer_id = handle.active.layer_id.clone();
    let current_state = capture_working_state(&handle.root, &active_layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &active_layer_id).ok();
    if changed_file_count(previous_state.as_ref(), &current_state) > 0 && config.auto_local_steps {
        write_step(&handle.layrs_dir, &active_layer_id, &current_state)?;
    }
    write_layer_state(&handle.layrs_dir, &active_layer_id, &current_state)?;

    let request = CreateSpaceFromLocalRequest {
        name: handle.meta.name.clone(),
        description: None,
        local_space_id: handle.meta.local_space_id.clone(),
        layers: handle
            .meta
            .layers
            .iter()
            .map(|layer| LocalLayerImportRequest {
                local_layer_id: layer.layer_id.clone(),
                name: layer.display_name.clone(),
                parent_local_layer_id: layer.parent_layer_id.clone(),
            })
            .collect(),
    };
    let import_path = format!(
        "/v1/workspaces/{}/spaces/from-local",
        url_path_segment(&workspace_id)
    );
    let created: CreateSpaceFromLocalResponse = post_json(
        &config.server_endpoint,
        &import_path,
        Some(&token),
        &request,
    )?;
    if created.layer_mappings.is_empty() {
        return Err(
            "Layrs server did not return Layer mappings for the imported Draft.".to_string(),
        );
    }

    let mut published_layers = 0usize;
    for mapping in &created.layer_mappings {
        let state = read_layer_state(&handle.layrs_dir, &mapping.local_layer_id)
            .or_else(|_| read_layer_index(&handle.layrs_dir, &mapping.local_layer_id))?;
        let publish_paths = state
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<BTreeSet<_>>();
        if !publish_paths.is_empty() {
            let pending_steps = pending_publish_steps(&handle.layrs_dir, &mapping.local_layer_id)?;
            let publish_path = format!(
                "/v1/workspaces/{}/spaces/{}/sync/publish",
                url_path_segment(&workspace_id),
                url_path_segment(&created.space.id)
            );
            let body = build_publish_v2_request(
                &handle,
                &config,
                &mapping.server_layer_id,
                None,
                &state,
                publish_paths.iter().cloned().collect(),
                Vec::new(),
                &pending_steps,
            )?;
            upload_publish_chunks(
                &handle,
                &config.server_endpoint,
                Some(&token),
                &workspace_id,
                &created.space.id,
                &body.store_objects,
            )?;
            let _: Value = post_json(&config.server_endpoint, &publish_path, Some(&token), &body)?;
        }
        published_layers += 1;
    }

    apply_server_mapping_to_draft(&mut handle, &created)?;
    if let Ok(bootstrap) = get_json::<BootstrapData>(
        &config.server_endpoint,
        "/v1/desktop/bootstrap",
        Some(&token),
    )
    .and_then(|bootstrap| validate_desktop_bootstrap(bootstrap, "/v1/desktop/bootstrap"))
    {
        let _ = save_cached_bootstrap(&bootstrap);
    }

    Ok(SendDraftLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        workspace_id,
        server_space_id: created.space.id,
        layer_mappings: created.layer_mappings,
        published_layers,
    })
}

pub fn open_local_space(local_space_id_or_path: String) -> Result<LocalSpaceSummary, String> {
    let handle = open_local_space_handle(&local_space_id_or_path)?;
    remember_local_space(&handle.meta, Some(handle.active.layer_id.clone()))?;
    Ok(summary_from_handle(&handle))
}

pub fn forget_local_space(local_space: String) -> Result<ForgetLocalSpaceResult, String> {
    let mut config = DesktopConfig::load_or_create()?;
    let selector = local_space.trim();
    let opened = open_local_space_handle(selector).ok();
    let entry = opened.as_ref().map(|handle| LocalSpaceConfigEntry {
        local_space_id: handle.meta.local_space_id.clone(),
        space_id: handle.meta.space_id.clone(),
        root_path: handle.root.display().to_string(),
        active_layer_id: Some(handle.active.layer_id.clone()),
        updated_at_unix: unix_now(),
    })
    .or_else(|| find_local_space_config_entry(&config, selector))
    .ok_or_else(|| {
        format!(
            "Layrs Desktop could not find a Local Space entry for {selector}. It may already be disconnected."
        )
    })?;

    let local_space_id = entry.local_space_id.clone();
    let root = opened
        .as_ref()
        .map(|handle| handle.root.clone())
        .unwrap_or_else(|| PathBuf::from(&entry.root_path));
    let root = absolute_path(&root).unwrap_or(root);
    let archived_layrs_path = archive_layrs_dir_at(&root, &root.join(LAYRS_DIR))?;
    let root_path = root.display().to_string();
    let root_key = path_compare_key(&root);
    config.local_spaces.retain(|entry| {
        entry.local_space_id != local_space_id
            && path_compare_key(&PathBuf::from(&entry.root_path)) != root_key
    });
    config.save()?;

    let message = if archived_layrs_path.is_some() {
        "Local Space was forgotten on this machine. Project files were kept, and the old .layrs metadata was archived."
    } else {
        "Local Space was disconnected on this machine. Project files were kept, and local .layrs metadata was already absent."
    };

    Ok(ForgetLocalSpaceResult {
        local_space_id,
        root_path,
        archived_layrs_path: archived_layrs_path.map(|path| path.display().to_string()),
        message: message.to_string(),
    })
}

pub fn switch_layer(
    local_space: String,
    target_layer_id: String,
) -> Result<LayerSwitchResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let previous_layer_id = handle.active.layer_id.clone();
    let target_layer = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == target_layer_id)
        .cloned()
        .ok_or_else(|| format!("Layrs Desktop does not know Layer {target_layer_id}."))?;

    if !target_layer.can_open {
        return Err(format!(
            "Layrs Desktop cannot switch to Layer {} because its access is {:?}.",
            target_layer.layer_id, target_layer.access
        ));
    }

    let config = DesktopConfig::load_or_create()?;
    let current_state = capture_working_state(&handle.root, &previous_layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &previous_layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let saved_step_id = if changed_files > 0 && config.auto_local_steps {
        let step_id =
            write_step_and_pending_publish(&handle.layrs_dir, &previous_layer_id, &current_state)?;
        propagate_step_to_linked_children(&mut handle, &previous_layer_id, &step_id)?;
        Some(step_id)
    } else {
        None
    };
    write_working_state(&handle.layrs_dir, &previous_layer_id, &current_state)?;

    if previous_layer_id != target_layer_id {
        let target_state = read_layer_state(&handle.layrs_dir, &target_layer_id)
            .or_else(|_| read_layer_index(&handle.layrs_dir, &target_layer_id))?;
        materialize_state(&handle.root, &target_state)?;

        handle.active = ActiveLayerFile {
            schema: ACTIVE_LAYER_SCHEMA.to_string(),
            layer_id: target_layer_id.clone(),
            updated_at_unix: unix_now(),
        };
        write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
        write_access_pointer_from_meta(&handle)?;
        remember_local_space(&handle.meta, Some(target_layer_id.clone()))?;
    }

    Ok(LayerSwitchResult {
        local_space: summary_from_handle(&handle),
        previous_layer_id,
        active_layer_id: target_layer_id,
        saved_step_id,
        changed_files,
    })
}

pub fn create_layer_from_current(
    local_space: String,
    name: String,
) -> Result<LayerSwitchResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let previous_layer_id = handle.active.layer_id.clone();
    let display_name = if name.trim().is_empty() {
        "New Layer".to_string()
    } else {
        name.trim().to_string()
    };

    let current_state = capture_working_state(&handle.root, &previous_layer_id, true)?;
    let previous_state = read_layer_state(&handle.layrs_dir, &previous_layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let saved_step_id = if changed_files > 0 {
        let step_id =
            write_step_and_pending_publish(&handle.layrs_dir, &previous_layer_id, &current_state)?;
        propagate_step_to_linked_children(&mut handle, &previous_layer_id, &step_id)?;
        Some(step_id)
    } else {
        None
    };
    let layer_id = if is_server_linked_space(&handle) {
        create_linked_server_layer(&handle, &display_name, Some(&previous_layer_id))?
    } else {
        unique_layer_id(&handle, &display_name)
    };

    let layer = LocalLayerMetadata {
        layer_id: layer_id.clone(),
        display_name,
        parent_layer_id: Some(previous_layer_id.clone()),
        lineage_status: default_layer_lineage_status(),
        access: LayerAccessKind::Open,
        can_open: true,
    };
    handle.meta.layers.push(layer.clone());
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;

    let view = LayerAccessView {
        layer_id: layer.layer_id.clone(),
        workspace_id: handle.meta.workspace_id.clone(),
        space_id: handle.meta.space_id.clone(),
        display_name: layer.display_name.clone(),
        access: LayerAccessKind::Open,
        can_open: true,
        local_path: Some(
            layer_dir(&handle.layrs_dir, &layer.layer_id)
                .display()
                .to_string(),
        ),
        reason: None,
    };
    scaffold_layer(&handle.layrs_dir, &handle.meta.local_space_id, &view)?;
    inherit_layer_access_file(&handle, &previous_layer_id, &layer_id, &layer.display_name)?;
    let mut new_state = current_state.clone();
    new_state.layer_id = layer_id.clone();
    write_layer_state(&handle.layrs_dir, &layer_id, &new_state)?;
    inherit_parent_steps(&handle.layrs_dir, &previous_layer_id, &layer_id)?;
    if is_server_linked_space(&handle) {
        write_linked_layer_sync_state(&handle, &layer_id, Some(&previous_layer_id))?;
    }

    handle.active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        updated_at_unix: unix_now(),
    };
    write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
    write_access_pointer_from_meta(&handle)?;
    remember_local_space(&handle.meta, Some(layer_id.clone()))?;

    Ok(LayerSwitchResult {
        local_space: summary_from_handle(&handle),
        previous_layer_id,
        active_layer_id: layer_id,
        saved_step_id,
        changed_files,
    })
}

pub fn delete_layer(local_space: String, layer_id: String) -> Result<DeleteLayerResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let target_layer_id = layer_id.trim().to_string();
    if target_layer_id.is_empty() {
        return Err("Choose a Layer to delete.".to_string());
    }
    if handle.active.layer_id == target_layer_id {
        return Err("Switch to another Layer before deleting this one.".to_string());
    }
    if handle.meta.layers.len() <= 1 {
        return Err("A Local Space must keep at least one Layer.".to_string());
    }
    if handle
        .meta
        .layers
        .iter()
        .any(|layer| layer.parent_layer_id.as_deref() == Some(target_layer_id.as_str()))
    {
        return Err("Delete child Layers before deleting their parent Layer.".to_string());
    }

    let target_index = handle
        .meta
        .layers
        .iter()
        .position(|layer| layer.layer_id == target_layer_id)
        .ok_or_else(|| format!("Layrs Desktop does not know Layer {target_layer_id}."))?;
    let target_layer = handle.meta.layers[target_index].clone();

    if is_server_linked_space(&handle) && is_probably_server_layer_id(&target_layer_id) {
        enqueue_pending_layer_deletion(
            &handle.layrs_dir,
            &target_layer_id,
            &target_layer.display_name,
        )?;
        if let Ok(config) = DesktopConfig::load_or_create() {
            let _ = flush_pending_layer_deletions(&handle, &config);
        }
    }

    let layer_path = layer_dir(&handle.layrs_dir, &target_layer_id);
    if layer_path.exists() {
        fs::remove_dir_all(&layer_path).map_err(|error| {
            format!(
                "Layrs Desktop could not remove local Layer directory {}: {error}",
                layer_path.display()
            )
        })?;
    }

    handle.meta.layers.remove(target_index);
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_access_pointer_from_meta(&handle)?;
    remember_local_space(&handle.meta, Some(handle.active.layer_id.clone()))?;

    Ok(DeleteLayerResult {
        local_space: summary_from_handle(&handle),
        deleted_layer_id: target_layer_id,
        message: "Layer deleted locally. Sync will also delete it from Studio if it exists there."
            .to_string(),
    })
}

pub fn disconnect_layer_from_parent(
    local_space: String,
    layer_id: String,
) -> Result<LayerSettingsResult, String> {
    let mut handle = open_local_space_handle(&local_space)?;
    let target_layer_id = resolve_known_layer_id(&handle, &layer_id)?;
    let layer = handle
        .meta
        .layers
        .iter_mut()
        .find(|layer| layer.layer_id == target_layer_id)
        .ok_or_else(|| format!("Layrs Desktop does not know Layer {target_layer_id}."))?;

    if layer.parent_layer_id.is_none() {
        return Err(format!(
            "Layer {} has no parent to disconnect from.",
            layer.display_name
        ));
    }

    layer.lineage_status = "unlinked".to_string();
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_access_pointer_from_meta(&handle)?;
    remember_local_space(&handle.meta, Some(handle.active.layer_id.clone()))?;

    Ok(LayerSettingsResult {
        local_space: summary_from_handle(&handle),
        layer_id: target_layer_id,
        message: "Layer disconnected from its parent. Future parent Steps will not propagate automatically.".to_string(),
        archived_steps_path: None,
    })
}

pub fn clear_layer_steps(
    local_space: String,
    layer_id: String,
    archive: bool,
) -> Result<LayerSettingsResult, String> {
    let handle = open_local_space_handle(&local_space)?;
    if active_weave_id(&handle.layrs_dir)?.is_some() {
        return Err("Finish or abort the active Weave before clearing Layer Steps.".to_string());
    }

    let target_layer_id = resolve_known_layer_id(&handle, &layer_id)?;
    let layer_path = layer_dir(&handle.layrs_dir, &target_layer_id);
    let steps_dir = layer_path.join("steps");
    let pending_dir = pending_publish_dir(&handle.layrs_dir, &target_layer_id);
    let archive_path = if archive {
        Some(layer_path.join("archived-steps").join(format!(
            "{}-{}",
            unix_now(),
            safe_id_fragment(&target_layer_id)
        )))
    } else {
        None
    };

    let mut moved = 0usize;
    if let Some(archive_path) = archive_path.as_ref() {
        fs::create_dir_all(archive_path.join("steps")).map_err(|error| {
            format!(
                "Layrs Desktop could not create Layer Step archive {}: {error}",
                archive_path.display()
            )
        })?;
        fs::create_dir_all(archive_path.join("pending-publish")).map_err(|error| {
            format!(
                "Layrs Desktop could not create pending publish archive {}: {error}",
                archive_path.display()
            )
        })?;
    }

    let steps_archive = archive_path.as_ref().map(|path| path.join("steps"));
    let pending_archive = archive_path.as_ref().map(|path| path.join("pending-publish"));
    moved += move_or_remove_json_files(&steps_dir, steps_archive.as_deref())?;
    moved += move_or_remove_json_files(&pending_dir, pending_archive.as_deref())?;

    Ok(LayerSettingsResult {
        local_space: summary_from_handle(&handle),
        layer_id: target_layer_id,
        message: format!(
            "Layer Steps cleared from the active history. {moved} metadata file(s) were {} and project files were kept.",
            if archive { "archived" } else { "removed" }
        ),
        archived_steps_path: archive_path.map(|path| path.display().to_string()),
    })
}

pub fn weave_active_layer_to_parent(
    local_space: String,
    preview: bool,
) -> Result<WeaveOperationResult, String> {
    let handle = open_local_space_handle(&local_space)?;
    let active_layer_id = handle.active.layer_id.clone();
    let active_layer = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == active_layer_id)
        .ok_or_else(|| format!("Layrs Desktop does not know active Layer {active_layer_id}."))?;
    let parent_layer_id = active_layer.parent_layer_id.clone().ok_or_else(|| {
        format!(
            "Layer {} has no parent to Weave into.",
            active_layer.display_name
        )
    })?;
    if active_layer.lineage_status != "linked" {
        return Err(format!(
            "Layer {} is disconnected from its parent.",
            active_layer.display_name
        ));
    }
    weave_layers(local_space, active_layer_id, parent_layer_id, preview)
}

fn resolve_known_layer_id(handle: &LocalSpaceHandle, selector: &str) -> Result<String, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("Choose a Layer.".to_string());
    }
    handle
        .meta
        .layers
        .iter()
        .find(|layer| {
            layer.layer_id == selector || layer.display_name.eq_ignore_ascii_case(selector)
        })
        .map(|layer| layer.layer_id.clone())
        .ok_or_else(|| format!("Layrs Desktop does not know Layer {selector}."))
}

fn move_or_remove_json_files(dir: &Path, archive_dir: Option<&Path>) -> Result<usize, String> {
    if !dir.exists() {
        return Ok(0);
    }

    let mut moved = 0usize;
    for entry in fs::read_dir(dir).map_err(|error| {
        format!(
            "Layrs Desktop could not read Layer metadata directory {}: {error}",
            dir.display()
        )
    })? {
        let path = entry
            .map_err(|error| {
                format!(
                    "Layrs Desktop could not read Layer metadata entry {}: {error}",
                    dir.display()
                )
            })?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        if let Some(archive_dir) = archive_dir {
            fs::create_dir_all(archive_dir).map_err(|error| {
                format!(
                    "Layrs Desktop could not create archive directory {}: {error}",
                    archive_dir.display()
                )
            })?;
            let file_name = path.file_name().ok_or_else(|| {
                format!(
                    "Layrs Desktop could not read Layer metadata filename {}.",
                    path.display()
                )
            })?;
            let target = archive_dir.join(file_name);
            fs::rename(&path, &target).map_err(|error| {
                format!(
                    "Layrs Desktop could not archive Layer metadata {} to {}: {error}",
                    path.display(),
                    target.display()
                )
            })?;
        } else {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "Layrs Desktop could not remove Layer metadata {}: {error}",
                    path.display()
                )
            })?;
        }
        moved += 1;
    }
    Ok(moved)
}

fn create_linked_server_layer(
    handle: &LocalSpaceHandle,
    display_name: &str,
    parent_layer_id: Option<&str>,
) -> Result<String, String> {
    ensure_linked_space_ready(handle)?;
    if let Some(parent_layer_id) = parent_layer_id {
        if !is_probably_server_layer_id(parent_layer_id) {
            return Err(unlinked_layer_message(parent_layer_id));
        }
    }

    let config = DesktopConfig::load_or_create()?;
    let token = desktop_token(&config)?;
    let create_path = format!(
        "/v1/workspaces/{}/spaces/{}/layers",
        url_path_segment(&handle.meta.workspace_id),
        url_path_segment(&handle.meta.space_id)
    );
    let request = CreateServerLayerRequest {
        name: display_name.to_string(),
        parent_layer_id: parent_layer_id.map(ToString::to_string),
    };
    let created: CreatedServerLayer = post_json(
        &config.server_endpoint,
        &create_path,
        Some(&token),
        &request,
    )
    .map_err(|error| {
        parent_layer_id
            .map(|parent| link_layer_error_message(parent, error.clone()))
            .unwrap_or(error)
    })?;
    let server_layer_id = created.id.trim().to_string();
    if server_layer_id.is_empty() {
        return Err("Layrs server created a Layer but did not return its id.".to_string());
    }
    if handle
        .meta
        .layers
        .iter()
        .any(|layer| layer.layer_id == server_layer_id)
    {
        return Err(format!(
            "Layrs server returned Layer {server_layer_id}, but this Local Space already has a Layer with that id."
        ));
    }

    Ok(server_layer_id)
}

fn link_active_local_layer_for_sync(
    handle: &mut LocalSpaceHandle,
    operation: &str,
) -> Result<String, String> {
    let old_layer_id = handle.active.layer_id.clone();
    let display_name = handle
        .meta
        .layers
        .iter()
        .find(|layer| layer.layer_id == old_layer_id)
        .map(|layer| layer.display_name.clone())
        .unwrap_or_else(|| "Local Layer".to_string());
    let parent_layer_id = handle
        .meta
        .layers
        .iter()
        .map(|layer| layer.layer_id.as_str())
        .find(|layer_id| *layer_id != old_layer_id && is_probably_server_layer_id(layer_id))
        .map(str::to_string);
    let current_state = capture_working_state(&handle.root, &old_layer_id, true)?;
    let server_layer_id =
        create_linked_server_layer(handle, &display_name, parent_layer_id.as_deref())?;

    let old_dir = layer_dir(&handle.layrs_dir, &old_layer_id);
    let new_dir = layer_dir(&handle.layrs_dir, &server_layer_id);
    if old_dir != new_dir && old_dir.exists() && !new_dir.exists() {
        fs::rename(&old_dir, &new_dir).map_err(|error| {
            format!(
                "Layrs Desktop could not link local Layer {old_layer_id} to server Layer {server_layer_id}: {error}"
            )
        })?;
    } else if !new_dir.exists() {
        let view = LayerAccessView {
            layer_id: server_layer_id.clone(),
            workspace_id: handle.meta.workspace_id.clone(),
            space_id: handle.meta.space_id.clone(),
            display_name: display_name.clone(),
            access: LayerAccessKind::Open,
            can_open: true,
            local_path: Some(new_dir.display().to_string()),
            reason: None,
        };
        scaffold_layer(&handle.layrs_dir, &handle.meta.local_space_id, &view)?;
    }

    rewrite_layer_files_after_mapping(
        &handle.layrs_dir,
        &old_layer_id,
        &server_layer_id,
        &handle.meta.workspace_id,
        &handle.meta.space_id,
    )?;
    let mut linked_state = current_state;
    linked_state.layer_id = server_layer_id.clone();
    write_layer_state(&handle.layrs_dir, &server_layer_id, &linked_state)?;

    for layer in &mut handle.meta.layers {
        if layer.layer_id == old_layer_id {
            layer.layer_id = server_layer_id.clone();
            layer.display_name = display_name.clone();
            layer.parent_layer_id = parent_layer_id.clone();
        }
    }
    handle.meta.updated_at_unix = unix_now();
    handle.active.layer_id = server_layer_id.clone();
    handle.active.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
    write_access_pointer_from_meta(handle)?;
    let _ = write_sync_state(handle, operation, "linked", 0, None)?;
    remember_local_space(&handle.meta, Some(server_layer_id.clone()))?;
    Ok(server_layer_id)
}

fn inherit_layer_access_file(
    handle: &LocalSpaceHandle,
    parent_layer_id: &str,
    layer_id: &str,
    display_name: &str,
) -> Result<(), String> {
    let parent_access_path = layer_dir(&handle.layrs_dir, parent_layer_id).join("access.json");
    if !parent_access_path.exists() {
        return Ok(());
    }

    let mut access = read_json::<LayerAccessFile>(&parent_access_path)?;
    access.workspace_id = handle.meta.workspace_id.clone();
    access.space_id = handle.meta.space_id.clone();
    access.layer_id = layer_id.to_string();
    access.display_name = display_name.to_string();
    access.generated_at_unix = unix_now();
    write_json(
        &layer_dir(&handle.layrs_dir, layer_id).join("access.json"),
        &access,
    )
}
