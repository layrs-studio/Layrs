struct StoreObjectIndex {
    root_tree_id: Option<String>,
    file_by_path: HashMap<String, StoreFileObject>,
    deleted_paths: Vec<String>,
}

#[derive(Clone, Debug)]
struct StoreFileObject {
    file_object_id: String,
    digest: String,
    size_bytes: i64,
    media_type: Option<String>,
}

async fn load_store_file_object_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    file_object_id: &str,
) -> Result<Option<StoreFileObject>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT f.file_object_id, f.digest, f.size_bytes, f.media_type
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
    .fetch_optional(&mut **tx)
    .await?;

    Ok(row.map(|row| StoreFileObject {
        file_object_id: row.get("file_object_id"),
        digest: row.get("digest"),
        size_bytes: row.get("size_bytes"),
        media_type: row.try_get("media_type").ok(),
    }))
}

async fn upsert_store_objects_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    store_objects: Vec<PublishStoreObjectBody>,
) -> Result<StoreObjectIndex, ApiError> {
    let mut root_tree_id = None;
    let mut file_by_path = HashMap::new();
    let mut file_by_id = HashMap::new();
    let mut tree_entries_by_tree: HashMap<String, Vec<PublishTreeEntryBody>> = HashMap::new();
    let mut deleted_paths = Vec::new();

    for object in store_objects {
        let object_type = object
            .object_type
            .or(object.object_type_camel)
            .unwrap_or_else(|| "file".to_string());
        let raw_object_id = object.object_id.or(object.object_id_camel);
        match object_type.as_str() {
            "tree" => {
                let object_id =
                    validate_object_digest(raw_object_id.as_deref().ok_or_else(|| {
                        ApiError::bad_request("storeObjects.objectId is required")
                    })?)?;
                let tree_id = upsert_tree_object_shell_in_tx(
                    tx,
                    workspace_id,
                    space_id,
                    &object_id,
                    object.size.map(|value| value as i32).unwrap_or_default(),
                    account_id,
                )
                .await?;
                tree_entries_by_tree.insert(tree_id.clone(), object.entries);
                root_tree_id = Some(tree_id);
            }
            "file" => {
                let object_id =
                    validate_object_digest(raw_object_id.as_deref().ok_or_else(|| {
                        ApiError::bad_request("storeObjects.objectId is required")
                    })?)?;
                let path = object
                    .path
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(validate_publish_path)
                    .transpose()?;
                let digest = validate_object_digest(
                    object
                        .hash
                        .or(object.digest)
                        .as_deref()
                        .unwrap_or(&object_id),
                )?;
                let media_type = object.media_type.or(object.media_type_camel);
                let chunks = upsert_store_object_chunks_in_tx(
                    tx,
                    workspace_id,
                    space_id,
                    account_id,
                    object.chunks,
                )
                .await?;
                let size_bytes = object
                    .size_bytes
                    .or(object.size_bytes_camel)
                    .or_else(|| object.size.map(|value| value as i64))
                    .unwrap_or_else(|| chunks.iter().map(|chunk| chunk.size_bytes).sum());
                let file_object_id = upsert_file_object_in_tx(
                    tx,
                    workspace_id,
                    space_id,
                    Some(&object_id),
                    &digest,
                    size_bytes,
                    media_type.as_deref().unwrap_or("application/octet-stream"),
                    &chunks,
                    account_id,
                )
                .await?;
                let file = StoreFileObject {
                    file_object_id: file_object_id.clone(),
                    digest,
                    size_bytes,
                    media_type,
                };
                if let Some(path) = path {
                    file_by_path.insert(path, file.clone());
                }
                file_by_id.insert(file_object_id.clone(), file.clone());
                if file_object_id != object_id {
                    file_by_id.insert(object_id, file);
                }
            }
            "tombstone" => {
                if let Some(path) = object.path {
                    deleted_paths.push(validate_publish_path(path.trim())?);
                }
            }
            _ => {
                return Err(ApiError::bad_request("unsupported store object type"));
            }
        }
    }

    for (tree_id, entries) in tree_entries_by_tree {
        for entry in entries {
            let path = validate_publish_path(entry.path.trim())?;
            let file_object_id = entry
                .file_object_id
                .or(entry.file_object_id_camel)
                .or(entry.object_id)
                .or(entry.object_id_camel)
                .ok_or_else(|| ApiError::bad_request("tree entry fileObjectId is required"))?;
            let file_object_id = validate_object_digest(&file_object_id)?;
            let file = match file_by_id.get(&file_object_id) {
                Some(file) => file.clone(),
                None => load_store_file_object_in_tx(tx, workspace_id, space_id, &file_object_id)
                    .await?
                    .ok_or_else(|| {
                        ApiError::bad_request("tree entry references missing file object")
                    })?,
            };
            sqlx::query(
                r#"
                INSERT INTO tree_entries
                    (tree_id, logical_path, entry_kind, file_object_id)
                VALUES
                    ($1, $2, 'file', $3)
                ON CONFLICT (tree_id, logical_path) DO UPDATE SET
                    file_object_id = EXCLUDED.file_object_id
                "#,
            )
            .bind(&tree_id)
            .bind(&path)
            .bind(&file.file_object_id)
            .execute(&mut **tx)
            .await?;
            file_by_path.insert(path, file);
        }
    }

    Ok(StoreObjectIndex {
        root_tree_id,
        file_by_path,
        deleted_paths,
    })
}

