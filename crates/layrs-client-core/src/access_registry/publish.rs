struct PublishV2RequestInput<'a> {
    layer_id: &'a str,
    base_tree_id: Option<String>,
    state: &'a WorkingStateFile,
    changed_paths: Vec<String>,
    deleted_paths: Vec<String>,
    steps: &'a [StepFile],
}

// Kept as a narrow adapter for API-operation and test call sites outside this
// file. The implementation itself uses a named request to make invariants
// explicit and prevent positional argument mistakes.
#[expect(
    clippy::too_many_arguments,
    reason = "compatibility adapter for existing cross-module callers"
)]
fn build_publish_v2_request(
    handle: &LocalSpaceHandle,
    config: &DesktopConfig,
    layer_id: &str,
    base_tree_id: Option<String>,
    state: &WorkingStateFile,
    changed_paths: Vec<String>,
    deleted_paths: Vec<String>,
    steps: &[StepFile],
) -> Result<PublishLayerRequest, String> {
    build_publish_v2_request_from(
        handle,
        config,
        PublishV2RequestInput {
            layer_id,
            base_tree_id,
            state,
            changed_paths,
            deleted_paths,
            steps,
        },
    )
}

fn build_publish_v2_request_from(
    handle: &LocalSpaceHandle,
    config: &DesktopConfig,
    input: PublishV2RequestInput<'_>,
) -> Result<PublishLayerRequest, String> {
    let publish_paths = input
        .changed_paths
        .iter()
        .filter(|path| !input.deleted_paths.contains(path))
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut store_objects = publish_store_objects_for_paths(handle, input.state, &publish_paths)?;
    for step in input.steps {
        let step_state = state_from_step(&handle.layrs_dir, step)?;
        // A later Step may delete a path that an earlier pending Step introduced.
        // Send the complete closure of every pending Step tree, not only the
        // paths still present in the final working state.
        let step_paths = step_state
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<BTreeSet<_>>();
        extend_publish_store_objects_for_paths(
            &mut store_objects,
            handle,
            step_state.root_tree_id.clone(),
            &step_state.files,
            &step_paths,
        )?;
    }
    Ok(PublishLayerRequest {
        layer_id: input.layer_id.to_string(),
        protocol: SYNC_PROTOCOL_V2.to_string(),
        policy_epoch: layer_policy_epoch(handle, input.layer_id),
        idempotency_key: publish_idempotency_key(
            input.layer_id,
            input.state.root_tree_id.as_deref(),
            input.steps,
        ),
        source_client_id: config.device_id.clone(),
        base_tree_id: input.base_tree_id,
        root_tree_id: input.state.root_tree_id.clone(),
        changed_paths: input.changed_paths,
        store_objects,
        artifacts: Vec::new(),
        deleted_paths: input.deleted_paths,
        steps: input
            .steps
            .iter()
            .map(PublishStepRequest::from_step)
            .collect(),
    })
}

fn acknowledged_publish_step_ids(
    response: &Value,
    requested_steps: &[StepFile],
) -> Result<BTreeSet<String>, String> {
    let requested_step_ids = requested_steps
        .iter()
        .map(|step| step.step_id.clone())
        .collect::<Vec<_>>();
    acknowledged_publish_step_ids_exact(response, &requested_step_ids)
}

