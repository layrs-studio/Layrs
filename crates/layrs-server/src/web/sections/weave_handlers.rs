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

async fn ensure_weave_request_read(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
    account_id: &str,
) -> Result<(String, String), ApiError> {
    let (source_layer_id, target_layer_id) =
        weave_request_layer_ids(pool, workspace_id, space_id, weave_id).await?;
    ensure_layer_read(pool, workspace_id, space_id, &source_layer_id, account_id).await?;
    ensure_layer_read(pool, workspace_id, space_id, &target_layer_id, account_id).await?;
    let paths = sqlx::query_scalar::<_, String>(
        "SELECT logical_path FROM weave_conflicts WHERE weave_id = $1 ORDER BY logical_path",
    )
    .bind(weave_id)
    .fetch_all(pool)
    .await?;
    for path in paths {
        let source = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            &source_layer_id,
            &path,
            None,
            account_id,
        )
        .await?;
        let target = access_decision_for_path(
            pool,
            workspace_id,
            space_id,
            &target_layer_id,
            &path,
            None,
            account_id,
        )
        .await?;
        if !source.can_read || !target.can_read {
            return Err(ApiError::forbidden(
                "Weave conflict content is restricted by Layer access policy",
            ));
        }
    }
    Ok((source_layer_id, target_layer_id))
}

async fn ensure_weave_request_write(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
    account_id: &str,
) -> Result<(String, String), ApiError> {
    let (source_layer_id, target_layer_id) =
        ensure_weave_request_read(pool, workspace_id, space_id, weave_id, account_id).await?;
    ensure_layer_write(pool, workspace_id, space_id, &target_layer_id, account_id).await?;
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

    Ok(Json(json!(rows
        .iter()
        .map(weave_request_json)
        .collect::<Vec<_>>())))
}

async fn create_weave_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id)): Path<(String, String)>,
    Json(body): Json<CreateWeaveRequestBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_layer_read(
        &state.pool,
        &workspace_id,
        &space_id,
        &body.source_layer_id,
        &user.id,
    )
    .await?;
    ensure_layer_write(
        &state.pool,
        &workspace_id,
        &space_id,
        &body.target_layer_id,
        &user.id,
    )
    .await?;
    if body.source_layer_id == body.target_layer_id {
        return Err(ApiError::bad_request("source and target Layers must be different"));
    }

    let pre_weave_target_tree_id = current_layer_head_tree_id(
        &state.pool,
        &workspace_id,
        &space_id,
        &body.target_layer_id,
    )
    .await?;
    let pre_weave_target_step_id =
        latest_layer_step_id(&state.pool, &workspace_id, &space_id, &body.target_layer_id).await?;
    let source_steps = source_step_replays_missing_from_target(
        &state.pool,
        &workspace_id,
        &space_id,
        &body.source_layer_id,
        &body.target_layer_id,
    )
    .await?;
    let weave_id = prefixed_id("weave");
    let title = body
        .title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "Weave request".to_string());
    let mut tx = state.pool.begin().await?;

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
        &state.pool,
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
        "conflictCount": conflict_count
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
    ensure_weave_request_read(
        &state.pool,
        &workspace_id,
        &space_id,
        &weave_id,
        &user.id,
    )
    .await?;
    Ok(Json(
        weave_request_detail_value(&state.pool, &workspace_id, &space_id, &weave_id).await?,
    ))
}

