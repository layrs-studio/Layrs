async fn publish_artifact_v2_in_tx(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
    body: PublishArtifactBody,
) -> Result<(String, String, Option<String>), ApiError> {
    let logical_path = required_artifact_path(&body)?;
    let artifact_kind =
        normalize_artifact_kind(body.kind.as_deref().or(body.artifact_type.as_deref()))?;
    let provided_artifact_id = body.id.or(body.artifact_id).or(body.artifact_id_camel);
    let file_object_id = body
        .file_object_id
        .or(body.file_object_id_camel)
        .or(body.object_id)
        .or(body.object_id_camel);
    let media_type = body
        .media_type
        .or(body.media_type_camel)
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let explicit_digest = body
        .sha256
        .or(body.content_hash)
        .map(|value| validate_object_digest(&value))
        .transpose()?;
    let explicit_size = body.size_bytes.or(body.size_bytes_camel);

    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        &logical_path,
        provided_artifact_id.as_deref(),
        account_id,
    )
    .await?;
    if !decision.can_write {
        return Err(ApiError::forbidden(format!(
            "path cannot be published: {}",
            decision.reason
        )));
    }
    if existing_redacted_artifact(pool, workspace_id, space_id, layer_id, &logical_path).await? {
        return Err(ApiError::forbidden(
            "path collides with a redacted artifact and cannot be published",
        ));
    }

    let chunks = chunk_descriptors(pool, workspace_id, space_id, body.chunks).await?;
    let final_file_object_id = if chunks.is_empty() {
        let file_object_id = file_object_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ApiError::bad_request("fileObjectId or chunks are required for V2 publish")
            })?;
        ensure_file_object_in_space_in_tx(tx, workspace_id, space_id, &file_object_id).await?;
        file_object_id
    } else {
        let digest = explicit_digest
            .clone()
            .or_else(|| {
                if chunks.len() == 1 {
                    Some(chunks[0].digest.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| hash_chunk_manifest(&chunks));
        let size_bytes =
            explicit_size.unwrap_or_else(|| chunks.iter().map(|chunk| chunk.size_bytes).sum());
        upsert_file_object_in_tx(
            tx,
            workspace_id,
            space_id,
            file_object_id.as_deref(),
            &digest,
            size_bytes,
            &media_type,
            &chunks,
            account_id,
        )
        .await?
    };
    let artifact_id = upsert_artifact_metadata_v2_in_tx(
        tx,
        workspace_id,
        space_id,
        layer_id,
        provided_artifact_id.as_deref(),
        &logical_path,
        artifact_kind,
        account_id,
        Some(&final_file_object_id),
        None,
    )
    .await?;
    let event_id = insert_timeline_event_in_tx(
        tx,
        workspace_id,
        Some(space_id),
        Some(layer_id),
        "artifact.published",
        "Artifact published",
        json!({
            "artifactId": artifact_id,
            "path": logical_path,
            "kind": artifact_kind,
            "fileObjectId": final_file_object_id,
            "contentHash": explicit_digest,
            "digest": final_file_object_id,
            "mediaType": media_type,
            "chunks": chunks.iter().map(|chunk| json!({
                "chunkId": chunk.chunk_id,
                "digest": chunk.digest,
                "sizeBytes": chunk.size_bytes,
                "byteOffset": chunk.byte_offset
            })).collect::<Vec<_>>(),
            "storage": "file_object_chunks_v2"
        }),
    )
    .await?;
    Ok((artifact_id, event_id, Some(final_file_object_id)))
}

async fn delete_artifact_tombstone_in_tx(
    pool: &PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    account_id: &str,
    logical_path: &str,
) -> Result<(String, Value, String), ApiError> {
    let existing = sqlx::query(
        r#"
        SELECT artifact_id, artifact_kind
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = $4
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(logical_path)
    .fetch_optional(pool)
    .await?;
    let existing_artifact_id = existing
        .as_ref()
        .map(|row| row.get::<String, _>("artifact_id"));
    let decision = access_decision_for_path(
        pool,
        workspace_id,
        space_id,
        layer_id,
        logical_path,
        existing_artifact_id.as_deref(),
        account_id,
    )
    .await?;
    if !decision.can_write {
        return Err(ApiError::forbidden(format!(
            "path cannot be deleted: {}",
            decision.reason
        )));
    }
    let artifact_kind = existing
        .as_ref()
        .map(|row| row.get::<String, _>("artifact_kind"))
        .unwrap_or_else(|| "file".to_string());
    let artifact_id = existing_artifact_id.unwrap_or_else(|| prefixed_id("artifact"));
    if existing.is_some() {
        sqlx::query(
            "UPDATE artifacts SET state = 'deleted', current_file_object_id = NULL, current_tree_id = NULL, updated_at = now() WHERE artifact_id = $1",
        )
        .bind(&artifact_id)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO artifacts
                (artifact_id, workspace_id, space_id, layer_id, logical_path, artifact_kind, state, created_by_account_id)
            VALUES
                ($1, $2, $3, $4, $5, $6, 'deleted', $7)
            "#,
        )
        .bind(&artifact_id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(layer_id)
        .bind(logical_path)
        .bind(&artifact_kind)
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
    }
    let event_id = insert_timeline_event_in_tx(
        tx,
        workspace_id,
        Some(space_id),
        Some(layer_id),
        "artifact.deleted",
        "Artifact deleted",
        json!({
            "artifactId": artifact_id,
            "path": logical_path,
            "kind": artifact_kind,
            "state": "deleted",
            "contentAvailable": false,
            "storage": "artifact_state_deleted"
        }),
    )
    .await?;
    let artifact = json!({
        "id": artifact_id,
        "workspaceId": workspace_id,
        "spaceId": space_id,
        "layerId": layer_id,
        "name": logical_path.rsplit('/').next().unwrap_or(logical_path),
        "type": artifact_type(&artifact_kind),
        "summary": "Deleted artifact tombstone",
        "location": logical_path,
        "state": "deleted",
        "proofIds": [],
        "access": {
            "mode": "none",
            "canOpen": false,
            "isRedacted": false,
            "isDeleted": true,
            "reason": "Artifact was deleted"
        }
    });
    Ok((artifact_id, artifact, event_id))
}

async fn chunk_descriptors(
    pool: &PgPool,
    workspace_id: &str,
    space_id: &str,
    chunks: Vec<PublishArtifactChunkBody>,
) -> Result<Vec<ChunkDescriptor>, ApiError> {
    let mut descriptors = Vec::new();
    let mut next_offset = 0;
    for chunk in chunks {
        let chunk_id = chunk
            .id
            .or(chunk.chunk_id)
            .or(chunk.chunk_id_camel)
            .or_else(|| chunk.sha256.as_ref().map(|value| value.to_string()))
            .ok_or_else(|| ApiError::bad_request("chunkId is required"))?;
        let chunk_id = validate_chunk_id(&chunk_id)?;
        let row = sqlx::query(
            r#"
            SELECT oc.digest, oc.size_bytes
            FROM object_chunks oc
            JOIN space_object_chunks soc ON soc.chunk_id = oc.chunk_id
            WHERE soc.workspace_id = $1
              AND soc.space_id = $2
              AND oc.chunk_id = $3
              AND oc.state = 'available'
            "#,
        )
        .bind(workspace_id)
        .bind(space_id)
        .bind(&chunk_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::bad_request(format!("chunk {chunk_id} is not available")))?;
        let digest = row.get::<String, _>("digest");
        if let Some(expected) = chunk.sha256.as_deref() {
            if validate_object_digest(expected)? != digest {
                return Err(ApiError::bad_request(format!(
                    "chunk {chunk_id} digest does not match"
                )));
            }
        }
        let size_bytes = row.get::<i64, _>("size_bytes");
        if let Some(expected_size) = chunk.size_bytes.or(chunk.size_bytes_camel) {
            if expected_size != size_bytes {
                return Err(ApiError::bad_request(format!(
                    "chunk {chunk_id} size does not match"
                )));
            }
        }
        let byte_offset = chunk
            .byte_offset
            .or(chunk.byte_offset_camel)
            .unwrap_or(next_offset);
        descriptors.push(ChunkDescriptor {
            chunk_id,
            digest,
            size_bytes,
            byte_offset,
        });
        next_offset = byte_offset + size_bytes;
    }
    Ok(descriptors)
}

async fn upsert_file_object_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    provided_file_object_id: Option<&str>,
    digest: &str,
    size_bytes: i64,
    media_type: &str,
    chunks: &[ChunkDescriptor],
    account_id: &str,
) -> Result<String, ApiError> {
    let file_object_id = provided_file_object_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| prefixed_id("file_object"));
    let file_object_id = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO file_objects
            (file_object_id, workspace_id, space_id, digest, size_bytes, media_type, chunk_count, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (file_object_id) DO UPDATE SET
            media_type = EXCLUDED.media_type,
            chunk_count = EXCLUDED.chunk_count
        RETURNING file_object_id
        "#,
    )
    .bind(&file_object_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(digest)
    .bind(size_bytes)
    .bind(media_type)
    .bind(chunks.len() as i32)
    .bind(account_id)
    .fetch_one(&mut **tx)
    .await?;
    mark_file_available_for_space_in_tx(tx, workspace_id, space_id, &file_object_id, account_id)
        .await?;
    for (index, chunk) in chunks.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO file_object_chunks
                (file_object_id, chunk_index, chunk_id, byte_offset, size_bytes)
            VALUES
                ($1, $2, $3, $4, $5)
            ON CONFLICT (file_object_id, chunk_index) DO UPDATE SET
                chunk_id = EXCLUDED.chunk_id,
                byte_offset = EXCLUDED.byte_offset,
                size_bytes = EXCLUDED.size_bytes
            "#,
        )
        .bind(&file_object_id)
        .bind(index as i32)
        .bind(&chunk.chunk_id)
        .bind(chunk.byte_offset)
        .bind(chunk.size_bytes)
        .execute(&mut **tx)
        .await?;
    }
    Ok(file_object_id)
}

