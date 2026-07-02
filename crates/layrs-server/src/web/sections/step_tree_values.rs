async fn step_diff_window_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    step_id: &str,
    requested_path: Option<&str>,
    account_id: &str,
    window_request: WindowRequest,
) -> Result<Value, ApiError> {
    let mut steps = layer_step_detail_values(
        pool,
        workspace_id,
        space_id,
        layer_id,
        Some(step_id),
        account_id,
    )
    .await?;
    let step = steps
        .pop()
        .ok_or_else(|| ApiError::not_found("step not found"))?;
    let root_tree_id = step.get("rootTreeId").and_then(Value::as_str);
    let base_tree_id = step.get("baseTreeId").and_then(Value::as_str);
    let base_layer_id = step
        .get("baseLayerId")
        .and_then(Value::as_str)
        .unwrap_or(layer_id);
    let changed_paths = step
        .get("changedPaths")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let path = step_diff_path(
        pool,
        workspace_id,
        space_id,
        requested_path,
        &changed_paths,
        root_tree_id,
        base_tree_id,
    )
    .await?;
    let target = tree_text_window_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        root_tree_id,
        &path,
        account_id,
        window_request,
    )
    .await?;
    let base = tree_text_window_for_path(
        pool,
        workspace_id,
        space_id,
        base_layer_id,
        base_tree_id,
        &path,
        account_id,
        window_request,
    )
    .await?;
    if target.is_none() && base.is_none() {
        return Err(ApiError::not_found("step path content not found"));
    }
    let summary = match (base.is_some(), target.is_some()) {
        (false, true) => "Windowed step diff preview; file added in this step",
        (true, false) => "Windowed step diff preview; file deleted in this step",
        (true, true) => "Windowed step diff preview",
        (false, false) => "Windowed step diff preview; file content is not available",
    };

    Ok(lens_runtime_diff_value(LensRuntimeDiffRender {
        workspace_id,
        space_id,
        layer_id,
        artifact_id: None,
        step_id: Some(step_id),
        base_layer_id: Some(base_layer_id),
        path: &path,
        target: target.as_ref(),
        base: base.as_ref(),
        window_request,
        summary,
        mode: "stepWindow",
        limitation: Some(
            "Windowed step comparison is line-aligned inside the requested line and column window.",
        ),
    }))
}

async fn step_diff_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    requested_path: Option<&str>,
    changed_paths: &[Value],
    root_tree_id: Option<&str>,
    base_tree_id: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(path) = cleaned_optional_text(requested_path) {
        return validate_publish_path(&path);
    }
    if let Some(path) = changed_paths
        .iter()
        .filter_map(Value::as_str)
        .find_map(|path| cleaned_optional_text(Some(path)))
    {
        return validate_publish_path(&path);
    }
    if let Some(path) = first_tree_file_path(pool, workspace_id, space_id, root_tree_id).await? {
        return Ok(path);
    }
    if let Some(path) = first_tree_file_path(pool, workspace_id, space_id, base_tree_id).await? {
        return Ok(path);
    }
    Err(ApiError::not_found("step does not reference any file path"))
}

async fn first_tree_file_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    tree_id: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let Some(tree_id) = tree_id else {
        return Ok(None);
    };
    sqlx::query_scalar(
        r#"
        SELECT te.logical_path
        FROM tree_entries te
        JOIN space_tree_objects sto ON sto.tree_id = te.tree_id
        WHERE te.tree_id = $1
          AND sto.workspace_id = $2
          AND sto.space_id = $3
          AND te.entry_kind = 'file'
        ORDER BY te.logical_path ASC
        LIMIT 1
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)
}

async fn tree_text_window_for_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    tree_id: Option<&str>,
    path: &str,
    account_id: &str,
    window_request: WindowRequest,
) -> Result<Option<ArtifactTextWindow>, ApiError> {
    let Some(tree_id) = tree_id else {
        return Ok(None);
    };
    let Some(file) =
        tree_file_ref_for_path(pool, workspace_id, space_id, Some(tree_id), path).await?
    else {
        return Ok(None);
    };
    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        path,
        None,
        account_id,
    )
    .await?;
    if !decision.can_read {
        return Err(ApiError::forbidden(
            "step path is redacted by layer access policy",
        ));
    }
    let mut window = file_object_text_window(
        pool,
        workspace_id,
        space_id,
        path,
        "file".to_string(),
        &file.file_object_id,
        window_request,
    )
    .await?;
    window.source = json!({
        "kind": "tree_entry",
        "treeId": tree_id,
        "path": path,
        "fileObject": window.source
    });
    Ok(Some(window))
}

