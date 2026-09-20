async fn rebuild_layer_tree_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
) -> Result<Option<String>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT a.logical_path, a.current_file_object_id, f.size_bytes
        FROM artifacts a
        JOIN file_objects f ON f.file_object_id = a.current_file_object_id
        WHERE a.workspace_id = $1
          AND a.space_id = $2
          AND a.layer_id = $3
          AND a.state = 'active'
          AND a.current_file_object_id IS NOT NULL
        ORDER BY a.logical_path ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_all(&mut **tx)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let entries = rows
        .into_iter()
        .map(|row| ImmutableTreeEntry {
            path: row.get("logical_path"),
            file_object_id: row.get("current_file_object_id"),
            size_bytes: row.get("size_bytes"),
        })
        .collect();
    let tree_id = insert_or_compare_tree_in_tx(
        tx,
        workspace_id,
        space_id,
        account_id,
        None,
        entries,
    )
    .await?;
    Ok(Some(tree_id))
}

async fn ensure_tree_in_space_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    tree_id: &str,
) -> Result<(), ApiError> {
    let tree_id = validate_object_digest(tree_id)?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM tree_objects t
            JOIN space_tree_objects sto ON sto.tree_id = t.tree_id
            WHERE sto.workspace_id = $1
              AND sto.space_id = $2
              AND t.tree_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(&tree_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::bad_request("rootTreeId is not available"))
    }
}

#[expect(clippy::too_many_arguments, reason = "Keep explicit transaction and scope arguments in the retained legacy Web API")]
async fn insert_layer_step_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    step: Option<&SyncStepBody>,
    requested_base_tree_id: Option<&str>,
    root_tree_id: Option<&str>,
    changed_paths: &[String],
    source_client_id: Option<&str>,
    sync_batch_id: Option<&str>,
    account_id: &str,
) -> Result<String, ApiError> {
    let step_id = step
        .and_then(|step| cleaned_optional_text(step.step_id.as_deref()))
        .unwrap_or_else(|| prefixed_id("step"));
    let parent_step_id =
        match step.and_then(|step| cleaned_optional_text(step.parent_step_id.as_deref())) {
            Some(candidate) => {
                let exists: bool = sqlx::query_scalar(
                    r#"
                SELECT EXISTS(
                    SELECT 1 FROM layer_steps
                    WHERE workspace_id = $1
                      AND space_id = $2
                      AND layer_id = $3
                      AND step_id = $4
                )
                "#,
                )
                .bind(workspace_id)
                .bind(space_id)
                .bind(layer_id)
                .bind(&candidate)
                .fetch_one(&mut **tx)
                .await?;
                exists.then_some(candidate)
            }
            None => None,
        };
    let base_layer_id =
        match step.and_then(|step| cleaned_optional_text(step.base_layer_id.as_deref())) {
            Some(candidate) => {
                let exists: bool = sqlx::query_scalar(
                    r#"
                SELECT EXISTS(
                    SELECT 1 FROM layers
                    WHERE workspace_id = $1
                      AND space_id = $2
                      AND layer_id = $3
                )
                "#,
                )
                .bind(workspace_id)
                .bind(space_id)
                .bind(&candidate)
                .fetch_one(&mut **tx)
                .await?;
                if exists {
                    Some(candidate)
                } else {
                    Some(layer_id.to_string())
                }
            }
            None => Some(layer_id.to_string()),
        };
    let step_root_tree_id =
        step.and_then(|step| cleaned_optional_text(step.root_tree_id.as_deref()));
    if let Some(step_root_tree_id) = step_root_tree_id.as_deref() {
        validate_object_digest(step_root_tree_id)?;
    }
    let stored_root_tree_id = optional_existing_tree_id_in_tx(
        tx,
        workspace_id,
        space_id,
        step_root_tree_id.as_deref().or(root_tree_id),
    )
    .await?;
    let step_base_tree_id = step
        .and_then(|step| cleaned_optional_text(step.base_tree_id.as_deref()))
        .or_else(|| cleaned_optional_text(requested_base_tree_id));
    let stored_base_tree_id =
        optional_existing_tree_id_in_tx(tx, workspace_id, space_id, step_base_tree_id.as_deref())
            .await?;
    let step_changed_paths = step
        .filter(|step| !step.changed_paths.is_empty())
        .map(|step| step.changed_paths.clone())
        .unwrap_or_else(|| changed_paths.to_vec());
    let timeline_position = step
        .and_then(|step| step.timeline_position)
        .filter(|value| *value >= 0);
    let origin_layer_id =
        match step.and_then(|step| cleaned_optional_text(step.origin_layer_id.as_deref())) {
            Some(candidate) => {
                let exists: bool = sqlx::query_scalar(
                    r#"
                SELECT EXISTS(
                    SELECT 1 FROM layers
                    WHERE workspace_id = $1
                      AND space_id = $2
                      AND layer_id = $3
                )
                "#,
                )
                .bind(workspace_id)
                .bind(space_id)
                .bind(&candidate)
                .fetch_one(&mut **tx)
                .await?;
                if exists {
                    Some(candidate)
                } else {
                    Some(layer_id.to_string())
                }
            }
            None => Some(layer_id.to_string()),
        };
    let origin_step_id = step
        .and_then(|step| cleaned_optional_text(step.origin_step_id.as_deref()))
        .or_else(|| Some(step_id.clone()));
    let origin_layer_name = match step.and_then(|step| cleaned_optional_text(step.origin_layer_name.as_deref())) {
        Some(name) => name,
        None => {
            let fallback_layer_id = origin_layer_id.as_deref().unwrap_or(layer_id);
            sqlx::query_scalar::<_, String>(
                r#"
                SELECT name
                FROM layers
                WHERE workspace_id = $1
                  AND space_id = $2
                  AND layer_id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(space_id)
            .bind(fallback_layer_id)
            .fetch_optional(&mut **tx)
            .await?
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| fallback_layer_id.to_string())
        }
    };
    let step_kind = step
        .and_then(|step| cleaned_optional_text(step.step_kind.as_deref()))
        .filter(|kind| matches!(kind.as_str(), "native" | "inherited" | "woven"))
        .unwrap_or_else(|| "native".to_string());
    let captured_at_unix = step
        .and_then(|step| step.captured_at_unix)
        .filter(|value| *value > 0)
        .map(|value| value as f64);

    sqlx::query(
        r#"
        INSERT INTO layer_steps
            (step_id, workspace_id, space_id, layer_id, parent_step_id,
             base_layer_id, base_tree_id, root_tree_id, changed_paths,
             source_client_id, sync_batch_id, created_by_account_id,
             timeline_position, origin_layer_id, origin_layer_name, origin_step_id, step_kind, captured_at)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, $9,
             $10, $11, $12, $13, $14, $15, $16, $17,
             COALESCE(to_timestamp($18::double precision), now()))
        ON CONFLICT (step_id) DO UPDATE SET
            parent_step_id = COALESCE(EXCLUDED.parent_step_id, layer_steps.parent_step_id),
            base_layer_id = EXCLUDED.base_layer_id,
            base_tree_id = EXCLUDED.base_tree_id,
            root_tree_id = EXCLUDED.root_tree_id,
            changed_paths = EXCLUDED.changed_paths,
            source_client_id = EXCLUDED.source_client_id,
            sync_batch_id = EXCLUDED.sync_batch_id,
            timeline_position = COALESCE(EXCLUDED.timeline_position, layer_steps.timeline_position),
            origin_layer_id = COALESCE(EXCLUDED.origin_layer_id, layer_steps.origin_layer_id),
            origin_layer_name = COALESCE(NULLIF(EXCLUDED.origin_layer_name, ''), layer_steps.origin_layer_name),
            origin_step_id = COALESCE(EXCLUDED.origin_step_id, layer_steps.origin_step_id),
            step_kind = EXCLUDED.step_kind
        "#,
    )
    .bind(&step_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(parent_step_id.as_deref())
    .bind(base_layer_id.as_deref())
    .bind(stored_base_tree_id.as_deref())
    .bind(stored_root_tree_id.as_deref())
    .bind(&step_changed_paths)
    .bind(source_client_id)
    .bind(sync_batch_id)
    .bind(account_id)
    .bind(timeline_position)
    .bind(origin_layer_id.as_deref())
    .bind(&origin_layer_name)
    .bind(origin_step_id.as_deref())
    .bind(&step_kind)
    .bind(captured_at_unix)
    .execute(&mut **tx)
    .await?;

    Ok(step_id)
}

