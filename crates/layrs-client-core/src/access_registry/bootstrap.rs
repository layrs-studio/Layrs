fn load_live_bootstrap_or_cache() -> Result<(BootstrapData, String, Option<String>), String> {
    let config = DesktopConfig::load_or_create()?;
    match desktop_token(&config).and_then(|token| {
        let bootstrap = get_json::<BootstrapData>(
            &config.server_endpoint,
            "/v1/desktop/bootstrap",
            Some(&token),
        )?;
        validate_desktop_bootstrap(bootstrap, "/v1/desktop/bootstrap")
    }) {
        Ok(bootstrap) => {
            save_cached_bootstrap(&bootstrap)?;
            Ok((bootstrap, "fresh".to_string(), None))
        }
        Err(error) if is_invalid_desktop_token_error(&error) => {
            clear_desktop_session(&OsSecretStore::new(), &config.device_id)?;
            Ok((
                BootstrapData::default(),
                "offline".to_string(),
                Some(
                    "Layrs Desktop session expired or was revoked by the server. Reconnect this device to continue."
                        .to_string(),
                ),
            ))
        }
        Err(error) => {
            if let Some(bootstrap) = load_cached_bootstrap()? {
                Ok((
                    bootstrap,
                    "stale".to_string(),
                    Some(format!("Showing cached Spaces: {error}")),
                ))
            } else {
                Ok((BootstrapData::default(), "offline".to_string(), Some(error)))
            }
        }
    }
}

fn desktop_token(config: &DesktopConfig) -> Result<String, String> {
    #[cfg(test)]
    if let Ok(token) = env::var("LAYRS_DESKTOP_TEST_TOKEN") {
        return Ok(token);
    }

    let store = OsSecretStore::new();
    store
        .get_token(&config.device_id)
        .map_err(|error| format!("Layrs Desktop could not read OS secret store: {error}"))?
        .ok_or_else(|| "Layrs Desktop is not connected. Connect a device first.".to_string())
}

fn ensure_linked_space_ready(handle: &LocalSpaceHandle) -> Result<(), String> {
    if handle.meta.workspace_id.trim().is_empty() || handle.meta.space_id.trim().is_empty() {
        return Err("This Local Space is not linked to a server Space.".to_string());
    }
    Ok(())
}

fn is_server_linked_space(handle: &LocalSpaceHandle) -> bool {
    handle.meta.state == LOCAL_SPACE_STATE_LINKED
        && !handle.meta.workspace_id.trim().is_empty()
        && !handle.meta.space_id.trim().is_empty()
}

fn is_probably_server_layer_id(layer_id: &str) -> bool {
    layer_id.starts_with("layer_")
}

fn unlinked_layer_message(layer_id: &str) -> String {
    format!(
        "Layer {layer_id} exists only on this machine and is not linked to Studio yet. Create a new Layer from a linked server Layer, or refresh/recreate this Local Space before publishing."
    )
}

fn is_layer_not_found_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("http 404") && lower.contains("layer not found")
}

fn link_layer_error_message(parent_layer_id: &str, error: String) -> String {
    if is_layer_not_found_error(&error) {
        return format!(
            "{} Original server response: {error}",
            unlinked_layer_message(parent_layer_id)
        );
    }
    error
}

fn bootstrap_space(
    bootstrap: &BootstrapData,
    space_id: &str,
    initial_layer_id: Option<&str>,
) -> (SpaceSummary, Vec<LayerSummary>) {
    let space = bootstrap
        .spaces
        .iter()
        .find(|space| space.id == space_id)
        .cloned()
        .unwrap_or_else(|| SpaceSummary {
            id: space_id.to_string(),
            workspace_id: String::new(),
            name: space_id.to_string(),
            current_layer_id: initial_layer_id
                .filter(|value| !value.trim().is_empty())
                .map(ToString::to_string),
        });

    let layers = bootstrap
        .layers
        .iter()
        .filter(|layer| layer.space_id == space.id)
        .cloned()
        .collect::<Vec<_>>();

    (space, layers)
}

fn ad_hoc_layer(space: &SpaceSummary, layer_id: &str, name: &str) -> LayerSummary {
    LayerSummary {
        id: layer_id.to_string(),
        workspace_id: space.workspace_id.clone(),
        space_id: space.id.clone(),
        name: name.to_string(),
        kind: Some("local".to_string()),
        parent_layer_id: None,
        access: Some("open".to_string()),
    }
}

