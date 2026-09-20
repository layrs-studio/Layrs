use super::*;

pub(super) fn receive_linked_space_state(
    mut handle: LocalSpaceHandle,
    config: &DesktopConfig,
    materialize_active: bool,
) -> Result<(LocalSpaceHandle, PathBuf), String> {
    ensure_linked_space_ready(&handle)?;
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
    let target_head = read_layer_index(&handle.store, &handle.layrs_dir, &layer_id)
        .ok()
        .and_then(|state| state.root_tree_id);
    let expected_working_tree_head = inspect_working_state(&handle.root, &layer_id)?.root_tree_id;
    let preserved_pending_step_ids =
        pending_publish_steps(&handle.store, &handle.layrs_dir, &layer_id)?
            .into_iter()
            .map(|step| step.step_id)
            .collect::<Vec<_>>();
    let journal = serde_json::json!({
        "schema": "layrs.client_network_operation.v1",
        "operation": "receive",
        "targetLayerId": layer_id,
        "targetLayerHead": target_head,
        "expectedWorkingTreeHead": expected_working_tree_head,
        "preservedPendingStepIds": preserved_pending_step_ids,
        "expectedAckStepIds": [],
        "request": serde_json::to_value(&request)
            .map_err(|error| format!("Layrs could not journal receive request: {error}"))?
    });
    let lease = handle
        .store
        .prepare_network(&handle.meta.space_id, "receive", &journal)?;
    let root = handle.root.clone();
    let layrs_dir = handle.layrs_dir.clone();
    let token = desktop_token(config)?;
    drop(handle);

    let response: ReceiveLocalSpaceResponse = post_json(
        &config.server_endpoint,
        &receive_path,
        Some(&token),
        &request,
    )?;
    prefetch_receive_response_chunks(
        &layrs_dir,
        &config.server_endpoint,
        Some(token.as_str()),
        &response,
    )?;
    let response_root = response.root_tree_id.clone();
    let response_cursor = response.cursor.clone();
    let observed_working_tree_head = inspect_working_state(&root, &layer_id)?.root_tree_id;
    if observed_working_tree_head != expected_working_tree_head {
        return Err(
            "Layrs local files changed while receive was in flight; finalize was refused and pending Steps were preserved."
                .to_string(),
        );
    }

    let mut handle = open_local_space_at(root)?;
    handle.store.begin_network_finalize(&lease)?;
    let sync_path = apply_receive_response(&mut handle, response, materialize_active)?;
    handle.store.finish_network(
        &lease,
        &serde_json::json!({
            "receivedLayerId": layer_id,
            "receivedLayerHead": response_root,
            "serverCursor": response_cursor,
            "acknowledgedStepIds": []
        }),
    )?;
    Ok((handle, sync_path))
}