fn acknowledged_publish_step_ids_exact(
    response: &Value,
    requested_step_ids: &[String],
) -> Result<BTreeSet<String>, String> {
    if requested_step_ids.is_empty() {
        return Ok(BTreeSet::new());
    }
    let acknowledged_steps = response
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "Layrs server did not acknowledge the pending Steps in its publish response."
                .to_string()
        })?;
    let acknowledged_ids = acknowledged_steps
        .iter()
        .map(|step| {
            step.get("stepId")
                .and_then(Value::as_str)
                .filter(|step_id| !step_id.trim().is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    "Layrs server returned a publish Step acknowledgement without a stepId; the local pending queue was preserved."
                        .to_string()
                })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let acknowledged = acknowledged_ids.iter().cloned().collect::<BTreeSet<_>>();
    if acknowledged.len() != acknowledged_ids.len() {
        return Err(
            "Layrs server returned duplicate publish Step acknowledgements; the local pending queue was preserved."
                .to_string(),
        );
    }
    let requested = requested_step_ids.iter().cloned().collect::<BTreeSet<_>>();
    let missing = requested.difference(&acknowledged).cloned().collect::<Vec<_>>();
    let unexpected = acknowledged.difference(&requested).cloned().collect::<Vec<_>>();
    if !missing.is_empty() || !unexpected.is_empty() {
        return Err(format!(
            "Layrs server Step acknowledgement mismatch (missing: {}; unexpected: {}). The local pending queue was preserved.",
            if missing.is_empty() { "none".to_string() } else { missing.join(", ") },
            if unexpected.is_empty() { "none".to_string() } else { unexpected.join(", ") }
        ));
    }
    Ok(requested)
}

fn remove_acknowledged_pending_steps(
    store: &mut LocalStoreSession,
    layrs_dir: &Path,
    layer_id: &str,
    acknowledged_step_ids: &BTreeSet<String>,
) -> Result<(), String> {
    if acknowledged_step_ids.is_empty() {
        return Ok(());
    }
    for pending in read_pending_publish_files(store, layrs_dir, layer_id)? {
        if !acknowledged_step_ids.contains(&pending.step_id) {
            continue;
        }
        let path =
            pending_publish_dir(layrs_dir, layer_id).join(format!("{}.json", pending.step_id));
        store.remove(&path)?;
    }
    Ok(())
}

fn upload_publish_chunks(
    layrs_dir: &Path,
    endpoint: &str,
    token: Option<&str>,
    workspace_id: &str,
    space_id: &str,
    store_objects: &PublishStoreObjectsRequest,
) -> Result<(), String> {
    let mut chunks_by_id = store_objects
        .chunks
        .iter()
        .cloned()
        .map(|chunk| (chunk.chunk_id.clone(), chunk))
        .collect::<BTreeMap<_, _>>();

    for file_object in &store_objects.file_objects {
        for chunk_ref in &file_object.chunks {
            chunks_by_id
                .entry(chunk_ref.chunk_id.clone())
                .or_insert_with(|| PublishChunkObjectRequest {
                    digest: chunk_ref.chunk_id.clone(),
                    chunk_id: chunk_ref.chunk_id.clone(),
                    size: chunk_ref.size,
                    raw_size: chunk_ref.raw_size,
                    stored_size: chunk_ref.stored_size.unwrap_or(chunk_ref.raw_size),
                    compression: chunk_ref.compression.clone(),
                });
        }
    }

    if chunks_by_id.is_empty() {
        return Ok(());
    }

    let prepare_path = format!(
        "/v1/workspaces/{}/spaces/{}/chunks/prepare",
        url_path_segment(workspace_id),
        url_path_segment(space_id)
    );
    let prepare = PrepareChunkUploadRequest {
        chunks: chunks_by_id
            .values()
            .map(|chunk| PrepareChunkUploadItem {
                chunk_id: chunk.chunk_id.clone(),
                size_bytes: chunk.size,
            })
            .collect(),
    };
    let prepared: PrepareChunkUploadResponse = post_json(endpoint, &prepare_path, token, &prepare)?;

    for item in prepared.items {
        if !item.upload_required {
            continue;
        }
        let chunk = chunks_by_id.get(&item.chunk_id).ok_or_else(|| {
            format!(
                "Layrs Desktop could not match prepared upload chunk {} to the publish manifest.",
                item.chunk_id
            )
        })?;
        let file_chunk = FileChunkRef {
            chunk_id: chunk.chunk_id.clone(),
            size: chunk.raw_size,
            compression: chunk.compression.clone(),
            stored_size: Some(chunk.stored_size),
        };
        let encoded = read_chunk_encoded(layrs_dir, &file_chunk)?;
        if encoded.raw_size != chunk.raw_size {
            return Err(format!(
                "Layrs Desktop chunk {} has raw size {}, expected {}.",
                chunk.chunk_id, encoded.raw_size, chunk.raw_size
            ));
        }
        if encoded.digest != chunk.chunk_id || encoded.digest != chunk.digest {
            return Err(format!(
                "Layrs Desktop chunk {} failed local hash verification before upload.",
                chunk.chunk_id
            ));
        }
        let upload_path = item.upload_url.unwrap_or_else(|| {
            format!(
                "/v1/workspaces/{}/spaces/{}/chunks/{}",
                url_path_segment(workspace_id),
                url_path_segment(space_id),
                url_path_segment(&chunk.chunk_id)
            )
        });
        let _: Value = put_bytes_json_with_headers(
            endpoint,
            &upload_path,
            token,
            &encoded.bytes,
            &[
                ("x-layrs-chunk-compression", encoded.compression.clone()),
                ("x-layrs-raw-size", encoded.raw_size.to_string()),
                ("x-layrs-stored-size", encoded.stored_size.to_string()),
            ],
        )?;
    }

    Ok(())
}

