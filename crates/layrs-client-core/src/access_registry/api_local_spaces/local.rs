use super::super::*;

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

pub(crate) fn create_local_space_internal(
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
    fs::create_dir_all(&layrs_dir).map_err(|error| {
        format!(
            "Layrs Desktop could not create Local Space directory {}: {error}",
            layrs_dir.display()
        )
    })?;
    let mut store = LocalStoreSession::open_exclusive(&layrs_dir)?;
    create_local_space_directories(&layrs_dir)?;
    let created = !store.exists(&layrs_dir.join("local-space.json"))?;

    let now = unix_now();
    let local_space_id = format!("local-{}-{}", safe_id_fragment(&space.id), now);
    let existing_meta =
        read_json::<LocalSpaceFile>(&store, &layrs_dir.join("local-space.json")).ok();
    let local_space_id = existing_meta
        .as_ref()
        .map(|meta| meta.local_space_id.clone())
        .unwrap_or(local_space_id);
    let created_at_unix = existing_meta
        .as_ref()
        .map(|meta| meta.created_at_unix)
        .unwrap_or(now);

    for view in &layer_views {
        scaffold_layer(&mut store, &layrs_dir, &local_space_id, view)?;
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
    write_json(&mut store, &layrs_dir.join("local-space.json"), &meta)?;

    let active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: active_layer_id.clone(),
        updated_at_unix: now,
    };
    write_json(&mut store, &layrs_dir.join("active-layer.json"), &active)?;
    write_access_pointer(
        &mut store,
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
        store,
    };

    if receive_from_server {
        let cleanup_layrs_dir = handle.layrs_dir.clone();
        let cleanup_local_space_id = handle.meta.local_space_id.clone();
        let cleanup_root = handle.root.clone();
        match receive_linked_space_state(handle, &config, true) {
            Ok((received, _)) => handle = received,
            Err(error) => {
                if created {
                    let _ = fs::remove_dir_all(&cleanup_layrs_dir);
                }
                let _ = remove_local_space_config_entry(&cleanup_local_space_id, &cleanup_root);
                return Err(format!(
                    "Layrs Desktop could not copy Space {} from Studio: {error}",
                    space.name
                ));
            }
        }
    } else {
        let state = capture_working_state(&mut handle.store, &handle.root, &active_layer_id)?;
        write_layer_state(
            &mut handle.store,
            &handle.layrs_dir,
            &active_layer_id,
            &state,
        )?;
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
    fs::create_dir_all(&layrs_dir).map_err(|error| {
        format!(
            "Layrs Desktop could not create Draft Local Space directory {}: {error}",
            layrs_dir.display()
        )
    })?;
    let mut store = LocalStoreSession::open_exclusive(&layrs_dir)?;
    create_local_space_directories(&layrs_dir)?;
    if store.exists(&layrs_dir.join("local-space.json"))? {
        return Err(format!(
            "Layrs Desktop found an existing Local Space at {}.",
            root.display()
        ));
    }

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
    scaffold_layer(&mut store, &layrs_dir, &local_space_id, &layer_view)?;

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
    write_json(&mut store, &layrs_dir.join("local-space.json"), &meta)?;
    let active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        updated_at_unix: now,
    };
    write_json(&mut store, &layrs_dir.join("active-layer.json"), &active)?;
    write_access_pointer(
        &mut store,
        &layrs_dir,
        &local_space_id,
        Some(&layer_id),
        &[layer_view],
    )?;

    let state = capture_working_state(&mut store, &root, &layer_id)?;
    write_layer_state(&mut store, &layrs_dir, &layer_id, &state)?;
    remember_local_space(&meta, Some(layer_id.clone()))?;

    let handle = LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
        store,
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
    fs::create_dir_all(&layrs_dir).map_err(|error| {
        format!(
            "Layrs could not create Local Space directory {}: {error}",
            layrs_dir.display()
        )
    })?;
    let mut store = LocalStoreSession::open_exclusive(&layrs_dir)?;
    create_local_space_directories(&layrs_dir)?;
    if store.exists(&layrs_dir.join("local-space.json"))? {
        return Err(format!(
            "Layrs found an existing Local Space at {}.",
            root.display()
        ));
    }

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
    scaffold_layer(&mut store, &layrs_dir, &local_space_id, &layer_view)?;

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
    write_json(&mut store, &layrs_dir.join("local-space.json"), &meta)?;
    let active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        updated_at_unix: now,
    };
    write_json(&mut store, &layrs_dir.join("active-layer.json"), &active)?;
    write_access_pointer(
        &mut store,
        &layrs_dir,
        &local_space_id,
        Some(&layer_id),
        &[layer_view],
    )?;

    let empty_state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        captured_at_unix: now,
        root_tree_id: None,
        files: Vec::new(),
    };
    write_layer_state(&mut store, &layrs_dir, &layer_id, &empty_state)?;
    remember_local_space(&meta, Some(layer_id.clone()))?;

    let mut handle = LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
        store,
    };
    let current_state = capture_working_state(&mut handle.store, &handle.root, &layer_id)?;
    let scanned_files = current_state.files.len();
    let initial_step_id = if scanned_files > 0 {
        let step_id = write_step(
            &mut handle.store,
            &handle.layrs_dir,
            &layer_id,
            &current_state,
        )?;
        let step = read_step_file(&handle.store, &handle.layrs_dir, &layer_id, &step_id)?;
        write_pending_publish(&mut handle.store, &handle.layrs_dir, &step)?;
        write_working_state(
            &mut handle.store,
            &handle.layrs_dir,
            &layer_id,
            &current_state,
        )?;
        Some(step_id)
    } else {
        None
    };
    let pending_publish_count =
        read_pending_publish_files(&handle.store, &handle.layrs_dir, &layer_id)?.len();

    Ok(InitLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        created: true,
        initial_step_id,
        scanned_files,
        pending_publish_count,
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
    drop(opened);
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
