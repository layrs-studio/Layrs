use super::*;

#[tokio::test]
async fn cross_space_object_ids_are_immutable() {
    let Some(first) = SyncTestFixture::create().await else {
        return;
    };
    let Some(second) = SyncTestFixture::create().await else {
        return;
    };
    let bytes = Bytes::from_static(b"immutable shared bytes\n");
    let chunk_id = blake3_digest_for_bytes(&bytes);
    let file_object_id = chunk_id.clone();
    let tree_id = test_tree_id(&[("shared.txt", &file_object_id, bytes.len())]);

    let _ = put_space_chunk(
        State(test_state(first.pool.clone())),
        Path((
            first.workspace_id.clone(),
            first.space_id.clone(),
            chunk_id.clone(),
        )),
        first.bearer_headers(),
        bytes.clone(),
    )
    .await
    .expect("first Space stores immutable chunk");
    let _: Json<Value> = publish_local_space_sync(
        State(test_state(first.pool.clone())),
        Path((first.workspace_id.clone(), first.space_id.clone())),
        first.bearer_headers(),
        Json(
            serde_json::from_value(json!({
                "protocol": "layrs.sync.v2",
                "layerId": first.layer_id,
                "policyEpoch": 1,
                "idempotencyKey": format!("immutable_first_{}", Uuid::new_v4().simple()),
                "rootTreeId": tree_id,
                "changedPaths": ["shared.txt"],
                "storeObjects": {
                    "chunks": [{ "chunkId": chunk_id, "size": bytes.len() }],
                    "fileObjects": [{
                        "fileObjectId": file_object_id,
                        "digest": file_object_id,
                        "size": bytes.len(),
                        "mediaType": "text/plain",
                        "chunks": [{ "chunkId": chunk_id, "size": bytes.len() }]
                    }],
                    "treeObjects": [{
                        "treeId": tree_id,
                        "entries": [{ "path": "shared.txt", "fileObjectId": file_object_id }]
                    }]
                }
            }))
            .expect("first immutable publish body"),
        ),
    )
    .await
    .expect("first Space publishes immutable objects");

    let tampered = Bytes::from_static(b"different bytes\n");
    let error = put_space_chunk(
        State(test_state(second.pool.clone())),
        Path((
            second.workspace_id.clone(),
            second.space_id.clone(),
            chunk_id.clone(),
        )),
        second.bearer_headers(),
        tampered,
    )
    .await
    .expect_err("another Space cannot replace bytes under an existing chunk id");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);

    let compressed = Bytes::from(
        zstd::stream::encode_all(std::io::Cursor::new(bytes.as_ref()), 3)
            .expect("zstd compression succeeds"),
    );
    let mut alternate_encoding_headers = second.bearer_headers();
    alternate_encoding_headers.insert(
        "x-layrs-chunk-compression",
        HeaderValue::from_static(CHUNK_COMPRESSION_ZSTD),
    );
    alternate_encoding_headers.insert(
        "x-layrs-raw-size",
        HeaderValue::from_str(&bytes.len().to_string()).expect("raw size header"),
    );
    alternate_encoding_headers.insert(
        "x-layrs-stored-size",
        HeaderValue::from_str(&compressed.len().to_string()).expect("stored size header"),
    );
    let Json(reused_chunk) = put_space_chunk(
        State(test_state(second.pool.clone())),
        Path((
            second.workspace_id.clone(),
            second.space_id.clone(),
            chunk_id.clone(),
        )),
        alternate_encoding_headers,
        compressed,
    )
    .await
    .expect("equivalent raw content can reuse an immutable chunk across encodings");
    assert_eq!(
        reused_chunk.get("compression").and_then(Value::as_str),
        Some(CHUNK_COMPRESSION_IDENTITY),
        "the stored representation remains unchanged"
    );

    let _ = put_space_chunk(
        State(test_state(second.pool.clone())),
        Path((
            second.workspace_id.clone(),
            second.space_id.clone(),
            chunk_id.clone(),
        )),
        second.bearer_headers(),
        bytes.clone(),
    )
    .await
    .expect("identical immutable chunk can be shared");
    let conflicting_media = publish_local_space_sync(
        State(test_state(second.pool.clone())),
        Path((second.workspace_id.clone(), second.space_id.clone())),
        second.bearer_headers(),
        Json(
            serde_json::from_value(json!({
                "protocol": "layrs.sync.v2",
                "layerId": second.layer_id,
                "policyEpoch": 1,
                "idempotencyKey": format!("immutable_second_{}", Uuid::new_v4().simple()),
                "rootTreeId": tree_id,
                "changedPaths": ["shared.txt"],
                "storeObjects": {
                    "chunks": [{ "chunkId": chunk_id, "size": bytes.len() }],
                    "fileObjects": [{
                        "fileObjectId": file_object_id,
                        "digest": file_object_id,
                        "size": bytes.len(),
                        "mediaType": "application/octet-stream",
                        "chunks": [{ "chunkId": chunk_id, "size": bytes.len() }]
                    }],
                    "treeObjects": [{
                        "treeId": tree_id,
                        "entries": [{ "path": "shared.txt", "fileObjectId": file_object_id }]
                    }]
                }
            }))
            .expect("conflicting immutable publish body"),
        ),
    )
    .await
    .expect_err("file object metadata cannot be mutated cross-Space");
    assert_eq!(conflicting_media.status, StatusCode::CONFLICT);

    let stored = sqlx::query(
        "SELECT media_type, size_bytes FROM file_objects WHERE file_object_id = $1",
    )
    .bind(&file_object_id)
    .fetch_one(&first.pool)
    .await
    .expect("original immutable file object remains");
    assert_eq!(stored.get::<String, _>("media_type"), "text/plain");
    assert_eq!(stored.get::<i64, _>("size_bytes"), bytes.len() as i64);
    let second_file_reference: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM space_file_objects WHERE space_id = $1 AND file_object_id = $2",
    )
    .bind(&second.space_id)
    .bind(&file_object_id)
    .fetch_one(&second.pool)
    .await
    .expect("second Space reference count");
    assert_eq!(second_file_reference, 0);
}

