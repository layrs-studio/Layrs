#[derive(Clone, Debug)]
struct SourceStepReplay {
    source_step_id: String,
    base_tree_id: Option<String>,
    root_tree_id: Option<String>,
    changed_paths: Vec<String>,
}

async fn source_step_replays_missing_from_target(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    source_layer_id: &str,
    target_layer_id: &str,
) -> Result<Vec<SourceStepReplay>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT s.step_id, s.base_tree_id, s.root_tree_id, s.changed_paths
        FROM layer_steps s
        WHERE s.workspace_id = $1
          AND s.space_id = $2
          AND s.layer_id = $3
          AND s.cleared_at IS NULL
          AND NOT EXISTS (
              SELECT 1
              FROM layer_steps target
              WHERE target.workspace_id = s.workspace_id
                AND target.space_id = s.space_id
                AND target.layer_id = $4
                AND target.cleared_at IS NULL
                AND (
                    target.step_id = s.step_id
                    OR target.step_id = COALESCE(s.origin_step_id, s.step_id)
                    OR target.origin_step_id = COALESCE(s.origin_step_id, s.step_id)
                    OR (
                        target.origin_step_id = s.step_id
                        AND COALESCE(target.origin_layer_id, $3) = $3
                    )
                )
          )
        ORDER BY COALESCE(s.timeline_position, 9223372036854775807),
                 s.captured_at ASC, s.created_at ASC, s.step_id ASC
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(source_layer_id)
    .bind(target_layer_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| SourceStepReplay {
            source_step_id: row.get("step_id"),
            base_tree_id: row.try_get("base_tree_id").ok(),
            root_tree_id: row.try_get("root_tree_id").ok(),
            changed_paths: row.try_get("changed_paths").unwrap_or_default(),
        })
        .collect())
}

async fn create_weave_step_replay_plan_in_tx(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    weave_id: &str,
    workspace_id: &str,
    space_id: &str,
    source_layer_id: &str,
    target_layer_id: &str,
    target_tree_id: Option<&str>,
    account_id: &str,
    source_steps: &[SourceStepReplay],
) -> Result<(Vec<String>, i64), ApiError> {
    let mut planned_steps = Vec::new();
    let mut conflict_count = 0i64;
    let source_layer_name = layer_name_in_tx(tx, workspace_id, space_id, source_layer_id)
        .await?
        .unwrap_or_else(|| source_layer_id.to_string());
    for source_step in source_steps {
        let actual_paths = actual_step_changed_paths(
            pool,
            workspace_id,
            space_id,
            source_step.base_tree_id.as_deref(),
            source_step.root_tree_id.as_deref(),
            &source_step.changed_paths,
        )
        .await?;
        if actual_paths.is_empty() {
            continue;
        }

        let mut path_replays = Vec::new();
        for path in actual_paths {
            if let Some((path_replay, inserted_conflict)) = create_weave_path_replay_in_tx(
                pool,
                tx,
                weave_id,
                workspace_id,
                space_id,
                source_layer_id,
                target_layer_id,
                source_step.base_tree_id.as_deref(),
                source_step.root_tree_id.as_deref(),
                target_tree_id,
                account_id,
                &path,
            )
            .await?
            {
                if inserted_conflict {
                    conflict_count += 1;
                }
                path_replays.push(path_replay);
            }
        }
        if path_replays.is_empty() {
            continue;
        }

        let changed_paths = path_replays
            .iter()
            .filter_map(|value| value.get("path").and_then(Value::as_str).map(str::to_string))
            .collect::<Vec<_>>();
        let replay_status = if path_replays.iter().any(|value| {
            matches!(
                value.get("reconcileStatus").and_then(Value::as_str),
                Some("conflicted" | "unsupported")
            )
        }) {
            "conflicted"
        } else {
            "planned"
        };
        sqlx::query(
            r#"
            INSERT INTO weave_step_replays
                (replay_id, weave_id, order_index, source_step_id, origin_layer_id,
                 origin_layer_name, origin_step_id, target_before_tree_id, incoming_tree_id,
                 source_base_tree_id, source_root_tree_id, changed_paths, path_replays, status)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            "#,
        )
        .bind(prefixed_id("weave_replay"))
        .bind(weave_id)
        .bind(planned_steps.len() as i32)
        .bind(&source_step.source_step_id)
        .bind(source_layer_id)
        .bind(&source_layer_name)
        .bind(&source_step.source_step_id)
        .bind(target_tree_id)
        .bind(source_step.root_tree_id.as_deref())
        .bind(source_step.base_tree_id.as_deref())
        .bind(source_step.root_tree_id.as_deref())
        .bind(&changed_paths)
        .bind(Value::Array(path_replays))
        .bind(replay_status)
        .execute(&mut **tx)
        .await?;
        planned_steps.push(source_step.source_step_id.clone());
    }
    Ok((planned_steps, conflict_count))
}

