async fn space_value(pool: &PgPool, workspace_id: &str, space_id: &str) -> Result<Value, ApiError> {
    space_values(pool, workspace_id)
        .await?
        .into_iter()
        .find(|space| space.get("id").and_then(Value::as_str) == Some(space_id))
        .ok_or_else(|| ApiError::not_found("space not found"))
}

async fn layer_values_for_space(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
) -> Result<Vec<Value>, ApiError> {
    Ok(layer_values(pool, workspace_id)
        .await?
        .into_iter()
        .filter(|layer| layer.get("spaceId").and_then(Value::as_str) == Some(space_id))
        .collect())
}

async fn access_registry_values_for_layers(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for layer_id in layer_ids {
        values.push(layer_access_policy_value(pool, workspace_id, space_id, layer_id).await?);
    }
    Ok(values)
}

async fn artifact_values_for_layers(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
    account_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for layer_id in layer_ids {
        values.extend(
            artifact_values_for_layer(pool, workspace_id, space_id, layer_id, account_id).await?,
        );
    }
    Ok(values)
}

async fn receive_store_objects_for_layers(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
    account_id: &str,
) -> Result<Value, ApiError> {
    let mut chunks_by_id = BTreeMap::<String, Value>::new();
    let mut file_objects_by_id = BTreeMap::<String, Value>::new();
    let mut tree_objects = Vec::new();

    for layer_id in layer_ids {
        let root_tree_id = sqlx::query_scalar::<_, Option<String>>(
            r#"
            SELECT root_tree_id
            FROM layer_heads
            WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
            "#,
        )
        .bind(workspace_id)
        .bind(space_id)
        .bind(layer_id)
        .fetch_optional(pool)
        .await?
        .flatten();
        let Some(root_tree_id) = root_tree_id else {
            tree_objects.push(json!({
                "treeId": blake3_digest_for_bytes(b""),
                "layerId": layer_id,
                "entries": []
            }));
            continue;
        };

        push_received_tree_object_for_layer(
            pool,
            workspace_id,
            space_id,
            layer_id,
            &root_tree_id,
            account_id,
            &mut chunks_by_id,
            &mut file_objects_by_id,
            &mut tree_objects,
        )
        .await?;
    }

    let step_tree_rows = sqlx::query(
        r#"
        SELECT DISTINCT layer_id,
               COALESCE(base_layer_id, layer_id) AS base_layer_id,
               root_tree_id,
               base_tree_id
        FROM layer_steps
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = ANY($3)
          AND cleared_at IS NULL
          AND (root_tree_id IS NOT NULL OR base_tree_id IS NOT NULL)
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_ids)
    .fetch_all(pool)
    .await?;
    let mut seen_step_trees = tree_objects
        .iter()
        .filter_map(|tree| {
            tree.get("treeId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<HashSet<_>>();
    for row in step_tree_rows {
        let layer_id = row.get::<String, _>("layer_id");
        let base_layer_id = row.get::<String, _>("base_layer_id");
        for (tree_id, access_layer_id) in [
            (
                row.try_get::<String, _>("root_tree_id").ok(),
                layer_id.as_str(),
            ),
            (
                row.try_get::<String, _>("base_tree_id").ok(),
                base_layer_id.as_str(),
            ),
        ] {
            let Some(tree_id) = tree_id else {
                continue;
            };
            if !seen_step_trees.insert(tree_id.clone()) {
                continue;
            }
            push_received_tree_object_for_layer(
                pool,
                workspace_id,
                space_id,
                access_layer_id,
                &tree_id,
                account_id,
                &mut chunks_by_id,
                &mut file_objects_by_id,
                &mut tree_objects,
            )
            .await?;
        }
    }

    Ok(json!({
        "chunks": chunks_by_id.into_values().collect::<Vec<_>>(),
        "fileObjects": file_objects_by_id.into_values().collect::<Vec<_>>(),
        "treeObjects": tree_objects
    }))
}

async fn push_received_tree_object_for_layer(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    tree_id: &str,
    account_id: &str,
    chunks_by_id: &mut BTreeMap<String, Value>,
    file_objects_by_id: &mut BTreeMap<String, Value>,
    tree_objects: &mut Vec<Value>,
) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT te.logical_path, te.file_object_id,
               f.digest, f.size_bytes, f.media_type,
               a.artifact_id, a.state
        FROM tree_entries te
        JOIN file_objects f ON f.file_object_id = te.file_object_id
        JOIN space_tree_objects sto ON sto.tree_id = te.tree_id
        JOIN space_file_objects sfo ON sfo.file_object_id = f.file_object_id
        LEFT JOIN artifacts a ON a.workspace_id = $2
            AND a.space_id = $3
            AND a.layer_id = $4
            AND a.logical_path = te.logical_path
        WHERE te.tree_id = $1
          AND sto.workspace_id = $2
          AND sto.space_id = $3
          AND sfo.workspace_id = $2
          AND sfo.space_id = $3
        ORDER BY te.logical_path ASC
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_all(pool)
    .await?;

    let mut entries = Vec::new();
    for row in rows {
        let path = row.get::<String, _>("logical_path");
        let artifact_id = row.try_get::<String, _>("artifact_id").ok();
        let state = row.try_get::<String, _>("state").ok();
        let decision = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            layer_id,
            &path,
            artifact_id.as_deref(),
            account_id,
        )
        .await?;
        if state.as_deref() == Some("redacted") || !decision.can_read {
            continue;
        }

        let file_object_id = row.get::<String, _>("file_object_id");
        let size_bytes = row.get::<i64, _>("size_bytes");
        let chunks =
            chunk_values_for_file_object(pool, workspace_id, space_id, &file_object_id).await?;
        for chunk in &chunks {
            if let Some(chunk_id) = chunk.get("chunkId").and_then(Value::as_str) {
                chunks_by_id
                    .entry(chunk_id.to_string())
                    .or_insert_with(|| chunk.clone());
            }
        }
        file_objects_by_id
            .entry(file_object_id.clone())
            .or_insert_with(|| {
                json!({
                    "fileObjectId": file_object_id,
                    "hash": row.get::<String, _>("digest"),
                    "digest": row.get::<String, _>("digest"),
                    "size": size_bytes,
                    "sizeBytes": size_bytes,
                    "mediaType": row.try_get::<String, _>("media_type").ok().unwrap_or_else(|| "application/octet-stream".to_string()),
                    "chunks": chunks.iter().map(|chunk| json!({
                        "chunkId": chunk.get("chunkId").cloned().unwrap_or(Value::Null),
                        "digest": chunk.get("digest").cloned().unwrap_or(Value::Null),
                        "size": chunk.get("size").cloned().unwrap_or(Value::Null),
                        "sizeBytes": chunk.get("sizeBytes").cloned().unwrap_or(Value::Null),
                        "byteOffset": chunk.get("byteOffset").cloned().unwrap_or(Value::Null)
                    })).collect::<Vec<_>>()
                })
            });
        entries.push(json!({
            "path": path,
            "fileObjectId": file_object_id,
            "size": size_bytes,
            "sizeBytes": size_bytes
        }));
    }

    tree_objects.push(json!({
        "treeId": tree_id,
        "layerId": layer_id,
        "entries": entries
    }));
    Ok(())
}

async fn layer_step_values_for_layers(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
) -> Result<Vec<Value>, ApiError> {
    if layer_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT step_id, layer_id, parent_step_id, base_layer_id, base_tree_id,
               root_tree_id, changed_paths, source_client_id, sync_batch_id,
               timeline_position, origin_layer_id, origin_layer_name, origin_step_id, step_kind,
               EXTRACT(EPOCH FROM captured_at)::bigint AS captured_at_unix,
               to_char(captured_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS captured_at
        FROM layer_steps
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = ANY($3)
          AND cleared_at IS NULL
        ORDER BY captured_at ASC, created_at ASC, step_id ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_ids)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "stepId": row.get::<String, _>("step_id"),
                "layerId": row.get::<String, _>("layer_id"),
                "parentStepId": row.try_get::<String, _>("parent_step_id").ok(),
                "baseLayerId": row.try_get::<String, _>("base_layer_id").ok(),
                "baseTreeId": row.try_get::<String, _>("base_tree_id").ok(),
                "rootTreeId": row.try_get::<String, _>("root_tree_id").ok(),
                "changedPaths": row.try_get::<Vec<String>, _>("changed_paths").unwrap_or_default(),
                "sourceClientId": row.try_get::<String, _>("source_client_id").ok(),
                "syncBatchId": row.try_get::<String, _>("sync_batch_id").ok(),
                "timelinePosition": row.try_get::<i64, _>("timeline_position").ok(),
                "originLayerId": row.try_get::<String, _>("origin_layer_id").ok(),
                "originLayerName": row.try_get::<String, _>("origin_layer_name").ok(),
                "originStepId": row.try_get::<String, _>("origin_step_id").ok(),
                "stepKind": row.try_get::<String, _>("step_kind").ok(),
                "capturedAtUnix": row.get::<i64, _>("captured_at_unix"),
                "capturedAt": row.get::<String, _>("captured_at")
            })
        })
        .collect())
}

