#[tokio::test]
async fn weave_list_and_detail_hide_requests_with_an_inaccessible_layer() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (_, viewer_headers) = create_test_workspace_member_headers(&fixture, "viewer").await;
    let source_layer_id = create_weave_test_layer(&fixture, "Restricted Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "private/secret.txt",
        b"source secret\n",
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
            title: Some("Hidden Weave title".to_string()),
            body: Some("Hidden Weave body".to_string()),
        }),
    )
    .await
    .expect("owner prepares restricted-source Weave");
    let weave_id = created
        .get("weaveId")
        .and_then(Value::as_str)
        .expect("weave id")
        .to_string();

    restrict_weave_path(
        &fixture,
        &source_layer_id,
        "**",
        &[fixture.account_id.clone()],
        &[fixture.account_id.clone()],
    )
    .await;

    let Json(listed) = list_weave_requests(
        State(test_state(fixture.pool.clone())),
        viewer_headers.clone(),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
    )
    .await
    .expect("inaccessible Weave requests are filtered from the list");
    let listed = listed.as_array().expect("Weave list is an array");
    assert!(listed.is_empty());
    let serialized = serde_json::to_string(listed).expect("Weave list serializes");
    assert!(!serialized.contains(&weave_id));
    assert!(!serialized.contains("Hidden Weave title"));
    assert!(!serialized.contains("Hidden Weave body"));

    let detail_error = get_weave_request(
        State(test_state(fixture.pool.clone())),
        viewer_headers,
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id,
        )),
    )
    .await
    .expect_err("viewer cannot inspect a Weave with an inaccessible source Layer");
    assert_eq!(detail_error.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn weave_apply_rejects_policy_drift_without_mutating_target() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (writer_id, writer_headers) =
        create_test_workspace_member_headers(&fixture, "member").await;
    publish_weave_test_file(
        &fixture,
        &fixture.layer_id,
        "baseline.txt",
        b"baseline\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let source_layer_id = create_weave_test_layer(&fixture, "Policy Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "restricted.txt",
        b"incoming restricted content\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let Json(created) = create_weave_request(
        State(test_state(fixture.pool.clone())),
        writer_headers.clone(),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
        Json(CreateWeaveRequestBody {
            source_layer_id,
            target_layer_id: fixture.layer_id.clone(),
            title: Some("Policy drift guard".to_string()),
            body: None,
        }),
    )
    .await
    .expect("writer prepares Weave while target path is writable");
    let weave_id = created
        .get("weaveId")
        .and_then(Value::as_str)
        .expect("weave id")
        .to_string();
    let before = weave_target_snapshot(&fixture, &weave_id).await;

    restrict_weave_path(
        &fixture,
        &fixture.layer_id,
        "restricted.txt",
        &[fixture.account_id.clone(), writer_id],
        &[fixture.account_id.clone()],
    )
    .await;

    let error = apply_weave_request(
        State(test_state(fixture.pool.clone())),
        writer_headers,
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id.clone(),
        )),
    )
    .await
    .expect_err("writer cannot apply a replay path made read-only after preparation");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert!(error.message.contains("no longer writable"));
    assert_eq!(weave_target_snapshot(&fixture, &weave_id).await, before);
    assert_eq!(
        target_file_bytes(&fixture, &fixture.layer_id, "baseline.txt").await,
        b"baseline\n"
    );
    let restricted_artifacts: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM artifacts WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = 'restricted.txt'",
        )
        .bind(&fixture.workspace_id)
        .bind(&fixture.space_id)
        .bind(&fixture.layer_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("restricted target artifact count loads");
    assert_eq!(restricted_artifacts, 0);
    let weave_status: String =
        sqlx::query_scalar("SELECT status FROM weave_requests WHERE weave_id = $1")
            .bind(&weave_id)
            .fetch_one(&fixture.pool)
            .await
            .expect("Weave status remains queryable");
    assert_eq!(weave_status, "open");
}