fn layer_policy_epoch(handle: &LocalSpaceHandle, layer_id: &str) -> u64 {
    read_json::<LayerAccessFile>(
        &handle.store,
        &layer_dir(&handle.layrs_dir, layer_id).join("access.json"),
    )
        .map(|access| access.policy_epoch)
        .unwrap_or(1)
}

fn publish_idempotency_key(
    layer_id: &str,
    root_tree_id: Option<&str>,
    steps: &[StepFile],
) -> String {
    let mut material = layer_id.as_bytes().to_vec();
    material.push(0);
    material.extend_from_slice(root_tree_id.unwrap_or("none").as_bytes());
    for step in steps {
        material.push(0);
        material.extend_from_slice(step.step_id.as_bytes());
        material.push(0);
        material.extend_from_slice(step.root_tree_id.as_deref().unwrap_or("none").as_bytes());
    }
    format!("publish-{}", object_file_stem(&blake3_id(&material)))
}

fn publish_store_objects_for_paths(
    handle: &LocalSpaceHandle,
    state: &WorkingStateFile,
    paths: &BTreeSet<String>,
) -> Result<PublishStoreObjectsRequest, String> {
    let mut objects = PublishStoreObjectsRequest::default();
    extend_publish_store_objects_for_paths(
        &mut objects,
        handle,
        state.root_tree_id.clone(),
        &state.files,
        paths,
    )?;
    Ok(objects)
}

fn extend_publish_store_objects_for_paths(
    objects: &mut PublishStoreObjectsRequest,
    handle: &LocalSpaceHandle,
    root_tree_id: Option<String>,
    files: &[FileSnapshotEntry],
    paths: &BTreeSet<String>,
) -> Result<(), String> {
    if let Some(root_tree_id) = root_tree_id {
        if !objects
            .tree_objects
            .iter()
            .any(|tree| tree.tree_id == root_tree_id)
        {
            objects.tree_objects.push(PublishTreeObjectRequest {
                tree_id: root_tree_id,
                entries: files
                    .iter()
                    .map(|file| PublishTreeEntryRequest {
                        path: file.path.clone(),
                        file_object_id: file.hash.clone(),
                        size: file.size,
                    })
                    .collect(),
            });
        }
    }
    for file in files {
        if !paths.contains(&file.path) {
            continue;
        }
        let (file_object, chunks) = publish_file_object_for_file(handle, file)?;
        if !objects
            .file_objects
            .iter()
            .any(|object| object.file_object_id == file_object.file_object_id)
        {
            objects.file_objects.push(file_object);
        }
        for chunk in chunks {
            if !objects
                .chunks
                .iter()
                .any(|object| object.chunk_id == chunk.chunk_id)
            {
                objects.chunks.push(chunk);
            }
        }
    }
    Ok(())
}

