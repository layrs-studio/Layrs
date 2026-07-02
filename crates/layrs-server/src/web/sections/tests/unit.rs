    #[test]
    fn digest_does_not_reveal_secret() {
        let digest = digest_secret("session_secret");
        assert_ne!(digest, "session_secret");
        assert_eq!(digest.len(), 64);
    }

    #[test]
    fn slugify_keeps_workspace_slugs_stable() {
        assert_eq!(slugify("Game Prototype!"), "game-prototype");
    }

    #[test]
    fn text_window_respects_start_limit_and_has_more() {
        let window = text_window_from_str("one\ntwo\nthree\nfour", 1, 2);

        assert_eq!(
            text_segments(&window),
            vec!["two".to_string(), "three".to_string()]
        );
        assert_eq!(window.total_lines, 4);
        assert!(window_has_more(1, window.lines.len(), window.total_lines));
    }

    #[test]
    fn text_window_handles_crlf_without_trailing_empty_line() {
        let window = text_window_from_str("one\r\ntwo\r\n", 0, 10);

        assert_eq!(
            text_segments(&window),
            vec!["one".to_string(), "two".to_string()]
        );
        assert_eq!(window.total_lines, 2);
        assert!(!window_has_more(0, window.lines.len(), window.total_lines));
    }

    #[test]
    fn text_window_segments_long_lines_by_columns() {
        let mut builder = TextLineWindowBuilder::new(WindowRequest {
            start: 0,
            limit: 2,
            column_start: 3,
            column_limit: Some(4),
        });
        builder.push_text("0123456789\nabc");
        let window = builder.finish();

        assert_eq!(window.total_lines, 2);
        assert_eq!(window.lines[0].text_segment, "3456");
        assert_eq!(window.lines[0].text_length, 10);
        assert_eq!(window.lines[0].column_start, 3);
        assert_eq!(window.lines[0].column_end, 7);
        assert!(window.lines[0].has_more_columns);
        assert_eq!(window.lines[1].text_segment, "");
        assert_eq!(window.lines[1].text_length, 3);
        assert_eq!(window.lines[1].column_start, 3);
        assert_eq!(window.lines[1].column_end, 3);
        assert!(!window.lines[1].has_more_columns);
    }

    fn text_segments(window: &TextLineWindow) -> Vec<String> {
        window
            .lines
            .iter()
            .map(|line| line.text_segment.clone())
            .collect()
    }

    #[test]
    fn desktop_user_json_uses_only_camel_case_display_name() {
        let user = UserPrincipal {
            id: "account_test".to_string(),
            email: "desktop@example.com".to_string(),
            display_name: "Layrs Desktop Dev".to_string(),
        };

        let value = desktop_user_json(&user);
        let object = value.as_object().expect("desktop user json is an object");

        assert_eq!(
            value.get("displayName").and_then(Value::as_str),
            Some("Layrs Desktop Dev")
        );
        assert!(!object.contains_key("display_name"));
    }

    #[test]
    fn web_user_json_uses_only_legacy_snake_case_display_name() {
        let user = UserPrincipal {
            id: "account_test".to_string(),
            email: "web@example.com".to_string(),
            display_name: "Layrs Web Dev".to_string(),
        };

        let value = user_wire_json(&user);
        let object = value.as_object().expect("web user json is an object");

        assert_eq!(
            value.get("display_name").and_then(Value::as_str),
            Some("Layrs Web Dev")
        );
        assert!(!object.contains_key("displayName"));
    }

    #[test]
    fn device_verification_without_session_has_no_approve_form() {
        let html = device_verification_html(
            "LAYRS-123456",
            "Sign in before approving.",
            true,
            None,
            false,
        );

        assert!(html.contains("No Studio session is active"));
        assert!(!html.contains("method=\"post\""));
    }

    #[test]
    fn device_verification_with_session_names_account() {
        let user = UserPrincipal {
            id: "account_test".to_string(),
            email: "player@example.com".to_string(),
            display_name: "Player".to_string(),
        };
        let html = device_verification_html(
            "LAYRS-123456",
            "Approve this device.",
            false,
            Some(&user),
            true,
        );

        assert!(html.contains("player@example.com"));
        assert!(html.contains("method=\"post\""));
    }

    #[tokio::test]
    async fn list_lenses_endpoint_returns_contract_manifests() {
        let Json(payload) = list_lenses().await;
        let manifests = payload
            .get("items")
            .and_then(Value::as_array)
            .expect("/v1/lenses returns an items array");
        assert!(
            payload.get("errors").and_then(Value::as_array).is_some(),
            "/v1/lenses returns non-fatal manifest errors"
        );

        assert_lens_manifest_contract(manifests);
    }

    fn assert_lens_manifest_contract(manifests: &[Value]) {
        let ids = manifests
            .iter()
            .filter_map(|lens| lens.get("id").and_then(Value::as_str).map(str::to_string))
            .collect::<Vec<_>>();

        assert!(
            ids.starts_with(&[
                "layrs.code".to_string(),
                "layrs.text".to_string(),
                "layrs.image".to_string(),
                "layrs.raw".to_string()
            ]),
            "built-in lenses should be first"
        );

        for manifest in manifests {
            let object = manifest
                .as_object()
                .expect("lens manifest is a JSON object");

            assert!(object.get("id").and_then(Value::as_str).is_some());
            assert!(object.get("name").and_then(Value::as_str).is_some());
            assert!(object.get("version").and_then(Value::as_str).is_some());
            assert!(!object.contains_key("displayName"));
            assert!(!object.contains_key("serverProvided"));
            assert!(
                object
                    .get("applies_to")
                    .and_then(Value::as_object)
                    .is_some()
            );
            assert!(
                object
                    .get("capabilities")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                object
                    .get("permissions")
                    .and_then(Value::as_object)
                    .is_some()
            );

            let analyzer = object
                .get("analyzer")
                .and_then(Value::as_object)
                .expect("lens manifest includes analyzer");
            assert!(
                analyzer
                    .get("supportedMediaTypes")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                analyzer
                    .get("fileExtensions")
                    .and_then(Value::as_array)
                    .is_some()
            );
            assert!(
                analyzer
                    .get("capabilities")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );

            let viewer = object
                .get("viewer")
                .and_then(Value::as_object)
                .expect("lens manifest includes viewer");
            assert!(viewer.get("viewerId").and_then(Value::as_str).is_some());
            assert_eq!(
                viewer.get("schemaVersion").and_then(Value::as_str),
                Some("layrs.viewer.v1")
            );
            assert!(viewer.get("component").and_then(Value::as_str).is_some());
            assert!(
                viewer
                    .get("previewKinds")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                viewer
                    .get("diffKinds")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                viewer
                    .get("reconcileStatuses")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            );
            assert!(
                viewer
                    .get("inspectorFields")
                    .and_then(Value::as_array)
                    .is_some()
            );
        }
    }

    #[test]
    fn timeline_body_redacts_inline_content_from_sync_payloads() {
        let redacted = redact_timeline_body(json!({
            "artifactId": "artifact_1",
            "content": { "text": "secret" }
        }));

        assert!(redacted.get("content").is_none());
        assert_eq!(
            redacted.get("contentIncluded").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn access_path_rules_match_descendants() {
        assert!(path_matches_rule("src/main.rs", "src"));
        assert!(path_matches_rule("private/image.png", "private/**"));
        assert!(!path_matches_rule("srcs/main.rs", "src"));
    }

    #[tokio::test]
    async fn create_layer_accepts_desktop_bearer_principal() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };

        let Json(payload) = create_layer(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(CreateLayerBody {
                name: "Bearer Layer".to_string(),
                parent_id: Some(fixture.layer_id.clone()),
                parent_layer_id: None,
                summary: None,
            }),
        )
        .await
        .expect("desktop bearer can create a layer");

        assert!(payload.get("id").and_then(Value::as_str).is_some());
        assert_eq!(
            payload.get("parentId").and_then(Value::as_str),
            Some(fixture.layer_id.as_str())
        );
    }

    #[tokio::test]
    async fn delete_layer_accepts_desktop_bearer_principal() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };

        let Json(created) = create_layer(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(CreateLayerBody {
                name: "Temporary Layer".to_string(),
                parent_id: Some(fixture.layer_id.clone()),
                parent_layer_id: None,
                summary: None,
            }),
        )
        .await
        .expect("desktop bearer can create a layer");
        let layer_id = created
            .get("id")
            .and_then(Value::as_str)
            .expect("created layer id")
            .to_string();

        let Json(deleted) = delete_layer(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                layer_id.clone(),
            )),
            fixture.bearer_headers(),
        )
        .await
        .expect("desktop bearer can delete a non-parent layer");

        assert_eq!(
            deleted.get("id").and_then(Value::as_str),
            Some(layer_id.as_str())
        );
        assert_eq!(deleted.get("deleted").and_then(Value::as_bool), Some(true));
    }

    #[tokio::test]
    async fn delete_space_removes_layers_artifacts_and_v2_objects() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let bytes = Bytes::from_static(b"delete me\n");
        let chunk_id = blake3_digest_for_bytes(&bytes);
        let file_object_id = blake3_digest_for_bytes(&bytes);
        let root_tree_id = blake3_digest_for_bytes(b"delete-space-tree");

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
        .expect("chunk upload succeeds");

        let _ = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(
                serde_json::from_value(json!({
                    "protocol": "layrs.sync.v2",
                    "layerId": fixture.layer_id,
                    "policyEpoch": 1,
                    "idempotencyKey": format!("delete_space_{}", Uuid::new_v4().simple()),
                    "sourceClientId": "test-client",
                    "rootTreeId": root_tree_id,
                    "changedPaths": ["delete.txt"],
                    "storeObjects": {
                        "chunks": [{
                            "chunkId": chunk_id,
                            "digest": chunk_id,
                            "size": bytes.len()
                        }],
                        "fileObjects": [{
                            "fileObjectId": file_object_id,
                            "digest": file_object_id,
                            "size": bytes.len(),
                            "mediaType": "text/plain",
                            "chunks": [{
                                "chunkId": chunk_id,
                                "digest": chunk_id,
                                "size": bytes.len(),
                                "byteOffset": 0
                            }]
                        }],
                        "treeObjects": [{
                            "treeId": root_tree_id,
                            "entries": [{
                                "path": "delete.txt",
                                "fileObjectId": file_object_id,
                                "size": bytes.len()
                            }]
                        }]
                    }
                }))
                .expect("publish body is valid"),
            ),
        )
        .await
        .expect("publish succeeds");

        let Json(deleted) = delete_space(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
        )
        .await
        .expect("space delete succeeds");

        assert_eq!(
            deleted.get("id").and_then(Value::as_str),
            Some(fixture.space_id.as_str())
        );
        assert_eq!(deleted.get("deleted").and_then(Value::as_bool), Some(true));
        let spaces: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM spaces WHERE space_id = $1")
                .bind(&fixture.space_id)
                .fetch_one(&fixture.pool)
                .await
                .expect("space count");
        let artifacts: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM artifacts WHERE space_id = $1")
                .bind(&fixture.space_id)
                .fetch_one(&fixture.pool)
                .await
                .expect("artifact count");
        let chunks: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM space_object_chunks WHERE space_id = $1",
        )
        .bind(&fixture.space_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("chunk count");
        assert_eq!(spaces, 0);
        assert_eq!(artifacts, 0);
        assert_eq!(chunks, 0);
    }

    #[tokio::test]
    async fn publish_then_receive_returns_authorized_content() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let bytes = Bytes::from_static(b"fn main() {}\n");
        let chunk_id = blake3_digest_for_bytes(&bytes);
        let file_object_id = blake3_digest_for_bytes(&bytes);
        let first_root_tree_id = blake3_digest_for_bytes(b"authorized-content-intermediate-tree");
        let root_tree_id = blake3_digest_for_bytes(b"authorized-content-tree");
        let first_step_id = format!("step_{}", Uuid::new_v4().simple());
        let second_step_id = format!("step_{}", Uuid::new_v4().simple());

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

        let Json(publish_payload) = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(
                serde_json::from_value(json!({
                    "protocol": "layrs.sync.v2",
                    "layerId": fixture.layer_id,
                    "policyEpoch": 1,
                    "idempotencyKey": format!("idem_{}", Uuid::new_v4().simple()),
                    "sourceClientId": "test-client",
                    "rootTreeId": root_tree_id,
                    "changedPaths": ["src/main.rs"],
                    "steps": [
                        {
                            "stepId": first_step_id,
                            "layerId": fixture.layer_id,
                            "rootTreeId": first_root_tree_id,
                            "changedPaths": ["src/main.rs"],
                            "capturedAtUnix": 1782910398
                        },
                        {
                            "stepId": second_step_id,
                            "layerId": fixture.layer_id,
                            "rootTreeId": root_tree_id,
                            "changedPaths": ["src/main.rs"],
                            "capturedAtUnix": 1782910399
                        }
                    ],
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
                            "treeId": first_root_tree_id,
                            "entries": [{
                                "path": "src/main.rs",
                                "fileObjectId": file_object_id,
                                "size": bytes.len()
                            }]
                        }, {
                            "treeId": root_tree_id,
                            "entries": [{
                                "path": "src/main.rs",
                                "fileObjectId": file_object_id,
                                "size": bytes.len()
                            }]
                        }]
                    }
                }))
                .expect("chunked publish body deserializes"),
            ),
        )
        .await
        .expect("chunked publish succeeds");
        assert_eq!(
            publish_payload
                .get("published")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert!(
            publish_payload
                .get("serverCursor")
                .and_then(Value::as_str)
                .is_some()
        );
        assert_eq!(
            publish_payload
                .pointer("/step/stepId")
                .and_then(Value::as_str),
            Some(second_step_id.as_str())
        );
        assert_eq!(
            publish_payload
                .get("steps")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        let stored_step_count: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM layer_steps WHERE step_id IN ($1, $2)",
        )
        .bind(&first_step_id)
        .bind(&second_step_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("step count");
        assert_eq!(stored_step_count, 2);

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

        assert!(receive_payload.get("content").is_none());
        assert!(
            receive_payload
                .get("layers")
                .and_then(Value::as_array)
                .is_some_and(|values| !values.is_empty())
        );
        assert!(
            receive_payload
                .get("accessRegistries")
                .and_then(Value::as_array)
                .is_some_and(|values| !values.is_empty())
        );
        let received_step_ids = receive_payload
            .get("steps")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .filter_map(|step| step.get("stepId").and_then(Value::as_str))
            .collect::<std::collections::BTreeSet<_>>();
        assert!(received_step_ids.contains(first_step_id.as_str()));
        assert!(received_step_ids.contains(second_step_id.as_str()));
        assert_eq!(
            receive_payload
                .get("contents")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
        let content_objects = receive_payload
            .get("contentObjects")
            .expect("receive includes chunked store manifest");
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
        assert!(
            serde_json::to_string(content_objects)
                .expect("contentObjects serializes")
                .contains("downloadUrl")
        );
        assert!(
            !serde_json::to_string(&receive_payload)
                .expect("receive serializes")
                .contains("fn main")
        );
    }

    #[tokio::test]
    async fn inline_artifact_publish_is_rejected() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };

        let error = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(SyncPublishBody {
                layer_id: Some(fixture.layer_id.clone()),
                layer_id_camel: None,
                cursor: None,
                artifacts: vec![publish_artifact(
                    "src/main.rs",
                    "code",
                    "text/plain",
                    json!("fn main() {}\n"),
                )],
                artifact: None,
                deleted_paths: Vec::new(),
                deleted_paths_camel: Vec::new(),
                policy_epoch: None,
                policy_epoch_camel: None,
                idempotency_key: None,
                idempotency_key_camel: None,
                source_client_id: None,
                source_client_id_camel: None,
                root_tree_id: None,
                root_tree_id_camel: None,
                base_tree_id: None,
                base_tree_id_camel: None,
                protocol: Some("layrs.sync.v2".to_string()),
                changed_paths: Vec::new(),
                changed_paths_camel: Vec::new(),
                store_objects: None,
                store_objects_camel: None,
                step: None,
                steps: Vec::new(),
            }),
        )
        .await
        .expect_err("inline artifact content must not publish");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "invalid_request");
        assert!(error.message.contains("inline artifact content"));
    }

    #[tokio::test]
    async fn compressed_chunk_upload_stores_encoded_bytes_with_raw_digest() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let raw = Bytes::from("compress me\n".repeat(4096));
        let chunk_id = blake3_digest_for_bytes(&raw);
        let compressed = Bytes::from(
            zstd::stream::encode_all(std::io::Cursor::new(raw.as_ref()), 3)
                .expect("zstd compression succeeds"),
        );
        assert!(compressed.len() < raw.len());
        let mut headers = fixture.bearer_headers();
        headers.insert(
            "x-layrs-chunk-compression",
            HeaderValue::from_static(CHUNK_COMPRESSION_ZSTD),
        );
        headers.insert(
            "x-layrs-raw-size",
            HeaderValue::from_str(&raw.len().to_string()).expect("raw size header"),
        );
        headers.insert(
            "x-layrs-stored-size",
            HeaderValue::from_str(&compressed.len().to_string()).expect("stored size header"),
        );

        let Json(upload_payload) = put_space_chunk(
            State(test_state(fixture.pool.clone())),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                chunk_id.clone(),
            )),
            headers,
            compressed.clone(),
        )
        .await
        .expect("compressed chunk upload succeeds");

        assert_eq!(
            upload_payload.get("chunkId").and_then(Value::as_str),
            Some(chunk_id.as_str())
        );
        assert_eq!(
            upload_payload.get("compression").and_then(Value::as_str),
            Some(CHUNK_COMPRESSION_ZSTD)
        );
        let row = sqlx::query(
            "SELECT size_bytes, stored_size_bytes, compression, content_bytes FROM object_chunks WHERE chunk_id = $1",
        )
        .bind(&chunk_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("chunk row exists");
        assert_eq!(row.get::<i64, _>("size_bytes"), raw.len() as i64);
        assert_eq!(
            row.get::<i64, _>("stored_size_bytes"),
            compressed.len() as i64
        );
        assert_eq!(row.get::<String, _>("compression"), CHUNK_COMPRESSION_ZSTD);
        assert_eq!(row.get::<Vec<u8>, _>("content_bytes"), compressed.to_vec());
    }

