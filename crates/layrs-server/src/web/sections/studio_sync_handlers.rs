async fn studio_snapshot(
    State(state): State<AppState>,
    Query(query): Query<SnapshotQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    let workspaces = workspace_values(&state.pool, &user.id).await?;
    let workspace_id = query
        .workspace_id
        .or_else(|| {
            workspaces
                .first()
                .and_then(|workspace| workspace.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .ok_or_else(|| ApiError::not_found("no workspace exists for this account"))?;
    let workspace = workspaces
        .iter()
        .find(|workspace| workspace.get("id").and_then(Value::as_str) == Some(&workspace_id))
        .cloned()
        .ok_or_else(|| ApiError::not_found("workspace not found"))?;

    Ok(Json(json!({
        "account": studio_account_json(&user),
        "session": session_json(&user, Some(&workspace_id)),
        "workspace": workspace,
        "workspaces": workspaces,
        "teams": team_values(&state.pool, &workspace_id).await?,
        "members": workspace_member_values(&state.pool, &workspace_id).await?,
        "invitations": invitation_values_for_workspace(&state.pool, &workspace_id).await?,
        "spaces": space_values(&state.pool, &workspace_id).await?,
        "layers": layer_values(&state.pool, &workspace_id).await?,
        "artifacts": artifact_values(&state.pool, &workspace_id).await?,
        "steps": [],
        "weaves": [],
        "proofs": [],
        "gates": [],
        "policies": [],
        "timeline": [],
        "accessRegistries": access_registry_values(&state.pool, &workspace_id).await?,
        "devices": device_values(&state.pool, &user.id).await?,
        "auditEvents": audit_event_values(&state.pool, &workspace_id).await?
    })))
}

async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    Ok(Json(
        json!({ "items": device_values(&state.pool, &user.id).await? }),
    ))
}

async fn list_audit_events(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(
        json!({ "items": audit_event_values(&state.pool, &workspace_id).await? }),
    ))
}

async fn local_space_bootstrap(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    let workspace = workspace_value_for_account(&state.pool, &workspace_id, &user.id).await?;
    let space = space_value(&state.pool, &workspace_id, &space_id).await?;
    let layers = layer_values_for_space(&state.pool, &workspace_id, &space_id).await?;
    let layer_ids = layers
        .iter()
        .filter_map(|layer| layer.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let timeline = timeline_event_values(
        &state.pool,
        &workspace_id,
        Some(&space_id),
        None,
        None,
        Some(50),
    )
    .await?;

    Ok(Json(json!({
        "workspace": workspace,
        "space": space,
        "layers": layers,
        "accessRegistries": access_registry_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids).await?,
        "timeline": {
            "cursor": latest_timeline_cursor(&timeline),
            "events": timeline
        },
        "lenses": crate::lenses::load_lens_registry_from_env().items,
        "artifacts": artifact_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids, &user.id).await?
    })))
}

async fn receive_local_space_sync(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<SyncReceiveBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    let layer_id = body
        .layer_id
        .or(body.layer_id_camel)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(layer_id) = layer_id.as_deref() {
        ensure_layer_in_space(&state.pool, &workspace_id, &space_id, layer_id).await?;
    } else {
        ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    }
    let layers = layer_values_for_space(&state.pool, &workspace_id, &space_id).await?;
    let layer_ids = layers
        .iter()
        .filter_map(|layer| layer.get("id").and_then(Value::as_str).map(str::to_string))
        .collect::<Vec<_>>();
    let request_cursor = body.cursor;
    let timeline = timeline_event_values(
        &state.pool,
        &workspace_id,
        Some(&space_id),
        None,
        request_cursor.as_deref(),
        body.limit,
    )
    .await?;
    let response_cursor = latest_timeline_cursor(&timeline).or(request_cursor);
    let access_registries =
        access_registry_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids)
            .await?;
    let steps =
        layer_step_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids).await?;
    let content_objects = receive_store_objects_for_layers(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_ids,
        &user.id,
    )
    .await?;
    let layer_head =
        layer_head_value(&state.pool, &workspace_id, &space_id, layer_id.as_deref()).await?;
    let root_tree_id = layer_head.get("rootTreeId").cloned().unwrap_or(Value::Null);

    Ok(Json(json!({
        "protocol": "layrs.sync.v2",
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "rootTreeId": root_tree_id,
        "cursor": response_cursor,
        "layerHead": layer_head,
        "layers": layers,
        "accessRegistries": access_registries,
        "steps": steps,
        "timeline": timeline,
        "artifacts": artifact_values_for_layers(&state.pool, &workspace_id, &space_id, &layer_ids, &user.id).await?,
        "contentObjects": content_objects,
        "contents": []
    })))
}

