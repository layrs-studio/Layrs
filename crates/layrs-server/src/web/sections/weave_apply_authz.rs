#[derive(Debug)]
struct RevalidatedWeaveAccess {
    source_policy_epoch: i64,
    target_policy_epoch: i64,
}

struct WeavePathAccessCheck<'a> {
    source_layer_id: &'a str,
    target_layer_id: &'a str,
    account_id: &'a str,
    paths: &'a [String],
    source_denied_message: &'static str,
    target_denied_message: &'static str,
}

fn normalize_weave_apply_access_error(error: ApiError, preserve_detail: bool) -> ApiError {
    if !preserve_detail && matches!(error.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND) {
        ApiError::forbidden("Weave request is inaccessible")
    } else {
        error
    }
}

async fn lock_and_revalidate_weave_paths_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    check: WeavePathAccessCheck<'_>,
) -> Result<RevalidatedWeaveAccess, ApiError> {
    let layer_ids = vec![
        check.source_layer_id.to_string(),
        check.target_layer_id.to_string(),
    ];
    let snapshot = load_weave_authorization_snapshot_in_tx(
        tx,
        workspace_id,
        space_id,
        &layer_ids,
        check.account_id,
    )
    .await?;
    revalidate_weave_paths_in_tx(tx, workspace_id, space_id, &snapshot, check).await
}

fn ensure_weave_coarse_layer_access(
    snapshot: &WeaveAuthorizationSnapshot,
    source_layer_id: &str,
    target_layer_id: &str,
    target_denied_message: &'static str,
) -> Result<(), ApiError> {
    snapshot.layer(source_layer_id)?;
    if snapshot.layer(target_layer_id)?.repository_role == RepositoryRole::Reader {
        return Err(ApiError::forbidden(target_denied_message));
    }
    Ok(())
}

async fn revalidate_weave_paths_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    snapshot: &WeaveAuthorizationSnapshot,
    check: WeavePathAccessCheck<'_>,
) -> Result<RevalidatedWeaveAccess, ApiError> {
    ensure_weave_coarse_layer_access(
        snapshot,
        check.source_layer_id,
        check.target_layer_id,
        check.target_denied_message,
    )?;
    let layer_ids = vec![
        check.source_layer_id.to_string(),
        check.target_layer_id.to_string(),
    ];
    let artifacts =
        weave_artifact_ids_for_paths_in_tx(tx, workspace_id, space_id, &layer_ids, check.paths)
            .await?;

    for path in check.paths {
        let (source, target) = weave_path_access_decisions(
            snapshot,
            &artifacts,
            check.source_layer_id,
            check.target_layer_id,
            path,
        )?;
        if !source.can_read {
            return Err(ApiError::forbidden(check.source_denied_message));
        }
        if !target.can_write {
            return Err(ApiError::forbidden(check.target_denied_message));
        }
    }

    Ok(RevalidatedWeaveAccess {
        source_policy_epoch: snapshot.policy_epoch(check.source_layer_id)?,
        target_policy_epoch: snapshot.policy_epoch(check.target_layer_id)?,
    })
}

async fn weave_replay_paths_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    weave_id: &str,
) -> Result<Vec<String>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT changed_paths, path_replays
        FROM weave_step_replays
        WHERE weave_id = $1
        ORDER BY order_index ASC
        FOR UPDATE
        "#,
    )
    .bind(weave_id)
    .fetch_all(&mut **tx)
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
        r#"
        SELECT logical_path
        FROM weave_conflicts
        WHERE weave_id = $1
        ORDER BY logical_path ASC
        FOR SHARE
        "#,
    )
    .bind(weave_id)
    .fetch_all(&mut **tx)
    .await?;
    for path in conflict_paths {
        paths.insert(validate_publish_path(&path)?, ());
    }
    Ok(paths.into_keys().collect())
}

async fn weave_replay_paths_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
    .fetch_all(&mut **tx)
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
    let conflicts = sqlx::query_scalar::<_, String>(
        "SELECT logical_path FROM weave_conflicts WHERE weave_id = $1 ORDER BY logical_path",
    )
    .bind(weave_id)
    .fetch_all(&mut **tx)
    .await?;
    for path in conflicts {
        paths.insert(validate_publish_path(&path)?, ());
    }
    Ok(paths.into_keys().collect())
}
