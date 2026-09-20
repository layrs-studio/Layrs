use super::super::*;
use super::draft_linking::create_linked_server_layer;

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
    let current_state = capture_working_state(&mut handle.store, &handle.root, &previous_layer_id)?;
    let previous_state =
        read_layer_state(&handle.store, &handle.layrs_dir, &previous_layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let saved_step_id = if changed_files > 0 && config.auto_local_steps {
        let step_id = write_step_and_pending_publish(
            &mut handle.store,
            &handle.layrs_dir,
            &previous_layer_id,
            &current_state,
        )?;
        propagate_step_to_linked_children(&mut handle, &previous_layer_id, &step_id)?;
        Some(step_id)
    } else {
        None
    };
    write_working_state(
        &mut handle.store,
        &handle.layrs_dir,
        &previous_layer_id,
        &current_state,
    )?;

    if previous_layer_id != target_layer_id {
        let mut target_state = read_layer_state(&handle.store, &handle.layrs_dir, &target_layer_id)
            .or_else(|_| read_layer_index(&handle.store, &handle.layrs_dir, &target_layer_id))?;
        if let Some(weave_id) = active_weave_id(&handle.store, &handle.layrs_dir)? {
            let session = read_weave_session(&handle.store, &handle.layrs_dir, &weave_id)?;
            if session.target_layer_id == target_layer_id && session.status == "conflicted" {
                target_state = read_state_file(
                    &handle.store,
                    &handle.layrs_dir,
                    &proposed_state_path(&handle.layrs_dir, &weave_id),
                )?;
            }
        }
        materialize_state(&mut handle.store, &handle.root, &target_state)?;

        handle.active = ActiveLayerFile {
            schema: ACTIVE_LAYER_SCHEMA.to_string(),
            layer_id: target_layer_id.clone(),
            updated_at_unix: unix_now(),
        };
        write_json(
            &mut handle.store,
            &handle.layrs_dir.join("active-layer.json"),
            &handle.active,
        )?;
        write_access_pointer_from_meta(&mut handle)?;
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

    let current_state = capture_working_state(&mut handle.store, &handle.root, &previous_layer_id)?;
    let previous_state =
        read_layer_state(&handle.store, &handle.layrs_dir, &previous_layer_id).ok();
    let changed_files = changed_file_count(previous_state.as_ref(), &current_state);
    let saved_step_id = if changed_files > 0 {
        let step_id = write_step_and_pending_publish(
            &mut handle.store,
            &handle.layrs_dir,
            &previous_layer_id,
            &current_state,
        )?;
        propagate_step_to_linked_children(&mut handle, &previous_layer_id, &step_id)?;
        Some(step_id)
    } else {
        None
    };
    let layer_id = if is_server_linked_space(&handle) {
        let (next_handle, layer_id) =
            create_linked_server_layer(handle, &display_name, Some(&previous_layer_id))?;
        handle = next_handle;
        layer_id
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
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )?;

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
    scaffold_layer(
        &mut handle.store,
        &handle.layrs_dir,
        &handle.meta.local_space_id,
        &view,
    )?;
    inherit_layer_access_file(
        &mut handle,
        &previous_layer_id,
        &layer_id,
        &layer.display_name,
    )?;
    let mut new_state = current_state.clone();
    new_state.layer_id = layer_id.clone();
    write_layer_state(&mut handle.store, &handle.layrs_dir, &layer_id, &new_state)?;
    inherit_parent_steps(&mut handle, &previous_layer_id, &layer_id)?;
    if is_server_linked_space(&handle) {
        write_linked_layer_sync_state(&mut handle, &layer_id, Some(&previous_layer_id))?;
    }

    handle.active = ActiveLayerFile {
        schema: ACTIVE_LAYER_SCHEMA.to_string(),
        layer_id: layer_id.clone(),
        updated_at_unix: unix_now(),
    };
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("active-layer.json"),
        &handle.active,
    )?;
    write_access_pointer_from_meta(&mut handle)?;
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
            &mut handle.store,
            &handle.layrs_dir,
            &target_layer_id,
            &target_layer.display_name,
        )?;
    }

    let layer_path = layer_dir(&handle.layrs_dir, &target_layer_id);
    handle.store.remove_listed::<Value>(&layer_path, ".json")?;

    handle.meta.layers.remove(target_index);
    handle.meta.updated_at_unix = unix_now();
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )?;
    write_access_pointer_from_meta(&mut handle)?;
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
    write_json(
        &mut handle.store,
        &handle.layrs_dir.join("local-space.json"),
        &handle.meta,
    )?;
    write_access_pointer_from_meta(&mut handle)?;
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
    let mut handle = open_local_space_handle(&local_space)?;
    if active_weave_id(&handle.store, &handle.layrs_dir)?.is_some() {
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
    let steps_archive = archive_path.as_ref().map(|path| path.join("steps"));
    let pending_archive = archive_path
        .as_ref()
        .map(|path| path.join("pending-publish"));
    moved += move_or_remove_json_files(&mut handle.store, &steps_dir, steps_archive.as_deref())?;
    moved +=
        move_or_remove_json_files(&mut handle.store, &pending_dir, pending_archive.as_deref())?;

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
    drop(handle);
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

fn move_or_remove_json_files(
    store: &mut LocalStoreSession,
    dir: &Path,
    archive_dir: Option<&Path>,
) -> Result<usize, String> {
    if let Some(archive_dir) = archive_dir {
        store.rename_prefix(dir, archive_dir)
    } else {
        store
            .remove_listed::<Value>(dir, ".json")
            .map(|documents| documents.len())
    }
}

fn inherit_layer_access_file(
    handle: &mut LocalSpaceHandle,
    parent_layer_id: &str,
    layer_id: &str,
    display_name: &str,
) -> Result<(), String> {
    let parent_access_path = layer_dir(&handle.layrs_dir, parent_layer_id).join("access.json");
    if !handle.store.exists(&parent_access_path)? {
        return Ok(());
    }

    let mut access = read_json::<LayerAccessFile>(&handle.store, &parent_access_path)?;
    access.workspace_id = handle.meta.workspace_id.clone();
    access.space_id = handle.meta.space_id.clone();
    access.layer_id = layer_id.to_string();
    access.display_name = display_name.to_string();
    access.generated_at_unix = unix_now();
    write_json(
        &mut handle.store,
        &layer_dir(&handle.layrs_dir, layer_id).join("access.json"),
        &access,
    )
}