async fn publish_local_space_sync(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<SyncPublishBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_write(&state.pool, &workspace_id, &user.id).await?;
    let layer_id = require_layer_id(body.layer_id, body.layer_id_camel)?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let received_cursor = body.cursor;
    let protocol = body.protocol;
    let policy_epoch = body.policy_epoch.or(body.policy_epoch_camel);
    let idempotency_key = body.idempotency_key.or(body.idempotency_key_camel);
    let source_client_id = body.source_client_id.or(body.source_client_id_camel);
    let root_tree_id = body.root_tree_id.or(body.root_tree_id_camel);
    let base_tree_id = body.base_tree_id.or(body.base_tree_id_camel);
    let mut steps = body.steps;
    if let Some(step) = body.step {
        steps.push(step);
    }
    let mut changed_paths = body.changed_paths;
    changed_paths.extend(body.changed_paths_camel);
    let mut store_objects = Vec::new();
    if let Some(objects) = body.store_objects {
        store_objects.extend(objects.into_flat()?);
    }
    if let Some(objects) = body.store_objects_camel {
        store_objects.extend(objects.into_flat()?);
    }
    let mut artifacts = body.artifacts;
    if let Some(artifact) = body.artifact {
        artifacts.push(artifact);
    }
    let mut deleted_paths = body.deleted_paths;
    deleted_paths.extend(body.deleted_paths_camel);
    let mut publish_artifacts = Vec::new();
    for artifact in artifacts {
        if artifact_requests_deletion(&artifact) {
            deleted_paths.push(required_artifact_path(&artifact)?);
        } else {
            publish_artifacts.push(artifact);
        }
    }
    let deleted_paths = normalize_deleted_paths(&deleted_paths)?;

    let protocol_value = protocol.as_deref();
    if protocol_value != Some("layrs.sync.v2") {
        return Err(ApiError::bad_request(
            "protocol layrs.sync.v2 is required; inline artifact publish is not supported",
        ));
    }
    if store_objects.is_empty()
        && publish_artifacts.is_empty()
        && deleted_paths.is_empty()
        && steps.is_empty()
    {
        return Err(ApiError::bad_request(
            "at least one store object, artifact, deleted path, or step is required",
        ));
    }

    publish_local_space_sync_v2(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_id,
        &user.id,
        received_cursor,
        policy_epoch,
        idempotency_key,
        source_client_id,
        base_tree_id,
        root_tree_id,
        protocol,
        changed_paths,
        steps,
        store_objects,
        publish_artifacts,
        deleted_paths,
    )
    .await
    .map(Json)
}

