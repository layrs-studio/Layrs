#[derive(Clone, Debug)]
struct StoredChunkMetadata {
    digest: String,
    raw_size: i64,
    stored_size: i64,
    compression: String,
    object_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ImmutableTreeEntry {
    path: String,
    file_object_id: String,
    size_bytes: i64,
}

async fn lock_layer_paths_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    layer_ids: &[String],
    logical_paths: &[String],
) -> Result<(), ApiError> {
    // Global mutable Layer order: Layer rows -> identity/roles -> policies ->
    // canonical advisory keys (Layer, path) -> artifacts -> heads -> Steps.
    // Callers that skip the leading authorization locks still enter at the
    // advisory-key stage before touching artifacts or heads.
    let logical_paths = sorted_unique_strings(logical_paths);
    for layer_id in sorted_unique_strings(layer_ids) {
        for logical_path in &logical_paths {
            sqlx::query(
                r#"
                SELECT pg_advisory_xact_lock(hashtextextended(
                    jsonb_build_array(
                        'layrs.layer-path-mutation.v1',
                        $1::text,
                        $2::text,
                        $3::text,
                        $4::text
                    )::text,
                    0
                ))
                "#,
            )
            .bind(workspace_id)
            .bind(space_id)
            .bind(&layer_id)
            .bind(logical_path)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

async fn lock_content_id_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    object_id: &str,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(object_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn insert_or_compare_chunk_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    chunk_id: &str,
    encoded_bytes: &[u8],
    compression: &str,
) -> Result<StoredChunkMetadata, ApiError> {
    let chunk_id = validate_object_digest(chunk_id)?;
    let raw_bytes = decode_chunk_bytes(encoded_bytes, compression)?;
    let digest = blake3_digest_for_bytes(&raw_bytes);
    if digest != chunk_id {
        return Err(ApiError::bad_request(
            "chunk_id must match the decoded chunk bytes blake3 digest",
        ));
    }

    lock_content_id_in_tx(tx, &chunk_id).await?;
    let existing = sqlx::query(
        r#"
        SELECT digest, size_bytes, stored_size_bytes, compression, object_key,
               state, content_bytes
        FROM object_chunks
        WHERE chunk_id = $1
        FOR UPDATE
        "#,
    )
    .bind(&chunk_id)
    .fetch_optional(&mut **tx)
    .await?;

    let metadata = if let Some(row) = existing {
        let existing_digest = row.get::<String, _>("digest");
        let existing_raw_size = row.get::<i64, _>("size_bytes");
        let existing_stored_size = row
            .try_get::<i64, _>("stored_size_bytes")
            .unwrap_or(existing_raw_size);
        let existing_compression = row
            .try_get::<String, _>("compression")
            .unwrap_or_else(|_| CHUNK_COMPRESSION_IDENTITY.to_string());
        let existing_state = row.get::<String, _>("state");
        let existing_object_key = row.get::<String, _>("object_key");
        let existing_bytes = row
            .try_get::<Vec<u8>, _>("content_bytes")
            .map_err(|_| ApiError::conflict("existing chunk has no verifiable bytes"))?;
        let existing_raw = decode_chunk_bytes(&existing_bytes, &existing_compression)?;
        let existing_recomputed = blake3_digest_for_bytes(&existing_raw);
        if existing_state != "available"
            || existing_digest != chunk_id
            || existing_recomputed != chunk_id
            || existing_raw_size != existing_raw.len() as i64
            || existing_stored_size != existing_bytes.len() as i64
            || existing_object_key != format!("chunks/global/{chunk_id}")
            || existing_raw != raw_bytes
        {
            return Err(ApiError::conflict(
                "immutable chunk id already exists with different content or metadata",
            ));
        }
        StoredChunkMetadata {
            digest: existing_digest,
            raw_size: existing_raw_size,
            stored_size: existing_stored_size,
            compression: existing_compression,
            object_key: existing_object_key,
        }
    } else {
        let object_key = format!("chunks/global/{chunk_id}");
        let stored_size = encoded_bytes.len() as i64;
        sqlx::query(
            r#"
            INSERT INTO object_chunks
                (chunk_id, workspace_id, space_id, digest, size_bytes,
                 stored_size_bytes, object_key, compression, state,
                 content_bytes, created_by_account_id)
            VALUES
                ($1, $2, $3, $1, $4, $5, $6, $7, 'available', $8, $9)
            "#,
        )
        .bind(&chunk_id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(raw_bytes.len() as i64)
        .bind(stored_size)
        .bind(&object_key)
        .bind(compression)
        .bind(encoded_bytes)
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
        StoredChunkMetadata {
            digest: chunk_id.clone(),
            raw_size: raw_bytes.len() as i64,
            stored_size,
            compression: compression.to_string(),
            object_key,
        }
    };

    mark_chunk_available_for_space_in_tx(tx, workspace_id, space_id, &chunk_id, account_id)
        .await?;
    Ok(metadata)
}

async fn file_digest_for_chunks_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    chunks: &[ChunkDescriptor],
) -> Result<(String, i64), ApiError> {
    let mut hasher = blake3::Hasher::new();
    let mut expected_offset = 0i64;
    for chunk in chunks {
        if chunk.byte_offset != expected_offset {
            return Err(ApiError::bad_request(
                "file object chunks must be contiguous and ordered",
            ));
        }
        let row = sqlx::query(
            r#"
            SELECT digest, size_bytes, compression, content_bytes, state
            FROM object_chunks
            WHERE chunk_id = $1
            "#,
        )
        .bind(&chunk.chunk_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ApiError::bad_request("file object references a missing chunk"))?;
        let compression = row
            .try_get::<String, _>("compression")
            .unwrap_or_else(|_| CHUNK_COMPRESSION_IDENTITY.to_string());
        let encoded = row
            .try_get::<Vec<u8>, _>("content_bytes")
            .map_err(|_| ApiError::conflict("file object chunk has no verifiable bytes"))?;
        let raw = decode_chunk_bytes(&encoded, &compression)?;
        let digest = blake3_digest_for_bytes(&raw);
        if row.get::<String, _>("state") != "available"
            || digest != chunk.chunk_id
            || row.get::<String, _>("digest") != chunk.chunk_id
            || row.get::<i64, _>("size_bytes") != raw.len() as i64
            || chunk.digest != chunk.chunk_id
            || chunk.size_bytes != raw.len() as i64
        {
            return Err(ApiError::conflict(
                "file object references a corrupt or mismatched chunk",
            ));
        }
        hasher.update(&raw);
        expected_offset += raw.len() as i64;
    }
    Ok((format!("blake3:{}", hasher.finalize().to_hex()), expected_offset))
}

async fn compare_file_object_chunks_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    file_object_id: &str,
    chunks: &[ChunkDescriptor],
) -> Result<(), ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT chunk_id, byte_offset, size_bytes
        FROM file_object_chunks
        WHERE file_object_id = $1
        ORDER BY chunk_index ASC
        "#,
    )
    .bind(file_object_id)
    .fetch_all(&mut **tx)
    .await?;
    let matches = rows.len() == chunks.len()
        && rows.iter().zip(chunks).all(|(row, chunk)| {
            row.get::<String, _>("chunk_id") == chunk.chunk_id
                && row.get::<i64, _>("byte_offset") == chunk.byte_offset
                && row.get::<i64, _>("size_bytes") == chunk.size_bytes
        });
    if matches {
        Ok(())
    } else {
        Err(ApiError::conflict(
            "immutable file object id already exists with a different chunk manifest",
        ))
    }
}