async fn file_object_bytes(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<Vec<u8>, ApiError> {
    let chunk_rows = sqlx::query(
        r#"
        SELECT oc.content_bytes, oc.compression
        FROM file_object_chunks foc
        JOIN object_chunks oc ON oc.chunk_id = foc.chunk_id
        JOIN space_object_chunks soc ON soc.chunk_id = oc.chunk_id
        WHERE foc.file_object_id = $1
          AND soc.workspace_id = $2
          AND soc.space_id = $3
          AND oc.state = 'available'
        ORDER BY foc.chunk_index ASC
        "#,
    )
    .bind(file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?;
    let mut bytes = Vec::new();
    for row in chunk_rows {
        let chunk_bytes = row
            .try_get::<Vec<u8>, _>("content_bytes")
            .ok()
            .ok_or_else(|| ApiError::not_found("chunk bytes are not available"))?;
        let compression = row
            .try_get::<String, _>("compression")
            .ok()
            .unwrap_or_else(|| CHUNK_COMPRESSION_IDENTITY.to_string());
        bytes.extend(decode_chunk_bytes(&chunk_bytes, &compression)?);
    }
    Ok(bytes)
}

async fn chunk_values_for_file_object(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT foc.chunk_index, foc.byte_offset, foc.size_bytes,
               oc.chunk_id, oc.digest, oc.object_key, oc.compression, oc.stored_size_bytes
        FROM file_object_chunks foc
        JOIN object_chunks oc ON oc.chunk_id = foc.chunk_id
        JOIN space_object_chunks soc ON soc.chunk_id = oc.chunk_id
        WHERE foc.file_object_id = $1
          AND soc.workspace_id = $2
          AND soc.space_id = $3
          AND oc.state = 'available'
        ORDER BY foc.chunk_index ASC
        "#,
    )
    .bind(file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "chunkId": row.get::<String, _>("chunk_id"),
                "digest": row.get::<String, _>("digest"),
                "objectKey": row.get::<String, _>("object_key"),
                "index": row.get::<i32, _>("chunk_index"),
                "byteOffset": row.get::<i64, _>("byte_offset"),
                "size": row.get::<i64, _>("size_bytes"),
                "sizeBytes": row.get::<i64, _>("size_bytes"),
                "rawSize": row.get::<i64, _>("size_bytes"),
                "storedSize": row.try_get::<i64, _>("stored_size_bytes").ok(),
                "compression": row.try_get::<String, _>("compression").ok().unwrap_or_else(|| CHUNK_COMPRESSION_IDENTITY.to_string()),
                "downloadUrl": format!("/v1/workspaces/{workspace_id}/spaces/{space_id}/chunks/{}", row.get::<String, _>("chunk_id"))
            })
        })
        .collect())
}

async fn layer_head_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    requested_layer_id: Option<&str>,
) -> Result<Value, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT layer_id, layer_state_id, root_tree_id, policy_epoch, server_cursor,
               to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM layer_heads
        WHERE workspace_id = $1
          AND space_id = $2
          AND ($3::text IS NULL OR layer_id = $3)
        ORDER BY updated_at DESC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(requested_layer_id)
    .fetch_all(pool)
    .await?;
    if let Some(layer_id) = requested_layer_id {
        if let Some(row) = rows.first() {
            return Ok(json!({
                "layerId": row.get::<String, _>("layer_id"),
                "layerStateId": row.try_get::<String, _>("layer_state_id").ok(),
                "rootTreeId": row.try_get::<String, _>("root_tree_id").ok(),
                "policyEpoch": row.get::<i64, _>("policy_epoch"),
                "serverCursor": row.try_get::<String, _>("server_cursor").ok(),
                "updatedAt": row.get::<String, _>("updated_at")
            }));
        }
        let policy_epoch = current_policy_epoch(pool, workspace_id, space_id, layer_id)
            .await
            .unwrap_or(1);
        return Ok(json!({
            "layerId": layer_id,
            "rootTreeId": Value::Null,
            "policyEpoch": policy_epoch,
            "serverCursor": Value::Null
        }));
    }
    Ok(json!(
        rows.iter()
            .map(|row| json!({
                "layerId": row.get::<String, _>("layer_id"),
                "layerStateId": row.try_get::<String, _>("layer_state_id").ok(),
                "rootTreeId": row.try_get::<String, _>("root_tree_id").ok(),
                "policyEpoch": row.get::<i64, _>("policy_epoch"),
                "serverCursor": row.try_get::<String, _>("server_cursor").ok(),
                "updatedAt": row.get::<String, _>("updated_at")
            }))
            .collect::<Vec<_>>()
    ))
}

#[derive(Debug)]
struct AccessDecision {
    can_read: bool,
    can_write: bool,
    reason: String,
}

