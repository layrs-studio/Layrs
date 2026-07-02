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
        let root_tree_id = blake3_digest_for_bytes(b"test-root-tree");

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
        let root_tree_id = blake3_digest_for_bytes(b"shared-chunk-second-space-tree");
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
            "SELECT count(*)::bigint FROM space_object_chunks WHERE chunk_id = $1",
        )
        .bind(&chunk_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("availability count");
        assert_eq!(availability_count, 2);
    }

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
        let base_tree_id = blake3_digest_for_bytes(b"step-diff-base-tree");
        let target_tree_id = blake3_digest_for_bytes(b"step-diff-target-tree");
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
        let root_tree_id = blake3_digest_for_bytes(b"secret-redacted-tree");

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

    struct SyncTestFixture {
        pool: PgPool,
        account_id: String,
        workspace_id: String,
        space_id: String,
        layer_id: String,
        bearer_token: String,
    }

    impl SyncTestFixture {
        async fn create() -> Option<Self> {
            let database_url = test_database_url()?;
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&database_url)
                .await
                .expect("test database URL is set but cannot be reached");
            MIGRATOR
                .run(&pool)
                .await
                .expect("test database migrations run");

            let suffix = Uuid::new_v4().simple().to_string();
            let account_id = format!("account_{suffix}");
            let workspace_id = format!("workspace_{suffix}");
            let space_id = format!("space_{suffix}");
            let layer_id = format!("layer_{suffix}");
            let device_id = format!("device_{suffix}");
            let bearer_token = token("desktop_test");

            sqlx::query(
                "INSERT INTO accounts (account_id, email, display_name) VALUES ($1, $2, 'Sync Tester')",
            )
            .bind(&account_id)
            .bind(format!("sync-{suffix}@example.com"))
            .execute(&pool)
            .await
            .expect("account inserted");
            sqlx::query(
                "INSERT INTO desktop_devices (device_id, account_id, display_name, public_key_thumbprint, last_seen_at) VALUES ($1, $2, 'Test Desktop', 'thumbprint-test', now())",
            )
            .bind(&device_id)
            .bind(&account_id)
            .execute(&pool)
            .await
            .expect("device inserted");
            sqlx::query(
                r#"
                INSERT INTO desktop_device_tokens
                    (token_id, device_id, account_id, access_token_digest, refresh_token_digest, expires_at)
                VALUES
                    ($1, $2, $3, $4, $5, now() + interval '1 day')
                "#,
            )
            .bind(prefixed_id("desktop_token"))
            .bind(&device_id)
            .bind(&account_id)
            .bind(digest_secret(&bearer_token))
            .bind(digest_secret(&token("desktop_refresh_test")))
            .execute(&pool)
            .await
            .expect("desktop token inserted");

            let mut tx = pool.begin().await.expect("test tx begins");
            sqlx::query(
                "INSERT INTO workspaces (workspace_id, slug, name, created_by_account_id) VALUES ($1, $2, 'Sync Workspace', $3)",
            )
            .bind(&workspace_id)
            .bind(format!("sync-{suffix}"))
            .bind(&account_id)
            .execute(&mut *tx)
            .await
            .expect("workspace inserted");
            sqlx::query(
                "INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role) VALUES ($1, $2, $3, 'owner')",
            )
            .bind(prefixed_id("membership"))
            .bind(&workspace_id)
            .bind(&account_id)
            .execute(&mut *tx)
            .await
            .expect("membership inserted");
            sqlx::query(
                "INSERT INTO spaces (space_id, workspace_id, slug, name, created_by_account_id) VALUES ($1, $2, 'sync-space', 'Sync Space', $3)",
            )
            .bind(&space_id)
            .bind(&workspace_id)
            .bind(&account_id)
            .execute(&mut *tx)
            .await
            .expect("space inserted");
            sqlx::query(
                "INSERT INTO layers (layer_id, workspace_id, space_id, name, created_by_account_id) VALUES ($1, $2, $3, 'Main', $4)",
            )
            .bind(&layer_id)
            .bind(&workspace_id)
            .bind(&space_id)
            .bind(&account_id)
            .execute(&mut *tx)
            .await
            .expect("layer inserted");
            insert_empty_layer_policy(
                &mut tx,
                &workspace_id,
                &space_id,
                &layer_id,
                Some(&account_id),
            )
            .await
            .expect("policy inserted");
            tx.commit().await.expect("test tx commits");

            Some(Self {
                pool,
                account_id,
                workspace_id,
                space_id,
                layer_id,
                bearer_token,
            })
        }

        fn bearer_headers(&self) -> HeaderMap {
            let mut headers = HeaderMap::new();
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", self.bearer_token))
                    .expect("bearer header is valid"),
            );
            headers
        }
    }

    fn test_database_url() -> Option<String> {
        std::env::var("LAYRS_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("LAYRS_DATABASE_URL"))
            .or_else(|_| std::env::var("DATABASE_URL"))
            .ok()
    }

    fn test_state(pool: PgPool) -> AppState {
        AppState {
            pool,
            config: WebServerConfig {
                addr: "127.0.0.1:0".to_string(),
                studio_url: "http://127.0.0.1:5173".to_string(),
                database_url: "postgres://test".to_string(),
                deployment_id: "test".to_string(),
                cookie_secure: false,
            },
        }
    }

    fn publish_artifact(
        path: &str,
        kind: &str,
        media_type: &str,
        content: Value,
    ) -> PublishArtifactBody {
        PublishArtifactBody {
            id: None,
            artifact_id: None,
            artifact_id_camel: None,
            path: Some(path.to_string()),
            logical_path: None,
            logical_path_camel: None,
            kind: Some(kind.to_string()),
            artifact_type: None,
            media_type: Some(media_type.to_string()),
            media_type_camel: None,
            content: Some(content),
            file_object_id: None,
            file_object_id_camel: None,
            object_id: None,
            object_id_camel: None,
            tree_id: None,
            tree_id_camel: None,
            sha256: None,
            content_hash: None,
            size_bytes: None,
            size_bytes_camel: None,
            chunks: Vec::new(),
            state: None,
            operation: None,
            action: None,
            deleted: None,
        }
    }