#[tokio::test]
async fn weave_viewer_cannot_create_resolve_apply_or_abort() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (_, viewer_headers) = create_test_workspace_member_headers(&fixture, "viewer").await;
    let source_layer_id = create_weave_test_layer(&fixture, "Viewer Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "viewer.txt",
        b"viewer source\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let create_error = create_weave_request(
        State(test_state(fixture.pool.clone())),
        viewer_headers.clone(),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
        Json(CreateWeaveRequestBody {
            source_layer_id: source_layer_id.clone(),
            target_layer_id: fixture.layer_id.clone(),
            title: Some("Viewer must not mutate".to_string()),
            body: None,
        }),
    )
    .await
    .expect_err("viewer cannot create a Weave request");
    assert_eq!(create_error.status, StatusCode::FORBIDDEN);

    let (conflicted_weave_id, conflict_id) = create_raw_conflicted_weave(&fixture).await;
    let Json(conflict_detail) = get_weave_request(
        State(test_state(fixture.pool.clone())),
        viewer_headers.clone(),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            conflicted_weave_id.clone(),
        )),
    )
    .await
    .expect("viewer may inspect an unrestricted Weave conflict");
    assert_eq!(
        conflict_detail
            .get("conflicts")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    let resolve_error = resolve_weave_conflict(
        State(test_state(fixture.pool.clone())),
        viewer_headers.clone(),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            conflicted_weave_id,
            conflict_id,
        )),
        Json(ResolveWeaveConflictBody {
            method: "incoming".to_string(),
            block_id: None,
            manual_text: None,
        }),
    )
    .await
    .expect_err("viewer cannot resolve an unrestricted Weave conflict");
    assert_eq!(resolve_error.status, StatusCode::FORBIDDEN);

    let Json(created) = create_weave_request(
        State(test_state(fixture.pool.clone())),
        fixture.bearer_headers(),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
        Json(CreateWeaveRequestBody {
            source_layer_id,
            target_layer_id: fixture.layer_id.clone(),
            title: Some("Owner request".to_string()),
            body: None,
        }),
    )
    .await
    .expect("owner creates Weave request");
    let weave_id = created
        .get("weaveId")
        .and_then(Value::as_str)
        .expect("weave id")
        .to_string();
    let apply_error = apply_weave_request(
        State(test_state(fixture.pool.clone())),
        viewer_headers.clone(),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id.clone(),
        )),
    )
    .await
    .expect_err("viewer cannot apply a Weave");
    assert_eq!(apply_error.status, StatusCode::FORBIDDEN);
    let abort_error = abort_weave_request(
        State(test_state(fixture.pool.clone())),
        viewer_headers,
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id,
        )),
    )
    .await
    .expect_err("viewer cannot abort a Weave");
    assert_eq!(abort_error.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn weave_conflict_bytes_obey_layer_path_policy() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (viewer_id, viewer_headers) =
        create_test_workspace_member_headers(&fixture, "viewer").await;
    let (weave_id, conflict_id) = create_raw_conflicted_weave(&fixture).await;
    let policy_id = policy_id_for_layer(
        &fixture.pool,
        &fixture.workspace_id,
        &fixture.space_id,
        &fixture.layer_id,
    )
    .await
    .expect("target policy exists");
    sqlx::query(
        r#"
            INSERT INTO layer_access_policy_rules
                (rule_id, policy_id, path, mode, visibility, read_account_ids,
                 write_account_ids)
            VALUES ($1, $2, 'asset.bin', 'restricted', 'stub', $3, $3)
            "#,
    )
    .bind(prefixed_id("access_rule"))
    .bind(&policy_id)
    .bind(vec![fixture.account_id.clone()])
    .execute(&fixture.pool)
    .await
    .expect("restricted target path rule inserted");

    let detail_error = get_weave_request(
        State(test_state(fixture.pool.clone())),
        viewer_headers.clone(),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id.clone(),
        )),
    )
    .await
    .expect_err("viewer cannot read restricted conflict bytes");
    assert_eq!(detail_error.status, StatusCode::FORBIDDEN);
    let resolve_error = resolve_weave_conflict(
        State(test_state(fixture.pool.clone())),
        viewer_headers,
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id,
            conflict_id,
        )),
        Json(ResolveWeaveConflictBody {
            method: "incoming".to_string(),
            block_id: None,
            manual_text: None,
        }),
    )
    .await
    .expect_err("viewer cannot resolve restricted conflict");
    assert_eq!(resolve_error.status, StatusCode::FORBIDDEN);
    assert!(!viewer_id.is_empty());
}

async fn restrict_weave_path(
    fixture: &SyncTestFixture,
    layer_id: &str,
    path: &str,
    read_account_ids: &[String],
    write_account_ids: &[String],
) {
    let policy_id = policy_id_for_layer(
        &fixture.pool,
        &fixture.workspace_id,
        &fixture.space_id,
        layer_id,
    )
    .await
    .expect("Layer policy exists");
    insert_layer_access_rule(
        &fixture.pool,
        &policy_id,
        &prefixed_id("access_rule"),
        &LayerAccessRuleBody {
            id: None,
            path: path.to_string(),
            artifact_id: None,
            mode: "restricted".to_string(),
            visibility: "stub".to_string(),
            permissions: LayerAccessPermissionsBody {
                read: LayerAccessPrincipalsBody {
                    accounts: read_account_ids.to_vec(),
                    teams: Vec::new(),
                },
                write: LayerAccessPrincipalsBody {
                    accounts: write_account_ids.to_vec(),
                    teams: Vec::new(),
                },
                admin: LayerAccessPrincipalsBody::default(),
            },
        },
        Some(&fixture.account_id),
    )
    .await
    .expect("Weave path restriction is installed and bumps policy epoch");
}

#[derive(Debug, PartialEq)]
struct WeaveTargetSnapshot {
    head: Option<Value>,
    steps: Value,
    replays: Value,
}

async fn weave_target_snapshot(fixture: &SyncTestFixture, weave_id: &str) -> WeaveTargetSnapshot {
    let head = sqlx::query_scalar(
            "SELECT to_jsonb(h) FROM layer_heads h WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
        )
        .bind(&fixture.workspace_id)
        .bind(&fixture.space_id)
        .bind(&fixture.layer_id)
        .fetch_optional(&fixture.pool)
        .await
        .expect("target head snapshot loads");
    let steps = sqlx::query_scalar(
            "SELECT COALESCE(jsonb_agg(to_jsonb(s) ORDER BY s.created_at, s.step_id), '[]'::jsonb) FROM layer_steps s WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3",
        )
        .bind(&fixture.workspace_id)
        .bind(&fixture.space_id)
        .bind(&fixture.layer_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("target Steps snapshot loads");
    let replays = sqlx::query_scalar(
            "SELECT COALESCE(jsonb_agg(to_jsonb(r) ORDER BY r.order_index), '[]'::jsonb) FROM weave_step_replays r WHERE weave_id = $1",
        )
        .bind(weave_id)
        .fetch_one(&fixture.pool)
        .await
        .expect("Weave replay snapshot loads");
    WeaveTargetSnapshot {
        head,
        steps,
        replays,
    }
}