async fn create_weave_path_replay_in_tx(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    weave_id: &str,
    workspace_id: &str,
    space_id: &str,
    source_layer_id: &str,
    target_layer_id: &str,
    source_base_tree_id: Option<&str>,
    source_root_tree_id: Option<&str>,
    target_tree_id: Option<&str>,
    account_id: &str,
    path: &str,
) -> Result<Option<(Value, bool)>, ApiError> {
    let base_ref =
        tree_file_ref_for_path(pool, workspace_id, space_id, source_base_tree_id, path).await?;
    let ours_ref = tree_file_ref_for_path(pool, workspace_id, space_id, target_tree_id, path).await?;
    let theirs_ref =
        tree_file_ref_for_path(pool, workspace_id, space_id, source_root_tree_id, path).await?;
    if same_tree_file_ref(base_ref.as_ref(), theirs_ref.as_ref()) {
        return Ok(None);
    }

    let base = weave_side_from_tree_file_ref(pool, workspace_id, space_id, base_ref.as_ref()).await?;
    let ours = weave_side_from_tree_file_ref(pool, workspace_id, space_id, ours_ref.as_ref()).await?;
    let theirs =
        weave_side_from_tree_file_ref(pool, workspace_id, space_id, theirs_ref.as_ref()).await?;
    let media_type = conflict_media_type_for_path(path, &base, &ours, &theirs);
    let lens_id = lens_id_for_path_and_media_type(path, &media_type).to_string();
    let row = WeaveConflictRow {
        conflict_id: prefixed_id("weave_conflict"),
        logical_path: path.to_string(),
        lens_id: lens_id.clone(),
        status: "open".to_string(),
        message: String::new(),
        base_file_object_id: base_ref.as_ref().map(|file| file.file_object_id.clone()),
        ours_file_object_id: ours_ref.as_ref().map(|file| file.file_object_id.clone()),
        theirs_file_object_id: theirs_ref.as_ref().map(|file| file.file_object_id.clone()),
        resolved_file_object_id: None,
        source_layer_id: source_layer_id.to_string(),
        target_layer_id: target_layer_id.to_string(),
    };
    let reconcile = reconcile_weave_conflict(&row, &base, &ours, &theirs);
    let mut inserted_conflict = false;
    let planned_file_object_id = match reconcile.status {
        LensReconcileResultStatus::AutoResolved => {
            let resolved = reconcile
                .resolved
                .as_ref()
                .ok_or_else(|| ApiError::internal("auto-resolved Weave path has no content"))?;
            file_object_id_for_resolved_content_in_tx(
                tx,
                workspace_id,
                space_id,
                account_id,
                path,
                &media_type,
                resolved,
                base_ref.as_ref(),
                &base,
                ours_ref.as_ref(),
                &ours,
                theirs_ref.as_ref(),
                &theirs,
            )
            .await?
        }
        LensReconcileResultStatus::Conflicted | LensReconcileResultStatus::Unsupported => {
            let result = sqlx::query(
                r#"
                INSERT INTO weave_conflicts
                    (conflict_id, weave_id, logical_path, lens_id, status, message,
                     base_file_object_id, ours_file_object_id, theirs_file_object_id)
                VALUES
                    ($1, $2, $3, $4, 'open', $5, $6, $7, $8)
                ON CONFLICT (weave_id, logical_path) DO NOTHING
                "#,
            )
            .bind(&row.conflict_id)
            .bind(weave_id)
            .bind(path)
            .bind(&lens_id)
            .bind(&reconcile.summary)
            .bind(row.base_file_object_id.as_deref())
            .bind(row.ours_file_object_id.as_deref())
            .bind(row.theirs_file_object_id.as_deref())
            .execute(&mut **tx)
            .await?;
            inserted_conflict = result.rows_affected() > 0;
            theirs_ref.as_ref().map(|file| file.file_object_id.clone())
        }
    };

    Ok(Some((
        json!({
            "path": path,
            "action": if planned_file_object_id.is_some() { "upsert" } else { "delete" },
            "lensId": lens_id,
            "reconcileStatus": reconcile.status.as_str(),
            "baseFileObjectId": base_ref.as_ref().map(|file| file.file_object_id.as_str()),
            "targetFileObjectId": ours_ref.as_ref().map(|file| file.file_object_id.as_str()),
            "sourceFileObjectId": theirs_ref.as_ref().map(|file| file.file_object_id.as_str()),
            "plannedFileObjectId": planned_file_object_id
        }),
        inserted_conflict,
    )))
}

