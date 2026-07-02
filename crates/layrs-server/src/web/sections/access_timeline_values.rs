async fn access_registry_values(pool: &PgPool, workspace_id: &str) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT policy_id, layer_id,
               to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
        FROM layer_access_policies
        WHERE workspace_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("policy_id"),
                "workspaceId": workspace_id,
                "layerId": row.get::<String, _>("layer_id"),
                "rules": [],
                "updatedAt": row.get::<String, _>("updated_at")
            })
        })
        .collect())
}

async fn timeline_event_values(
    pool: &PgPool,
    workspace_id: &str,
    space_id: Option<&str>,
    layer_id: Option<&str>,
    cursor: Option<&str>,
    limit: Option<i64>,
) -> Result<Vec<Value>, ApiError> {
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let rows = sqlx::query(
        r#"
        SELECT event_id, space_id, layer_id, event_kind, title, body_json,
               to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM timeline_events
        WHERE workspace_id = $1
          AND ($2::text IS NULL OR space_id = $2)
          AND ($3::text IS NULL OR layer_id = $3)
          AND (
            $4::text IS NULL
            OR created_at > COALESCE(
                (SELECT created_at FROM timeline_events WHERE workspace_id = $1 AND event_id = $4),
                '-infinity'::timestamptz
            )
          )
        ORDER BY created_at ASC
        LIMIT $5
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(cursor)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.get::<String, _>("event_id"),
                "workspaceId": workspace_id,
                "spaceId": row.try_get::<String, _>("space_id").ok(),
                "layerId": row.try_get::<String, _>("layer_id").ok(),
                "kind": row.get::<String, _>("event_kind"),
                "title": row.get::<String, _>("title"),
                "body": redact_timeline_body(row.get::<Value, _>("body_json")),
                "createdAt": row.get::<String, _>("created_at")
            })
        })
        .collect())
}

async fn timeline_event_by_id(
    pool: &PgPool,
    workspace_id: &str,
    event_id: &str,
) -> Result<Value, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT event_id, space_id, layer_id, event_kind, title, body_json,
               to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
        FROM timeline_events
        WHERE workspace_id = $1 AND event_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(event_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("timeline event not found"))?;

    Ok(json!({
        "id": row.get::<String, _>("event_id"),
        "workspaceId": workspace_id,
        "spaceId": row.try_get::<String, _>("space_id").ok(),
        "layerId": row.try_get::<String, _>("layer_id").ok(),
        "kind": row.get::<String, _>("event_kind"),
        "title": row.get::<String, _>("title"),
        "body": redact_timeline_body(row.get::<Value, _>("body_json")),
        "createdAt": row.get::<String, _>("created_at")
    }))
}

async fn existing_redacted_artifact(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    path: &str,
) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS(
          SELECT 1 FROM artifacts
          WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = $4 AND state = 'redacted'
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(path)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)
}

fn publish_artifact_uses_v2(body: &PublishArtifactBody) -> bool {
    body.file_object_id.is_some()
        || body.file_object_id_camel.is_some()
        || body.object_id.is_some()
        || body.object_id_camel.is_some()
        || body.tree_id.is_some()
        || body.tree_id_camel.is_some()
        || body.sha256.is_some()
        || body.content_hash.is_some()
        || body.size_bytes.is_some()
        || body.size_bytes_camel.is_some()
        || !body.chunks.is_empty()
}

async fn load_sync_batch_response(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    idempotency_key: &str,
) -> Result<Option<Value>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT response_json
        FROM sync_batches
        WHERE workspace_id = $1 AND space_id = $2 AND idempotency_key = $3 AND status = 'applied'
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)
}

async fn current_policy_epoch(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<i64, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT policy_epoch
        FROM layer_access_policies
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("layer access policy not found"))
}

