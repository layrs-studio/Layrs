use layrs_lens_sdk::{
    LensBlockResolutionInput, LensConflictSegmentKind, LensFileResolutionInput,
    LensReconcileContent, LensReconcileInput, LensReconcileResult, LensReconcileResultStatus,
    LensReconcileSide, LensResolutionSegment, ResolutionMethod, ResolutionMethods,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWeaveRequestBody {
    source_layer_id: String,
    target_layer_id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveWeaveConflictBody {
    method: String,
    #[serde(default, alias = "block_id")]
    block_id: Option<String>,
    #[serde(default, alias = "manual_text")]
    manual_text: Option<String>,
}

#[derive(Clone, Debug)]
struct WeaveConflictRow {
    conflict_id: String,
    logical_path: String,
    lens_id: String,
    status: String,
    message: String,
    base_file_object_id: Option<String>,
    ours_file_object_id: Option<String>,
    theirs_file_object_id: Option<String>,
    resolved_file_object_id: Option<String>,
    source_layer_id: String,
    target_layer_id: String,
}

#[derive(Clone, Debug)]
struct WeaveConflictSide {
    exists: bool,
    digest: Option<String>,
    media_type: Option<String>,
    bytes: Vec<u8>,
}

async fn weave_request_layer_ids(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
) -> Result<(String, String), ApiError> {
    sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT source_layer_id, target_layer_id
        FROM weave_requests
        WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(weave_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("weave request not found"))
}

async fn weave_request_access_paths(
    pool: &PgPool,
    weave_id: &str,
) -> Result<Vec<String>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT changed_paths, path_replays
        FROM weave_step_replays
        WHERE weave_id = $1
        ORDER BY order_index ASC
        "#,
    )
    .bind(weave_id)
    .fetch_all(pool)
    .await?;
    let mut paths = BTreeMap::new();
    for row in rows {
        for path in row.get::<Vec<String>, _>("changed_paths") {
            paths.insert(validate_publish_path(&path)?, ());
        }
        let path_replays = row.get::<Value, _>("path_replays");
        let path_replays = path_replays
            .as_array()
            .ok_or_else(|| ApiError::internal("Weave replay path plan is invalid"))?;
        for path_replay in path_replays {
            let path = path_replay_path(path_replay)?;
            paths.insert(validate_publish_path(&path)?, ());
        }
    }

    let conflict_paths = sqlx::query_scalar::<_, String>(
        "SELECT logical_path FROM weave_conflicts WHERE weave_id = $1 ORDER BY logical_path",
    )
    .bind(weave_id)
    .fetch_all(pool)
    .await?;
    for path in conflict_paths {
        paths.insert(validate_publish_path(&path)?, ());
    }
    Ok(paths.into_keys().collect())
}

async fn ensure_weave_layers_read(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
    source_layer_id: &str,
    target_layer_id: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    ensure_layer_read(pool, workspace_id, space_id, source_layer_id, account_id).await?;
    ensure_layer_read(pool, workspace_id, space_id, target_layer_id, account_id).await?;
    let paths = weave_request_access_paths(pool, weave_id).await?;
    let layer_ids = vec![source_layer_id.to_string(), target_layer_id.to_string()];
    let artifacts =
        weave_artifact_ids_for_paths(pool, workspace_id, space_id, &layer_ids, &paths).await?;
    for path in paths {
        let source = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            source_layer_id,
            &path,
            artifacts.get(source_layer_id, &path),
            account_id,
        )
        .await?;
        let target = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            target_layer_id,
            &path,
            artifacts.get(target_layer_id, &path),
            account_id,
        )
        .await?;
        if !source.can_read || !target.can_read {
            return Err(ApiError::forbidden(
                "Weave request is restricted by Layer access policy",
            ));
        }
    }
    Ok(())
}