fn publish_file_object_for_file(
    handle: &LocalSpaceHandle,
    file: &FileSnapshotEntry,
) -> Result<(PublishFileObjectRequest, Vec<PublishChunkObjectRequest>), String> {
    let chunks = publish_chunks_for_file(handle, file)?;
    let refs = chunks
        .iter()
        .map(|chunk| PublishChunkRefRequest {
            chunk_id: chunk.chunk_id.clone(),
            size: chunk.raw_size,
            raw_size: chunk.raw_size,
            stored_size: Some(chunk.stored_size),
            compression: chunk.compression.clone(),
        })
        .collect();
    Ok((
        PublishFileObjectRequest {
            file_object_id: file.hash.clone(),
            size: file.size,
            chunks: refs,
        },
        chunks,
    ))
}

fn publish_chunks_for_file(
    handle: &LocalSpaceHandle,
    file: &FileSnapshotEntry,
) -> Result<Vec<PublishChunkObjectRequest>, String> {
    if file.object.starts_with("objects/files/") {
        let manifest = read_object_json::<FileObjectFile>(&handle.layrs_dir.join(&file.object))?;
        let mut chunks = Vec::with_capacity(manifest.chunks.len());
        for chunk in manifest.chunks {
            chunks.push(PublishChunkObjectRequest {
                digest: chunk.chunk_id.clone(),
                chunk_id: chunk.chunk_id,
                size: chunk.size,
                raw_size: chunk.size,
                stored_size: chunk.stored_size.unwrap_or(chunk.size),
                compression: chunk.compression,
            });
        }
        return Ok(chunks);
    }

    let bytes = read_snapshot_object_bytes(&handle.layrs_dir, file)?;
    let mut chunks = Vec::new();
    write_file_object_manifest(
        &handle.layrs_dir,
        &file.hash,
        &bytes,
        media_type_for_path(&file.path),
    )?;
    let manifest = read_object_json::<FileObjectFile>(&handle.layrs_dir.join(&file.object))?;
    for chunk in manifest.chunks {
        chunks.push(PublishChunkObjectRequest {
            digest: chunk.chunk_id.clone(),
            chunk_id: chunk.chunk_id,
            size: chunk.size,
            raw_size: chunk.size,
            stored_size: chunk.stored_size.unwrap_or(chunk.size),
            compression: chunk.compression,
        });
    }
    Ok(chunks)
}

