async fn create_space(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateSpaceBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let space_id = prefixed_id("space");
    let layer_id = prefixed_id("layer");
    let name = required_body_text("name", &body.name)?;
    let slug = slugify(&name);

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO spaces (space_id, workspace_id, slug, name, description, created_by_account_id) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&space_id)
    .bind(&workspace_id)
    .bind(&slug)
    .bind(&name)
    .bind(body.description.as_deref().unwrap_or_default())
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO space_memberships (membership_id, space_id, account_id, role) VALUES ($1, $2, $3, 'admin')",
    )
    .bind(prefixed_id("membership"))
    .bind(&space_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO layers (layer_id, workspace_id, space_id, name, created_by_account_id) VALUES ($1, $2, $3, 'Main', $4)",
    )
    .bind(&layer_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    insert_empty_layer_policy(&mut tx, &workspace_id, &space_id, &layer_id, Some(&user.id)).await?;
    write_timeline_in_tx(
        &mut tx,
        &workspace_id,
        Some(&space_id),
        Some(&layer_id),
        "layer.created",
        "Layer created",
        json!({
            "layerId": layer_id,
            "name": "Main",
            "parentLayerId": Value::Null,
            "source": "space.create"
        }),
    )
    .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "id": space_id,
        "workspaceId": workspace_id,
        "teamId": body.team_id.unwrap_or_default(),
        "name": name,
        "description": body.description.unwrap_or_default(),
        "status": "pending",
        "currentLayerId": layer_id,
        "updatedAt": "2026-06-29T00:00:00Z"
    })))
}