async fn ensure_weave_request_read(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
    account_id: &str,
) -> Result<(String, String), ApiError> {
    let (source_layer_id, target_layer_id) =
        weave_request_layer_ids(pool, workspace_id, space_id, weave_id).await?;
    ensure_weave_layers_read(
        pool,
        workspace_id,
        space_id,
        weave_id,
        &source_layer_id,
        &target_layer_id,
        account_id,
    )
    .await?;
    Ok((source_layer_id, target_layer_id))
}

async fn list_weave_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT weave_id, source_layer_id, target_layer_id, title, body, status,
               pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
               applied_steps, created_at::text AS created_at, updated_at::text AS updated_at
        FROM weave_requests
        WHERE workspace_id = $1 AND space_id = $2
        ORDER BY updated_at DESC, weave_id ASC
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .fetch_all(&state.pool)
    .await?;

    let mut visible = Vec::new();
    for row in rows {
        let weave_id = row.get::<String, _>("weave_id");
        let source_layer_id = row.get::<String, _>("source_layer_id");
        let target_layer_id = row.get::<String, _>("target_layer_id");
        match ensure_weave_layers_read(
            &state.pool,
            &workspace_id,
            &space_id,
            &weave_id,
            &source_layer_id,
            &target_layer_id,
            &user.id,
        )
        .await
        {
            Ok(()) => visible.push(weave_request_json(&row)),
            Err(error) if error.status == StatusCode::FORBIDDEN => {}
            Err(error) => return Err(error),
        }
    }

    Ok(Json(json!(visible)))
}