async fn actual_step_changed_paths(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    base_tree_id: Option<&str>,
    root_tree_id: Option<&str>,
    candidate_paths: &[String],
) -> Result<Vec<String>, ApiError> {
    let candidates = if candidate_paths.is_empty() {
        tree_union_file_paths(pool, workspace_id, space_id, base_tree_id, root_tree_id).await?
    } else {
        unique_strings(candidate_paths)
            .into_iter()
            .map(|path| validate_publish_path(&path))
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut changed = Vec::new();
    for path in candidates {
        let base = tree_file_ref_for_path(pool, workspace_id, space_id, base_tree_id, &path).await?;
        let root = tree_file_ref_for_path(pool, workspace_id, space_id, root_tree_id, &path).await?;
        if !same_tree_file_ref(base.as_ref(), root.as_ref()) {
            changed.push(path);
        }
    }
    Ok(changed)
}

async fn tree_union_file_paths(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    left_tree_id: Option<&str>,
    right_tree_id: Option<&str>,
) -> Result<Vec<String>, ApiError> {
    let mut paths = BTreeMap::new();
    for tree_id in [left_tree_id, right_tree_id].into_iter().flatten() {
        let rows = sqlx::query(
            r#"
            SELECT te.logical_path
            FROM tree_entries te
            JOIN space_tree_objects sto ON sto.tree_id = te.tree_id
            WHERE te.tree_id = $1
              AND sto.workspace_id = $2
              AND sto.space_id = $3
              AND te.entry_kind = 'file'
            ORDER BY te.logical_path ASC
            "#,
        )
        .bind(tree_id)
        .bind(workspace_id)
        .bind(space_id)
        .fetch_all(pool)
        .await?;
        for row in rows {
            paths.insert(row.get::<String, _>("logical_path"), ());
        }
    }
    Ok(paths.into_keys().collect())
}

async fn apply_weave_step_replays_in_tx(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    weave_id: &str,
    source_layer_id: &str,
    target_layer_id: &str,
    policy_epoch: i64,
    account_id: &str,
) -> Result<Vec<String>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT replay_id, source_step_id, changed_paths, path_replays
        FROM weave_step_replays
        WHERE weave_id = $1
        ORDER BY order_index ASC
        FOR UPDATE
        "#,
    )
    .bind(weave_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut applied_steps = Vec::new();
    if rows.is_empty() {
        return Ok(applied_steps);
    }

    let source_layer_name = layer_name_in_tx(tx, workspace_id, space_id, source_layer_id)
        .await?
        .unwrap_or_else(|| source_layer_id.to_string());
    let mut current_tree_id =
        current_layer_head_tree_id_for_update_in_tx(tx, workspace_id, space_id, target_layer_id)
            .await?;
    let mut parent_step_id =
        latest_layer_step_id_in_tx(tx, workspace_id, space_id, target_layer_id).await?;
    let mut next_timeline_position =
        max_layer_timeline_position_in_tx(tx, workspace_id, space_id, target_layer_id).await? + 1;
    let resolved_conflicts = resolved_conflict_file_by_path_in_tx(tx, weave_id).await?;

    for row in rows {
        let replay_id = row.get::<String, _>("replay_id");
        let source_step_id = row.get::<String, _>("source_step_id");
        let path_replays = row.get::<Value, _>("path_replays");
        let path_replays = path_replays
            .as_array()
            .ok_or_else(|| ApiError::internal("Weave replay path plan is invalid"))?;
        let base_tree_id = current_tree_id.clone();
        let mut event_ids = Vec::new();
        let mut changed_paths = Vec::new();

        for path_replay in path_replays {
            let path = path_replay_path(path_replay)?;
            let planned_file_object_id = match resolved_conflicts.get(&path) {
                Some(resolved) => resolved.clone(),
                None if path_replay_requires_resolution(path_replay) => {
                    return Err(ApiError::bad_request(format!(
                        "Weave path {path} requires a resolved conflict before apply"
                    )));
                }
                None => path_replay_planned_file_object_id(path_replay),
            };
            if let Some(file_object_id) = planned_file_object_id {
                event_ids.push(
                    publish_weave_file_to_target_in_tx(
                        pool,
                        tx,
                        workspace_id,
                        space_id,
                        target_layer_id,
                        account_id,
                        &path,
                        &file_object_id,
                    )
                    .await?,
                );
            } else {
                let (_, _, event_id) = delete_artifact_tombstone_in_tx(
                    pool,
                    tx,
                    workspace_id,
                    space_id,
                    target_layer_id,
                    account_id,
                    &path,
                )
                .await?;
                event_ids.push(event_id);
            }
            changed_paths.push(path);
        }

        let root_tree_id =
            rebuild_layer_tree_in_tx(tx, workspace_id, space_id, target_layer_id, account_id)
                .await?;
        let server_cursor = event_ids.last().map(String::as_str);
        advance_layer_head_in_tx(
            tx,
            workspace_id,
            space_id,
            target_layer_id,
            root_tree_id.as_deref(),
            policy_epoch,
            server_cursor,
            account_id,
        )
        .await?;
        let step = SyncStepBody {
            step_id: None,
            parent_step_id: parent_step_id.clone(),
            base_layer_id: Some(target_layer_id.to_string()),
            base_tree_id: base_tree_id.clone(),
            root_tree_id: root_tree_id.clone(),
            changed_paths: changed_paths.clone(),
            timeline_position: Some(next_timeline_position),
            origin_layer_id: Some(source_layer_id.to_string()),
            origin_layer_name: Some(source_layer_name.clone()),
            origin_step_id: Some(source_step_id.clone()),
            step_kind: Some("woven".to_string()),
            captured_at_unix: None,
        };
        let applied_step_id = insert_layer_step_in_tx(
            tx,
            workspace_id,
            space_id,
            target_layer_id,
            Some(&step),
            base_tree_id.as_deref(),
            root_tree_id.as_deref(),
            &changed_paths,
            Some("weave"),
            None,
            account_id,
        )
        .await?;
        sqlx::query(
            "UPDATE weave_step_replays SET status = 'applied', target_step_id = $2, target_after_tree_id = $3, updated_at = now() WHERE replay_id = $1",
        )
        .bind(&replay_id)
        .bind(&applied_step_id)
        .bind(root_tree_id.as_deref())
        .execute(&mut **tx)
        .await?;
        current_tree_id = root_tree_id;
        parent_step_id = Some(applied_step_id.clone());
        next_timeline_position += 1;
        applied_steps.push(applied_step_id);
    }
    Ok(applied_steps)
}