async fn optional_existing_tree_id_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    tree_id: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let Some(tree_id) = tree_id else {
        return Ok(None);
    };
    let tree_id = validate_object_digest(tree_id)?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM tree_objects t
            JOIN space_tree_objects sto ON sto.tree_id = t.tree_id
            WHERE sto.workspace_id = $1
              AND sto.space_id = $2
              AND t.tree_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(&tree_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.then_some(tree_id))
}

fn cleaned_optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[expect(clippy::too_many_arguments, reason = "Keep explicit transaction and scope arguments in the retained legacy Web API")]
async fn advance_layer_head_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    root_tree_id: Option<&str>,
    policy_epoch: i64,
    server_cursor: Option<&str>,
    account_id: &str,
) -> Result<String, ApiError> {
    let layer_state_id = prefixed_id("layer_state");
    sqlx::query(
        r#"
        INSERT INTO layer_states
            (layer_state_id, workspace_id, space_id, layer_id, root_tree_id, policy_epoch, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(&layer_state_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(root_tree_id)
    .bind(policy_epoch)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO layer_heads
            (workspace_id, space_id, layer_id, layer_state_id, root_tree_id,
             policy_epoch, server_cursor, updated_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (workspace_id, space_id, layer_id) DO UPDATE SET
            layer_state_id = EXCLUDED.layer_state_id,
            root_tree_id = EXCLUDED.root_tree_id,
            policy_epoch = EXCLUDED.policy_epoch,
            server_cursor = EXCLUDED.server_cursor,
            updated_by_account_id = EXCLUDED.updated_by_account_id,
            updated_at = now()
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(&layer_state_id)
    .bind(root_tree_id)
    .bind(policy_epoch)
    .bind(server_cursor)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(layer_state_id)
}

#[expect(clippy::too_many_arguments, reason = "Keep explicit transaction and scope arguments in the retained legacy Web API")]
async fn insert_sync_batch_change_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    sync_batch_id: &str,
    change_index: i32,
    change_kind: &str,
    artifact_id: Option<&str>,
    logical_path: Option<&str>,
    file_object_id: Option<&str>,
    tree_id: Option<&str>,
    body: Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO sync_batch_changes
            (sync_batch_change_id, sync_batch_id, change_index, change_kind,
             artifact_id, logical_path, file_object_id, tree_id, body_json)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(prefixed_id("sync_batch_change"))
    .bind(sync_batch_id)
    .bind(change_index)
    .bind(change_kind)
    .bind(artifact_id)
    .bind(logical_path)
    .bind(file_object_id)
    .bind(tree_id)
    .bind(body)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn write_timeline_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: Option<&str>,
    layer_id: Option<&str>,
    event_kind: &str,
    title: &str,
    body: Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO timeline_events
            (event_id, workspace_id, space_id, layer_id, event_kind, title, body_json)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(prefixed_id("event"))
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(event_kind)
    .bind(title)
    .bind(body)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

