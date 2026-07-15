use super::*;

#[tokio::test]
async fn step_and_artifact_diff_endpoints_return_lens_window_segments() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let path = "src/main.rs";
    let base_bytes = Bytes::from_static(b"base-line-ABCDEFGHIJ\nbase-two\n");
    let target_bytes = Bytes::from_static(b"next-line-ABCDEFGHIJ\nnext-two\n");
    let base_chunk_id = blake3_digest_for_bytes(&base_bytes);
    let target_chunk_id = blake3_digest_for_bytes(&target_bytes);
    let base_file_object_id = blake3_digest_for_bytes(&base_bytes);
    let target_file_object_id = blake3_digest_for_bytes(&target_bytes);
    let base_tree_id = test_tree_id(&[(path, &base_file_object_id, base_bytes.len())]);
    let target_tree_id = test_tree_id(&[(path, &target_file_object_id, target_bytes.len())]);
    let base_step_id = format!("step_{}", Uuid::new_v4().simple());
    let target_step_id = format!("step_{}", Uuid::new_v4().simple());

    for (chunk_id, bytes) in [
        (base_chunk_id.clone(), base_bytes.clone()),
        (target_chunk_id.clone(), target_bytes.clone()),
    ] {
        let _ = put_space_chunk(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                chunk_id,
            )),
            fixture.bearer_headers(),
            bytes,
        )
        .await
        .expect("chunk upload succeeds");
    }

    for (step_id, tree_id, file_object_id, chunk_id, bytes, base_tree) in [
        (
            base_step_id.as_str(),
            base_tree_id.as_str(),
            base_file_object_id.as_str(),
            base_chunk_id.as_str(),
            base_bytes.len(),
            None,
        ),
        (
            target_step_id.as_str(),
            target_tree_id.as_str(),
            target_file_object_id.as_str(),
            target_chunk_id.as_str(),
            target_bytes.len(),
            Some(base_tree_id.as_str()),
        ),
    ] {
        let mut body = json!({
            "protocol": "layrs.sync.v2",
            "layerId": fixture.layer_id,
            "policyEpoch": 1,
            "idempotencyKey": format!("idem_{}", Uuid::new_v4().simple()),
            "sourceClientId": "test-client",
            "rootTreeId": tree_id,
            "changedPaths": [path],
            "step": {
                "stepId": step_id,
                "layerId": fixture.layer_id,
                "rootTreeId": tree_id,
                "changedPaths": [path]
            },
            "storeObjects": {
                "chunks": [{
                    "chunkId": chunk_id,
                    "digest": chunk_id,
                    "size": bytes
                }],
                "fileObjects": [{
                    "fileObjectId": file_object_id,
                    "digest": file_object_id,
                    "size": bytes,
                    "mediaType": "text/plain",
                    "chunks": [{
                        "chunkId": chunk_id,
                        "digest": chunk_id,
                        "size": bytes,
                        "byteOffset": 0
                    }]
                }],
                "treeObjects": [{
                    "treeId": tree_id,
                    "entries": [{
                        "path": path,
                        "fileObjectId": file_object_id,
                        "size": bytes
                    }]
                }]
            }
        });
        if let Some(base_tree) = base_tree {
            body["baseTreeId"] = json!(base_tree);
            body["step"]["baseTreeId"] = json!(base_tree);
        }
        let _: Json<Value> = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(serde_json::from_value(body).expect("publish body deserializes")),
        )
        .await
        .expect("publish succeeds");
    }

    let Json(steps_payload) = list_layer_steps(
        State(test_state(fixture.pool.clone())),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            fixture.layer_id.clone(),
        )),
        fixture.bearer_headers(),
    )
    .await
    .expect("steps list succeeds");
    assert!(
        steps_payload
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|steps| steps
                .iter()
                .any(|step| step.get("stepId").and_then(Value::as_str)
                    == Some(target_step_id.as_str())))
    );

    let Json(step_payload) = get_layer_step(
        State(test_state(fixture.pool.clone())),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            fixture.layer_id.clone(),
            target_step_id.clone(),
        )),
        fixture.bearer_headers(),
    )
    .await
    .expect("step detail succeeds");
    assert_eq!(
        step_payload
            .pointer("/files/0/action")
            .and_then(Value::as_str),
        Some("modified")
    );

    let Json(step_diff) = get_layer_step_diff(
        State(test_state(fixture.pool.clone())),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            fixture.layer_id.clone(),
            target_step_id.clone(),
        )),
        Query(StepDiffQuery {
            path: None,
            start: Some(0),
            limit: Some(1),
            column_start: Some(0),
            column_start_camel: None,
            column_limit: Some(4),
            column_limit_camel: None,
        }),
        fixture.bearer_headers(),
    )
    .await
    .expect("step diff succeeds");
    let diff_lines = step_diff
        .pointer("/diff/hunks/0/lines")
        .and_then(Value::as_array)
        .expect("step diff includes hunk lines");
    assert_eq!(
        diff_lines[0].get("op").and_then(Value::as_str),
        Some("delete")
    );
    assert_eq!(
        diff_lines[0].get("textSegment").and_then(Value::as_str),
        Some("base")
    );
    assert_eq!(
        diff_lines[1].get("op").and_then(Value::as_str),
        Some("insert")
    );
    assert_eq!(
        diff_lines[1].get("textSegment").and_then(Value::as_str),
        Some("next")
    );
    assert_eq!(
        diff_lines[1].get("textLength").and_then(Value::as_u64),
        Some(20)
    );
    assert_eq!(
        diff_lines[1].get("columnStart").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        diff_lines[1].get("columnEnd").and_then(Value::as_u64),
        Some(4)
    );
    assert_eq!(
        diff_lines[1].get("hasMoreColumns").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        step_diff
            .pointer("/source/runtime/id")
            .and_then(Value::as_str),
        Some("layrs.server.lens-runtime.text")
    );

    let Json(artifacts_payload) = list_layer_artifacts(
        State(test_state(fixture.pool.clone())),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            fixture.layer_id.clone(),
        )),
        fixture.bearer_headers(),
    )
    .await
    .expect("artifacts list succeeds");
    let artifact_id = artifacts_payload
        .get("items")
        .and_then(Value::as_array)
        .and_then(|artifacts| artifacts.first())
        .and_then(|artifact| artifact.get("id"))
        .and_then(Value::as_str)
        .expect("artifact id exists")
        .to_string();
    let Json(artifact_diff) = get_layer_artifact_diff(
        State(test_state(fixture.pool.clone())),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            fixture.layer_id.clone(),
            artifact_id,
        )),
        Query(ArtifactDiffQuery {
            start: Some(0),
            limit: Some(1),
            column_start: Some(0),
            column_start_camel: None,
            column_limit: Some(4),
            column_limit_camel: None,
            base_layer_id: None,
            base_layer_id_camel: None,
        }),
        fixture.bearer_headers(),
    )
    .await
    .expect("artifact diff succeeds");
    assert_eq!(
        artifact_diff
            .pointer("/diff/hunks/0/lines/0/textSegment")
            .and_then(Value::as_str),
        Some("next")
    );
    assert_eq!(
        artifact_diff
            .pointer("/diff/hunks/0/lines/0/hasMoreColumns")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[tokio::test]
async fn receive_does_not_return_redacted_content() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let bytes = Bytes::from_static(b"TOP_SECRET");
    let chunk_id = blake3_digest_for_bytes(&bytes);
    let file_object_id = blake3_digest_for_bytes(&bytes);
    let root_tree_id = test_tree_id(&[(
        "secret/token.txt",
        &file_object_id,
        bytes.len(),
    )]);

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
    .expect("secret chunk upload succeeds");

    let publish_body: SyncPublishBody = serde_json::from_value(json!({
        "protocol": "layrs.sync.v2",
        "layerId": fixture.layer_id,
        "policyEpoch": 1,
        "idempotencyKey": format!("idem_{}", Uuid::new_v4().simple()),
        "sourceClientId": "test-client",
        "rootTreeId": root_tree_id,
        "changedPaths": ["secret/token.txt"],
        "storeObjects": {
            "chunks": [{
                "chunkId": chunk_id,
                "digest": chunk_id,
                "size": bytes.len()
            }],
            "fileObjects": [{
                "fileObjectId": file_object_id,
                "size": bytes.len(),
                "mediaType": "text/plain",
                "chunks": [{
                    "chunkId": chunk_id,
                    "size": bytes.len()
                }]
            }],
            "treeObjects": [{
                "treeId": root_tree_id,
                "entries": [{
                    "path": "secret/token.txt",
                    "fileObjectId": file_object_id,
                    "size": bytes.len()
                }]
            }]
        }
    }))
    .expect("secret chunked publish body deserializes");
    let _ = publish_local_space_sync(
        State(test_state(fixture.pool.clone())),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
        fixture.bearer_headers(),
        Json(publish_body),
    )
    .await
    .expect("secret chunked publish succeeds");

    let policy_id = policy_id_for_layer(
        &fixture.pool,
        &fixture.workspace_id,
        &fixture.space_id,
        &fixture.layer_id,
    )
    .await
    .expect("layer has policy");
    sqlx::query(
        r#"
        INSERT INTO layer_access_policy_rules
            (rule_id, policy_id, path, mode, visibility, read_account_ids)
        VALUES
            ($1, $2, 'secret/**', 'restricted', 'stub', $3)
        "#,
    )
    .bind(prefixed_id("access_rule"))
    .bind(&policy_id)
    .bind(vec!["account_somebody_else".to_string()])
    .execute(&fixture.pool)
    .await
    .expect("restricted rule inserted");

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

    assert_eq!(
        receive_payload
            .get("contents")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        receive_payload
            .get("contentObjects")
            .and_then(|objects| objects.get("chunks"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    let serialized = serde_json::to_string(&receive_payload).expect("receive serializes");
    assert!(!serialized.contains("TOP_SECRET"));
    assert!(
        receive_payload
            .get("artifacts")
            .and_then(Value::as_array)
            .and_then(|artifacts| artifacts.first())
            .and_then(|artifact| artifact.get("access"))
            .and_then(|access| access.get("isRedacted"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    );
}
