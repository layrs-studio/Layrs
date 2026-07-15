use super::*;

#[tokio::test]
async fn deleting_one_space_preserves_shared_objects_for_another_space() {
    let Some(first) = SyncTestFixture::create().await else {
        return;
    };
    let Some(second) = SyncTestFixture::create().await else {
        return;
    };
    let bytes = Bytes::from_static(b"shared survives deletion\n");
    let chunk_id = blake3_digest_for_bytes(&bytes);
    let file_object_id = chunk_id.clone();
    let tree_id = test_tree_id(&[("shared.txt", &file_object_id, bytes.len())]);
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
        .expect("shared chunk upload succeeds");
        let _: Json<Value> = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(
                serde_json::from_value(json!({
                    "protocol": "layrs.sync.v2",
                    "layerId": fixture.layer_id,
                    "policyEpoch": 1,
                    "idempotencyKey": format!("shared_delete_{}", Uuid::new_v4().simple()),
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
                .expect("shared object publish body"),
            ),
        )
        .await
        .expect("shared object publish succeeds");
    }

    let _ = delete_space(
        State(test_state(first.pool.clone())),
        Path((first.workspace_id.clone(), first.space_id.clone())),
        first.bearer_headers(),
    )
    .await
    .expect("first Space deletion succeeds");

    assert_eq!(
        file_object_bytes(
            &second.pool,
            &second.workspace_id,
            &second.space_id,
            &file_object_id,
        )
        .await
        .expect("second Space still resolves shared file"),
        bytes
    );
    for (table, id_column, id) in [
        ("space_object_chunks", "chunk_id", chunk_id.as_str()),
        (
            "space_file_objects",
            "file_object_id",
            file_object_id.as_str(),
        ),
        ("space_tree_objects", "tree_id", tree_id.as_str()),
    ] {
        let query = format!(
            "SELECT count(*)::bigint FROM {table} WHERE space_id = $1 AND {id_column} = $2"
        );
        let count: i64 = sqlx::query_scalar(&query)
            .bind(&second.space_id)
            .bind(id)
            .fetch_one(&second.pool)
            .await
            .expect("second Space reference count");
        assert_eq!(count, 1, "missing shared reference in {table}");
    }
}