async fn publish_weave_file_to_target_in_tx(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    target_layer_id: &str,
    account_id: &str,
    path: &str,
    file_object_id: &str,
) -> Result<String, ApiError> {
    let file = load_store_file_object_in_tx(tx, workspace_id, space_id, file_object_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Weave replay file object not found"))?;
    let media_type = file
        .media_type
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let (_, event_id, _) = publish_artifact_v2_in_tx(
        pool,
        tx,
        workspace_id,
        space_id,
        target_layer_id,
        account_id,
        PublishArtifactBody {
            id: None,
            artifact_id: None,
            artifact_id_camel: None,
            path: Some(path.to_string()),
            logical_path: None,
            logical_path_camel: None,
            kind: Some(artifact_kind_for_weave_file(path, &media_type).to_string()),
            artifact_type: None,
            media_type: Some(media_type),
            media_type_camel: None,
            content: None,
            file_object_id: Some(file.file_object_id),
            file_object_id_camel: None,
            object_id: None,
            object_id_camel: None,
            tree_id: None,
            tree_id_camel: None,
            sha256: None,
            content_hash: None,
            size_bytes: Some(file.size_bytes),
            size_bytes_camel: None,
            chunks: Vec::new(),
            state: None,
            operation: None,
            action: None,
            deleted: None,
        },
    )
    .await?;
    Ok(event_id)
}

async fn file_object_id_for_resolved_content_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    path: &str,
    media_type: &str,
    content: &LensReconcileContent,
    base_ref: Option<&TreeFileRef>,
    base: &WeaveConflictSide,
    ours_ref: Option<&TreeFileRef>,
    ours: &WeaveConflictSide,
    theirs_ref: Option<&TreeFileRef>,
    theirs: &WeaveConflictSide,
) -> Result<Option<String>, ApiError> {
    if !content.exists {
        return Ok(None);
    }
    if content_matches_side(content, base) {
        return Ok(base_ref.map(|file| file.file_object_id.clone()));
    }
    if content_matches_side(content, ours) {
        return Ok(ours_ref.map(|file| file.file_object_id.clone()));
    }
    if content_matches_side(content, theirs) {
        return Ok(theirs_ref.map(|file| file.file_object_id.clone()));
    }
    Ok(Some(
        write_resolved_file_object_in_tx(
            tx,
            workspace_id,
            space_id,
            account_id,
            &content.bytes,
            preview_media_type_for_path(path, media_type),
        )
        .await?,
    ))
}

