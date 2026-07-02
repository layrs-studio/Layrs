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
    let publish_paths = changed_paths
        .iter()
        .filter(|path| !deleted_paths.contains(path))
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut store_objects = publish_store_objects_for_paths(handle, state, &publish_paths)?;
    for step in steps {
        let step_state = state_from_step(&handle.layrs_dir, step)?;
        let step_paths = step
            .changed_paths
            .iter()
            .filter(|path| !deleted_paths.iter().any(|deleted| deleted == *path))
            .cloned()
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
        layer_id: layer_id.to_string(),
        protocol: SYNC_PROTOCOL_V2.to_string(),
        policy_epoch: layer_policy_epoch(handle, layer_id),
        idempotency_key: publish_idempotency_key(layer_id, state.root_tree_id.as_deref(), steps),
        source_client_id: config.device_id.clone(),
        base_tree_id,
        root_tree_id: state.root_tree_id.clone(),
        changed_paths,
        store_objects,
        artifacts: Vec::new(),
        deleted_paths,
        steps: steps.iter().map(PublishStepRequest::from_step).collect(),
    })
}

fn upload_publish_chunks(
    handle: &LocalSpaceHandle,
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
        let encoded = read_chunk_encoded(&handle.layrs_dir, &file_chunk)?;
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
    read_json::<LayerAccessFile>(&layer_dir(&handle.layrs_dir, layer_id).join("access.json"))
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
        let manifest = read_json::<FileObjectFile>(&handle.layrs_dir.join(&file.object))?;
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
    let manifest = read_json::<FileObjectFile>(&handle.layrs_dir.join(&file.object))?;
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
    for mapping in &created.layer_mappings {
        let old_dir = layer_dir(&handle.layrs_dir, &mapping.local_layer_id);
        let new_dir = layer_dir(&handle.layrs_dir, &mapping.server_layer_id);
        if old_dir != new_dir && old_dir.exists() && !new_dir.exists() {
            fs::rename(&old_dir, &new_dir).map_err(|error| {
                format!(
                    "Layrs Desktop could not map local Layer {} to server Layer {}: {error}",
                    mapping.local_layer_id, mapping.server_layer_id
                )
            })?;
        }

        rewrite_layer_files_after_mapping(
            &handle.layrs_dir,
            &mapping.local_layer_id,
            &mapping.server_layer_id,
            &created.space.workspace_id,
            &created.space.id,
        )?;
    }

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

    write_json(&handle.layrs_dir.join("local-space.json"), &handle.meta)?;
    write_json(&handle.layrs_dir.join("active-layer.json"), &handle.active)?;
    write_access_pointer_from_meta(handle)?;
    remember_local_space(&handle.meta, Some(handle.active.layer_id.clone()))
}

fn rewrite_layer_files_after_mapping(
    layrs_dir: &Path,
    old_layer_id: &str,
    server_layer_id: &str,
    workspace_id: &str,
    space_id: &str,
) -> Result<(), String> {
    let dir = layer_dir(layrs_dir, server_layer_id);
    for file_name in ["index.json", "working-state.json"] {
        let path = dir.join(file_name);
        if path.exists() {
            let mut state = read_state_file(layrs_dir, &path)?;
            state.layer_id = server_layer_id.to_string();
            let state = storage_state(layrs_dir, &state)?;
            write_json(&path, &state)?;
        }
    }

    let access_path = dir.join("access.json");
    if access_path.exists() {
        let mut access = read_json::<LayerAccessFile>(&access_path)?;
        access.workspace_id = workspace_id.to_string();
        access.space_id = space_id.to_string();
        access.layer_id = server_layer_id.to_string();
        write_json(&access_path, &access)?;
    }

    let sync_path = dir.join("sync-state.json");
    write_json(
        &sync_path,
        &serde_json::json!({
            "schema": SYNC_STATE_SCHEMA,
            "layerId": server_layer_id,
            "previousLocalLayerId": old_layer_id,
            "lastPublishUnix": unix_now(),
            "pending": false,
            "status": "linked"
        }),
    )
}
