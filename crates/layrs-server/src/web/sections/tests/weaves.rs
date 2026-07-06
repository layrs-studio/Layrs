    #[test]
    fn weave_text_conflict_detail_uses_lens_blocks_segments_and_methods() {
        let row = text_weave_conflict_row();
        let base = weave_side(b"a\nx\nb\n");
        let existing = weave_side(b"a\nexisting\nb\n");
        let incoming = weave_side(b"a\nincoming\nb\n");
        let reconcile = reconcile_weave_conflict(&row, &base, &existing, &incoming);

        let blocks = weave_conflict_block_values(&row, &reconcile, &json!({}));
        let segments = weave_conflict_segment_values(&reconcile);

        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].get("supportedMethods").and_then(Value::as_array).map(Vec::len),
            Some(4)
        );
        assert_eq!(
            blocks[0].get("existing").and_then(Value::as_str),
            Some("existing\n")
        );
        assert!(
            segments.iter().any(|segment| {
                segment.get("kind").and_then(Value::as_str) == Some("block")
                    && segment.get("blockId").and_then(Value::as_str) == Some("block-1")
            })
        );
    }

    #[test]
    fn weave_text_conflict_resolution_is_assembled_by_lens_segments() {
        let row = text_weave_conflict_row();
        let base = weave_side(b"a\nx\nb\n");
        let existing = weave_side(b"a\nexisting\nb\n");
        let incoming = weave_side(b"a\nincoming\nb\n");
        let reconcile = reconcile_weave_conflict(&row, &base, &existing, &incoming);
        let mut payload = json!({});

        set_block_resolution_payload(
            &mut payload,
            &row.conflict_id,
            "block-1",
            ResolutionMethod::Incoming,
            "incoming\n",
        );
        let content = assemble_text_resolution(&row, &reconcile, &payload)
            .expect("lens assembles text resolution");

        assert_eq!(content.bytes, b"a\nincoming\nb\n");
    }

    #[tokio::test]
    async fn weave_plan_without_conflict_apply_advances_target() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let source_layer_id = create_weave_test_layer(&fixture, "Source").await;
        let source_step = publish_weave_test_file(
            &fixture,
            &source_layer_id,
            "docs/readme.txt",
            b"source file\n",
            "text/plain",
            None,
            0,
        )
        .await;

        let Json(created) = create_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            Json(CreateWeaveRequestBody {
                source_layer_id: source_layer_id.clone(),
                target_layer_id: fixture.layer_id.clone(),
                title: Some("Replay source".to_string()),
                body: None,
            }),
        )
        .await
        .expect("weave request creates a replay plan");
        assert_eq!(created.get("status").and_then(Value::as_str), Some("open"));
        assert_eq!(
            created
                .get("plannedSteps")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );

        let weave_id = created
            .get("weaveId")
            .and_then(Value::as_str)
            .expect("weave id")
            .to_string();
        let Json(applied) = apply_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                weave_id.clone(),
            )),
        )
        .await
        .expect("apply replays the source step");

        assert_eq!(applied.get("status").and_then(Value::as_str), Some("applied"));
        let applied_step_id = applied
            .get("appliedSteps")
            .and_then(Value::as_array)
            .and_then(|steps| steps.first())
            .and_then(Value::as_str)
            .expect("applied target step id")
            .to_string();
        assert_eq!(
            target_file_bytes(&fixture, &fixture.layer_id, "docs/readme.txt").await,
            b"source file\n"
        );

        let step = sqlx::query(
            "SELECT step_kind, origin_step_id, origin_layer_id, timeline_position FROM layer_steps WHERE step_id = $1",
        )
        .bind(&applied_step_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("applied woven step exists");
        assert_eq!(step.get::<String, _>("step_kind"), "woven");
        assert_eq!(
            step.get::<Option<String>, _>("origin_step_id").as_deref(),
            Some(source_step.step_id.as_str())
        );
        assert_eq!(
            step.get::<Option<String>, _>("origin_layer_id").as_deref(),
            Some(source_layer_id.as_str())
        );
        assert_eq!(step.get::<Option<i64>, _>("timeline_position"), Some(0));
        let replay_status: String = sqlx::query_scalar(
            "SELECT status FROM weave_step_replays WHERE weave_id = $1 AND source_step_id = $2",
        )
        .bind(&weave_id)
        .bind(&source_step.step_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("replay row exists");
        assert_eq!(replay_status, "applied");
    }

    #[tokio::test]
    async fn weave_child_layer_full_tree_publish_replays_only_changed_paths() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let base_step = publish_weave_test_files(
            &fixture,
            &fixture.layer_id,
            &[
                ("README.md", b"base-readme-v1\n".as_slice(), "text/plain"),
                ("shared.txt", b"base-shared-v1\n".as_slice(), "text/plain"),
            ],
            None,
            &["README.md", "shared.txt"],
            0,
        )
        .await;
        let source_layer_id = create_child_weave_test_layer(&fixture, "Source Child").await;
        let source_step = publish_weave_test_files(
            &fixture,
            &source_layer_id,
            &[
                ("README.md", b"base-readme-v1\n".as_slice(), "text/plain"),
                ("shared.txt", b"base-shared-v1\n".as_slice(), "text/plain"),
                ("feature-a.txt", b"source-feature-a\n".as_slice(), "text/plain"),
                ("feature-b.txt", b"source-feature-b\n".as_slice(), "text/plain"),
            ],
            Some(&base_step.tree_id),
            &["feature-a.txt", "feature-b.txt"],
            1,
        )
        .await;

        let Json(created) = create_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            Json(CreateWeaveRequestBody {
                source_layer_id: source_layer_id.clone(),
                target_layer_id: fixture.layer_id.clone(),
                title: Some("Replay child additions".to_string()),
                body: None,
            }),
        )
        .await
        .expect("weave request creates a replay plan");
        assert_eq!(created.get("status").and_then(Value::as_str), Some("open"));
        assert_eq!(
            created
                .get("plannedSteps")
                .and_then(Value::as_array)
                .and_then(|steps| steps.first())
                .and_then(Value::as_str),
            Some(source_step.step_id.as_str())
        );

        let weave_id = created
            .get("weaveId")
            .and_then(Value::as_str)
            .expect("weave id")
            .to_string();
        let _ = apply_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                weave_id.clone(),
            )),
        )
        .await
        .expect("apply replays child additions");

        assert_eq!(
            target_file_bytes(&fixture, &fixture.layer_id, "feature-a.txt").await,
            b"source-feature-a\n"
        );
        assert_eq!(
            target_file_bytes(&fixture, &fixture.layer_id, "feature-b.txt").await,
            b"source-feature-b\n"
        );
        let target_artifact_count: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)::bigint
            FROM artifacts
            WHERE workspace_id = $1
              AND space_id = $2
              AND layer_id = $3
              AND state = 'active'
            "#,
        )
        .bind(&fixture.workspace_id)
        .bind(&fixture.space_id)
        .bind(&fixture.layer_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("target artifact count");
        assert_eq!(target_artifact_count, 4);
        let target_head_tree_id: String = sqlx::query_scalar(
            r#"
            SELECT root_tree_id
            FROM layer_heads
            WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3
            "#,
        )
        .bind(&fixture.workspace_id)
        .bind(&fixture.space_id)
        .bind(&fixture.layer_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("target head tree id");
        assert!(
            target_head_tree_id.starts_with("blake3:"),
            "Weave apply must keep layer_heads.root_tree_id as a Merkle digest, got {target_head_tree_id}"
        );
    }

    #[tokio::test]
    async fn weave_text_conflict_resolution_applies_resolved_content() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let source_layer_id = create_weave_test_layer(&fixture, "Text Source").await;
        let base = publish_weave_test_file(
            &fixture,
            &fixture.layer_id,
            "note.txt",
            b"a\nx\nb\n",
            "text/plain",
            None,
            0,
        )
        .await;
        publish_weave_test_file(
            &fixture,
            &fixture.layer_id,
            "note.txt",
            b"a\nexisting\nb\n",
            "text/plain",
            Some(&base.tree_id),
            1,
        )
        .await;
        let source_step = publish_weave_test_file(
            &fixture,
            &source_layer_id,
            "note.txt",
            b"a\nincoming\nb\n",
            "text/plain",
            Some(&base.tree_id),
            0,
        )
        .await;

        let (weave_id, conflict_id, block_id) =
            create_text_conflicted_weave(&fixture, &source_layer_id).await;
        let Json(resolved) = resolve_weave_conflict(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                weave_id.clone(),
                conflict_id,
            )),
            Json(ResolveWeaveConflictBody {
                method: "incoming".to_string(),
                block_id: Some(block_id),
                manual_text: None,
            }),
        )
        .await
        .expect("text block conflict resolves");
        assert_eq!(resolved.get("status").and_then(Value::as_str), Some("resolved"));

        let Json(applied) = apply_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                weave_id,
            )),
        )
        .await
        .expect("resolved weave applies");
        assert_eq!(applied.get("status").and_then(Value::as_str), Some("applied"));
        assert_eq!(
            target_file_bytes(&fixture, &fixture.layer_id, "note.txt").await,
            b"a\nincoming\nb\n"
        );
        let applied_origin: String = sqlx::query_scalar(
            "SELECT origin_step_id FROM layer_steps WHERE step_id = $1",
        )
        .bind(
            applied
                .get("appliedSteps")
                .and_then(Value::as_array)
                .and_then(|steps| steps.first())
                .and_then(Value::as_str)
                .expect("applied step id"),
        )
        .fetch_one(&fixture.pool)
        .await
        .expect("applied step has origin");
        assert_eq!(applied_origin, source_step.step_id);
    }

    #[tokio::test]
    async fn weave_raw_both_resolution_is_rejected_by_server() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let (weave_id, conflict_id) = create_raw_conflicted_weave(&fixture).await;

        let error = resolve_weave_conflict(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                weave_id,
                conflict_id,
            )),
            Json(ResolveWeaveConflictBody {
                method: "both".to_string(),
                block_id: None,
                manual_text: None,
            }),
        )
        .await
        .expect_err("raw lens does not allow both");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("Available methods: existing, incoming"));
    }

    #[tokio::test]
    async fn weave_apply_blocks_when_conflict_is_unresolved() {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let (weave_id, _) = create_raw_conflicted_weave(&fixture).await;

        let error = apply_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                weave_id,
            )),
        )
        .await
        .expect_err("apply refuses unresolved conflicts");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("resolve all Weave conflicts"));
    }

    #[derive(Debug)]
    struct PublishedWeaveStep {
        step_id: String,
        tree_id: String,
    }

    async fn create_text_conflicted_weave(
        fixture: &SyncTestFixture,
        source_layer_id: &str,
    ) -> (String, String, String) {
        let Json(created) = create_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            Json(CreateWeaveRequestBody {
                source_layer_id: source_layer_id.to_string(),
                target_layer_id: fixture.layer_id.clone(),
                title: Some("Text conflict".to_string()),
                body: None,
            }),
        )
        .await
        .expect("conflicted text weave creates");
        assert_eq!(created.get("status").and_then(Value::as_str), Some("conflicted"));
        let weave_id = created
            .get("weaveId")
            .and_then(Value::as_str)
            .expect("weave id")
            .to_string();
        let Json(detail) = get_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                weave_id.clone(),
            )),
        )
        .await
        .expect("weave detail loads");
        let conflict = detail
            .get("conflicts")
            .and_then(Value::as_array)
            .and_then(|conflicts| conflicts.first())
            .expect("one conflict");
        let conflict_id = conflict
            .get("conflictId")
            .and_then(Value::as_str)
            .expect("conflict id")
            .to_string();
        let block_id = conflict
            .get("blocks")
            .and_then(Value::as_array)
            .and_then(|blocks| blocks.first())
            .and_then(|block| block.get("blockId"))
            .and_then(Value::as_str)
            .expect("text block id")
            .to_string();
        (weave_id, conflict_id, block_id)
    }

    async fn create_raw_conflicted_weave(fixture: &SyncTestFixture) -> (String, String) {
        let source_layer_id = create_weave_test_layer(fixture, "Raw Source").await;
        let base = publish_weave_test_file(
            fixture,
            &fixture.layer_id,
            "asset.bin",
            b"base-bytes",
            "application/octet-stream",
            None,
            0,
        )
        .await;
        publish_weave_test_file(
            fixture,
            &fixture.layer_id,
            "asset.bin",
            b"target-bytes",
            "application/octet-stream",
            Some(&base.tree_id),
            1,
        )
        .await;
        publish_weave_test_file(
            fixture,
            &source_layer_id,
            "asset.bin",
            b"source-bytes",
            "application/octet-stream",
            Some(&base.tree_id),
            0,
        )
        .await;

        let Json(created) = create_weave_request(
            State(test_state(fixture.pool.clone())),
            fixture.bearer_headers(),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            Json(CreateWeaveRequestBody {
                source_layer_id,
                target_layer_id: fixture.layer_id.clone(),
                title: Some("Raw conflict".to_string()),
                body: None,
            }),
        )
        .await
        .expect("raw conflicted weave creates");
        assert_eq!(created.get("status").and_then(Value::as_str), Some("conflicted"));
        let weave_id = created
            .get("weaveId")
            .and_then(Value::as_str)
            .expect("weave id")
            .to_string();
        let conflict_id: String = sqlx::query_scalar(
            "SELECT conflict_id FROM weave_conflicts WHERE weave_id = $1",
        )
        .bind(&weave_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("raw conflict exists");
        (weave_id, conflict_id)
    }

    async fn create_weave_test_layer(fixture: &SyncTestFixture, name: &str) -> String {
        let Json(created) = create_layer(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(CreateLayerBody {
                name: name.to_string(),
                parent_id: None,
                parent_layer_id: None,
                summary: None,
            }),
        )
        .await
        .expect("test layer creates");
        created
            .get("id")
            .and_then(Value::as_str)
            .expect("created layer id")
            .to_string()
    }

    async fn create_child_weave_test_layer(fixture: &SyncTestFixture, name: &str) -> String {
        let Json(created) = create_layer(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(CreateLayerBody {
                name: name.to_string(),
                parent_id: Some(fixture.layer_id.clone()),
                parent_layer_id: None,
                summary: None,
            }),
        )
        .await
        .expect("test child layer creates");
        created
            .get("id")
            .and_then(Value::as_str)
            .expect("created child layer id")
            .to_string()
    }

    async fn publish_weave_test_files(
        fixture: &SyncTestFixture,
        layer_id: &str,
        files: &[(&str, &[u8], &str)],
        base_tree_id: Option<&str>,
        changed_paths: &[&str],
        timeline_position: i64,
    ) -> PublishedWeaveStep {
        let tree_id = blake3_digest_for_bytes(
            format!("weave-test-tree:{}:{}", files.len(), Uuid::new_v4().simple()).as_bytes(),
        );
        let step_id = format!("step_{}", Uuid::new_v4().simple());
        let mut chunks = Vec::new();
        let mut file_objects = Vec::new();
        let mut entries = Vec::new();
        for (path, bytes, media_type) in files {
            let bytes = Bytes::from(bytes.to_vec());
            let chunk_id = blake3_digest_for_bytes(&bytes);
            let file_object_id = blake3_digest_for_bytes(&bytes);
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
            .expect("chunk uploads");
            chunks.push(json!({
                "chunkId": chunk_id,
                "digest": chunk_id,
                "size": bytes.len()
            }));
            file_objects.push(json!({
                "fileObjectId": file_object_id,
                "digest": file_object_id,
                "size": bytes.len(),
                "mediaType": media_type,
                "chunks": [{
                    "chunkId": chunk_id,
                    "digest": chunk_id,
                    "size": bytes.len(),
                    "byteOffset": 0
                }]
            }));
            entries.push(json!({
                "path": path,
                "fileObjectId": file_object_id,
                "size": bytes.len()
            }));
        }
        let mut body = json!({
            "protocol": "layrs.sync.v2",
            "layerId": layer_id,
            "policyEpoch": 1,
            "idempotencyKey": format!("weave_publish_{}", Uuid::new_v4().simple()),
            "sourceClientId": "weave-test",
            "rootTreeId": tree_id,
            "changedPaths": changed_paths,
            "step": {
                "stepId": step_id,
                "layerId": layer_id,
                "rootTreeId": tree_id,
                "changedPaths": changed_paths,
                "timelinePosition": timeline_position,
                "originLayerId": layer_id,
                "originStepId": step_id,
                "stepKind": "native"
            },
            "storeObjects": {
                "chunks": chunks,
                "fileObjects": file_objects,
                "treeObjects": [{
                    "treeId": tree_id,
                    "entries": entries
                }]
            }
        });
        if let Some(base_tree_id) = base_tree_id {
            body["baseTreeId"] = json!(base_tree_id);
            body["step"]["baseTreeId"] = json!(base_tree_id);
        }
        let _: Json<Value> = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(serde_json::from_value(body).expect("publish body deserializes")),
        )
        .await
        .expect("test files publish");

        PublishedWeaveStep { step_id, tree_id }
    }

    async fn publish_weave_test_file(
        fixture: &SyncTestFixture,
        layer_id: &str,
        path: &str,
        bytes: &[u8],
        media_type: &str,
        base_tree_id: Option<&str>,
        timeline_position: i64,
    ) -> PublishedWeaveStep {
        let bytes = Bytes::from(bytes.to_vec());
        let chunk_id = blake3_digest_for_bytes(&bytes);
        let file_object_id = blake3_digest_for_bytes(&bytes);
        let tree_id = blake3_digest_for_bytes(
            format!("weave-test-tree:{path}:{}", Uuid::new_v4().simple()).as_bytes(),
        );
        let step_id = format!("step_{}", Uuid::new_v4().simple());
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
        .expect("chunk uploads");
        let mut body = json!({
            "protocol": "layrs.sync.v2",
            "layerId": layer_id,
            "policyEpoch": 1,
            "idempotencyKey": format!("weave_publish_{}", Uuid::new_v4().simple()),
            "sourceClientId": "weave-test",
            "rootTreeId": tree_id,
            "changedPaths": [path],
            "step": {
                "stepId": step_id,
                "layerId": layer_id,
                "rootTreeId": tree_id,
                "changedPaths": [path],
                "timelinePosition": timeline_position,
                "originLayerId": layer_id,
                "originStepId": step_id,
                "stepKind": "native"
            },
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
                    "mediaType": media_type,
                    "chunks": [{
                        "chunkId": chunk_id,
                        "digest": chunk_id,
                        "size": bytes.len(),
                        "byteOffset": 0
                    }]
                }],
                "treeObjects": [{
                    "treeId": tree_id,
                    "entries": [{
                        "path": path,
                        "fileObjectId": file_object_id,
                        "size": bytes.len()
                    }]
                }]
            }
        });
        if let Some(base_tree_id) = base_tree_id {
            body["baseTreeId"] = json!(base_tree_id);
            body["step"]["baseTreeId"] = json!(base_tree_id);
        }
        let _: Json<Value> = publish_local_space_sync(
            State(test_state(fixture.pool.clone())),
            Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
            fixture.bearer_headers(),
            Json(serde_json::from_value(body).expect("publish body deserializes")),
        )
        .await
        .expect("test file publishes");

        PublishedWeaveStep { step_id, tree_id }
    }

    async fn target_file_bytes(
        fixture: &SyncTestFixture,
        layer_id: &str,
        path: &str,
    ) -> Vec<u8> {
        let file_object_id: String = sqlx::query_scalar(
            r#"
            SELECT current_file_object_id
            FROM artifacts
            WHERE workspace_id = $1
              AND space_id = $2
              AND layer_id = $3
              AND logical_path = $4
              AND state = 'active'
            "#,
        )
        .bind(&fixture.workspace_id)
        .bind(&fixture.space_id)
        .bind(layer_id)
        .bind(path)
        .fetch_one(&fixture.pool)
        .await
        .expect("target artifact exists");
        file_object_bytes(&fixture.pool, &fixture.workspace_id, &fixture.space_id, &file_object_id)
            .await
            .expect("target bytes load")
    }