async fn weave_side_from_tree_file_ref(
    pool: &PgPool,
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
        bytes: file_object_bytes(pool, workspace_id, space_id, &file.file_object_id).await?,
    })
}

async fn load_weave_request_row_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(weave_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::not_found("weave request not found"))
}

async fn current_layer_head_tree_id(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT root_tree_id
        FROM layer_heads
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(pool)
    .await
    .map(|value| value.flatten())
    .map_err(ApiError::from)
}

async fn current_layer_head_tree_id_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT root_tree_id
        FROM layer_heads
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(&mut **tx)
    .await
    .map(|value| value.flatten())
    .map_err(ApiError::from)
}

async fn latest_layer_step_id(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT step_id
        FROM layer_steps
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
          AND cleared_at IS NULL
        ORDER BY COALESCE(timeline_position, 9223372036854775807) DESC,
                 captured_at DESC, created_at DESC, step_id DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)
}

async fn latest_layer_step_id_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT step_id
        FROM layer_steps
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
          AND cleared_at IS NULL
        ORDER BY COALESCE(timeline_position, 9223372036854775807) DESC,
                 captured_at DESC, created_at DESC, step_id DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(ApiError::from)
}

async fn max_layer_timeline_position_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<i64, ApiError> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        r#"
        SELECT MAX(timeline_position)
        FROM layer_steps
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
          AND cleared_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_one(&mut **tx)
    .await?
    .unwrap_or(-1))
}

async fn layer_name_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar(
        r#"
        SELECT name
        FROM layers
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(ApiError::from)
}

async fn resolved_conflict_file_by_path_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    weave_id: &str,
) -> Result<HashMap<String, Option<String>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT logical_path, resolved_file_object_id
        FROM weave_conflicts
        WHERE weave_id = $1 AND status = 'resolved'
        "#,
    )
    .bind(weave_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.get::<String, _>("logical_path"),
                row.try_get::<String, _>("resolved_file_object_id").ok(),
            )
        })
        .collect())
}

fn path_replay_path(value: &Value) -> Result<String, ApiError> {
    value
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ApiError::internal("Weave replay path is missing"))
}

fn path_replay_planned_file_object_id(value: &Value) -> Option<String> {
    value
        .get("plannedFileObjectId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn path_replay_requires_resolution(value: &Value) -> bool {
    matches!(
        value.get("reconcileStatus").and_then(Value::as_str),
        Some("conflicted" | "unsupported")
    )
}

fn same_tree_file_ref(left: Option<&TreeFileRef>, right: Option<&TreeFileRef>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.file_object_id == right.file_object_id,
        _ => false,
    }
}

fn content_matches_side(content: &LensReconcileContent, side: &WeaveConflictSide) -> bool {
    content.exists == side.exists && (!content.exists || content.bytes == side.bytes)
}

fn conflict_media_type_for_path(
    path: &str,
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
    preview_media_type_for_path(path, stored).to_string()
}

fn artifact_kind_for_weave_file(path: &str, media_type: &str) -> &'static str {
    if media_type.starts_with("image/") {
        "image"
    } else if is_text_path(path) {
        "note"
    } else {
        "file"
    }
}