async fn layer_step_detail_values(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    step_id: Option<&str>,
    account_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT step_id, layer_id, parent_step_id, base_layer_id, base_tree_id,
               root_tree_id, changed_paths, source_client_id, sync_batch_id,
               timeline_position, origin_layer_id, origin_layer_name, origin_step_id, step_kind,
               EXTRACT(EPOCH FROM captured_at)::bigint AS captured_at_unix,
               to_char(captured_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS captured_at
        FROM layer_steps
        WHERE workspace_id = $1
          AND space_id = $2
          AND layer_id = $3
          AND ($4::text IS NULL OR step_id = $4)
          AND cleared_at IS NULL
        ORDER BY captured_at DESC, created_at DESC, step_id DESC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(step_id)
    .fetch_all(pool)
    .await?;

    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        let step_id = row.get::<String, _>("step_id");
        let step_layer_id = row.get::<String, _>("layer_id");
        let base_layer_id = row.try_get::<String, _>("base_layer_id").ok();
        let base_tree_id = row.try_get::<String, _>("base_tree_id").ok();
        let root_tree_id = row.try_get::<String, _>("root_tree_id").ok();
        let changed_paths = row
            .try_get::<Vec<String>, _>("changed_paths")
            .unwrap_or_default();
        let files = step_changed_file_values(
            pool,
            workspace_id,
            space_id,
            &step_layer_id,
            base_layer_id.as_deref().unwrap_or(&step_layer_id),
            base_tree_id.as_deref(),
            root_tree_id.as_deref(),
            &changed_paths,
            account_id,
        )
        .await?;
        values.push(json!({
            "id": step_id,
            "stepId": step_id,
            "workspaceId": workspace_id,
            "spaceId": space_id,
            "layerId": step_layer_id,
            "status": "passing",
            "parentStepId": row.try_get::<String, _>("parent_step_id").ok(),
            "baseLayerId": base_layer_id,
            "baseTreeId": base_tree_id,
            "rootTreeId": root_tree_id,
            "changedPaths": changed_paths,
            "changedFiles": files.len(),
            "diffStats": {
                "files": files.len(),
                "additions": 0,
                "removals": 0
            },
            "files": files,
            "sourceClientId": row.try_get::<String, _>("source_client_id").ok(),
            "syncBatchId": row.try_get::<String, _>("sync_batch_id").ok(),
            "timelinePosition": row.try_get::<i64, _>("timeline_position").ok(),
            "originLayerId": row.try_get::<String, _>("origin_layer_id").ok(),
            "originLayerName": row.try_get::<String, _>("origin_layer_name").ok(),
            "originStepId": row.try_get::<String, _>("origin_step_id").ok(),
            "stepKind": row.try_get::<String, _>("step_kind").ok(),
            "capturedAtUnix": row.get::<i64, _>("captured_at_unix"),
            "capturedAt": row.get::<String, _>("captured_at")
        }));
    }
    Ok(values)
}

