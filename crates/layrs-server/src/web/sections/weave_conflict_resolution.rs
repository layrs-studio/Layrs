async fn weave_request_detail_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
) -> Result<Value, ApiError> {
    let row = load_weave_request_row(pool, workspace_id, space_id, weave_id).await?;
    let conflicts = weave_conflict_values(pool, workspace_id, space_id, weave_id).await?;
    let mut value = weave_request_json(&row);
    if let Some(object) = value.as_object_mut() {
        object.insert("conflicts".to_string(), Value::Array(conflicts));
    }
    Ok(value)
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

async fn weave_conflict_values(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT wc.conflict_id, wc.logical_path, wc.lens_id, wc.status, wc.message,
               wc.base_file_object_id, wc.ours_file_object_id, wc.theirs_file_object_id,
               wc.resolved_file_object_id, wr.source_layer_id, wr.target_layer_id
        FROM weave_conflicts wc
        JOIN weave_requests wr ON wr.weave_id = wc.weave_id
        WHERE wr.workspace_id = $1 AND wr.space_id = $2 AND wr.weave_id = $3
        ORDER BY wc.logical_path ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(weave_id)
    .fetch_all(pool)
    .await?;
    let session_payload = weave_session_payload(pool, weave_id).await?;
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(
            weave_conflict_value(
                pool,
                workspace_id,
                space_id,
                &weave_conflict_row(&row),
                &session_payload,
            )
            .await?,
        );
    }
    Ok(values)
}