#[derive(Debug)]
struct AccessRuleRow {
    path: String,
    artifact_id: Option<String>,
    mode: String,
    read_accounts: HashSet<String>,
    read_teams: HashSet<String>,
    write_accounts: HashSet<String>,
    write_teams: HashSet<String>,
    admin_accounts: HashSet<String>,
    admin_teams: HashSet<String>,
}

async fn access_decision_for_path(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    path: &str,
    artifact_id: Option<&str>,
    account_id: &str,
) -> Result<AccessDecision, ApiError> {
    let workspace_role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships WHERE workspace_id = $1 AND account_id = $2 AND state = 'active'",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    let Some(workspace_role) = workspace_role else {
        return Ok(AccessDecision {
            can_read: false,
            can_write: false,
            reason: "Workspace membership is required".to_string(),
        });
    };
    let team_ids = account_team_ids(pool, workspace_id, account_id).await?;
    let rules = access_rule_rows_for_layer(pool, workspace_id, space_id, layer_id).await?;
    let mut best: Option<AccessRuleRow> = None;
    for rule in rules {
        let artifact_matches = match (artifact_id, rule.artifact_id.as_deref()) {
            (Some(artifact_id), Some(rule_artifact_id)) => artifact_id == rule_artifact_id,
            _ => false,
        };
        if artifact_matches || path_matches_rule(path, &rule.path) {
            let replace = best
                .as_ref()
                .map(|current| rule.path.len() > current.path.len())
                .unwrap_or(true);
            if replace {
                best = Some(rule);
            }
        }
    }

    let workspace_can_write = matches!(workspace_role.as_str(), "owner" | "admin" | "member");
    let Some(rule) = best else {
        return Ok(AccessDecision {
            can_read: true,
            can_write: workspace_can_write,
            reason: "No restrictive Layer access rule matched".to_string(),
        });
    };
    if rule.mode == "reserved_redacted" {
        return Ok(AccessDecision {
            can_read: false,
            can_write: false,
            reason: "Path is reserved redacted by Layer access policy".to_string(),
        });
    }

    let account_read = rule.read_accounts.contains(account_id)
        || rule.write_accounts.contains(account_id)
        || rule.admin_accounts.contains(account_id);
    let team_read = intersects(&team_ids, &rule.read_teams)
        || intersects(&team_ids, &rule.write_teams)
        || intersects(&team_ids, &rule.admin_teams);
    let account_write =
        rule.write_accounts.contains(account_id) || rule.admin_accounts.contains(account_id);
    let team_write =
        intersects(&team_ids, &rule.write_teams) || intersects(&team_ids, &rule.admin_teams);

    Ok(AccessDecision {
        can_read: account_read || team_read,
        can_write: workspace_can_write && (account_write || team_write),
        reason: "Restricted by Layer access policy".to_string(),
    })
}

async fn access_rule_rows_for_layer(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Vec<AccessRuleRow>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT r.path, r.artifact_id, r.mode,
               r.read_account_ids, r.read_team_ids, r.write_account_ids, r.write_team_ids,
               r.admin_account_ids, r.admin_team_ids
        FROM layer_access_policy_rules r
        JOIN layer_access_policies p ON p.policy_id = r.policy_id
        WHERE p.workspace_id = $1 AND p.space_id = $2 AND p.layer_id = $3
        ORDER BY length(r.path) DESC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| AccessRuleRow {
            path: row.get("path"),
            artifact_id: row.try_get("artifact_id").ok(),
            mode: row.get("mode"),
            read_accounts: hashset(
                row.try_get::<Vec<String>, _>("read_account_ids")
                    .unwrap_or_default(),
            ),
            read_teams: hashset(
                row.try_get::<Vec<String>, _>("read_team_ids")
                    .unwrap_or_default(),
            ),
            write_accounts: hashset(
                row.try_get::<Vec<String>, _>("write_account_ids")
                    .unwrap_or_default(),
            ),
            write_teams: hashset(
                row.try_get::<Vec<String>, _>("write_team_ids")
                    .unwrap_or_default(),
            ),
            admin_accounts: hashset(
                row.try_get::<Vec<String>, _>("admin_account_ids")
                    .unwrap_or_default(),
            ),
            admin_teams: hashset(
                row.try_get::<Vec<String>, _>("admin_team_ids")
                    .unwrap_or_default(),
            ),
        })
        .collect())
}

async fn account_team_ids(
    pool: &PgPool,
    workspace_id: &str,
    account_id: &str,
) -> Result<HashSet<String>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT tm.team_id
        FROM team_memberships tm
        JOIN teams t ON t.team_id = tm.team_id
        WHERE t.workspace_id = $1 AND tm.account_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| row.get::<String, _>("team_id"))
        .collect())
}