#[tokio::test]
async fn cross_space_tree_manifest_cannot_mutate() {
    let Some(first) = SyncTestFixture::create().await else {
        return;
    };
    let Some(second) = SyncTestFixture::create().await else {
        return;
    };
    let bytes = Bytes::from_static(b"tree immutable bytes\n");
    let chunk_id = blake3_digest_for_bytes(&bytes);
    let file_object_id = chunk_id.clone();
    let tree_id = test_tree_id(&[("original.txt", &file_object_id, bytes.len())]);
    for fixture in [&first, &second] {
        let _ = put_space_chunk(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                chunk_id.clone(),
            )),
            fixture.bearer_headers(),
            bytes.clone(),
        )
        .await
        .expect("identical chunk upload succeeds");
    }
    let publish = |fixture: &SyncTestFixture, path: &str| {
        serde_json::from_value::<SyncPublishBody>(json!({
            "protocol": "layrs.sync.v2",
            "layerId": fixture.layer_id,
            "policyEpoch": 1,
            "idempotencyKey": format!("immutable_tree_{}", Uuid::new_v4().simple()),
            "rootTreeId": tree_id,
            "changedPaths": [path],
            "storeObjects": {
                "chunks": [{ "chunkId": chunk_id, "size": bytes.len() }],
                "fileObjects": [{
                    "fileObjectId": file_object_id,
                    "digest": file_object_id,
                    "size": bytes.len(),
                    "mediaType": "text/plain",
                    "chunks": [{ "chunkId": chunk_id, "size": bytes.len() }]
                }],
                "treeObjects": [{
                    "treeId": tree_id,
                    "entries": [{ "path": path, "fileObjectId": file_object_id }]
                }]
            }
        }))
        .expect("immutable tree publish body")
    };
    let _: Json<Value> = publish_local_space_sync(
        State(test_state(first.pool.clone())),
        Path((first.workspace_id.clone(), first.space_id.clone())),
        first.bearer_headers(),
        Json(publish(&first, "original.txt")),
    )
    .await
    .expect("first tree publish succeeds");
    let error = publish_local_space_sync(
        State(test_state(second.pool.clone())),
        Path((second.workspace_id.clone(), second.space_id.clone())),
        second.bearer_headers(),
        Json(publish(&second, "renamed.txt")),
    )
    .await
    .expect_err("same tree id cannot name a different manifest");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    let second_tree_reference: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM space_tree_objects WHERE space_id = $1 AND tree_id = $2",
    )
    .bind(&second.space_id)
    .bind(&tree_id)
    .fetch_one(&second.pool)
    .await
    .expect("second tree reference count");
    assert_eq!(second_tree_reference, 0);
}