async fn delete_space(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;

    let row = sqlx::query(
        r#"
        SELECT
            s.name,
            (SELECT count(*)::bigint FROM layers l WHERE l.workspace_id = s.workspace_id AND l.space_id = s.space_id) AS layer_count,
            (SELECT count(*)::bigint FROM artifacts a WHERE a.workspace_id = s.workspace_id AND a.space_id = s.space_id) AS artifact_count
        FROM spaces s
        WHERE s.workspace_id = $1 AND s.space_id = $2
        "#,
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .fetch_one(&state.pool)
    .await?;
    let space_name = row.get::<String, _>("name");
    let layer_count = row.get::<i64, _>("layer_count");
    let artifact_count = row.get::<i64, _>("artifact_count");

    let mut tx = state.pool.begin().await?;
    delete_space_storage_in_tx(&mut tx, &workspace_id, &space_id).await?;
    sqlx::query("DELETE FROM spaces WHERE workspace_id = $1 AND space_id = $2")
        .bind(&workspace_id)
        .bind(&space_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    write_audit(
        &state.pool,
        Some(&workspace_id),
        Some(&user.id),
        "space.deleted",
        "space",
        Some(&space_id),
        json!({
            "spaceId": space_id,
            "name": space_name,
            "deletedLayers": layer_count,
            "deletedArtifacts": artifact_count
        }),
    )
    .await?;

    Ok(Json(json!({
        "id": space_id,
        "workspaceId": workspace_id,
        "deleted": true,
        "deletedLayers": layer_count,
        "deletedArtifacts": artifact_count
    })))
}

async fn create_space_from_local(
    State(state): State<AppState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateSpaceFromLocalBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;

    let name = required_body_text("name", &body.name)?;
    let slug = slugify(&name);
    let space_id = prefixed_id("space");
    let main_layer_id = prefixed_id("layer");
    let local_space_id = body
        .local_space_id
        .or(body.local_space_id_camel)
        .unwrap_or_default();
    let mut local_layers = body.layers;
    if local_layers.is_empty() {
        local_layers.push(LocalLayerImportBody {
            local_layer_id: Some("local_layer_main".to_string()),
            local_layer_id_camel: None,
            name: Some("Main".to_string()),
            parent_local_layer_id: None,
            parent_local_layer_id_camel: None,
        });
    }

    let first_local_layer_id = local_layer_import_id(&local_layers[0], "local_layer_main");
    let mut local_to_server = BTreeMap::<String, String>::new();
    local_to_server.insert(first_local_layer_id.clone(), main_layer_id.clone());
    let mut layer_mappings = vec![json!({
        "localLayerId": first_local_layer_id,
        "serverLayerId": main_layer_id,
        "name": "Main"
    })];

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO spaces (space_id, workspace_id, slug, name, description, created_by_account_id) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&space_id)
    .bind(&workspace_id)
    .bind(&slug)
    .bind(&name)
    .bind(body.description.as_deref().unwrap_or_default())
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO space_memberships (membership_id, space_id, account_id, role) VALUES ($1, $2, $3, 'admin')",
    )
    .bind(prefixed_id("membership"))
    .bind(&space_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO layers (layer_id, workspace_id, space_id, name, created_by_account_id) VALUES ($1, $2, $3, 'Main', $4)",
    )
    .bind(&main_layer_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    insert_empty_layer_policy(
        &mut tx,
        &workspace_id,
        &space_id,
        &main_layer_id,
        Some(&user.id),
    )
    .await?;
    write_timeline_in_tx(
        &mut tx,
        &workspace_id,
        Some(&space_id),
        Some(&main_layer_id),
        "layer.created",
        "Layer created",
        json!({
            "layerId": main_layer_id,
            "name": "Main",
            "parentLayerId": Value::Null,
            "source": "local-space.import",
            "localSpaceId": local_space_id
        }),
    )
    .await?;

    for local_layer in local_layers.iter().skip(1) {
        let local_layer_id = local_layer_import_id(local_layer, "local_layer");
        let display_name = local_layer
            .name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Imported Layer")
            .trim()
            .to_string();
        let server_layer_id = prefixed_id("layer");
        let parent_local_id = local_layer
            .parent_local_layer_id
            .clone()
            .or_else(|| local_layer.parent_local_layer_id_camel.clone());
        let parent_layer_id = parent_local_id
            .as_ref()
            .and_then(|id| local_to_server.get(id))
            .cloned()
            .unwrap_or_else(|| main_layer_id.clone());

        sqlx::query(
            "INSERT INTO layers (layer_id, workspace_id, space_id, parent_layer_id, name, created_by_account_id) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&server_layer_id)
        .bind(&workspace_id)
        .bind(&space_id)
        .bind(&parent_layer_id)
        .bind(&display_name)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
        insert_empty_layer_policy(
            &mut tx,
            &workspace_id,
            &space_id,
            &server_layer_id,
            Some(&user.id),
        )
        .await?;
        write_timeline_in_tx(
            &mut tx,
            &workspace_id,
            Some(&space_id),
            Some(&server_layer_id),
            "layer.created",
            "Layer created",
            json!({
                "layerId": server_layer_id,
                "name": display_name,
                "parentLayerId": parent_layer_id,
                "source": "local-space.import",
                "localSpaceId": local_space_id,
                "localLayerId": local_layer_id
            }),
        )
        .await?;

        local_to_server.insert(local_layer_id.clone(), server_layer_id.clone());
        layer_mappings.push(json!({
            "localLayerId": local_layer_id,
            "serverLayerId": server_layer_id,
            "name": display_name,
            "parentServerLayerId": parent_layer_id
        }));
    }

    tx.commit().await?;

    Ok(Json(json!({
        "space": {
            "id": space_id,
            "workspaceId": workspace_id,
            "name": name,
            "description": body.description.unwrap_or_default(),
            "currentLayerId": main_layer_id,
            "status": "linked"
        },
        "localSpaceId": local_space_id,
        "layerMappings": layer_mappings
    })))
}

async fn create_layer(
    State(state): State<AppState>,
    Path((workspace_id, space_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<CreateLayerBody>,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    let layer_id = prefixed_id("layer");
    let name = required_body_text("name", &body.name)?;
    let parent_layer_id = body.parent_id.or(body.parent_layer_id);
    if let Some(parent_layer_id) = &parent_layer_id {
        ensure_layer_in_space(&state.pool, &workspace_id, &space_id, parent_layer_id).await?;
    } else {
        ensure_space_in_workspace(&state.pool, &workspace_id, &space_id).await?;
    }

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO layers (layer_id, workspace_id, space_id, parent_layer_id, name, created_by_account_id) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&layer_id)
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(parent_layer_id.as_deref())
    .bind(&name)
    .bind(&user.id)
    .execute(&mut *tx)
    .await?;
    let policy_id =
        insert_empty_layer_policy(&mut tx, &workspace_id, &space_id, &layer_id, Some(&user.id))
            .await?;
    if let Some(parent_layer_id) = &parent_layer_id {
        inherit_layer_access_rules_in_tx(
            &mut tx,
            &workspace_id,
            &space_id,
            parent_layer_id,
            &policy_id,
        )
        .await?;
    }
    write_timeline_in_tx(
        &mut tx,
        &workspace_id,
        Some(&space_id),
        Some(&layer_id),
        "layer.created",
        "Layer created",
        json!({
            "layerId": layer_id,
            "name": name,
            "parentLayerId": parent_layer_id,
            "source": "layers.create"
        }),
    )
    .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "id": layer_id,
        "spaceId": space_id,
        "parentId": parent_layer_id,
        "name": name,
        "kind": if parent_layer_id.is_some() { "proposal" } else { "base" },
        "status": "active",
        "summary": body.summary.unwrap_or_default(),
        "artifactIds": [],
        "stepIds": [],
        "gateIds": []
    })))
}

