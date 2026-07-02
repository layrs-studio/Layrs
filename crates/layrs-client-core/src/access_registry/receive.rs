fn write_sync_state(
    handle: &LocalSpaceHandle,
    operation: &str,
    status: &str,
    changed_files: usize,
    server_cursor: Option<String>,
) -> Result<PathBuf, String> {
    let path = layer_dir(&handle.layrs_dir, &handle.active.layer_id).join("sync-state.json");
    let sync_state = serde_json::json!({
        "schema": SYNC_STATE_SCHEMA,
        "layerId": handle.active.layer_id,
        "lastManualOperation": operation,
        "lastManualOperationUnix": unix_now(),
        "changedFiles": changed_files,
        "serverCursor": server_cursor,
        "pending": false,
        "status": status
    });
    write_json(&path, &sync_state)?;
    Ok(path)
}

fn write_linked_layer_sync_state(
    handle: &LocalSpaceHandle,
    layer_id: &str,
    parent_layer_id: Option<&str>,
) -> Result<PathBuf, String> {
    let path = layer_dir(&handle.layrs_dir, layer_id).join("sync-state.json");
    let sync_state = serde_json::json!({
        "schema": SYNC_STATE_SCHEMA,
        "layerId": layer_id,
        "parentLayerId": parent_layer_id,
        "lastReceiveUnix": null,
        "lastPublishUnix": null,
        "pending": false,
        "status": "linked"
    });
    write_json(&path, &sync_state)?;
    Ok(path)
}