async fn upsert_store_object_chunks_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    chunks: Vec<PublishStoreObjectChunkBody>,
) -> Result<Vec<ChunkDescriptor>, ApiError> {
    let mut descriptors = Vec::new();
    let mut next_offset = 0;
    for chunk in chunks {
        let raw_chunk_id = chunk.chunk_id.or(chunk.chunk_id_camel);
        let expected_digest = chunk.digest.or(chunk.hash);
        let declared_size_bytes = chunk
            .size_bytes
            .or(chunk.size_bytes_camel)
            .or(chunk.raw_size)
            .or_else(|| chunk.size.map(|value| value as i64));
        let byte_offset = chunk
            .byte_offset
            .or(chunk.byte_offset_camel)
            .unwrap_or(next_offset);

        let chunk_id = validate_object_digest(
            raw_chunk_id
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("storeObjects chunkId is required"))?,
        )?;
        if let Some(expected) = expected_digest {
            if validate_object_digest(&expected)? != chunk_id {
                return Err(ApiError::bad_request("chunk digest does not match chunkId"));
            }
        }
        let row = sqlx::query(
            r#"
            SELECT digest, size_bytes, stored_size_bytes, compression
            FROM object_chunks
            WHERE chunk_id = $1
              AND state = 'available'
            "#,
        )
        .bind(&chunk_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            ApiError::bad_request("storeObjects chunk bytes must be uploaded before publish")
        })?;
        mark_chunk_available_for_space_in_tx(tx, workspace_id, space_id, &chunk_id, account_id)
            .await?;
        let stored_digest = row.get::<String, _>("digest");
        let actual_digest = validate_object_digest(&stored_digest)?;
        if actual_digest != chunk_id {
            return Err(ApiError::bad_request(
                "stored chunk digest does not match chunkId",
            ));
        }
        let size_bytes = row.get::<i64, _>("size_bytes");
        if let Some(expected_size) = declared_size_bytes {
            if expected_size != size_bytes {
                return Err(ApiError::bad_request(
                    "chunk size does not match uploaded bytes",
                ));
            }
        }
        if let Some(expected_stored_size) = chunk.stored_size {
            let stored_size = row
                .try_get::<i64, _>("stored_size_bytes")
                .ok()
                .unwrap_or(size_bytes);
            if expected_stored_size != stored_size {
                return Err(ApiError::bad_request(
                    "chunk stored size does not match uploaded bytes",
                ));
            }
        }
        if let Some(expected_compression) = chunk.compression.as_deref() {
            let stored_compression = row
                .try_get::<String, _>("compression")
                .ok()
                .unwrap_or_else(|| CHUNK_COMPRESSION_IDENTITY.to_string());
            if expected_compression != stored_compression {
                return Err(ApiError::bad_request(
                    "chunk compression does not match uploaded bytes",
                ));
            }
        }
        descriptors.push(ChunkDescriptor {
            chunk_id,
            digest: actual_digest,
            size_bytes,
            byte_offset,
        });
        next_offset = byte_offset + size_bytes;
    }
    Ok(descriptors)
}

async fn upsert_tree_object_shell_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    tree_id: &str,
    entry_count: i32,
    account_id: &str,
) -> Result<String, ApiError> {
    sqlx::query(
        r#"
        INSERT INTO tree_objects
            (tree_id, workspace_id, space_id, digest, entry_count, created_by_account_id)
        VALUES
            ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (tree_id) DO UPDATE SET
            entry_count = EXCLUDED.entry_count
        "#,
    )
    .bind(tree_id)
    .bind(workspace_id)
    .bind(space_id)
    .bind(tree_id)
    .bind(entry_count)
    .bind(account_id)
    .execute(&mut **tx)
    .await?;
    mark_tree_available_for_space_in_tx(tx, workspace_id, space_id, tree_id, account_id).await?;
    Ok(tree_id.to_string())
}

fn apply_store_object_to_artifact(
    artifact: &mut PublishArtifactBody,
    store_index: &StoreObjectIndex,
) -> Result<(), ApiError> {
    let path = required_artifact_path(artifact)?;
    if let Some(file) = store_index.file_by_path.get(&path) {
        artifact.file_object_id = Some(file.file_object_id.clone());
        artifact.sha256 = Some(file.digest.clone());
        artifact.size_bytes = Some(file.size_bytes);
        if artifact.media_type.is_none() && artifact.media_type_camel.is_none() {
            artifact.media_type = file.media_type.clone();
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ChunkDescriptor {
    chunk_id: String,
    digest: String,
    size_bytes: i64,
    byte_offset: i64,
}