fn apply_server_mapping_to_draft(
    handle: &mut LocalSpaceHandle,
    created: &CreateSpaceFromLocalResponse,
) -> Result<(), String> {
    let mapping_by_local = created
        .layer_mappings
        .iter()
        .map(|mapping| {
            (
                mapping.local_layer_id.clone(),
                mapping.server_layer_id.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let old_active_layer = handle.active.layer_id.clone();

    for layer in &mut handle.meta.layers {
        if let Some(server_layer_id) = mapping_by_local.get(&layer.layer_id) {
            layer.parent_layer_id = layer
                .parent_layer_id
                .as_ref()
                .and_then(|parent| mapping_by_local.get(parent).cloned())
                .or_else(|| layer.parent_layer_id.clone());
            layer.layer_id = server_layer_id.clone();
        }
    }
    handle.meta.state = LOCAL_SPACE_STATE_LINKED.to_string();
    handle.meta.workspace_id = created.space.workspace_id.clone();
    handle.meta.server_space_id = Some(created.space.id.clone());
    handle.meta.space_id = created.space.id.clone();
    handle.meta.updated_at_unix = unix_now();

    if let Some(server_active_layer) = mapping_by_local.get(&old_active_layer) {
        handle.active.layer_id = server_active_layer.clone();
    } else if let Some(server_active_layer) = created.space.current_layer_id.clone() {
        handle.active.layer_id = server_active_layer;
    }
    handle.active.updated_at_unix = unix_now();

    rewrite_layer_files_after_mapping(
        handle,
        &mapping_by_local,
        &created.space.workspace_id,
        &created.space.id,
    )?;
    remember_local_space(&handle.meta, Some(handle.active.layer_id.clone()))
}

fn rewrite_layer_files_after_mapping(
    handle: &mut LocalSpaceHandle,
    mappings: &BTreeMap<String, String>,
    workspace_id: &str,
    space_id: &str,
) -> Result<(), String> {
    let mut documents = handle.store.document_snapshot()?;
    for (old_layer_id, server_layer_id) in mappings {
        let old_prefix = format!(
            "{}/",
            handle
                .store
                .document_key(&layer_dir(&handle.layrs_dir, old_layer_id))?
        );
        let new_prefix = format!(
            "{}/",
            handle
                .store
                .document_key(&layer_dir(&handle.layrs_dir, server_layer_id))?
        );
        let paths = documents
            .keys()
            .filter(|path| path.starts_with(&old_prefix))
            .cloned()
            .collect::<Vec<_>>();
        for old_path in paths {
            if let Some(value) = documents.remove(&old_path) {
                let suffix = old_path.strip_prefix(&old_prefix).ok_or_else(|| {
                    format!("Layrs could not remap metadata path {old_path}.")
                })?;
                documents.insert(format!("{new_prefix}{suffix}"), value);
            }
        }
    }
    for value in documents.values_mut() {
        remap_document_layer_ids(value, mappings, workspace_id, space_id);
    }
    documents.insert(
        "local-space.json".to_string(),
        serde_json::to_value(&handle.meta)
            .map_err(|error| format!("Layrs could not encode mapped Local Space: {error}"))?,
    );
    documents.insert(
        "active-layer.json".to_string(),
        serde_json::to_value(&handle.active)
            .map_err(|error| format!("Layrs could not encode mapped active Layer: {error}"))?,
    );
    documents.insert(
        "access.json".to_string(),
        serde_json::to_value(access_pointer_from_meta(handle))
            .map_err(|error| format!("Layrs could not encode mapped access pointer: {error}"))?,
    );
    for (old_layer_id, server_layer_id) in mappings {
        let sync_path = handle.store.document_key(
            &layer_dir(&handle.layrs_dir, server_layer_id).join("sync-state.json"),
        )?;
        documents.insert(
            sync_path,
            serde_json::json!({
                "schema": SYNC_STATE_SCHEMA,
                "layerId": server_layer_id,
                "previousLocalLayerId": old_layer_id,
                "lastPublishUnix": unix_now(),
                "pending": false,
                "status": "linked"
            }),
        );
    }
    handle.store.replace_document_snapshot(documents)?;
    Ok(())
}

fn remap_document_layer_ids(
    value: &mut Value,
    mappings: &BTreeMap<String, String>,
    workspace_id: &str,
    space_id: &str,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                remap_document_layer_ids(value, mappings, workspace_id, space_id);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "layerId"
                        | "layer_id"
                        | "parentLayerId"
                        | "parent_layer_id"
                        | "baseLayerId"
                        | "base_layer_id"
                        | "originLayerId"
                        | "origin_layer_id"
                        | "sourceLayerId"
                        | "source_layer_id"
                        | "targetLayerId"
                        | "target_layer_id"
                        | "activeLayerId"
                        | "active_layer_id"
                ) {
                    if let Some(mapped) = value.as_str().and_then(|id| mappings.get(id)) {
                        *value = Value::String(mapped.clone());
                    }
                } else if matches!(key.as_str(), "workspaceId" | "workspace_id") {
                    *value = Value::String(workspace_id.to_string());
                } else if matches!(key.as_str(), "spaceId" | "space_id") {
                    *value = Value::String(space_id.to_string());
                } else {
                    remap_document_layer_ids(value, mappings, workspace_id, space_id);
                }
            }
        }
        _ => {}
    }
}