async fn step_changed_file_values(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    base_layer_id: &str,
    base_tree_id: Option<&str>,
    root_tree_id: Option<&str>,
    changed_paths: &[String],
    account_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let mut values = Vec::new();
    for path in changed_paths {
        let path = validate_publish_path(path.trim())?;
        let decision = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            layer_id,
            &path,
            None,
            account_id,
        )
        .await?;
        let base =
            tree_file_ref_for_path(pool, workspace_id, space_id, base_tree_id, &path).await?;
        let target =
            tree_file_ref_for_path(pool, workspace_id, space_id, root_tree_id, &path).await?;
        let action = match (base.is_some(), target.is_some()) {
            (false, true) => "added",
            (true, false) => "deleted",
            (true, true) => "modified",
            (false, false) => "missing",
        };
        let media_type = target
            .as_ref()
            .or(base.as_ref())
            .and_then(|file| file.media_type.clone())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        values.push(json!({
            "path": path,
            "name": path.rsplit('/').next().unwrap_or(path.as_str()),
            "action": action,
            "lensId": lens_id_for_path_and_media_type(&path, &media_type),
            "mediaType": media_type,
            "baseLayerId": base_layer_id,
            "baseFileObjectId": base.as_ref().map(|file| file.file_object_id.as_str()),
            "targetFileObjectId": target.as_ref().map(|file| file.file_object_id.as_str()),
            "sizeBytes": target.as_ref().or(base.as_ref()).map(|file| file.size_bytes),
            "access": {
                "canOpen": decision.can_read,
                "isRedacted": !decision.can_read,
                "reason": if decision.can_read { None } else { Some(decision.reason) }
            }
        }));
    }
    Ok(values)
}

#[derive(Clone, Debug)]
struct TreeFileRef {
    file_object_id: String,
    size_bytes: i64,
    media_type: Option<String>,
}

async fn tree_file_ref_for_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    tree_id: Option<&str>,
    path: &str,
) -> Result<Option<TreeFileRef>, ApiError> {
    let Some(tree_id) = tree_id else {
        return Ok(None);
    };
    let row = sqlx::query(
        r#"
        SELECT f.file_object_id, f.digest, f.size_bytes, f.media_type
        FROM tree_entries te
        JOIN file_objects f ON f.file_object_id = te.file_object_id
        JOIN space_tree_objects sto ON sto.tree_id = te.tree_id
        JOIN space_file_objects sfo ON sfo.file_object_id = f.file_object_id
        WHERE te.tree_id = $1
          AND sto.workspace_id = $2
          AND sto.space_id = $3
          AND sfo.workspace_id = $2
          AND sfo.space_id = $3
          AND te.logical_path = $4
          AND te.entry_kind = 'file'
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(path)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| TreeFileRef {
        file_object_id: row.get("file_object_id"),
        size_bytes: row.get("size_bytes"),
        media_type: row.try_get("media_type").ok(),
    }))
}

fn lens_id_for_path_and_media_type(path: &str, media_type: &str) -> &'static str {
    if is_code_path(path) {
        "layrs.code"
    } else if is_textual_artifact(path, media_type) {
        "layrs.text"
    } else if media_type.starts_with("image/") {
        "layrs.image"
    } else {
        "layrs.raw"
    }
}
