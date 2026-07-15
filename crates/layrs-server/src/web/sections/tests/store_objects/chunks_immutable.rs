use super::*;

#[test]
fn canonical_store_objects_reject_inline_chunk_data() {
    let chunk_id = blake3_digest_for_bytes(b"inline chunk");
    let payload = json!({
        "chunks": [{
            "chunkId": chunk_id,
            "digest": chunk_id,
            "size": 12,
            "encoding": "base64",
            "data": "aW5saW5lIGNodW5r"
        }],
        "fileObjects": [],
        "treeObjects": []
    });

    let error = match serde_json::from_value::<PublishStoreObjectsBody>(payload) {
        Ok(_) => panic!("inline chunk data is not part of canonical storeObjects"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("unknown field"));
}

#[tokio::test]
async fn v2_chunk_publish_receive_and_content_round_trips() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let bytes = Bytes::from_static(b"MERKLE_V2_CONTENT");
    let chunk_id = blake3_digest_for_bytes(&bytes);
    let file_object_id = blake3_digest_for_bytes(&bytes);
    let root_tree_id = test_tree_id(&[("assets/hero.bin", &file_object_id, bytes.len())]);

    let Json(upload_payload) = put_space_chunk(
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
    .expect("chunk upload succeeds before metadata publish");
    assert_eq!(
        upload_payload.get("chunkId").and_then(Value::as_str),
        Some(chunk_id.as_str())
    );

    let publish_body: SyncPublishBody = serde_json::from_value(json!({
        "protocol": "layrs.sync.v2",
        "layerId": fixture.layer_id,
        "policyEpoch": 1,
        "idempotencyKey": format!("idem_{}", Uuid::new_v4().simple()),
        "sourceClientId": "test-client",
        "rootTreeId": root_tree_id,
        "changedPaths": ["assets/hero.bin"],
        "storeObjects": {
            "chunks": [{
                "chunkId": chunk_id,
                "digest": chunk_id,
                "size": bytes.len()
            }],
            "fileObjects": [{
                "fileObjectId": file_object_id,
                "size": bytes.len(),
                "chunks": [{
                    "chunkId": chunk_id,
                    "size": bytes.len()
                }]
            }],
            "treeObjects": [{
                "treeId": root_tree_id,
                "entries": [{
                    "path": "assets/hero.bin",
                    "fileObjectId": file_object_id,
                    "size": bytes.len()
                }]
            }],
            "tombstones": [],
            "deletedPaths": []
        }
    }))
    .expect("canonical storeObjects payload deserializes");

    let Json(publish_payload) = publish_local_space_sync(
        State(test_state(fixture.pool.clone())),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
        fixture.bearer_headers(),
        Json(publish_body),
    )
    .await
    .expect("v2 publish succeeds");

    assert!(
        publish_payload
            .pointer("/layerHead/rootTreeId")
            .and_then(Value::as_str)
            .is_some()
    );
    let artifact_id = publish_payload
        .get("published")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|artifact| artifact.get("id"))
        .and_then(Value::as_str)
        .expect("published artifact id")
        .to_string();

    let Json(receive_payload) = receive_local_space_sync(
        State(test_state(fixture.pool.clone())),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
        fixture.bearer_headers(),
        Json(SyncReceiveBody {
            layer_id: Some(fixture.layer_id.clone()),
            layer_id_camel: None,
            cursor: None,
            limit: Some(200),
        }),
    )
    .await
    .expect("receive succeeds");
    let content_objects = receive_payload
        .get("contentObjects")
        .expect("receive has content objects");
    assert_eq!(
        content_objects
            .get("chunks")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        content_objects
            .pointer("/chunks/0/chunkId")
            .and_then(Value::as_str),
        Some(chunk_id.as_str())
    );
    assert_eq!(
        receive_payload
            .get("contents")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert!(
        !serde_json::to_string(&receive_payload)
            .expect("receive serializes")
            .contains(BASE64.encode(&bytes).as_str())
    );

    let Json(content_payload) = get_layer_artifact_content(
        State(test_state(fixture.pool.clone())),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            fixture.layer_id.clone(),
            artifact_id,
        )),
        fixture.bearer_headers(),
    )
    .await
    .expect("content endpoint assembles chunks");
    assert_eq!(
        content_payload
            .pointer("/content/encoding")
            .and_then(Value::as_str),
        Some("base64")
    );
    assert_eq!(
        content_payload
            .pointer("/content/value")
            .and_then(Value::as_str),
        Some(BASE64.encode(bytes).as_str())
    );
}