async fn create_weave_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id)): Path<(String, String)>,
    Json(body): Json<CreateWeaveRequestBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    if body.source_layer_id == body.target_layer_id {
        return Err(ApiError::bad_request(
            "source and target Layers must be different",
        ));
    }

    let weave_id = prefixed_id("weave");
    let title = body
        .title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "Weave request".to_string());
    let mut tx = state.pool.begin().await?;
    let layer_ids = vec![body.source_layer_id.clone(), body.target_layer_id.clone()];
    let snapshot = load_weave_authorization_snapshot_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        &layer_ids,
        &user.id,
    )
    .await?;
    if snapshot.layer(&body.target_layer_id)?.repository_role == RepositoryRole::Reader {
        return Err(ApiError::forbidden("Layer write permission is required"));
    }
    let mut source_steps = source_step_replays_missing_from_target_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        &body.source_layer_id,
        &body.target_layer_id,
    )
    .await?;
    let source_step_snapshot = source_steps.clone();
    let mut paths = Vec::new();
    for step in &mut source_steps {
        step.changed_paths = actual_step_changed_paths_in_tx(
            &mut tx,
            &workspace_id,
            &space_id,
            step.base_tree_id.as_deref(),
            step.root_tree_id.as_deref(),
            &step.changed_paths,
        )
        .await?;
        paths.extend(step.changed_paths.iter().cloned());
    }
    let paths = sorted_unique_strings(&paths);
    let artifacts =
        weave_artifact_ids_for_paths_in_tx(&mut tx, &workspace_id, &space_id, &layer_ids, &paths)
            .await?;
    for path in &paths {
        let (source, target) = weave_path_access_decisions(
            &snapshot,
            &artifacts,
            &body.source_layer_id,
            &body.target_layer_id,
            path,
        )?;
        if !source.can_read {
            return Err(ApiError::forbidden(
                "Weave source path is restricted by Layer access policy",
            ));
        }
        if !target.can_write {
            return Err(ApiError::forbidden(
                "Weave target path is not writable under Layer access policy",
            ));
        }
    }
    let generations =
        lock_weave_layer_generations_in_tx(&mut tx, &workspace_id, &space_id, &layer_ids).await?;
    let locked_source_steps = source_step_replays_missing_from_target_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        &body.source_layer_id,
        &body.target_layer_id,
    )
    .await?;
    if locked_source_steps != source_step_snapshot {
        return Err(ApiError::conflict(
            "Layer history changed during Weave preparation; prepare a new Weave plan",
        ));
    }
    let source_generation = generations
        .get(&body.source_layer_id)
        .ok_or_else(|| ApiError::internal("source Layer generation snapshot is missing"))?;
    let target_generation = generations
        .get(&body.target_layer_id)
        .ok_or_else(|| ApiError::internal("target Layer generation snapshot is missing"))?;
    let pre_weave_source_tree_id = source_generation.root_tree_id.clone();
    let pre_weave_source_step_id = source_generation.latest_step_id.clone();
    let pre_weave_target_tree_id = target_generation.root_tree_id.clone();
    let pre_weave_target_step_id = target_generation.latest_step_id.clone();
    let source_policy_epoch = snapshot.policy_epoch(&body.source_layer_id)?;
    let target_policy_epoch = snapshot.policy_epoch(&body.target_layer_id)?;

    sqlx::query(
        r#"
        INSERT INTO weave_requests
            (weave_id, workspace_id, space_id, source_layer_id, target_layer_id, title, body,
             status, pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
             requested_by_account_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'open', $8, $9, '{}', $10)
        "#,
    )
    .bind(&weave_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&body.source_layer_id)
    .bind(&body.target_layer_id)
    .bind(title)
    .bind(body.body.unwrap_or_default())
    .bind(pre_weave_target_tree_id.as_deref())
    .bind(pre_weave_target_step_id.as_deref())
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;

    let (planned_steps, conflict_count) = create_weave_step_replay_plan_in_tx(
        &mut tx,
        &weave_id,
        &workspace_id,
        &space_id,
        &body.source_layer_id,
        &body.target_layer_id,
        pre_weave_target_tree_id.as_deref(),
        &user.id,
        &source_steps,
    )
    .await?;
    let status = if conflict_count > 0 {
        "conflicted"
    } else if planned_steps.is_empty() {
        "resolved"
    } else {
        "open"
    };
    let session_status = match status {
        "conflicted" => "conflicted",
        "resolved" => "resolved",
        _ => "preview",
    };

    let row = sqlx::query(
        r#"
        UPDATE weave_requests
        SET status = $4, planned_steps = $5, updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3
        RETURNING weave_id, source_layer_id, target_layer_id, title, body, status,
                  pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
                  applied_steps, created_at::text AS created_at, updated_at::text AS updated_at
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&weave_id)
    .bind(status)
    .bind(&planned_steps)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO weave_sessions (weave_id, status, session_payload)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(&weave_id)
    .bind(session_status)
    .bind(json!({
        "plannedSteps": planned_steps,
        "replayCount": planned_steps.len(),
        "conflictCount": conflict_count,
        "sourcePolicyEpoch": source_policy_epoch,
        "sourceHeadTreeId": pre_weave_source_tree_id,
        "sourceHeadStepId": pre_weave_source_step_id,
        "targetPolicyEpoch": target_policy_epoch
    }))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(weave_request_json(&row)))
}

async fn get_weave_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id, weave_id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_weave_request_read(&state.pool, &workspace_id, &space_id, &weave_id, &user.id).await?;
    Ok(Json(
        weave_request_detail_value(&state.pool, &workspace_id, &space_id, &weave_id).await?,
    ))
}

