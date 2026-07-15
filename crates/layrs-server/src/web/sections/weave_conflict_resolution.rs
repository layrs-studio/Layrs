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
    let base = load_weave_conflict_side(
        pool,
        workspace_id,
        space_id,
        row.base_file_object_id.as_deref(),
    )
    .await?;
    let ours = load_weave_conflict_side(
        pool,
        workspace_id,
        space_id,
        row.ours_file_object_id.as_deref(),
    )
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

async fn load_weave_conflict_side_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
        SELECT f.digest, f.media_type
        FROM file_objects f
        JOIN space_file_objects sfo ON sfo.file_object_id = f.file_object_id
        WHERE sfo.workspace_id = $1 AND sfo.space_id = $2 AND f.file_object_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Weave conflict file object not found"))?;
    Ok(WeaveConflictSide {
        exists: true,
        digest: row.try_get("digest").ok(),
        media_type: row.try_get("media_type").ok(),
        bytes: file_object_bytes_in_tx(tx, workspace_id, space_id, file_object_id).await?,
    })
}

fn reconcile_weave_conflict(
    row: &WeaveConflictRow,
    base: &WeaveConflictSide,
    ours: &WeaveConflictSide,
    theirs: &WeaveConflictSide,
) -> LensReconcileResult {
    let Some(manifest) = crate::lenses::built_in_lens_manifest(&row.lens_id) else {
        return LensReconcileResult::unsupported(format!(
            "Lens {} is not registered for server-side Weave reconciliation",
            row.lens_id
        ));
    };
    if !manifest.supports_capability(&layrs_lens_sdk::LensCapability::Reconcile) {
        return LensReconcileResult::unsupported(format!(
            "Lens {} does not support Weave reconciliation",
            manifest.id
        ));
    }

    let input = LensReconcileInput {
        path: Some(std::path::Path::new(&row.logical_path)),
        media_type: preferred_conflict_media_type(base, ours, theirs),
        base: lens_side(base),
        ours: lens_side(ours),
        theirs: lens_side(theirs),
        ours_label: &row.target_layer_id,
        theirs_label: &row.source_layer_id,
    };
    match manifest.id.as_str() {
        layrs_lens_text::TEXT_LENS_ID => layrs_lens_text::reconcile_text(input),
        layrs_lens_raw::RAW_LENS_ID => layrs_lens_raw::reconcile_raw(input),
        lens_id => LensReconcileResult::unsupported(format!(
            "Lens {lens_id} has no server-side Weave reconciler"
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

#[expect(
    clippy::too_many_arguments,
    reason = "transactional resolution keeps the loaded conflict sides explicit"
)]
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
    let resolved_text = layrs_lens_text::resolve_text_block_choice_by_method(
        &block.ours,
        &block.theirs,
        method,
        manual_text,
    )
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

#[expect(
    clippy::too_many_arguments,
    reason = "transactional resolution keeps the loaded conflict sides explicit"
)]
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
        layrs_lens_raw::RAW_LENS_ID => layrs_lens_raw::resolve_raw_conflict(input),
        lens_id => {
            return Err(unsupported_lens_reconcile_error(
                lens_id,
                "file",
                &row.logical_path,
            ));
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
    insert_or_compare_chunk_in_tx(
        tx,
        workspace_id,
        space_id,
        account_id,
        &digest,
        bytes,
        CHUNK_COMPRESSION_IDENTITY,
    )
    .await?;
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
    let updated = sqlx::query(
        r#"
        UPDATE weave_conflicts wc
        SET status = 'resolved', resolved_file_object_id = $1, updated_at = now()
        WHERE wc.conflict_id = $2
          AND EXISTS (
              SELECT 1 FROM weave_requests wr
              WHERE wr.weave_id = wc.weave_id
                AND wr.status IN ('open', 'preview', 'conflicted')
          )
        "#,
    )
    .bind(resolved_file_object_id)
    .bind(&row.conflict_id)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::bad_request(
            "Weave request is no longer open for resolution",
        ));
    }
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
        SELECT wc.weave_id, 'conflicted', $2
        FROM weave_conflicts wc
        JOIN weave_requests wr ON wr.weave_id = wc.weave_id
        WHERE wc.conflict_id = $1
          AND wr.status IN ('open', 'preview', 'conflicted')
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
        "UPDATE weave_requests SET status = $1, updated_at = now() WHERE weave_id = $2 AND status IN ('open', 'preview', 'conflicted')",
    )
    .bind(status)
    .bind(weave_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE weave_sessions SET status = $1, updated_at = now() WHERE weave_id = $2 AND status IN ('preview', 'conflicted')",
    )
    .bind(status)
    .bind(weave_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

include!("weave_resolution_helpers.rs");