fn create_local_space_directories(layrs_dir: &Path) -> Result<(), String> {
    for path in [
        layrs_dir.to_path_buf(),
        layrs_dir.join("objects"),
        layrs_dir.join("objects").join("files"),
        layrs_dir.join("objects").join("trees"),
        layrs_dir.join("objects").join("chunks"),
        layrs_dir.join("layers"),
        layrs_dir.join("sync"),
        layrs_dir.join("tmp"),
    ] {
        fs::create_dir_all(&path).map_err(|error| {
            format!(
                "Layrs Desktop could not create Local Space directory {}: {error}",
                path.display()
            )
        })?;
    }

    Ok(())
}

fn scaffold_layer(
    layrs_dir: &Path,
    local_space_id: &str,
    access: &LayerAccessView,
) -> Result<(), String> {
    let layer_path = access
        .local_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| layer_dir(layrs_dir, &access.layer_id));
    let access_path = layer_path.join("access.json");

    if access_path.exists() {
        let existing = read_json::<LayerAccessFile>(&access_path)?;
        if existing.access == LayerAccessKind::Redacted
            && access.access != LayerAccessKind::Redacted
        {
            return Err(format!(
                "Layrs Desktop refuses to replace reserved redacted Layer path {}.",
                layer_path.display()
            ));
        }
    }

    for path in [
        layer_path.clone(),
        layer_path.join("steps"),
        layer_path.join("pending-publish"),
        layer_path.join("lens-cache"),
    ] {
        fs::create_dir_all(&path).map_err(|error| {
            format!(
                "Layrs Desktop could not create Layer directory {}: {error}",
                path.display()
            )
        })?;
    }

    let access_file = LayerAccessFile {
        schema: LAYER_ACCESS_SCHEMA.to_string(),
        local_space_id: local_space_id.to_string(),
        workspace_id: access.workspace_id.clone(),
        space_id: access.space_id.clone(),
        layer_id: access.layer_id.clone(),
        display_name: access.display_name.clone(),
        access: access.access.clone(),
        can_open: access.can_open,
        reason: access.reason.clone(),
        policy_epoch: 1,
        generated_at_unix: unix_now(),
        rules: Vec::new(),
    };
    write_json(&access_path, &access_file)?;

    let empty_tree_id = write_tree_object(layrs_dir, &[])?;
    let empty_state = WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: access.layer_id.clone(),
        captured_at_unix: unix_now(),
        root_tree_id: Some(empty_tree_id),
        files: Vec::new(),
    };

    for file_name in ["index.json", "working-state.json"] {
        let path = layer_path.join(file_name);
        if !path.exists() {
            write_json(&path, &empty_state)?;
        }
    }

    let sync_state = serde_json::json!({
        "schema": SYNC_STATE_SCHEMA,
        "layerId": access.layer_id,
        "lastReceiveUnix": null,
        "lastPublishUnix": null,
        "pending": false
    });
    let sync_path = layer_path.join("sync-state.json");
    if !sync_path.exists() {
        write_json(&sync_path, &sync_state)?;
    }

    let timeline_path = layer_path.join("timeline-cache.json");
    if !timeline_path.exists() {
        write_json(
            &timeline_path,
            &serde_json::json!({ "schema": "layrs.timeline_cache.v1", "items": [] }),
        )?;
    }

    Ok(())
}

fn access_views(
    layers: &[LayerSummary],
    root: Option<&Path>,
) -> Result<Vec<LayerAccessView>, String> {
    let mut path_counts = BTreeMap::<String, usize>::new();
    for layer in layers {
        if let Some(path_key) = safe_layer_path_key(&layer.id) {
            *path_counts.entry(path_key).or_default() += 1;
        }
    }

    let mut emitted_paths = BTreeSet::<String>::new();
    let mut views = Vec::with_capacity(layers.len());
    for layer in layers {
        views.push(access_view(layer, &path_counts, &mut emitted_paths, root)?);
    }
    Ok(views)
}