async fn publish_local_space_sync_v2(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
    received_cursor: Option<String>,
    expected_policy_epoch: Option<i64>,
    idempotency_key: Option<String>,
    source_client_id: Option<String>,
    requested_base_tree_id: Option<String>,
    requested_root_tree_id: Option<String>,
    protocol: Option<String>,
    changed_paths: Vec<String>,
    steps: Vec<SyncStepBody>,
    store_objects: Vec<PublishStoreObjectBody>,
    mut publish_artifacts: Vec<PublishArtifactBody>,
    mut deleted_paths: Vec<String>,
) -> Result<Value, ApiError> {
    if let Some(key) = idempotency_key.as_deref() {
        if let Some(response) = load_sync_batch_response(pool, workspace_id, space_id, key).await? {
            return Ok(response);
        }
    }
    let policy_epoch = current_policy_epoch(pool, workspace_id, space_id, layer_id).await?;
    if let Some(expected) = expected_policy_epoch {
        if expected != policy_epoch {
            return Err(ApiError::conflict(format!(
                "policy_epoch mismatch: expected {expected}, current {policy_epoch}"
            )));
        }
    }

    let mut tx = pool.begin().await?;
    let sync_batch_id = prefixed_id("sync_batch");
    if let Some(key) = idempotency_key.as_deref() {
        sqlx::query(
            r#"
            INSERT INTO sync_batches
                (sync_batch_id, workspace_id, space_id, layer_id, idempotency_key,
                 source_client_id, base_cursor, policy_epoch, status, created_by_account_id, request_json)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, 'reserved', $9, $10)
            "#,
        )
        .bind(&sync_batch_id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(layer_id)
        .bind(key)
        .bind(source_client_id.as_deref())
        .bind(received_cursor.as_deref())
        .bind(policy_epoch)
        .bind(account_id)
        .bind(json!({
            "protocol": protocol,
            "changedPaths": changed_paths,
            "storeObjectCount": store_objects.len(),
            "artifactCount": publish_artifacts.len(),
            "deletedPathCount": deleted_paths.len(),
            "stepCount": steps.len()
        }))
        .execute(&mut *tx)
        .await?;
    }

    let store_index =
        upsert_store_objects_in_tx(&mut tx, workspace_id, space_id, account_id, store_objects)
            .await?;
    deleted_paths.extend(store_index.deleted_paths.iter().cloned());
    if publish_artifacts.is_empty() {
        for (path, file) in &store_index.file_by_path {
            publish_artifacts.push(PublishArtifactBody {
                id: None,
                artifact_id: None,
                artifact_id_camel: None,
                path: Some(path.clone()),
                logical_path: None,
                logical_path_camel: None,
                kind: Some("file".to_string()),
                artifact_type: None,
                media_type: file.media_type.clone(),
                media_type_camel: None,
                content: None,
                file_object_id: Some(file.file_object_id.clone()),
                file_object_id_camel: None,
                object_id: None,
                object_id_camel: None,
                tree_id: None,
                tree_id_camel: None,
                sha256: Some(file.digest.clone()),
                content_hash: None,
                size_bytes: Some(file.size_bytes),
                size_bytes_camel: None,
                chunks: Vec::new(),
                state: None,
                operation: None,
                action: None,
                deleted: None,
            });
        }
    }
    let mut published_ids = Vec::new();
    let mut deleted_values = Vec::new();
    let mut event_ids = Vec::new();
    let mut change_index = 0;
    for mut artifact in publish_artifacts {
        if artifact.content.is_some() {
            return Err(ApiError::bad_request(
                "inline artifact content is not supported; upload chunks before publish",
            ));
        }
        if !publish_artifact_uses_v2(&artifact) {
            apply_store_object_to_artifact(&mut artifact, &store_index)?;
        }
        let artifact_path = required_artifact_path(&artifact).ok();
        if !publish_artifact_uses_v2(&artifact) {
            return Err(ApiError::bad_request(
                "artifact publish requires a fileObjectId or uploaded chunks",
            ));
        }
        let (artifact_id, event_id, file_object_id) = publish_artifact_v2_in_tx(
            pool,
            &mut tx,
            workspace_id,
            space_id,
            layer_id,
            account_id,
            artifact,
        )
        .await?;
        if idempotency_key.is_some() {
            insert_sync_batch_change_in_tx(
                &mut tx,
                &sync_batch_id,
                change_index,
                "upsert_file",
                Some(&artifact_id),
                artifact_path.as_deref(),
                file_object_id.as_deref(),
                None,
                json!({ "eventId": event_id }),
            )
            .await?;
            change_index += 1;
        }
        published_ids.push(artifact_id);
        event_ids.push(event_id);
    }
    for path in deleted_paths {
        let (artifact_id, artifact_value, event_id) = delete_artifact_tombstone_in_tx(
            pool,
            &mut tx,
            workspace_id,
            space_id,
            layer_id,
            account_id,
            &path,
        )
        .await?;
        if idempotency_key.is_some() {
            insert_sync_batch_change_in_tx(
                &mut tx,
                &sync_batch_id,
                change_index,
                "delete_path",
                Some(&artifact_id),
                Some(&path),
                None,
                None,
                json!({ "eventId": event_id }),
            )
            .await?;
            change_index += 1;
        }
        deleted_values.push(artifact_value);
        event_ids.push(event_id);
    }

    let root_tree_id =
        if let Some(root_tree_id) = store_index.root_tree_id.or(requested_root_tree_id) {
            ensure_tree_in_space_in_tx(&mut tx, workspace_id, space_id, &root_tree_id).await?;
            Some(root_tree_id)
        } else {
            rebuild_layer_tree_in_tx(&mut tx, workspace_id, space_id, layer_id, account_id).await?
        };
    let server_cursor = event_ids.last().cloned();
    let layer_state_id = advance_layer_head_in_tx(
        &mut tx,
        workspace_id,
        space_id,
        layer_id,
        root_tree_id.as_deref(),
        policy_epoch,
        server_cursor.as_deref(),
        account_id,
    )
    .await?;
    let sync_batch_ref = if idempotency_key.is_some() {
        Some(sync_batch_id.as_str())
    } else {
        None
    };
    let mut recorded_steps = Vec::new();
    if steps.is_empty() {
        let step_id = insert_layer_step_in_tx(
            &mut tx,
            workspace_id,
            space_id,
            layer_id,
            None,
            requested_base_tree_id.as_deref(),
            root_tree_id.as_deref(),
            &changed_paths,
            source_client_id.as_deref(),
            sync_batch_ref,
            account_id,
        )
        .await?;
        recorded_steps.push(json!({
            "stepId": step_id,
            "layerId": layer_id,
            "rootTreeId": root_tree_id.as_deref()
        }));
    } else {
        for step in &steps {
            let step_id = insert_layer_step_in_tx(
                &mut tx,
                workspace_id,
                space_id,
                layer_id,
                Some(step),
                requested_base_tree_id.as_deref(),
                root_tree_id.as_deref(),
                &changed_paths,
                source_client_id.as_deref(),
                sync_batch_ref,
                account_id,
            )
            .await?;
            recorded_steps.push(json!({
                "stepId": step_id,
                "layerId": layer_id,
                "rootTreeId": step.root_tree_id.as_deref().or(root_tree_id.as_deref())
            }));
        }
    }
    if idempotency_key.is_some() {
        insert_sync_batch_change_in_tx(
            &mut tx,
            &sync_batch_id,
            change_index,
            "advance_head",
            None,
            None,
            None,
            root_tree_id.as_deref(),
            json!({ "layerStateId": layer_state_id }),
        )
        .await?;
        change_index += 1;
        for step in &recorded_steps {
            insert_sync_batch_change_in_tx(
                &mut tx,
                &sync_batch_id,
                change_index,
                "record_step",
                None,
                None,
                None,
                step.get("rootTreeId")
                    .and_then(Value::as_str)
                    .or(root_tree_id.as_deref()),
                json!({ "stepId": step.get("stepId").and_then(Value::as_str) }),
            )
            .await?;
            change_index += 1;
        }
        sqlx::query(
            "UPDATE sync_batches SET status = 'applied', server_cursor = $1, updated_at = now() WHERE sync_batch_id = $2",
        )
        .bind(server_cursor.as_deref())
        .bind(&sync_batch_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    let layer_artifacts =
        artifact_values_for_layer(pool, workspace_id, space_id, layer_id, account_id).await?;
    let published = published_ids
        .iter()
        .filter_map(|artifact_id| {
            layer_artifacts
                .iter()
                .find(|artifact| {
                    artifact.get("id").and_then(Value::as_str) == Some(artifact_id.as_str())
                })
                .cloned()
        })
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    for event_id in &event_ids {
        events.push(timeline_event_by_id(pool, workspace_id, event_id).await?);
    }
    let response = json!({
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "receivedCursor": received_cursor,
        "serverCursor": latest_timeline_cursor(&events).or(server_cursor),
        "policyEpoch": policy_epoch,
        "layerHead": {
            "layerId": layer_id,
            "layerStateId": layer_state_id,
            "rootTreeId": root_tree_id,
            "policyEpoch": policy_epoch
        },
        "step": recorded_steps.last().cloned(),
        "steps": recorded_steps,
        "syncBatchId": if idempotency_key.is_some() { Some(sync_batch_id.as_str()) } else { None },
        "published": published,
        "deleted": deleted_values,
        "timeline": events
    });
    if idempotency_key.is_some() {
        sqlx::query("UPDATE sync_batches SET response_json = $1, updated_at = now() WHERE sync_batch_id = $2")
            .bind(&response)
            .bind(&sync_batch_id)
            .execute(pool)
            .await?;
    }
    Ok(response)
}

async fn prepare_space_chunks(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<PrepareChunksBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_write(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    if body.chunks.is_empty() {
        return Err(ApiError::bad_request("at least one chunk is required"));
    }

    let mut items = Vec::new();
    for item in body.chunks {
        let prepared = prepared_chunk_from_item(item)?;
        let exists =
            object_chunk_exists(&state.pool, &workspace_id, &space_id, &prepared.chunk_id).await?;
        items.push(json!({
            "chunkId": prepared.chunk_id,
            "digest": prepared.digest,
            "sizeBytes": prepared.size_bytes,
            "mediaType": prepared.media_type,
            "exists": exists,
            "uploadRequired": !exists,
            "uploadUrl": format!("/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/{}", prepared.chunk_id)
        }));
    }

    Ok(Json(json!({
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "items": items,
        "missing": items.iter().filter(|item| item.get("uploadRequired").and_then(Value::as_bool) == Some(true)).cloned().collect::<Vec<_>>()
    })))
}

async fn put_space_chunk(
    State(state): State<AppState>,
    Path((workspace_id, space_id, chunk_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_write(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    let chunk_id = validate_chunk_id(&chunk_id)?;
    let compression = chunk_compression_from_headers(&headers)?;
    let raw_bytes = decode_chunk_bytes(&bytes, &compression)?;
    let raw_size = raw_bytes.len() as i64;
    if let Some(expected_raw_size) = header_i64(&headers, "x-layrs-raw-size")? {
        if expected_raw_size != raw_size {
            return Err(ApiError::bad_request(
                "chunk raw size header does not match decoded bytes",
            ));
        }
    }
    if let Some(expected_stored_size) = header_i64(&headers, "x-layrs-stored-size")? {
        if expected_stored_size != bytes.len() as i64 {
            return Err(ApiError::bad_request(
                "chunk stored size header does not match uploaded bytes",
            ));
        }
    }
    let digest = blake3_digest_for_bytes(&raw_bytes);
    if chunk_id != digest {
        return Err(ApiError::bad_request(
            "chunk_id must match the decoded chunk bytes blake3 digest",
        ));
    }
    let stored_size_bytes = bytes.len() as i64;
    let object_key = format!("chunks/global/{chunk_id}");
    sqlx::query(
        r#"
        INSERT INTO object_chunks
            (chunk_id, workspace_id, space_id, digest, size_bytes, stored_size_bytes, object_key, compression, state, content_bytes, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, 'available', $9, $10)
        ON CONFLICT (chunk_id) DO UPDATE SET
            digest = EXCLUDED.digest,
            size_bytes = EXCLUDED.size_bytes,
            stored_size_bytes = EXCLUDED.stored_size_bytes,
            object_key = EXCLUDED.object_key,
            compression = EXCLUDED.compression,
            state = 'available',
            content_bytes = EXCLUDED.content_bytes,
            updated_at = now()
        "#,
    )
    .bind(&chunk_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&digest)
    .bind(raw_size)
    .bind(stored_size_bytes)
    .bind(&object_key)
    .bind(&compression)
    .bind(bytes.to_vec())
    .bind(&user.id)
    .execute(&state.pool)
    .await?;

    mark_chunk_available_for_space(&state.pool, &workspace_id, &space_id, &chunk_id, &user.id)
        .await?;

    Ok(Json(json!({
        "chunkId": chunk_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "digest": digest,
        "sizeBytes": raw_size,
        "rawSize": raw_size,
        "storedSize": stored_size_bytes,
        "compression": compression,
        "objectKey": object_key,
        "state": "available"
    })))
}

async fn get_space_chunk(
    State(state): State<AppState>,
    Path((workspace_id, space_id, chunk_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    let chunk_id = validate_chunk_id(&chunk_id)?;
    let row = sqlx::query(
        r#"
        SELECT content_bytes, media_type, size_bytes, stored_size_bytes, digest, compression
        FROM object_chunks oc
        JOIN space_object_chunks soc ON soc.chunk_id = oc.chunk_id
        WHERE soc.workspace_id = $1
          AND soc.space_id = $2
          AND oc.chunk_id = $3
          AND oc.state = 'available'
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&chunk_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("chunk not found"))?;
    let bytes = row
        .try_get::<Vec<u8>, _>("content_bytes")
        .ok()
        .ok_or_else(|| ApiError::not_found("chunk bytes are not available"))?;
    let media_type = row
        .try_get::<String, _>("media_type")
        .ok()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let stored_size = row
        .try_get::<i64, _>("stored_size_bytes")
        .ok()
        .unwrap_or(bytes.len() as i64);
    let compression = row
        .try_get::<String, _>("compression")
        .ok()
        .unwrap_or_else(|| CHUNK_COMPRESSION_IDENTITY.to_string());
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, media_type)
        .header("x-layrs-chunk-id", chunk_id)
        .header("x-layrs-digest", row.get::<String, _>("digest"))
        .header(
            "x-layrs-raw-size",
            row.get::<i64, _>("size_bytes").to_string(),
        )
        .header("x-layrs-stored-size", stored_size.to_string())
        .header("x-layrs-chunk-compression", compression)
        .header("content-length", bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(response)
}