async fn resolve_weave_conflict(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id, weave_id, conflict_id)): Path<(String, String, String, String)>,
    Json(body): Json<ResolveWeaveConflictBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    let method = ResolutionMethod::from_label(&body.method)
        .ok_or_else(|| ApiError::bad_request("unsupported Weave resolution method"))?;

    let mut tx = state.pool.begin().await?;
    let request =
        load_weave_request_row_in_tx(&mut tx, &workspace_id, &space_id, &weave_id).await?;
    let source_layer_id = request.get::<String, _>("source_layer_id");
    let target_layer_id = request.get::<String, _>("target_layer_id");
    let logical_path = sqlx::query_scalar::<_, String>(
        "SELECT logical_path FROM weave_conflicts WHERE weave_id = $1 AND conflict_id = $2",
    )
    .bind(&weave_id)
    .bind(&conflict_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Weave conflict not found"))?;
    lock_and_revalidate_weave_paths_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        WeavePathAccessCheck {
            source_layer_id: &source_layer_id,
            target_layer_id: &target_layer_id,
            account_id: &user.id,
            paths: std::slice::from_ref(&logical_path),
            source_denied_message: "Weave source path is restricted by Layer access policy",
            target_denied_message: "Weave target path is not writable under Layer access policy",
        },
    )
    .await?;
    let request =
        load_weave_request_row_for_update_in_tx(&mut tx, &workspace_id, &space_id, &weave_id)
            .await?;
    let request_status = request.get::<String, _>("status");
    if !matches!(request_status.as_str(), "open" | "preview" | "conflicted") {
        return Err(ApiError::bad_request(format!(
            "Weave request cannot be resolved from {request_status}"
        )));
    }
    let row =
        load_weave_conflict_row_in_tx(&mut tx, &workspace_id, &space_id, &weave_id, &conflict_id)
            .await?;
    if row.status == "resolved" {
        return Err(ApiError::bad_request("Weave conflict is already resolved"));
    }
    let mut payload = load_weave_session_payload_for_update_in_tx(&mut tx, &weave_id).await?;
    let base = load_weave_conflict_side_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        row.base_file_object_id.as_deref(),
    )
    .await?;
    let ours = load_weave_conflict_side_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        row.ours_file_object_id.as_deref(),
    )
    .await?;
    let theirs = load_weave_conflict_side_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        row.theirs_file_object_id.as_deref(),
    )
    .await?;
    let reconcile = reconcile_weave_conflict(&row, &base, &ours, &theirs);

    if let Some(block_id) = cleaned_optional_text(body.block_id.as_deref()) {
        resolve_weave_text_block_in_tx(
            &mut tx,
            &row,
            &reconcile,
            &mut payload,
            &block_id,
            method,
            body.manual_text.as_deref(),
            &workspace_id,
            &space_id,
            &user.id,
            &base,
            &ours,
            &theirs,
        )
        .await?;
    } else {
        resolve_weave_file_in_tx(
            &mut tx,
            &row,
            method,
            &mut payload,
            &workspace_id,
            &space_id,
            &user.id,
            &base,
            &ours,
            &theirs,
        )
        .await?;
    }

    update_weave_resolution_status_in_tx(&mut tx, &weave_id).await?;
    tx.commit().await?;

    Ok(Json(
        weave_request_detail_value(&state.pool, &workspace_id, &space_id, &weave_id).await?,
    ))
}

async fn abort_weave_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id, weave_id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let existing =
        load_weave_request_row_in_tx(&mut tx, &workspace_id, &space_id, &weave_id).await?;
    let target_layer_id = existing.get::<String, _>("target_layer_id");
    let requested_by_account_id = existing
        .try_get::<Option<String>, _>("requested_by_account_id")
        .unwrap_or(None);
    lock_weave_layers_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        std::slice::from_ref(&target_layer_id),
    )
    .await?;
    ensure_weave_abort_actor_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        &target_layer_id,
        requested_by_account_id.as_deref(),
        &user.id,
    )
    .await?;
    let existing =
        load_weave_request_row_for_update_in_tx(&mut tx, &workspace_id, &space_id, &weave_id)
            .await?;
    let status = existing.get::<String, _>("status");
    let row = sqlx::query(
        r#"
        UPDATE weave_requests
        SET status = 'aborted', updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3
          AND status IN ('open', 'preview', 'conflicted', 'resolved')
        RETURNING weave_id, source_layer_id, target_layer_id, title, body, status,
                  pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
                  applied_steps, created_at::text AS created_at, updated_at::text AS updated_at
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&weave_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::bad_request(format!(
            "Weave request cannot be aborted from {status}"
        )));
    };
    sqlx::query(
        "UPDATE weave_sessions SET status = 'aborted', updated_at = now() WHERE weave_id = $1 AND status IN ('preview', 'conflicted', 'resolved')",
    )
    .bind(&weave_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(weave_request_json(&row)))
}

