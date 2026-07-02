async fn list_layer_timeline(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    Query(query): Query<TimelineQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let request_cursor = query.cursor;
    let events = timeline_event_values(
        &state.pool,
        &workspace_id,
        Some(&space_id),
        Some(&layer_id),
        request_cursor.as_deref(),
        query.limit,
    )
    .await?;
    let response_cursor = latest_timeline_cursor(&events).or(request_cursor);

    Ok(Json(json!({
        "cursor": response_cursor,
        "items": events
    })))
}

async fn list_layer_steps(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let steps = layer_step_detail_values(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_id,
        None,
        &user.id,
    )
    .await?;
    Ok(Json(json!({ "items": steps })))
}

async fn get_layer_step(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, step_id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let mut steps = layer_step_detail_values(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_id,
        Some(&step_id),
        &user.id,
    )
    .await?;
    let step = steps
        .pop()
        .ok_or_else(|| ApiError::not_found("step not found"))?;
    Ok(Json(step))
}

async fn get_layer_step_diff(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, step_id)): Path<(String, String, String, String)>,
    Query(query): Query<StepDiffQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let start = query.start.unwrap_or(0);
    let limit = artifact_window_limit(query.limit)?;
    let column_start = query.column_start.or(query.column_start_camel).unwrap_or(0);
    let column_limit = artifact_column_limit(query.column_limit.or(query.column_limit_camel))?;
    Ok(Json(
        step_diff_window_value(
            &state.pool,
            &workspace_id,
            &space_id,
            &layer_id,
            &step_id,
            query.path.as_deref(),
            &user.id,
            WindowRequest {
                start,
                limit,
                column_start,
                column_limit,
            },
        )
        .await?,
    ))
}

async fn list_layer_artifacts(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;

    Ok(Json(json!({
        "items": artifact_values_for_layer(&state.pool, &workspace_id, &space_id, &layer_id, &user.id).await?
    })))
}

async fn get_layer_artifact_content(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, artifact_id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    Ok(Json(
        artifact_content_value(
            &state.pool,
            &workspace_id,
            &space_id,
            &layer_id,
            &artifact_id,
            &user.id,
        )
        .await?,
    ))
}

async fn get_layer_artifact_diff(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, artifact_id)): Path<(String, String, String, String)>,
    Query(query): Query<ArtifactDiffQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let start = query.start.unwrap_or(0);
    let limit = artifact_window_limit(query.limit)?;
    let column_start = query.column_start.or(query.column_start_camel).unwrap_or(0);
    let column_limit = artifact_column_limit(query.column_limit.or(query.column_limit_camel))?;
    let base_layer_id = query.base_layer_id.or(query.base_layer_id_camel);

    Ok(Json(
        artifact_diff_window_value(
            &state.pool,
            &workspace_id,
            &space_id,
            &layer_id,
            &artifact_id,
            &user.id,
            WindowRequest {
                start,
                limit,
                column_start,
                column_limit,
            },
            base_layer_id.as_deref(),
        )
        .await?,
    ))
}

async fn get_layer_access_policy(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_member(&state.pool, &workspace_id, &user.id).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn put_layer_access_policy(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<LayerAccessPolicyBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let policy_id = upsert_layer_policy(
        &state.pool,
        &workspace_id,
        &space_id,
        &layer_id,
        Some(&user.id),
    )
    .await?;
    replace_layer_policy_rules(&state.pool, &workspace_id, &policy_id, &body).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn create_layer_access_rule(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<LayerAccessRuleBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    validate_layer_access_rule(&state.pool, &workspace_id, &body).await?;
    let rule_id = body
        .id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| prefixed_id("access_rule"));
    let policy_id = policy_id_for_layer(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    insert_layer_access_rule(&state.pool, &policy_id, &rule_id, &body, Some(&user.id)).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn update_layer_access_rule(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, rule_id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<LayerAccessRuleBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    validate_layer_access_rule(&state.pool, &workspace_id, &body).await?;
    let policy_id = policy_id_for_layer(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    update_layer_access_rule_row(&state.pool, &policy_id, &rule_id, &body, Some(&user.id)).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}

async fn delete_layer_access_rule(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id, rule_id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_session(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    let policy_id = policy_id_for_layer(&state.pool, &workspace_id, &space_id, &layer_id).await?;
    delete_layer_access_rule_row(&state.pool, &policy_id, &rule_id, Some(&user.id)).await?;
    Ok(Json(
        layer_access_policy_value(&state.pool, &workspace_id, &space_id, &layer_id).await?,
    ))
}