fn access_view(
    layer: &LayerSummary,
    path_counts: &BTreeMap<String, usize>,
    emitted_paths: &mut BTreeSet<String>,
    root: Option<&Path>,
) -> Result<LayerAccessView, String> {
    let requested_access = layer.access.as_deref().unwrap_or("open");
    let redacted = matches!(
        requested_access,
        "redacted" | "denied" | "restricted" | "no-access"
    );
    let display_name = if redacted {
        "Restricted layer".to_string()
    } else {
        layer.name.clone()
    };

    let Some(path_key) = safe_layer_path_key(&layer.id) else {
        return Ok(LayerAccessView {
            layer_id: layer.id.clone(),
            workspace_id: layer.workspace_id.clone(),
            space_id: layer.space_id.clone(),
            display_name,
            access: LayerAccessKind::Blocked,
            can_open: false,
            local_path: None,
            reason: Some("Layer id cannot be represented safely as a local path.".to_string()),
        });
    };

    if path_counts.get(&path_key).copied().unwrap_or_default() > 1
        || !emitted_paths.insert(path_key.clone())
    {
        return Ok(LayerAccessView {
            layer_id: layer.id.clone(),
            workspace_id: layer.workspace_id.clone(),
            space_id: layer.space_id.clone(),
            display_name,
            access: LayerAccessKind::Blocked,
            can_open: false,
            local_path: None,
            reason: Some(
                "Layer local path collision detected; opening is blocked on this client."
                    .to_string(),
            ),
        });
    }

    let local_path = root.map(|root| {
        root.join(LAYRS_DIR)
            .join("layers")
            .join(&path_key)
            .display()
            .to_string()
    });

    if redacted {
        return Ok(LayerAccessView {
            layer_id: layer.id.clone(),
            workspace_id: layer.workspace_id.clone(),
            space_id: layer.space_id.clone(),
            display_name,
            access: LayerAccessKind::Redacted,
            can_open: false,
            local_path,
            reason: Some("Layer metadata is redacted and its local path is reserved.".to_string()),
        });
    }

    Ok(LayerAccessView {
        layer_id: layer.id.clone(),
        workspace_id: layer.workspace_id.clone(),
        space_id: layer.space_id.clone(),
        display_name,
        access: LayerAccessKind::Open,
        can_open: true,
        local_path,
        reason: None,
    })
}

fn write_access_pointer(
    layrs_dir: &Path,
    local_space_id: &str,
    active_layer_id: Option<&str>,
    layers: &[LayerAccessView],
) -> Result<(), String> {
    let pointer = LocalAccessPointer {
        schema: ACCESS_POINTER_SCHEMA.to_string(),
        local_space_id: local_space_id.to_string(),
        active_layer_id: active_layer_id.map(ToString::to_string),
        redacted_reserved_paths: layers
            .iter()
            .filter(|layer| layer.access == LayerAccessKind::Redacted)
            .filter_map(|layer| layer.local_path.clone())
            .collect(),
        layers: layers.to_vec(),
    };

    write_json(&layrs_dir.join("access.json"), &pointer)
}

fn write_access_pointer_from_meta(handle: &LocalSpaceHandle) -> Result<(), String> {
    let layers = handle
        .meta
        .layers
        .iter()
        .map(|layer| LayerAccessView {
            layer_id: layer.layer_id.clone(),
            workspace_id: handle.meta.workspace_id.clone(),
            space_id: handle.meta.space_id.clone(),
            display_name: layer.display_name.clone(),
            access: layer.access.clone(),
            can_open: layer.can_open,
            local_path: Some(
                layer_dir(&handle.layrs_dir, &layer.layer_id)
                    .display()
                    .to_string(),
            ),
            reason: if layer.access == LayerAccessKind::Redacted {
                Some("Layer metadata is redacted and its local path is reserved.".to_string())
            } else {
                None
            },
        })
        .collect::<Vec<_>>();

    write_access_pointer(
        &handle.layrs_dir,
        &handle.meta.local_space_id,
        Some(&handle.active.layer_id),
        &layers,
    )
}

fn open_local_space_handle(selector: &str) -> Result<LocalSpaceHandle, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("Layrs Desktop needs a Local Space id or path.".to_string());
    }

    let config = DesktopConfig::load_or_create()?;
    if let Some(entry) = config
        .local_spaces
        .iter()
        .find(|entry| entry.local_space_id == selector)
    {
        return open_local_space_at(PathBuf::from(&entry.root_path));
    }

    open_local_space_at(PathBuf::from(selector))
}