async fn resolve_weave_conflict(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id, weave_id, conflict_id)): Path<(
        String,
        String,
        String,
        String,
    )>,
    Json(body): Json<ResolveWeaveConflictBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_weave_request_write(
        &state.pool,
        &workspace_id,
        &space_id,
        &weave_id,
        &user.id,
    )
    .await?;
    let method = ResolutionMethod::from_label(&body.method)
        .ok_or_else(|| ApiError::bad_request("unsupported Weave resolution method"))?;

    let mut tx = state.pool.begin().await?;
    let row = load_weave_conflict_row_in_tx(
        &mut tx,
        &workspace_id,
        &space_id,
        &weave_id,
        &conflict_id,
    )
    .await?;
    if row.status == "resolved" {
        return Err(ApiError::bad_request("Weave conflict is already resolved"));
    }
    let mut payload = load_weave_session_payload_for_update_in_tx(&mut tx, &weave_id).await?;
    let base = load_weave_conflict_side(
        &state.pool,
        &workspace_id,
        &space_id,
        row.base_file_object_id.as_deref(),
    )
    .await?;
    let ours = load_weave_conflict_side(
        &state.pool,
        &workspace_id,
        &space_id,
        row.ours_file_object_id.as_deref(),
    )
    .await?;
    let theirs = load_weave_conflict_side(
        &state.pool,
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
    ensure_weave_request_write(
        &state.pool,
        &workspace_id,
        &space_id,
        &weave_id,
        &user.id,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE weave_requests
        SET status = 'aborted', updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3
        RETURNING weave_id, source_layer_id, target_layer_id, title, body, status,
                  pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
                  applied_steps, created_at::text AS created_at, updated_at::text AS updated_at
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&weave_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("weave request not found"))?;
    sqlx::query("UPDATE weave_sessions SET status = 'aborted', updated_at = now() WHERE weave_id = $1")
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
    ensure_weave_request_write(
        &state.pool,
        &workspace_id,
        &space_id,
        &weave_id,
        &user.id,
    )
    .await?;
    let mut tx = state.pool.begin().await?;
    let existing =
        load_weave_request_row_for_update_in_tx(&mut tx, &workspace_id, &space_id, &weave_id)
            .await?;
    let status = existing.get::<String, _>("status");
    if matches!(status.as_str(), "applied" | "aborted" | "closed") {
        return Err(ApiError::bad_request(format!(
            "Weave request is already {status}"
        )));
    }
    let unresolved: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM weave_conflicts WHERE weave_id = $1 AND status <> 'resolved'",
    )
    .bind(&weave_id)
    .fetch_one(&mut *tx)
    .await?;
    if unresolved > 0 {
        return Err(ApiError::bad_request("resolve all Weave conflicts before applying"));
    }
    let source_layer_id = existing.get::<String, _>("source_layer_id");
    let target_layer_id = existing.get::<String, _>("target_layer_id");
    let planned_steps = existing.get::<Vec<String>, _>("planned_steps");
    let prepared_tree_id = existing
        .try_get::<Option<String>, _>("pre_weave_target_tree_id")
        .unwrap_or(None);
    let prepared_step_id = existing
        .try_get::<Option<String>, _>("pre_weave_target_step_id")
        .unwrap_or(None);
    let current_tree_id = sqlx::query_scalar::<_, Option<String>>(
        r#"
        SELECT root_tree_id
        FROM layer_heads
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        FOR UPDATE
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&target_layer_id)
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let current_step_id = sqlx::query_scalar::<_, String>(
        r#"
        SELECT step_id
        FROM layer_steps
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
          AND cleared_at IS NULL
        ORDER BY COALESCE(timeline_position, 9223372036854775807) DESC,
                 captured_at DESC, created_at DESC, step_id DESC
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&target_layer_id)
    .fetch_optional(&mut *tx)
    .await?;
    if current_tree_id != prepared_tree_id || current_step_id != prepared_step_id {
        return Err(ApiError::conflict(
            "target Layer changed after Weave preparation; prepare a new Weave plan",
        ));
    }
    let policy_epoch =
        current_policy_epoch(&state.pool, &workspace_id, &space_id, &target_layer_id).await?;
    sqlx::query(
        "UPDATE weave_requests SET status = 'applying', updated_at = now() WHERE weave_id = $1",
    )
    .bind(&weave_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE weave_sessions SET status = 'applying', updated_at = now() WHERE weave_id = $1",
    )
    .bind(&weave_id)
    .execute(&mut *tx)
    .await?;

    let applied_steps = apply_weave_step_replays_in_tx(
        &state.pool,
        &mut tx,
        &workspace_id,
        &space_id,
        &weave_id,
        &source_layer_id,
        &target_layer_id,
        policy_epoch,
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
        WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3
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
    let mut payload = load_weave_session_payload_for_update_in_tx(&mut tx, &weave_id).await?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("appliedSteps".to_string(), json!(applied_steps));
    } else {
        payload = json!({ "appliedSteps": applied_steps });
    }
    sqlx::query(
        "UPDATE weave_sessions SET status = 'applied', session_payload = $2, updated_at = now() WHERE weave_id = $1",
    )
    .bind(&weave_id)
    .bind(&payload)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(weave_request_json(&row)))
}

include!("weave_replay_plan.rs");

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
