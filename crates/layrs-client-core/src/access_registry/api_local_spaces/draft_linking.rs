use super::super::*;

struct DraftPublishPhase {
    local_layer_id: String,
    server_layer_id: String,
    path: String,
    body: PublishLayerRequest,
    expected_step_ids: Vec<String>,
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
    let current_state = capture_working_state(&mut handle.store, &handle.root, &active_layer_id)?;
    let previous_state = read_layer_state(&handle.store, &handle.layrs_dir, &active_layer_id).ok();
    if changed_file_count(previous_state.as_ref(), &current_state) > 0 && config.auto_local_steps {
        write_step(
            &mut handle.store,
            &handle.layrs_dir,
            &active_layer_id,
            &current_state,
        )?;
    }
    write_layer_state(
        &mut handle.store,
        &handle.layrs_dir,
        &active_layer_id,
        &current_state,
    )?;

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
    let layer_heads = handle
        .meta
        .layers
        .iter()
        .map(|layer| {
            let state = read_layer_state(&handle.store, &handle.layrs_dir, &layer.layer_id)
                .or_else(|_| read_layer_index(&handle.store, &handle.layrs_dir, &layer.layer_id))?;
            let expected_ack_step_ids =
                pending_publish_steps(&handle.store, &handle.layrs_dir, &layer.layer_id)?
                    .into_iter()
                    .map(|step| step.step_id)
                    .collect::<Vec<_>>();
            Ok(serde_json::json!({
                "localLayerId": layer.layer_id,
                "targetLayerHead": state.root_tree_id,
                "expectedAckStepIds": expected_ack_step_ids
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let journal = serde_json::json!({
        "schema": "layrs.client_network_operation.v1",
        "operation": "send-draft",
        "targetLayerId": active_layer_id,
        "targetLayerHead": current_state.root_tree_id,
        "layers": layer_heads,
        "request": serde_json::to_value(&request)
            .map_err(|error| format!("Layrs could not journal Draft import: {error}"))?
    });
    let lease = handle
        .store
        .prepare_network(&handle.meta.space_id, "send-draft", &journal)?;
    let root = handle.root.clone();
    let layrs_dir = handle.layrs_dir.clone();
    drop(handle);

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

    let mut handle = open_local_space_at(root.clone())?;
    handle.store.amend_network(
        &lease,
        &serde_json::json!({
            "createdSpaceId": created.space.id.clone(),
            "layerMappings": created.layer_mappings.clone()
        }),
    )?;
    let mut phases = Vec::new();
    for mapping in &created.layer_mappings {
        let state = read_layer_state(&handle.store, &handle.layrs_dir, &mapping.local_layer_id)
            .or_else(|_| {
                read_layer_index(&handle.store, &handle.layrs_dir, &mapping.local_layer_id)
            })?;
        let publish_paths = state
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<BTreeSet<_>>();
        if !publish_paths.is_empty() {
            let pending_steps =
                pending_publish_steps(&handle.store, &handle.layrs_dir, &mapping.local_layer_id)?;
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
            phases.push(DraftPublishPhase {
                local_layer_id: mapping.local_layer_id.clone(),
                server_layer_id: mapping.server_layer_id.clone(),
                path: publish_path,
                expected_step_ids: pending_steps
                    .iter()
                    .map(|step| step.step_id.clone())
                    .collect(),
                body,
            });
        }
    }
    handle.store.amend_network(
        &lease,
        &serde_json::json!({
            "publishRequests": phases
                .iter()
                .map(|phase| serde_json::json!({
                    "localLayerId": phase.local_layer_id,
                    "serverLayerId": phase.server_layer_id,
                    "targetLayerHead": phase.body.root_tree_id,
                    "expectedAckStepIds": phase.expected_step_ids,
                    "request": phase.body
                }))
                .collect::<Vec<_>>()
        }),
    )?;
    drop(handle);

    let mut acknowledged_by_layer = BTreeMap::new();
    for phase in &phases {
        upload_publish_chunks(
            &layrs_dir,
            &config.server_endpoint,
            Some(&token),
            &workspace_id,
            &created.space.id,
            &phase.body.store_objects,
        )?;
        let response: Value = post_json(
            &config.server_endpoint,
            &phase.path,
            Some(&token),
            &phase.body,
        )?;
        let acknowledged =
            acknowledged_publish_step_ids_exact(&response, &phase.expected_step_ids)?;
        acknowledged_by_layer.insert(phase.server_layer_id.clone(), acknowledged);
    }

    let refreshed_bootstrap = get_json::<BootstrapData>(
        &config.server_endpoint,
        "/v1/desktop/bootstrap",
        Some(&token),
    )
    .and_then(|bootstrap| validate_desktop_bootstrap(bootstrap, "/v1/desktop/bootstrap"))
    .ok();
    if let Some(bootstrap) = refreshed_bootstrap {
        let _ = save_cached_bootstrap(&bootstrap);
    }

    let observed_state = inspect_working_state(&root, &active_layer_id)?;
    if observed_state.root_tree_id != current_state.root_tree_id {
        return Err(
            "Layrs local files changed while the Draft was being sent; finalize was refused and local metadata was preserved."
                .to_string(),
        );
    }
    let mut handle = open_local_space_at(root)?;
    handle.store.begin_network_finalize(&lease)?;
    apply_server_mapping_to_draft(&mut handle, &created)?;
    for (server_layer_id, acknowledged) in &acknowledged_by_layer {
        remove_acknowledged_pending_steps(
            &mut handle.store,
            &handle.layrs_dir,
            server_layer_id,
            acknowledged,
        )?;
    }
    handle.store.finish_network(
        &lease,
        &serde_json::json!({
            "createdSpaceId": created.space.id.clone(),
            "acknowledgedStepIdsByLayer": acknowledged_by_layer
        }),
    )?;

    let published_layers = created.layer_mappings.len();
    Ok(SendDraftLocalSpaceResult {
        local_space: summary_from_handle(&handle),
        workspace_id,
        server_space_id: created.space.id,
        layer_mappings: created.layer_mappings,
        published_layers,
    })
}

pub(super) fn create_linked_server_layer(
    mut handle: LocalSpaceHandle,
    display_name: &str,
    parent_layer_id: Option<&str>,
) -> Result<(LocalSpaceHandle, String), String> {
    ensure_linked_space_ready(&handle)?;
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
    let target_head = parent_layer_id.and_then(|layer_id| {
        read_layer_index(&handle.store, &handle.layrs_dir, layer_id)
            .ok()
            .and_then(|state| state.root_tree_id)
    });
    let journal = serde_json::json!({
        "schema": "layrs.client_network_operation.v1",
        "operation": "create-layer",
        "targetLayerId": parent_layer_id,
        "targetLayerHead": target_head,
        "expectedAckStepIds": [],
        "request": serde_json::to_value(&request)
            .map_err(|error| format!("Layrs could not journal Layer creation: {error}"))?
    });
    let lease = handle
        .store
        .prepare_network(&handle.meta.space_id, "create-layer", &journal)?;
    let root = handle.root.clone();
    drop(handle);

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
    let mut handle = open_local_space_at(root)?;
    handle.store.begin_network_finalize(&lease)?;
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

    handle.store.finish_network(
        &lease,
        &serde_json::json!({
            "createdLayerId": server_layer_id.clone(),
            "acknowledgedStepIds": []
        }),
    )?;
    Ok((handle, server_layer_id))
}

pub(crate) fn link_active_local_layer_for_sync(
    mut handle: LocalSpaceHandle,
    operation: &str,
) -> Result<(LocalSpaceHandle, String), String> {
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
    capture_working_state(&mut handle.store, &handle.root, &old_layer_id)?;
    let (next_handle, server_layer_id) =
        create_linked_server_layer(handle, &display_name, parent_layer_id.as_deref())?;
    handle = next_handle;

    let old_dir = layer_dir(&handle.layrs_dir, &old_layer_id);
    let new_dir = layer_dir(&handle.layrs_dir, &server_layer_id);
    if old_dir == new_dir {
        return Err(format!(
            "Layrs server returned the existing local Layer id {server_layer_id}."
        ));
    }
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
    let mappings = BTreeMap::from([(old_layer_id.clone(), server_layer_id.clone())]);
    let workspace_id = handle.meta.workspace_id.clone();
    let space_id = handle.meta.space_id.clone();
    rewrite_layer_files_after_mapping(&mut handle, &mappings, &workspace_id, &space_id)?;
    let _ = write_sync_state(&mut handle, operation, "linked", 0, None)?;
    remember_local_space(&handle.meta, Some(server_layer_id.clone()))?;
    Ok((handle, server_layer_id))
}