async fn weave_conflict_value(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    row: &WeaveConflictRow,
    session_payload: &Value,
) -> Result<Value, ApiError> {
    let base =
        load_weave_conflict_side(pool, workspace_id, space_id, row.base_file_object_id.as_deref())
            .await?;
    let ours =
        load_weave_conflict_side(pool, workspace_id, space_id, row.ours_file_object_id.as_deref())
            .await?;
    let theirs = load_weave_conflict_side(
        pool,
        workspace_id,
        space_id,
        row.theirs_file_object_id.as_deref(),
    )
    .await?;
    let reconcile = reconcile_weave_conflict(row, &base, &ours, &theirs);
    let file_methods = resolution_method_labels(&lens_resolution_methods(&row.lens_id).file);
    let resolution = conflict_payload(session_payload, &row.conflict_id)
        .and_then(|payload| payload.get("resolution"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = if row.message.trim().is_empty() {
        reconcile.summary.clone()
    } else {
        row.message.clone()
    };
    let mut value = json!({
        "conflictId": &row.conflict_id,
        "path": &row.logical_path,
        "lensId": &row.lens_id,
        "status": &row.status,
        "message": message,
        "resolution": resolution,
        "supportedMethods": &file_methods,
        "methods": &file_methods,
        "blocks": weave_conflict_block_values(row, &reconcile, session_payload),
        "segments": weave_conflict_segment_values(&reconcile),
        "baseFileObjectId": &row.base_file_object_id,
        "existingFileObjectId": &row.ours_file_object_id,
        "incomingFileObjectId": &row.theirs_file_object_id,
        "oursFileObjectId": &row.ours_file_object_id,
        "theirsFileObjectId": &row.theirs_file_object_id,
        "resolvedFileObjectId": &row.resolved_file_object_id,
        "fields": {
            "reconcileStatus": reconcile.status.as_str(),
            "baseExists": base.exists,
            "existingExists": ours.exists,
            "incomingExists": theirs.exists,
            "resolvedFileObjectId": &row.resolved_file_object_id
        }
    });
    if row.lens_id == "layrs.text" {
        if let Some(object) = value.as_object_mut() {
            object.insert("base".to_string(), side_text_json(&base));
            object.insert("existing".to_string(), side_text_json(&ours));
            object.insert("incoming".to_string(), side_text_json(&theirs));
            object.insert("ours".to_string(), side_text_json(&ours));
            object.insert("theirs".to_string(), side_text_json(&theirs));
        }
    }
    Ok(value)
}

async fn weave_session_payload(pool: &PgPool, weave_id: &str) -> Result<Value, ApiError> {
    Ok(sqlx::query_scalar::<_, Value>(
        "SELECT session_payload FROM weave_sessions WHERE weave_id = $1",
    )
    .bind(weave_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(|| json!({})))
}

async fn load_weave_session_payload_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    weave_id: &str,
) -> Result<Value, ApiError> {
    Ok(sqlx::query_scalar::<_, Value>(
        "SELECT session_payload FROM weave_sessions WHERE weave_id = $1 FOR UPDATE",
    )
    .bind(weave_id)
    .fetch_optional(&mut **tx)
    .await?
    .unwrap_or_else(|| json!({})))
}

async fn load_weave_conflict_row_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
    conflict_id: &str,
) -> Result<WeaveConflictRow, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT wc.conflict_id, wc.logical_path, wc.lens_id, wc.status, wc.message,
               wc.base_file_object_id, wc.ours_file_object_id, wc.theirs_file_object_id,
               wc.resolved_file_object_id, wr.source_layer_id, wr.target_layer_id
        FROM weave_conflicts wc
        JOIN weave_requests wr ON wr.weave_id = wc.weave_id
        WHERE wr.workspace_id = $1
          AND wr.space_id = $2
          AND wr.weave_id = $3
          AND wc.conflict_id = $4
        FOR UPDATE OF wc
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(weave_id)
    .bind(conflict_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Weave conflict not found"))?;
    Ok(weave_conflict_row(&row))
}

fn weave_conflict_row(row: &sqlx::postgres::PgRow) -> WeaveConflictRow {
    WeaveConflictRow {
        conflict_id: row.get("conflict_id"),
        logical_path: row.get("logical_path"),
        lens_id: row.get("lens_id"),
        status: row.get("status"),
        message: row.get("message"),
        base_file_object_id: row.try_get("base_file_object_id").ok(),
        ours_file_object_id: row.try_get("ours_file_object_id").ok(),
        theirs_file_object_id: row.try_get("theirs_file_object_id").ok(),
        resolved_file_object_id: row.try_get("resolved_file_object_id").ok(),
        source_layer_id: row.get("source_layer_id"),
        target_layer_id: row.get("target_layer_id"),
    }
}

async fn load_weave_conflict_side(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    file_object_id: Option<&str>,
) -> Result<WeaveConflictSide, ApiError> {
    let Some(file_object_id) = file_object_id else {
        return Ok(WeaveConflictSide {
            exists: false,
            digest: None,
            media_type: None,
            bytes: Vec::new(),
        });
    };
    let row = sqlx::query(
        r#"
        SELECT f.file_object_id, f.digest, f.media_type
        FROM file_objects f
        JOIN space_file_objects sfo ON sfo.file_object_id = f.file_object_id
        WHERE sfo.workspace_id = $1
          AND sfo.space_id = $2
          AND f.file_object_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::not_found("Weave conflict file object not found"))?;
    Ok(WeaveConflictSide {
        exists: true,
        digest: row.try_get("digest").ok(),
        media_type: row.try_get("media_type").ok(),
        bytes: file_object_bytes(pool, workspace_id, space_id, file_object_id).await?,
    })
}

fn reconcile_weave_conflict(
    row: &WeaveConflictRow,
    base: &WeaveConflictSide,
    ours: &WeaveConflictSide,
    theirs: &WeaveConflictSide,
) -> LensReconcileResult {
    let input = LensReconcileInput {
        path: Some(std::path::Path::new(&row.logical_path)),
        media_type: preferred_conflict_media_type(base, ours, theirs),
        base: lens_side(base),
        ours: lens_side(ours),
        theirs: lens_side(theirs),
        ours_label: &row.target_layer_id,
        theirs_label: &row.source_layer_id,
    };
    match row.lens_id.as_str() {
        "layrs.text" => layrs_lens_text::reconcile_text(input),
        "layrs.raw" | "layrs.image" | "layrs.code" => layrs_lens_raw::reconcile_raw(input),
        lens_id => LensReconcileResult::unsupported(format!(
            "Lens {lens_id} does not support server-side Weave reconciliation"
        )),
    }
}

fn lens_side(side: &WeaveConflictSide) -> LensReconcileSide<'_> {
    if side.exists {
        LensReconcileSide::present(&side.bytes, side.digest.as_deref())
    } else {
        LensReconcileSide::absent()
    }
}

