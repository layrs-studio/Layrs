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
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &body.source_layer_id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &body.target_layer_id).await?;
    if body.source_layer_id == body.target_layer_id {
        return Err(ApiError::bad_request("source and target Layers must be different"));
    }

    let pre_weave_target_tree_id: Option<String> = sqlx::query_scalar(
        r#"
        SELECT ls.root_tree_id
        FROM layer_heads h
        JOIN layer_states ls ON ls.layer_state_id = h.layer_state_id
        WHERE h.workspace_id = $1 AND h.space_id = $2 AND h.layer_id = $3
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&body.target_layer_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    let pre_weave_target_step_id: Option<String> = sqlx::query_scalar(
        r#"
        SELECT step_id
        FROM layer_steps
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
          AND cleared_at IS NULL
        ORDER BY captured_at DESC, created_at DESC, step_id DESC
        LIMIT 1
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&body.target_layer_id)
    .fetch_optional(&state.pool)
    .await?;
    let planned_steps = source_steps_missing_from_target(
        &state.pool,
        &workspace_id,
        &space_id,
        &body.source_layer_id,
        &body.target_layer_id,
    )
    .await?;
    let status = if planned_steps.is_empty() { "resolved" } else { "open" };
    let weave_id = prefixed_id("weave");
    let title = body
        .title
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "Weave request".to_string());

    let row = sqlx::query(
        r#"
        INSERT INTO weave_requests
            (weave_id, workspace_id, space_id, source_layer_id, target_layer_id, title, body,
             status, pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
             requested_by_account_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        RETURNING weave_id, source_layer_id, target_layer_id, title, body, status,
                  pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
                  applied_steps, created_at::text AS created_at, updated_at::text AS updated_at
        "#,
    )
    .bind(&weave_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&body.source_layer_id)
    .bind(&body.target_layer_id)
    .bind(title)
    .bind(body.body.unwrap_or_default())
    .bind(status)
    .bind(pre_weave_target_tree_id)
    .bind(pre_weave_target_step_id)
    .bind(&planned_steps)
    .bind(&user.id)
    .fetch_one(&state.pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO weave_sessions (weave_id, status, session_payload)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(&weave_id)
    .bind(if planned_steps.is_empty() { "resolved" } else { "preview" })
    .bind(json!({ "plannedSteps": planned_steps }))
    .execute(&state.pool)
    .await?;

    Ok(Json(weave_request_json(&row)))
}

async fn get_weave_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id, weave_id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    let row = load_weave_request_row(&state.pool, &workspace_id, &space_id, &weave_id).await?;
    let conflicts = weave_conflict_values(&state.pool, &weave_id).await?;
    let mut value = weave_request_json(&row);
    if let Some(object) = value.as_object_mut() {
        object.insert("conflicts".to_string(), Value::Array(conflicts));
    }
    Ok(Json(value))
}

async fn abort_weave_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id, weave_id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
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
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("weave request not found"))?;
    sqlx::query("UPDATE weave_sessions SET status = 'aborted', updated_at = now() WHERE weave_id = $1")
        .bind(&weave_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(weave_request_json(&row)))
}

async fn apply_weave_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, space_id, weave_id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    let unresolved: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM weave_conflicts WHERE weave_id = $1 AND status <> 'resolved'",
    )
    .bind(&weave_id)
    .fetch_one(&state.pool)
    .await?;
    if unresolved > 0 {
        return Err(ApiError::bad_request("resolve all Weave conflicts before applying"));
    }
    let row = sqlx::query(
        r#"
        UPDATE weave_requests
        SET status = 'applied', applied_steps = planned_steps, updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3
        RETURNING weave_id, source_layer_id, target_layer_id, title, body, status,
                  pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
                  applied_steps, created_at::text AS created_at, updated_at::text AS updated_at
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&weave_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::not_found("weave request not found"))?;
    sqlx::query("UPDATE weave_sessions SET status = 'applied', updated_at = now() WHERE weave_id = $1")
        .bind(&weave_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(weave_request_json(&row)))
}

async fn source_steps_missing_from_target(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    source_layer_id: &str,
    target_layer_id: &str,
) -> Result<Vec<String>, ApiError> {
    let source_steps = sqlx::query(
        r#"
        SELECT step_id
        FROM layer_steps
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
          AND cleared_at IS NULL
        ORDER BY captured_at ASC, created_at ASC, step_id ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(source_layer_id)
    .fetch_all(pool)
    .await?;
    let target_step_ids = sqlx::query(
        r#"
        SELECT step_id
        FROM layer_steps
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
          AND cleared_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(target_layer_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| row.get::<String, _>("step_id"))
    .collect::<HashSet<_>>();
    Ok(source_steps
        .into_iter()
        .map(|row| row.get::<String, _>("step_id"))
        .filter(|step_id| !target_step_ids.contains(step_id))
        .collect())
}

async fn load_weave_request_row(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    sqlx::query(
        r#"
        SELECT weave_id, source_layer_id, target_layer_id, title, body, status,
               pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps,
               applied_steps, created_at::text AS created_at, updated_at::text AS updated_at
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

async fn weave_conflict_values(pool: &PgPool, weave_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT conflict_id, logical_path, lens_id, status, message, resolved_file_object_id
        FROM weave_conflicts
        WHERE weave_id = $1
        ORDER BY logical_path ASC
        "#,
    )
    .bind(weave_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "conflictId": row.get::<String, _>("conflict_id"),
                "path": row.get::<String, _>("logical_path"),
                "lensId": row.get::<String, _>("lens_id"),
                "status": row.get::<String, _>("status"),
                "message": row.get::<String, _>("message"),
                "resolvedFileObjectId": row.try_get::<String, _>("resolved_file_object_id").ok()
            })
        })
        .collect())
}

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