fn canonical_tree_id(entries: &[ImmutableTreeEntry]) -> String {
    let mut entries = entries.to_vec();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let mut material = String::new();
    for entry in &entries {
        material.push_str(&entry.path);
        material.push('\0');
        material.push_str(&entry.file_object_id);
        material.push('\0');
        material.push_str(&entry.size_bytes.to_string());
        material.push('\n');
    }
    blake3_digest_for_bytes(material.as_bytes())
}

async fn insert_or_compare_tree_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    space_id: &str,
    account_id: &str,
    expected_tree_id: Option<&str>,
    mut entries: Vec<ImmutableTreeEntry>,
) -> Result<String, ApiError> {
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    if entries
        .windows(2)
        .any(|pair| pair[0].path == pair[1].path)
    {
        return Err(ApiError::bad_request("tree contains duplicate logical paths"));
    }
    let tree_id = canonical_tree_id(&entries);
    if let Some(expected) = expected_tree_id {
        if validate_object_digest(expected)? != tree_id {
            return Err(ApiError::bad_request(
                "treeId does not match the canonical tree manifest digest",
            ));
        }
    }

    lock_content_id_in_tx(tx, &tree_id).await?;
    let existing = sqlx::query(
        "SELECT digest, entry_count FROM tree_objects WHERE tree_id = $1 FOR UPDATE",
    )
    .bind(&tree_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = existing {
        if row.get::<String, _>("digest") != tree_id
            || row.get::<i32, _>("entry_count") != entries.len() as i32
        {
            return Err(ApiError::conflict(
                "immutable tree id already exists with different metadata",
            ));
        }
        let stored = sqlx::query(
            r#"
            SELECT logical_path, entry_kind, file_object_id
            FROM tree_entries
            WHERE tree_id = $1
            ORDER BY logical_path ASC
            "#,
        )
        .bind(&tree_id)
        .fetch_all(&mut **tx)
        .await?;
        let matches = stored.len() == entries.len()
            && stored.iter().zip(&entries).all(|(row, entry)| {
                row.get::<String, _>("logical_path") == entry.path
                    && row.get::<String, _>("entry_kind") == "file"
                    && row.try_get::<String, _>("file_object_id").ok().as_deref()
                        == Some(entry.file_object_id.as_str())
            });
        if !matches {
            return Err(ApiError::conflict(
                "immutable tree id already exists with a different manifest",
            ));
        }
    } else {
        sqlx::query(
            r#"
            INSERT INTO tree_objects
                (tree_id, workspace_id, space_id, digest, entry_count, created_by_account_id)
            VALUES ($1, $2, $3, $1, $4, $5)
            "#,
        )
        .bind(&tree_id)
        .bind(workspace_id)
        .bind(space_id)
        .bind(entries.len() as i32)
        .bind(account_id)
        .execute(&mut **tx)
        .await?;
        for entry in &entries {
            sqlx::query(
                r#"
                INSERT INTO tree_entries
                    (tree_id, logical_path, entry_kind, file_object_id)
                VALUES ($1, $2, 'file', $3)
                "#,
            )
            .bind(&tree_id)
            .bind(&entry.path)
            .bind(&entry.file_object_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    mark_tree_available_for_space_in_tx(tx, workspace_id, space_id, &tree_id, account_id).await?;
    Ok(tree_id)
}