fn preferred_conflict_media_type<'a>(
    base: &'a WeaveConflictSide,
    ours: &'a WeaveConflictSide,
    theirs: &'a WeaveConflictSide,
) -> Option<&'a str> {
    ours.media_type
        .as_deref()
        .or(theirs.media_type.as_deref())
        .or(base.media_type.as_deref())
        .filter(|media_type| *media_type != "application/octet-stream")
}

fn weave_conflict_block_values(
    row: &WeaveConflictRow,
    reconcile: &LensReconcileResult,
    session_payload: &Value,
) -> Vec<Value> {
    reconcile
        .blocks
        .iter()
        .map(|block| {
            let stored = block_payload(session_payload, &row.conflict_id, &block.block_id);
            let resolution = stored
                .and_then(|payload| payload.get("method"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let status = stored
                .and_then(|payload| payload.get("status"))
                .and_then(Value::as_str)
                .unwrap_or(if row.status == "resolved" {
                    "resolved"
                } else {
                    "open"
                });
            let methods = if block.methods.is_empty() {
                lens_resolution_methods(&row.lens_id).block
            } else {
                block.methods.clone()
            };
            let method_labels = resolution_method_labels(&methods);
            json!({
                "blockId": &block.block_id,
                "status": status,
                "base": &block.base,
                "ours": &block.ours,
                "theirs": &block.theirs,
                "existing": &block.ours,
                "incoming": &block.theirs,
                "resolution": resolution,
                "supportedMethods": &method_labels,
                "methods": &method_labels
            })
        })
        .collect()
}

fn weave_conflict_segment_values(reconcile: &LensReconcileResult) -> Vec<Value> {
    reconcile
        .segments
        .iter()
        .map(|segment| {
            json!({
                "kind": match segment.kind {
                    LensConflictSegmentKind::Text => "text",
                    LensConflictSegmentKind::Block => "block",
                },
                "text": &segment.text,
                "blockId": &segment.block_id
            })
        })
        .collect()
}

async fn resolve_weave_text_block_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &WeaveConflictRow,
    reconcile: &LensReconcileResult,
    payload: &mut Value,
    block_id: &str,
    method: ResolutionMethod,
    manual_text: Option<&str>,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    base: &WeaveConflictSide,
    ours: &WeaveConflictSide,
    theirs: &WeaveConflictSide,
) -> Result<(), ApiError> {
    if row.lens_id != "layrs.text" {
        return Err(ApiError::bad_request(format!(
            "Lens {} does not support block-level Weave resolution",
            row.lens_id
        )));
    }
    if reconcile.status == LensReconcileResultStatus::Unsupported {
        return Err(ApiError::bad_request(reconcile.summary.clone()));
    }
    let normalized_block_id = normalize_weave_block_id(block_id);
    let block = reconcile
        .blocks
        .iter()
        .find(|block| block.block_id == normalized_block_id)
        .ok_or_else(|| ApiError::bad_request("Weave conflict block not found"))?;
    let methods = if block.methods.is_empty() {
        lens_resolution_methods(&row.lens_id).block
    } else {
        block.methods.clone()
    };
    ensure_resolution_method_allowed(&row.lens_id, "block", &row.logical_path, method, &methods)?;
    if method == ResolutionMethod::Manual && manual_text.is_none() {
        return Err(ApiError::bad_request(
            "manual text block resolution requires manualText",
        ));
    }
    let resolved_text =
        layrs_lens_text::resolve_text_block_choice_by_method(&block.ours, &block.theirs, method, manual_text)
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
    set_block_resolution_payload(
        payload,
        &row.conflict_id,
        &normalized_block_id,
        method,
        &resolved_text,
    );

    if reconcile
        .blocks
        .iter()
        .all(|block| block_payload(payload, &row.conflict_id, &block.block_id).is_some())
    {
        let content = assemble_text_resolution(row, reconcile, payload)?;
        let resolved_file_object_id = if content.exists {
            Some(
                write_resolved_file_object_in_tx(
                    tx,
                    workspace_id,
                    space_id,
                    account_id,
                    &content.bytes,
                    &conflict_media_type(row, base, ours, theirs),
                )
                .await?,
            )
        } else {
            None
        };
        mark_weave_conflict_resolved_in_tx(
            tx,
            row,
            resolved_file_object_id.as_deref(),
            if text_resolution_has_manual(row, payload) {
                "manual"
            } else {
                "file"
            },
            account_id,
        )
        .await?;
        set_conflict_resolution_payload(
            payload,
            &row.conflict_id,
            "blocks",
            resolved_file_object_id.as_deref(),
        );
    }
    save_weave_session_payload_in_tx(tx, row, payload).await
}

async fn resolve_weave_file_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &WeaveConflictRow,
    method: ResolutionMethod,
    payload: &mut Value,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    base: &WeaveConflictSide,
    ours: &WeaveConflictSide,
    theirs: &WeaveConflictSide,
) -> Result<(), ApiError> {
    let methods = lens_resolution_methods(&row.lens_id).file;
    ensure_resolution_method_allowed(&row.lens_id, "file", &row.logical_path, method, &methods)?;
    let input = LensFileResolutionInput {
        method,
        base: lens_side(base),
        existing: lens_side(ours),
        incoming: lens_side(theirs),
    };
    let content = match row.lens_id.as_str() {
        "layrs.raw" | "layrs.image" | "layrs.code" => layrs_lens_raw::resolve_raw_conflict(input),
        lens_id => {
            return Err(ApiError::bad_request(format!(
                "Lens {lens_id} does not support file-level Weave resolution"
            )));
        }
    }
    .map_err(|error| ApiError::bad_request(error.to_string()))?;
    let resolved_file_object_id = if content.exists {
        Some(
            write_resolved_file_object_in_tx(
                tx,
                workspace_id,
                space_id,
                account_id,
                &content.bytes,
                &conflict_media_type(row, base, ours, theirs),
            )
            .await?,
        )
    } else {
        None
    };
    mark_weave_conflict_resolved_in_tx(
        tx,
        row,
        resolved_file_object_id.as_deref(),
        resolution_kind_for_method(method),
        account_id,
    )
    .await?;
    set_conflict_resolution_payload(
        payload,
        &row.conflict_id,
        method.as_str(),
        resolved_file_object_id.as_deref(),
    );
    save_weave_session_payload_in_tx(tx, row, payload).await
}