#[tokio::test]
async fn same_global_chunk_can_be_published_in_another_space() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let bytes = Bytes::from_static(b"shared chunk bytes\n");
    let chunk_id = blake3_digest_for_bytes(&bytes);
    let file_object_id = blake3_digest_for_bytes(&bytes);
    let root_tree_id = test_tree_id(&[("shared.txt", &file_object_id, bytes.len())]);
    let second_space_id = format!("space_{}", Uuid::new_v4().simple());
    let second_layer_id = format!("layer_{}", Uuid::new_v4().simple());

    sqlx::query(
        "INSERT INTO spaces (space_id, workspace_id, slug, name, created_by_account_id) VALUES ($1, $2, $3, 'Second Sync Space', $4)",
    )
    .bind(&second_space_id)
    .bind(&fixture.workspace_id)
    .bind(format!("second-{}", Uuid::new_v4().simple()))
    .bind(&fixture.account_id)
    .execute(&fixture.pool)
    .await
    .expect("second space inserted");
    sqlx::query(
        "INSERT INTO layers (layer_id, workspace_id, space_id, name, created_by_account_id) VALUES ($1, $2, $3, 'Main', $4)",
    )
    .bind(&second_layer_id)
    .bind(&fixture.workspace_id)
    .bind(&second_space_id)
    .bind(&fixture.account_id)
    .execute(&fixture.pool)
    .await
    .expect("second layer inserted");
    let mut tx = fixture.pool.begin().await.expect("policy tx begins");
    insert_empty_layer_policy(
        &mut tx,
        &fixture.workspace_id,
        &second_space_id,
        &second_layer_id,
        Some(&fixture.account_id),
    )
    .await
    .expect("second policy inserted");
    tx.commit().await.expect("policy tx commits");

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
    .expect("chunk upload into first space succeeds");

    let _ = publish_local_space_sync(
        State(test_state(fixture.pool.clone())),
        Path((fixture.workspace_id.clone(), second_space_id.clone())),
        fixture.bearer_headers(),
        Json(
            serde_json::from_value(json!({
                "protocol": "layrs.sync.v2",
                "layerId": second_layer_id,
                "policyEpoch": 1,
                "idempotencyKey": format!("shared_chunk_{}", Uuid::new_v4().simple()),
                "sourceClientId": "test-client",
                "rootTreeId": root_tree_id,
                "changedPaths": ["shared.txt"],
                "storeObjects": {
                    "chunks": [{
                        "chunkId": chunk_id,
                        "digest": chunk_id,
                        "size": bytes.len(),
                        "rawSize": bytes.len(),
                        "storedSize": bytes.len(),
                        "compression": "identity"
                    }],
                    "fileObjects": [{
                        "fileObjectId": file_object_id,
                        "digest": file_object_id,
                        "size": bytes.len(),
                        "mediaType": "text/plain",
                        "chunks": [{
                            "chunkId": chunk_id,
                            "size": bytes.len(),
                            "rawSize": bytes.len(),
                            "storedSize": bytes.len(),
                            "compression": "identity"
                        }]
                    }],
                    "treeObjects": [{
                        "treeId": root_tree_id,
                        "entries": [{
                            "path": "shared.txt",
                            "fileObjectId": file_object_id
                        }]
                    }]
                }
            }))
            .expect("publish body is valid"),
        ),
    )
    .await
    .expect("second space publish reuses the global chunk");

    let availability_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)::bigint
        FROM space_object_chunks
        WHERE chunk_id = $1
          AND workspace_id = $2
          AND space_id = ANY($3)
        "#,
    )
    .bind(&chunk_id)
    .bind(&fixture.workspace_id)
    .bind(vec![fixture.space_id.clone(), second_space_id])
    .fetch_one(&fixture.pool)
    .await
    .expect("availability count");
    assert_eq!(availability_count, 2);
}