fn open_local_space_at(path: PathBuf) -> Result<LocalSpaceHandle, String> {
    let path = absolute_path(&path)?;
    let root = if path.file_name().and_then(|name| name.to_str()) == Some(LAYRS_DIR) {
        path.parent()
            .ok_or_else(|| "Layrs Desktop could not resolve Local Space root.".to_string())?
            .to_path_buf()
    } else if path.file_name().and_then(|name| name.to_str()) == Some("local-space.json") {
        path.parent()
            .and_then(Path::parent)
            .ok_or_else(|| "Layrs Desktop could not resolve Local Space root.".to_string())?
            .to_path_buf()
    } else {
        path
    };
    let layrs_dir = root.join(LAYRS_DIR);
    let meta = read_json::<LocalSpaceFile>(&layrs_dir.join("local-space.json"))?;
    let active = read_json::<ActiveLayerFile>(&layrs_dir.join("active-layer.json"))?;

    Ok(LocalSpaceHandle {
        root,
        layrs_dir,
        meta,
        active,
    })
}

fn find_local_space_config_entry(
    config: &DesktopConfig,
    selector: &str,
) -> Option<LocalSpaceConfigEntry> {
    let selector_path_key = if selector.trim().is_empty() {
        None
    } else {
        Some(path_compare_key(&PathBuf::from(selector)))
    };
    config
        .local_spaces
        .iter()
        .find(|entry| {
            entry.local_space_id == selector
                || selector_path_key
                    .as_ref()
                    .is_some_and(|key| path_compare_key(&PathBuf::from(&entry.root_path)) == *key)
        })
        .cloned()
}

fn archive_layrs_dir_at(root: &Path, layrs_dir: &Path) -> Result<Option<PathBuf>, String> {
    if !layrs_dir.exists() {
        return Ok(None);
    }

    let timestamp = unix_now();
    let mut archive_path = root.join(format!(".layrs-forgotten-{timestamp}"));
    let mut suffix = 2;
    while archive_path.exists() {
        archive_path = root.join(format!(".layrs-forgotten-{timestamp}-{suffix}"));
        suffix += 1;
    }

    fs::rename(layrs_dir, &archive_path).map_err(|error| {
        format!(
            "Layrs Desktop could not archive local metadata {} to {}: {error}",
            layrs_dir.display(),
            archive_path.display()
        )
    })?;
    Ok(Some(archive_path))
}

fn path_compare_key(path: &Path) -> String {
    let absolute = absolute_path(&path.to_path_buf()).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        absolute.display().to_string().to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        absolute.display().to_string()
    }
}

fn summary_from_handle(handle: &LocalSpaceHandle) -> LocalSpaceSummary {
    LocalSpaceSummary {
        local_space_id: handle.meta.local_space_id.clone(),
        space_id: handle.meta.space_id.clone(),
        workspace_id: handle.meta.workspace_id.clone(),
        server_space_id: handle.meta.server_space_id.clone(),
        state: handle.meta.state.clone(),
        name: handle.meta.name.clone(),
        root_path: handle.root.display().to_string(),
        active_layer_id: Some(handle.active.layer_id.clone()),
        layers: handle
            .meta
            .layers
            .iter()
            .map(|layer| LocalLayerSummary {
                layer_id: layer.layer_id.clone(),
                display_name: layer.display_name.clone(),
                parent_layer_id: layer.parent_layer_id.clone(),
                lineage_status: layer.lineage_status.clone(),
                access: layer.access.clone(),
                can_open: layer.can_open,
                path: layer_dir(&handle.layrs_dir, &layer.layer_id)
                    .display()
                    .to_string(),
                sync_status: layer_sync_status(&handle.meta, &layer.layer_id),
            })
            .collect(),
    }
}

fn layer_sync_status(meta: &LocalSpaceFile, layer_id: &str) -> String {
    if meta.state == LOCAL_SPACE_STATE_DRAFT {
        "local".to_string()
    } else if is_probably_server_layer_id(layer_id) {
        "linked".to_string()
    } else {
        "local-only".to_string()
    }
}