async fn write_resolved_file_object_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    bytes: &[u8],
    media_type: &str,
) -> Result<String, ApiError> {
    let digest = blake3_digest_for_bytes(bytes);
    let object_key = format!("chunks/global/{digest}");
    sqlx::query(
        r#"
        INSERT INTO object_chunks
            (chunk_id, workspace_id, space_id, digest, size_bytes, stored_size_bytes, object_key,
             compression, state, content_bytes, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $5, $6, $7, 'available', $8, $9)
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
    .bind(&digest)
    .bind(workspace_id)
    .bind(space_id)
    .bind(&digest)
    .bind(bytes.len() as i64)
    .bind(&object_key)
    .bind(CHUNK_COMPRESSION_IDENTITY)
    .bind(bytes.to_vec())
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    mark_chunk_available_for_space_in_tx(tx, workspace_id, space_id, &digest, account_id).await?;
    let chunks = vec![ChunkDescriptor {
        chunk_id: digest.clone(),
        digest: digest.clone(),
        size_bytes: bytes.len() as i64,
        byte_offset: 0,
    }];
    upsert_file_object_in_tx(
        tx,
        workspace_id,
        space_id,
        Some(&digest),
        &digest,
        bytes.len() as i64,
        media_type,
        &chunks,
        account_id,
    )
    .await
}

async fn mark_weave_conflict_resolved_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &WeaveConflictRow,
    resolved_file_object_id: Option<&str>,
    resolution_kind: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE weave_conflicts
        SET status = 'resolved', resolved_file_object_id = $1, updated_at = now()
        WHERE conflict_id = $2
        "#,
    )
    .bind(resolved_file_object_id)
    .bind(&row.conflict_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO weave_resolutions
            (resolution_id, conflict_id, resolution_kind, resolved_file_object_id, resolved_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(prefixed_id("weave_resolution"))
    .bind(&row.conflict_id)
    .bind(resolution_kind)
    .bind(resolved_file_object_id)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn save_weave_session_payload_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &WeaveConflictRow,
    payload: &Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO weave_sessions (weave_id, status, session_payload)
        SELECT weave_id, 'conflicted', $2
        FROM weave_conflicts
        WHERE conflict_id = $1
        ON CONFLICT (weave_id) DO UPDATE SET
            session_payload = EXCLUDED.session_payload,
            status = CASE
                WHEN weave_sessions.status IN ('applied', 'aborted') THEN weave_sessions.status
                ELSE EXCLUDED.status
            END,
            updated_at = now()
        "#,
    )
    .bind(&row.conflict_id)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn update_weave_resolution_status_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    weave_id: &str,
) -> Result<(), ApiError> {
    let unresolved: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM weave_conflicts WHERE weave_id = $1 AND status <> 'resolved'",
    )
    .bind(weave_id)
    .fetch_one(&mut **tx)
    .await?;
    let status = if unresolved == 0 {
        "resolved"
    } else {
        "conflicted"
    };
    sqlx::query(
        "UPDATE weave_requests SET status = $1, updated_at = now() WHERE weave_id = $2 AND status NOT IN ('applied', 'aborted', 'closed')",
    )
    .bind(status)
    .bind(weave_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE weave_sessions SET status = $1, updated_at = now() WHERE weave_id = $2 AND status NOT IN ('applied', 'aborted')",
    )
    .bind(status)
    .bind(weave_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn assemble_text_resolution(
    row: &WeaveConflictRow,
    reconcile: &LensReconcileResult,
    payload: &Value,
) -> Result<LensReconcileContent, ApiError> {
    let blocks = reconcile
        .blocks
        .iter()
        .map(|block| {
            let block_payload =
                block_payload(payload, &row.conflict_id, &block.block_id).ok_or_else(|| {
                    ApiError::bad_request(format!(
                        "Text conflict block {} is not resolved",
                        block.block_id
                    ))
                })?;
            let method = block_payload
                .get("method")
                .and_then(Value::as_str)
                .and_then(ResolutionMethod::from_label)
                .ok_or_else(|| ApiError::bad_request("stored text block resolution is invalid"))?;
            Ok(LensBlockResolutionInput {
                block_id: block.block_id.as_str(),
                base: block.base.as_str(),
                existing: block.ours.as_str(),
                incoming: block.theirs.as_str(),
                method,
                manual_text: None,
                resolved_text: block_payload.get("resolvedText").and_then(Value::as_str),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let segments = reconcile
        .segments
        .iter()
        .map(|segment| LensResolutionSegment {
            kind: segment.kind.clone(),
            text: segment.text.as_deref(),
            block_id: segment.block_id.as_deref(),
        })
        .collect::<Vec<_>>();
    layrs_lens_text::resolve_text_conflict(&blocks, &segments)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

fn lens_resolution_methods(lens_id: &str) -> ResolutionMethods {
    match lens_id {
        "layrs.text" => layrs_lens_text::resolution_methods(),
        "layrs.raw" | "layrs.image" | "layrs.code" => layrs_lens_raw::resolution_methods(),
        _ => ResolutionMethods::none(),
    }
}

fn resolution_method_labels(methods: &[ResolutionMethod]) -> Vec<String> {
    layrs_lens_sdk::resolution_method_labels(methods)
}

fn ensure_resolution_method_allowed(
    lens_id: &str,
    scope: &str,
    target: &str,
    method: ResolutionMethod,
    methods: &[ResolutionMethod],
) -> Result<(), ApiError> {
    if methods.contains(&method) {
        return Ok(());
    }
    let available = resolution_method_labels(methods);
    let available = if available.is_empty() {
        "none".to_string()
    } else {
        available.join(", ")
    };
    Err(ApiError::bad_request(format!(
        "Lens {lens_id} does not declare {scope}-level resolution method `{}` for {target}. Available methods: {available}.",
        method.as_str()
    )))
}

fn conflict_media_type(
    row: &WeaveConflictRow,
    base: &WeaveConflictSide,
    ours: &WeaveConflictSide,
    theirs: &WeaveConflictSide,
) -> String {
    let stored = ours
        .media_type
        .as_deref()
        .or(theirs.media_type.as_deref())
        .or(base.media_type.as_deref())
        .unwrap_or("application/octet-stream");
    preview_media_type_for_path(&row.logical_path, stored).to_string()
}

fn side_text_json(side: &WeaveConflictSide) -> Value {
    if !side.exists {
        return json!("");
    }
    match std::str::from_utf8(&side.bytes) {
        Ok(text) => json!(text),
        Err(_) => Value::Null,
    }
}

fn normalize_weave_block_id(block_id: &str) -> String {
    let block_id = block_id.trim();
    if block_id.starts_with("block-") {
        block_id.to_string()
    } else {
        format!("block-{block_id}")
    }
}

fn conflict_payload<'a>(payload: &'a Value, conflict_id: &str) -> Option<&'a Value> {
    payload
        .get("conflictResolutions")
        .and_then(|conflicts| conflicts.get(conflict_id))
}

fn block_payload<'a>(
    payload: &'a Value,
    conflict_id: &str,
    block_id: &str,
) -> Option<&'a Value> {
    conflict_payload(payload, conflict_id)
        .and_then(|conflict| conflict.get("blocks"))
        .and_then(|blocks| blocks.get(block_id))
}

fn set_block_resolution_payload(
    payload: &mut Value,
    conflict_id: &str,
    block_id: &str,
    method: ResolutionMethod,
    resolved_text: &str,
) {
    let block = block_payload_mut(payload, conflict_id, block_id);
    block.insert("status".to_string(), json!("resolved"));
    block.insert("method".to_string(), json!(method.as_str()));
    block.insert("resolvedText".to_string(), json!(resolved_text));
}

fn set_conflict_resolution_payload(
    payload: &mut Value,
    conflict_id: &str,
    resolution: &str,
    resolved_file_object_id: Option<&str>,
) {
    let conflict = conflict_payload_mut(payload, conflict_id);
    conflict.insert("status".to_string(), json!("resolved"));
    conflict.insert("resolution".to_string(), json!(resolution));
    conflict.insert(
        "resolvedFileObjectId".to_string(),
        resolved_file_object_id.map(Value::from).unwrap_or(Value::Null),
    );
}

fn conflict_payload_mut<'a>(
    payload: &'a mut Value,
    conflict_id: &str,
) -> &'a mut serde_json::Map<String, Value> {
    let root = ensure_json_object(payload);
    let conflicts = root
        .entry("conflictResolutions".to_string())
        .or_insert_with(|| json!({}));
    let conflicts = ensure_json_object(conflicts);
    let conflict = conflicts
        .entry(conflict_id.to_string())
        .or_insert_with(|| json!({}));
    ensure_json_object(conflict)
}