async fn delete_layer(
    State(state): State<AppState>,
    Path((workspace_id, space_id, layer_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_principal(&state.pool, &headers).await?;
    ensure_workspace_admin(&state.pool, &workspace_id, &user.id).await?;
    ensure_layer_in_space(&state.pool, &workspace_id, &space_id, &layer_id).await?;

    let layer_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM layers WHERE workspace_id = $1 AND space_id = $2",
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .fetch_one(&state.pool)
    .await?;
    if layer_count <= 1 {
        return Err(ApiError::conflict("space must keep at least one layer"));
    }

    let child_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM layers WHERE workspace_id = $1 AND space_id = $2 AND parent_layer_id = $3",
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&layer_id)
    .fetch_one(&state.pool)
    .await?;
    if child_count > 0 {
        return Err(ApiError::conflict(
            "delete child layers before deleting their parent layer",
        ));
    }

    let layer_name: String = sqlx::query_scalar(
        "SELECT name FROM layers WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(&workspace_id)
    .bind(&space_id)
    .bind(&layer_id)
    .fetch_one(&state.pool)
    .await?;

    let mut tx = state.pool.begin().await?;
    write_timeline_in_tx(
        &mut tx,
        &workspace_id,
        Some(&space_id),
        None,
        "layer.deleted",
        "Layer deleted",
        json!({
            "layerId": layer_id.clone(),
            "name": layer_name.clone(),
            "source": "layers.delete"
        }),
    )
    .await?;
    delete_layer_storage_in_tx(&mut tx, &workspace_id, &space_id, &layer_id).await?;
    sqlx::query("DELETE FROM layers WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3")
        .bind(&workspace_id)
        .bind(&space_id)
        .bind(&layer_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    Ok(Json(json!({
        "id": layer_id,
        "spaceId": space_id,
        "deleted": true
    })))
}

async fn delete_layer_storage_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        DELETE FROM sync_batch_changes
        WHERE sync_batch_id IN (
            SELECT sync_batch_id
            FROM sync_batches
            WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM sync_batches WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM layer_heads WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM layer_states WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE tree_entries
        SET artifact_id = NULL
        WHERE artifact_id IN (
            SELECT artifact_id
            FROM artifacts
            WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE artifacts
        SET current_file_object_id = NULL,
            current_tree_id = NULL,
            updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM artifacts WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn delete_space_storage_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        DELETE FROM sync_batch_changes
        WHERE sync_batch_id IN (
            SELECT sync_batch_id
            FROM sync_batches
            WHERE workspace_id = $1 AND space_id = $2
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM sync_batches WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM layer_heads WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM layer_states WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM space_tree_objects WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        r#"
        UPDATE artifacts
        SET current_file_object_id = NULL,
            current_tree_id = NULL,
            updated_at = now()
        WHERE workspace_id = $1 AND space_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM artifacts WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM space_file_objects WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM space_object_chunks WHERE workspace_id = $1 AND space_id = $2")
        .bind(workspace_id)
        .bind(space_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