fn apply_receive_response(
    handle: &mut LocalSpaceHandle,
    response: ReceiveLocalSpaceResponse,
    materialize_active: bool,
    endpoint: Option<&str>,
    token: Option<&str>,
) -> Result<PathBuf, String> {
    if response.workspace_id != handle.meta.workspace_id
        || response.space_id != handle.meta.space_id
    {
        return Err("Layrs Desktop received sync data for a different Space.".to_string());
    }

    let active_layer_id = handle.active.layer_id.clone();
    if response.layer_id != active_layer_id {
        return Err(format!(
            "Layrs Desktop received Layer {} while {} is active.",
            response.layer_id, active_layer_id
        ));
    }
    let response_uses_v2 = response.protocol.as_deref() == Some(SYNC_PROTOCOL_V2)
        || response.content_objects.is_some();
    if !response_uses_v2 {
        return Err("Layrs Desktop requires layrs.sync.v2 receive data.".to_string());
    }
    let policy_by_layer = response
        .access_registries
        .iter()
        .filter_map(|policy| {
            policy
                .get("layer_id")
                .or_else(|| policy.get("layerId"))
                .and_then(Value::as_str)
                .map(|layer_id| (layer_id.to_string(), policy.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let content_objects = response
        .content_objects
        .as_ref()
        .ok_or_else(|| "Layrs Desktop received V2 protocol without contentObjects.".to_string())?;
    ensure_received_v2_chunks(
        &handle.layrs_dir,
        endpoint,
        token,
        &response.workspace_id,
        &response.space_id,
        &content_objects.chunks,
    )?;
    write_received_v2_file_objects(&handle.layrs_dir, &content_objects.file_objects)?;
    write_received_v2_tree_objects(&handle.layrs_dir, content_objects)?;

    let mut metadata_layers = Vec::with_capacity(response.layers.len());
    let mut active_sync_path =
        layer_dir(&handle.layrs_dir, &active_layer_id).join("sync-state.json");

    for layer in &response.layers {
        let layer_id = layer.id.clone();
        let access_kind = match layer.access.as_deref() {
            Some("redacted" | "denied" | "restricted" | "no-access") => LayerAccessKind::Redacted,
            _ => LayerAccessKind::Open,
        };
        let layer_meta = LocalLayerMetadata {
            layer_id: layer_id.clone(),
            display_name: layer.name.clone(),
            parent_layer_id: layer.parent_layer_id.clone(),
            access: access_kind.clone(),
            can_open: access_kind == LayerAccessKind::Open,
        };
        metadata_layers.push(layer_meta.clone());

        let view = LayerAccessView {
            layer_id: layer_id.clone(),
            workspace_id: layer
                .workspace_id
                .clone()
                .unwrap_or_else(|| response.workspace_id.clone()),
            space_id: layer
                .space_id
                .clone()
                .unwrap_or_else(|| response.space_id.clone()),
            display_name: layer.name.clone(),
            access: access_kind,
            can_open: layer_meta.can_open,
            local_path: Some(
                layer_dir(&handle.layrs_dir, &layer_id)
                    .display()
                    .to_string(),
            ),
            reason: if layer_meta.can_open {
                None
            } else {
                Some("Layer metadata is redacted by Studio access policy.".to_string())
            },
        };
        scaffold_layer(&handle.layrs_dir, &handle.meta.local_space_id, &view)?;
        if let Some(policy) = policy_by_layer.get(&layer_id) {
            write_received_access_file(handle, &layer_id, &layer.name, policy)?;
        }

        let layer_root_tree_id = if layer_id == active_layer_id {
            response.root_tree_id.as_deref()
        } else {
            None
        };
        let state = received_v2_state(
            &handle.layrs_dir,
            endpoint,
            token,
            &response.workspace_id,
            &response.space_id,
            &layer_id,
            layer_root_tree_id,
            content_objects,
        )?;
        if let Some(state) = state {
            write_layer_state(&handle.layrs_dir, &layer_id, &state)?;
            if materialize_active && layer_id == active_layer_id {
                materialize_state(&handle.root, &state)?;
            }
        } else if layer_id == active_layer_id {
            return Err(format!(
                "Layrs Desktop received V2 content without a tree for active Layer {layer_id}."
            ));
        }

        let sync_path = write_received_sync_state(
            handle,
            &layer_id,
            response.cursor.clone(),
            layer.parent_layer_id.as_deref(),
        )?;
        if layer_id == active_layer_id {
            active_sync_path = sync_path;
        }
        write_received_timeline_cache(handle, &layer_id, &response.timeline)?;
    }
    write_received_steps(&handle.layrs_dir, &response.steps)?;

    if !metadata_layers
        .iter()
        .any(|layer| layer.layer_id == active_layer_id)
    {
        return Err(format!(
            "Studio did not return active Layer {active_layer_id} for this Local Space."
        ));
    }

    handle.meta.layers = metadata_layers;
    handle.meta.updated_at_unix = unix_now();
    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
    write_access_pointer_from_meta(handle)?;
    remember_local_space(&handle.meta, Some(active_layer_id))?;
    Ok(active_sync_path)
}

fn write_received_access_file(
    handle: &LocalSpaceHandle,
    layer_id: &str,
    display_name: &str,
    policy: &Value,
) -> Result<(), String> {
    let rules = policy
        .get("rules")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .map(|rule| LayerAccessRuleFile {
                    id: rule
                        .get("id")
                        .or_else(|| rule.get("rule_id"))
                        .and_then(Value::as_str)
                        .unwrap_or("access_rule")
                        .to_string(),
                    path: rule
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("*")
                        .to_string(),
                    mode: rule
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or("restricted")
                        .to_string(),
                    visibility: rule
                        .get("visibility")
                        .and_then(Value::as_str)
                        .unwrap_or("stub")
                        .to_string(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let access_file = LayerAccessFile {
        schema: LAYER_ACCESS_SCHEMA.to_string(),
        local_space_id: handle.meta.local_space_id.clone(),
        workspace_id: handle.meta.workspace_id.clone(),
        space_id: handle.meta.space_id.clone(),
        layer_id: layer_id.to_string(),
        display_name: display_name.to_string(),
        access: LayerAccessKind::Open,
        can_open: true,
        reason: None,
        policy_epoch: policy
            .get("policy_epoch")
            .or_else(|| policy.get("policyEpoch"))
            .and_then(Value::as_u64)
            .unwrap_or(1),
        generated_at_unix: unix_now(),
        rules,
    };
    write_json(
        &layer_dir(&handle.layrs_dir, layer_id).join("access.json"),
        &access_file,
    )
}

fn received_v2_state(
    layrs_dir: &Path,
    endpoint: Option<&str>,
    token: Option<&str>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    root_tree_id: Option<&str>,
    objects: &ReceivedContentObjects,
) -> Result<Option<WorkingStateFile>, String> {
    ensure_received_v2_chunks(
        layrs_dir,
        endpoint,
        token,
        workspace_id,
        space_id,
        &objects.chunks,
    )?;
    write_received_v2_file_objects(layrs_dir, &objects.file_objects)?;

    let tree = select_received_tree(layer_id, root_tree_id, &objects.tree_objects);
    let Some(tree) = tree else {
        return Ok(None);
    };
    let file_objects = objects
        .file_objects
        .iter()
        .map(|object| (object.file_object_id.clone(), object))
        .collect::<BTreeMap<_, _>>();
    let mut files = Vec::with_capacity(tree.entries.len());
    for entry in &tree.entries {
        validate_snapshot_key(&entry.path)?;
        let file_object_id = entry.file_object_id.clone().ok_or_else(|| {
            format!(
                "Layrs Desktop received V2 tree entry {} without fileObjectId.",
                entry.path
            )
        })?;
        validate_blake3_id(&file_object_id)?;
        let file_size = entry
            .size
            .or_else(|| {
                file_objects
                    .get(&file_object_id)
                    .and_then(|object| object.size)
            })
            .ok_or_else(|| {
                format!(
                    "Layrs Desktop received V2 tree entry {} without size.",
                    entry.path
                )
            })?;
        files.push(FileSnapshotEntry {
            path: entry.path.clone(),
            object: format!("objects/files/{}.json", object_file_stem(&file_object_id)),
            hash: file_object_id,
            size: file_size,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let tree_object = TreeObjectFile {
        schema: TREE_OBJECT_SCHEMA.to_string(),
        tree_id: tree.tree_id.clone(),
        files: files.clone(),
    };
    write_json(&tree_object_path(layrs_dir, &tree.tree_id), &tree_object)?;
    Ok(Some(WorkingStateFile {
        schema: WORKING_STATE_SCHEMA.to_string(),
        layer_id: layer_id.to_string(),
        captured_at_unix: unix_now(),
        root_tree_id: Some(tree.tree_id.clone()),
        files,
    }))
}

fn write_received_v2_tree_objects(
    layrs_dir: &Path,
    objects: &ReceivedContentObjects,
) -> Result<(), String> {
    let file_objects = objects
        .file_objects
        .iter()
        .map(|object| (object.file_object_id.clone(), object))
        .collect::<BTreeMap<_, _>>();
    for tree in &objects.tree_objects {
        validate_blake3_id(&tree.tree_id)?;
        let mut files = Vec::with_capacity(tree.entries.len());
        for entry in &tree.entries {
            validate_snapshot_key(&entry.path)?;
            let file_object_id = entry.file_object_id.clone().ok_or_else(|| {
                format!(
                    "Layrs Desktop received V2 tree entry {} without fileObjectId.",
                    entry.path
                )
            })?;
            validate_blake3_id(&file_object_id)?;
            let file_size = entry
                .size
                .or_else(|| {
                    file_objects
                        .get(&file_object_id)
                        .and_then(|object| object.size)
                })
                .ok_or_else(|| {
                    format!(
                        "Layrs Desktop received V2 tree entry {} without size.",
                        entry.path
                    )
                })?;
            files.push(FileSnapshotEntry {
                path: entry.path.clone(),
                object: format!("objects/files/{}.json", object_file_stem(&file_object_id)),
                hash: file_object_id,
                size: file_size,
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        write_json(
            &tree_object_path(layrs_dir, &tree.tree_id),
            &TreeObjectFile {
                schema: TREE_OBJECT_SCHEMA.to_string(),
                tree_id: tree.tree_id.clone(),
                files,
            },
        )?;
    }
    Ok(())
}

fn write_received_steps(layrs_dir: &Path, steps: &[ReceivedStep]) -> Result<(), String> {
    for step in steps {
        validate_step_file_id(&step.step_id)?;
        let layer_id = step.layer_id.trim();
        if layer_id.is_empty() {
            return Err("Layrs Desktop received a step without layerId.".to_string());
        }
        if let Some(root_tree_id) = step.root_tree_id.as_deref() {
            validate_blake3_id(root_tree_id)?;
        }
        if let Some(base_tree_id) = step.base_tree_id.as_deref() {
            validate_blake3_id(base_tree_id)?;
        }
        let step_file = StepFile {
            schema: STEP_SCHEMA.to_string(),
            step_id: step.step_id.clone(),
            layer_id: layer_id.to_string(),
            parent_step_id: step.parent_step_id.clone(),
            base_layer_id: step.base_layer_id.clone(),
            base_tree_id: step.base_tree_id.clone(),
            root_tree_id: step.root_tree_id.clone(),
            changed_paths: step.changed_paths.clone(),
            captured_at_unix: step.captured_at_unix.unwrap_or_else(unix_now),
            files: Vec::new(),
        };
        write_json(
            &layer_dir(layrs_dir, layer_id)
                .join("steps")
                .join(format!("{}.json", step.step_id)),
            &step_file,
        )?;
    }
    Ok(())
}

fn validate_step_file_id(step_id: &str) -> Result<(), String> {
    let valid = !step_id.trim().is_empty()
        && step_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "Layrs Desktop received an invalid stepId: {step_id}"
        ))
    }
}

fn select_received_tree<'a>(
    layer_id: &str,
    root_tree_id: Option<&str>,
    trees: &'a [ReceivedTreeObject],
) -> Option<&'a ReceivedTreeObject> {
    root_tree_id
        .and_then(|tree_id| trees.iter().find(|tree| tree.tree_id == tree_id))
        .or_else(|| {
            trees
                .iter()
                .find(|tree| tree.layer_id.as_deref() == Some(layer_id))
        })
        .or_else(|| (trees.len() == 1).then(|| &trees[0]))
}

fn ensure_received_v2_chunks(
    layrs_dir: &Path,
    endpoint: Option<&str>,
    token: Option<&str>,
    workspace_id: &str,
    space_id: &str,
    chunks: &[ReceivedChunkObject],
) -> Result<(), String> {
    for chunk in chunks {
        validate_blake3_id(&chunk.chunk_id)?;
        if let Some(digest) = chunk.digest.as_deref() {
            validate_blake3_id(digest)?;
            if digest != chunk.chunk_id {
                return Err(format!(
                    "Layrs Desktop rejected V2 chunk {} because its digest differs from chunkId.",
                    chunk.chunk_id
                ));
            }
        }
        let expected_size = chunk.raw_size.or(chunk.size).or(chunk.size_bytes);
        let hint = FileChunkRef {
            chunk_id: chunk.chunk_id.clone(),
            size: expected_size.unwrap_or(0),
            compression: chunk
                .compression
                .clone()
                .unwrap_or_else(|| "identity".to_string()),
            stored_size: chunk.stored_size,
        };
        if read_chunk_raw(layrs_dir, &chunk.chunk_id, Some(&hint)).is_ok() {
            continue;
        }

        let endpoint = endpoint.ok_or_else(|| {
            format!(
                "Layrs Desktop received V2 chunk {} without local bytes and no server endpoint.",
                chunk.chunk_id
            )
        })?;
        let download_path = chunk.download_url.clone().unwrap_or_else(|| {
            format!(
                "/v1/workspaces/{}/spaces/{}/chunks/{}",
                url_path_segment(workspace_id),
                url_path_segment(space_id),
                url_path_segment(&chunk.chunk_id)
            )
        });
        let response = get_bytes_with_headers(endpoint, &download_path, token)?;
        let compression = chunk
            .compression
            .clone()
            .or_else(|| response.headers.get("x-layrs-chunk-compression").cloned())
            .unwrap_or_else(|| "identity".to_string());
        let raw_size = expected_size
            .or_else(|| {
                response
                    .headers
                    .get("x-layrs-raw-size")
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(response.body.len() as u64);
        write_received_encoded_chunk(
            layrs_dir,
            &chunk.chunk_id,
            response.body,
            &compression,
            raw_size,
        )?;
    }
    Ok(())
}

fn write_received_v2_file_objects(
    layrs_dir: &Path,
    file_objects: &[ReceivedFileObject],
) -> Result<(), String> {
    for object in file_objects {
        validate_blake3_id(&object.file_object_id)?;
        if let Some(hash) = object.hash.as_deref() {
            validate_blake3_id(hash)?;
            if hash != object.file_object_id {
                return Err(format!(
                    "Layrs Desktop rejected V2 fileObject {} because hash differs from fileObjectId.",
                    object.file_object_id
                ));
            }
        }
        let chunks = object
            .chunks
            .iter()
            .map(|chunk| {
                validate_blake3_id(&chunk.chunk_id)?;
                Ok(FileChunkRef {
                    chunk_id: chunk.chunk_id.clone(),
                    size: chunk
                        .raw_size
                        .or(chunk.size)
                        .or(chunk.size_bytes)
                        .unwrap_or(0),
                    compression: chunk
                        .compression
                        .clone()
                        .unwrap_or_else(|| "identity".to_string()),
                    stored_size: chunk.stored_size,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let size = object
            .size
            .unwrap_or_else(|| chunks.iter().map(|chunk| chunk.size).sum());
        let manifest = FileObjectFile {
            schema: FILE_OBJECT_SCHEMA.to_string(),
            hash: object
                .hash
                .clone()
                .unwrap_or_else(|| object.file_object_id.clone()),
            size,
            chunks,
        };
        write_json(
            &layrs_dir
                .join("objects")
                .join("files")
                .join(format!("{}.json", object_file_stem(&object.file_object_id))),
            &manifest,
        )?;
    }
    Ok(())
}

fn write_received_sync_state(
    handle: &LocalSpaceHandle,
    layer_id: &str,
    server_cursor: Option<String>,
    parent_layer_id: Option<&str>,
) -> Result<PathBuf, String> {
    let path = layer_dir(&handle.layrs_dir, layer_id).join("sync-state.json");
    let sync_state = serde_json::json!({
        "schema": SYNC_STATE_SCHEMA,
        "layerId": layer_id,
        "parentLayerId": parent_layer_id,
        "lastManualOperation": "receive",
        "lastManualOperationUnix": unix_now(),
        "lastReceiveUnix": unix_now(),
        "serverCursor": server_cursor,
        "pending": false,
        "status": "received"
    });
    write_json(&path, &sync_state)?;
    Ok(path)
}

fn write_received_timeline_cache(
    handle: &LocalSpaceHandle,
    layer_id: &str,
    timeline: &[Value],
) -> Result<(), String> {
    let items = timeline
        .iter()
        .filter(|event| {
            event
                .get("layerId")
                .or_else(|| event.get("layer_id"))
                .and_then(Value::as_str)
                == Some(layer_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    write_json(
        &layer_dir(&handle.layrs_dir, layer_id).join("timeline-cache.json"),
        &serde_json::json!({ "schema": "layrs.timeline_cache.v1", "items": items }),
    )
}