async fn apply_weave_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id, weave_id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    let mut tx = state.pool.begin().await?;
    let existing = load_weave_request_row_in_tx(&mut tx, &workspace_id, &space_id, &weave_id)
        .await
        .map_err(|error| normalize_weave_apply_access_error(error, false))?;
    let source_layer_id = existing.get::<String, _>("source_layer_id");
    let target_layer_id = existing.get::<String, _>("target_layer_id");
    let preserve_access_detail = existing
        .try_get::<Option<String>, _>("requested_by_account_id")
        .unwrap_or(None)
        .as_deref()
        == Some(&user.id);
    let paths = weave_replay_paths_in_tx(&mut tx, &weave_id).await?;
    let access = lock_and_revalidate_weave_paths_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        WeavePathAccessCheck {
            source_layer_id: &source_layer_id,
            target_layer_id: &target_layer_id,
            account_id: &user.id,
            paths: &paths,
            source_denied_message:
                "Weave source path is no longer readable under Layer access policy",
            target_denied_message:
                "Weave target path is no longer writable under Layer access policy",
        },
    )
    .await
    .map_err(|error| normalize_weave_apply_access_error(error, preserve_access_detail))?;
    let layer_ids = vec![source_layer_id.clone(), target_layer_id.clone()];
    let generations =
        lock_weave_layer_generations_in_tx(&mut tx, &workspace_id, &space_id, &layer_ids)
            .await
            .map_err(|error| normalize_weave_apply_access_error(error, preserve_access_detail))?;

    let existing =
        load_weave_request_row_for_update_in_tx(&mut tx, &workspace_id, &space_id, &weave_id)
            .await
            .map_err(|error| normalize_weave_apply_access_error(error, preserve_access_detail))?;
    let status = existing.get::<String, _>("status");
    if matches!(status.as_str(), "applied" | "aborted" | "closed") {
        return Err(ApiError::bad_request(format!(
            "Weave request is already {status}"
        )));
    }
    let planned_steps = existing.get::<Vec<String>, _>("planned_steps");
    let prepared_target_tree_id = existing
        .try_get::<Option<String>, _>("pre_weave_target_tree_id")
        .unwrap_or(None);
    let prepared_target_step_id = existing
        .try_get::<Option<String>, _>("pre_weave_target_step_id")
        .unwrap_or(None);
    let mut payload = load_weave_session_payload_for_update_in_tx(&mut tx, &weave_id).await?;
    let expected_source_policy_epoch = payload
        .get("sourcePolicyEpoch")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            ApiError::conflict(
                "Weave plan predates source policy epoch capture; prepare a new Weave plan",
            )
        })?;
    let expected_target_policy_epoch = payload
        .get("targetPolicyEpoch")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            ApiError::conflict(
                "Weave plan predates target policy epoch capture; prepare a new Weave plan",
            )
        })?;
    let prepared_source_tree_id = weave_session_optional_string(&payload, "sourceHeadTreeId")?;
    let prepared_source_step_id = weave_session_optional_string(&payload, "sourceHeadStepId")?;
    let locked_paths = weave_replay_paths_for_update_in_tx(&mut tx, &weave_id).await?;
    if locked_paths != paths {
        return Err(ApiError::conflict(
            "Weave replay plan changed during Apply; prepare a new Weave plan",
        ));
    }
    let unresolved: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM weave_conflicts WHERE weave_id = $1 AND status <> 'resolved'",
    )
    .bind(&weave_id)
    .fetch_one(&mut *tx)
    .await?;
    if unresolved > 0 {
        return Err(ApiError::bad_request(
            "resolve all Weave conflicts before applying",
        ));
    }

    if access.source_policy_epoch != expected_source_policy_epoch {
        return Err(ApiError::conflict(
            "source Layer access policy changed after Weave preparation; prepare a new Weave plan",
        ));
    }
    if access.target_policy_epoch != expected_target_policy_epoch {
        return Err(ApiError::conflict(
            "target Layer access policy changed after Weave preparation; prepare a new Weave plan",
        ));
    }

    let source_generation = generations
        .get(&source_layer_id)
        .ok_or_else(|| ApiError::internal("source Layer generation snapshot is missing"))?;
    if source_generation.root_tree_id != prepared_source_tree_id
        || source_generation.latest_step_id != prepared_source_step_id
    {
        return Err(ApiError::conflict(
            "source Layer changed after Weave preparation; prepare a new Weave plan",
        ));
    }
    let target_generation = generations
        .get(&target_layer_id)
        .ok_or_else(|| ApiError::internal("target Layer generation snapshot is missing"))?;
    if target_generation.root_tree_id != prepared_target_tree_id
        || target_generation.latest_step_id != prepared_target_step_id
    {
        return Err(ApiError::conflict(
            "target Layer changed after Weave preparation; prepare a new Weave plan",
        ));
    }
    let applying = sqlx::query(
        "UPDATE weave_requests SET status = 'applying', updated_at = now() WHERE weave_id = $1 AND status IN ('open', 'preview', 'conflicted', 'resolved')",
    )
    .bind(&weave_id)
    .execute(&mut *tx)
    .await?;
    if applying.rows_affected() != 1 {
        return Err(ApiError::conflict(
            "Weave request is no longer open for Apply",
        ));
    }
    sqlx::query(
        "UPDATE weave_sessions SET status = 'applying', updated_at = now() WHERE weave_id = $1 AND status IN ('preview', 'conflicted', 'resolved')",
    )
    .bind(&weave_id)
    .execute(&mut *tx)
    .await?;

    let applied_steps = apply_weave_step_replays_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        &weave_id,
        &source_layer_id,
        &target_layer_id,
        access.target_policy_epoch,
        &user.id,
    )
    .await?;
    if !planned_steps.is_empty() && applied_steps.is_empty() {
        return Err(ApiError::bad_request(
            "Weave request has planned Steps but no replay plan to apply",
        ));
    }
    let row = sqlx::query(
        r#"
        UPDATE weave_requests
        SET status = 'applied', applied_steps = $4, updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3 AND status = 'applying'
        RETURNING weave_id, source_layer_id, target_layer_id, title, body, status,
                  pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
                  applied_steps, created_at::text AS created_at, updated_at::text AS updated_at
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&weave_id)
    .bind(&applied_steps)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("weave request not found"))?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("appliedSteps".to_string(), json!(applied_steps));
    } else {
        payload = json!({ "appliedSteps": applied_steps });
    }
    sqlx::query(
        "UPDATE weave_sessions SET status = 'applied', session_payload = $2, updated_at = now() WHERE weave_id = $1 AND status = 'applying'",
    )
    .bind(&weave_id)
    .bind(&payload)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(weave_request_json(&row)))
}

include!("weave_access_snapshot.rs");

include!("weave_transaction_queries.rs");

include!("weave_replay_plan.rs");

include!("weave_apply_authz.rs");

include!("weave_conflict_resolution.rs");

fn weave_request_json(row: &sqlx::postgres::PgRow) -> Value {
    json!({
        "weaveId": row.get::<String, _>("weave_id"),
        "id": row.get::<String, _>("weave_id"),
        "sourceLayerId": row.get::<String, _>("source_layer_id"),
        "targetLayerId": row.get::<String, _>("target_layer_id"),
        "title": row.get::<String, _>("title"),
        "body": row.get::<String, _>("body"),
        "status": row.get::<String, _>("status"),
        "preWeaveTargetTreeId": row.try_get::<String, _>("pre_weave_target_tree_id").ok(),
        "preWeaveTargetStepId": row.try_get::<String, _>("pre_weave_target_step_id").ok(),
        "plannedSteps": row.get::<Vec<String>, _>("planned_steps"),
        "appliedSteps": row.get::<Vec<String>, _>("applied_steps"),
        "createdAt": row.get::<String, _>("created_at"),
        "updatedAt": row.get::<String, _>("updated_at")
    })
}
