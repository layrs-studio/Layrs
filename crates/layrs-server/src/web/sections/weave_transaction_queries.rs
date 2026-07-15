async fn tree_file_ref_for_path_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
          AND sto.workspace_id = $2 AND sto.space_id = $3
          AND sfo.workspace_id = $2 AND sfo.space_id = $3
          AND te.logical_path = $4 AND te.entry_kind = 'file'
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(path)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| TreeFileRef {
        file_object_id: row.get("file_object_id"),
        digest: row.get("digest"),
        size_bytes: row.get("size_bytes"),
        media_type: row.try_get("media_type").ok(),
    }))
}

async fn weave_side_from_tree_file_ref_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    file: Option<&TreeFileRef>,
) -> Result<WeaveConflictSide, ApiError> {
    let Some(file) = file else {
        return Ok(WeaveConflictSide {
            exists: false,
            digest: None,
            media_type: None,
            bytes: Vec::new(),
        });
    };
    Ok(WeaveConflictSide {
        exists: true,
        digest: Some(file.digest.clone()),
        media_type: file.media_type.clone(),
        bytes: file_object_bytes_in_tx(tx, workspace_id, space_id, &file.file_object_id).await?,
    })
}

async fn file_object_bytes_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<Vec<u8>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT oc.content_bytes, oc.compression
        FROM file_object_chunks foc
        JOIN object_chunks oc ON oc.chunk_id = foc.chunk_id
        JOIN space_object_chunks soc ON soc.chunk_id = oc.chunk_id
        WHERE foc.file_object_id = $1
          AND soc.workspace_id = $2 AND soc.space_id = $3
          AND oc.state = 'available'
        ORDER BY foc.chunk_index ASC
        "#,
    )
    .bind(file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut bytes = Vec::new();
    for row in rows {
        let chunk = row
            .try_get::<Vec<u8>, _>("content_bytes")
            .map_err(|_| ApiError::not_found("chunk bytes are not available"))?;
        let compression = row
            .try_get::<String, _>("compression")
            .unwrap_or_else(|_| CHUNK_COMPRESSION_IDENTITY.to_string());
        bytes.extend(decode_chunk_bytes(&chunk, &compression)?);
    }
    Ok(bytes)
}

async fn load_weave_request_row_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    load_weave_request_row_with_lock_in_tx(tx, workspace_id, space_id, weave_id, true).await
}

async fn load_weave_request_row_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    load_weave_request_row_with_lock_in_tx(tx, workspace_id, space_id, weave_id, false).await
}

async fn load_weave_request_row_with_lock_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
    for_update: bool,
) -> Result<sqlx::postgres::PgRow, ApiError> {
    let query = if for_update {
        "SELECT weave_id, source_layer_id, target_layer_id, title, body, status, pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps, applied_steps, requested_by_account_id, created_at::text AS created_at, updated_at::text AS updated_at FROM weave_requests WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3 FOR UPDATE"
    } else {
        "SELECT weave_id, source_layer_id, target_layer_id, title, body, status, pre_weave_target_tree_id, pre_weave_target_step_id, planned_steps, applied_steps, requested_by_account_id, created_at::text AS created_at, updated_at::text AS updated_at FROM weave_requests WHERE workspace_id = $1 AND space_id = $2 AND weave_id = $3"
    };
    sqlx::query(query)
        .bind(workspace_id)
        .bind(space_id)
        .bind(weave_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::not_found("weave request not found"))
}