fn block_payload_mut<'a>(
    payload: &'a mut Value,
    conflict_id: &str,
    block_id: &str,
) -> &'a mut serde_json::Map<String, Value> {
    let conflict = conflict_payload_mut(payload, conflict_id);
    let blocks = conflict
        .entry("blocks".to_string())
        .or_insert_with(|| json!({}));
    let blocks = ensure_json_object(blocks);
    let block = blocks
        .entry(block_id.to_string())
        .or_insert_with(|| json!({}));
    ensure_json_object(block)
}

fn ensure_json_object(value: &mut Value) -> &mut serde_json::Map<String, Value> {
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("JSON value is an object")
}

fn text_resolution_has_manual(row: &WeaveConflictRow, payload: &Value) -> bool {
    conflict_payload(payload, &row.conflict_id)
        .and_then(|conflict| conflict.get("blocks"))
        .and_then(Value::as_object)
        .map(|blocks| {
            blocks.values().any(|block| {
                block.get("method").and_then(Value::as_str) == Some("manual")
            })
        })
        .unwrap_or(false)
}

fn resolution_kind_for_method(method: ResolutionMethod) -> &'static str {
    match method {
        ResolutionMethod::Existing => "ours",
        ResolutionMethod::Incoming => "theirs",
        ResolutionMethod::Both => "file",
        ResolutionMethod::Manual => "manual",
    }
}