async fn ensure_file_object_in_space_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM file_objects f
            JOIN space_file_objects sfo ON sfo.file_object_id = f.file_object_id
            WHERE sfo.workspace_id = $1
              AND sfo.space_id = $2
              AND f.file_object_id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(file_object_id)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::bad_request("fileObjectId is not available"))
    }
}

async fn upsert_artifact_metadata_v2_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_id: &str,
    artifact_id: Option<&str>,
    logical_path: &str,
    artifact_kind: &str,
    account_id: &str,
    current_file_object_id: Option<&str>,
    current_tree_id: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(existing_id) = sqlx::query_scalar::<_, String>(
        "SELECT artifact_id FROM artifacts WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = $4",
    )
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(logical_path)
    .fetch_optional(&mut **tx)
    .await?
    {
        sqlx::query(
            r#"
            UPDATE artifacts
            SET artifact_kind = $1,
                state = 'active',
                current_file_object_id = $2,
                current_tree_id = $3,
                updated_at = now()
            WHERE artifact_id = $4
            "#,
        )
        .bind(artifact_kind)
        .bind(current_file_object_id)
        .bind(current_tree_id)
        .bind(&existing_id)
        .execute(&mut **tx)
        .await?;
        return Ok(existing_id);
    }

    let artifact_id = artifact_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| prefixed_id("artifact"));
    sqlx::query(
        r#"
        INSERT INTO artifacts
            (artifact_id, workspace_id, space_id, layer_id, logical_path, artifact_kind,
             state, created_by_account_id, current_file_object_id, current_tree_id)
        VALUES
            ($1, $2, $3, $4, $5, $6, 'active', $7, $8, $9)
        "#,
    )
    .bind(&artifact_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(logical_path)
    .bind(artifact_kind)
    .bind(account_id)
    .bind(current_file_object_id)
    .bind(current_tree_id)
    .execute(&mut **tx)
    .await?;
    Ok(artifact_id)
}

async fn insert_timeline_event_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: Option<&str>,
    layer_id: Option<&str>,
    event_kind: &str,
    title: &str,
    body: Value,
) -> Result<String, ApiError> {
    let event_id = prefixed_id("event");
    sqlx::query(
        r#"
        INSERT INTO timeline_events
            (event_id, workspace_id, space_id, layer_id, event_kind, title, body_json)
        VALUES
            ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(&event_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(layer_id)
    .bind(event_kind)
    .bind(title)
    .bind(body)
    .execute(&mut **tx)
    .await?;
    Ok(event_id)
}

