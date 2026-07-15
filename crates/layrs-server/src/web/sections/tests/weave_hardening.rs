#[tokio::test]
async fn weave_artifact_rules_gate_list_detail_create_and_resolution() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (writer_id, writer_headers) =
        create_test_workspace_member_headers(&fixture, "member").await;
    let (_, viewer_headers) = create_test_workspace_member_headers(&fixture, "viewer").await;
    let (weave_id, conflict_id) = create_raw_conflicted_weave(&fixture).await;
    let source_layer_id: String =
        sqlx::query_scalar("SELECT source_layer_id FROM weave_requests WHERE weave_id = $1")
            .bind(&weave_id)
            .fetch_one(&fixture.pool)
            .await
            .expect("source Layer id loads");
    let artifact_id = weave_test_artifact_id(&fixture, &fixture.layer_id, "asset.bin").await;
    restrict_weave_artifact(
        &fixture,
        &fixture.layer_id,
        &artifact_id,
        &[fixture.account_id.clone(), writer_id.clone()],
        &[fixture.account_id.clone()],
    )
    .await;

    let _ = get_weave_request(
        State(test_state(fixture.pool.clone())),
        writer_headers.clone(),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id.clone(),
        )),
    )
    .await
    .expect("artifact reader can inspect conflict detail");

    let Json(listed) = list_weave_requests(
        State(test_state(fixture.pool.clone())),
        viewer_headers.clone(),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
    )
    .await
    .expect("artifact-restricted Weaves are filtered from list");
    assert!(listed.as_array().expect("Weave list").is_empty());
    let detail_error = get_weave_request(
        State(test_state(fixture.pool.clone())),
        viewer_headers,
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id.clone(),
        )),
    )
    .await
    .expect_err("artifact rule hides detail from non-reader");
    assert_eq!(detail_error.status, StatusCode::FORBIDDEN);

    let create_error = create_weave_request(
        State(test_state(fixture.pool.clone())),
        writer_headers.clone(),
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
        Json(CreateWeaveRequestBody {
            source_layer_id,
            target_layer_id: fixture.layer_id.clone(),
            title: Some("Artifact rule create guard".to_string()),
            body: None,
        }),
    )
    .await
    .expect_err("artifact reader without write cannot create a resolution plan");
    assert_eq!(create_error.status, StatusCode::FORBIDDEN);

    let resolve_error = resolve_weave_conflict(
        State(test_state(fixture.pool.clone())),
        writer_headers,
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            weave_id.clone(),
            conflict_id.clone(),
        )),
        Json(ResolveWeaveConflictBody {
            method: "incoming".to_string(),
            block_id: None,
            manual_text: None,
        }),
    )
    .await
    .expect_err("artifact reader without write cannot choose a resolution");
    assert_eq!(resolve_error.status, StatusCode::FORBIDDEN);
    let status: String =
        sqlx::query_scalar("SELECT status FROM weave_conflicts WHERE conflict_id = $1")
            .bind(conflict_id)
            .fetch_one(&fixture.pool)
            .await
            .expect("conflict status loads");
    assert_eq!(status, "open");
}

#[tokio::test]
async fn weave_apply_revalidates_target_artifact_write() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (writer_id, writer_headers) =
        create_test_workspace_member_headers(&fixture, "member").await;
    let base = publish_weave_test_file(
        &fixture,
        &fixture.layer_id,
        "artifact.txt",
        b"base\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let source_layer_id = create_child_weave_test_layer(&fixture, "Artifact Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "artifact.txt",
        b"incoming\n",
        "text/plain",
        Some(&base.tree_id),
        1,
    )
    .await;
    let weave_id = create_planned_weave(
        &fixture,
        writer_headers.clone(),
        &source_layer_id,
        "Artifact apply guard",
    )
    .await;
    let artifact_id = weave_test_artifact_id(&fixture, &fixture.layer_id, "artifact.txt").await;
    restrict_weave_artifact(
        &fixture,
        &fixture.layer_id,
        &artifact_id,
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
            weave_id,
        )),
    )
    .await
    .expect_err("artifact write loss blocks Apply");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert!(error.message.contains("no longer writable"));
    assert_eq!(
        target_file_bytes(&fixture, &fixture.layer_id, "artifact.txt").await,
        b"base\n"
    );
}

#[tokio::test]
async fn weave_abort_is_limited_to_requester_or_admin() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (requester_id, requester_headers) =
        create_test_workspace_member_headers(&fixture, "member").await;
    let (_, other_writer_headers) = create_test_workspace_member_headers(&fixture, "member").await;
    let source_layer_id = create_weave_test_layer(&fixture, "Abort Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "abort.txt",
        b"abort\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let admin_abort_id = create_planned_weave(
        &fixture,
        requester_headers.clone(),
        &source_layer_id,
        "Admin abort",
    )
    .await;

    let error = abort_weave_request(
        State(test_state(fixture.pool.clone())),
        other_writer_headers,
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            admin_abort_id.clone(),
        )),
    )
    .await
    .expect_err("ordinary writer cannot abort another request");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    let Json(admin_aborted) = abort_weave_request(
        State(test_state(fixture.pool.clone())),
        fixture.bearer_headers(),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            admin_abort_id,
        )),
    )
    .await
    .expect("workspace admin can abort another request");
    assert_eq!(admin_aborted["status"], "aborted");

    let requester_abort_id = create_planned_weave(
        &fixture,
        requester_headers.clone(),
        &source_layer_id,
        "Requester abort",
    )
    .await;
    sqlx::query(
        "UPDATE workspace_memberships SET role = 'viewer' WHERE workspace_id = $1 AND account_id = $2",
    )
    .bind(&fixture.workspace_id)
    .bind(&requester_id)
    .execute(&fixture.pool)
    .await
    .expect("requester is downgraded");
    let Json(requester_aborted) = abort_weave_request(
        State(test_state(fixture.pool.clone())),
        requester_headers,
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            requester_abort_id,
        )),
    )
    .await
    .expect("active requester can withdraw after losing write");
    assert_eq!(requester_aborted["status"], "aborted");
}

#[tokio::test]
async fn weave_apply_observes_policy_change_committed_while_waiting() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (_, writer_headers) = create_test_workspace_member_headers(&fixture, "member").await;
    let source_layer_id = create_weave_test_layer(&fixture, "Policy Race Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "policy-race.txt",
        b"policy race\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let weave_id = create_planned_weave(
        &fixture,
        writer_headers.clone(),
        &source_layer_id,
        "Policy race",
    )
    .await;
    let policy_id = policy_id_for_layer(
        &fixture.pool,
        &fixture.workspace_id,
        &fixture.space_id,
        &fixture.layer_id,
    )
    .await
    .expect("target policy id");
    let mut policy_tx = fixture.pool.begin().await.expect("policy tx begins");
    lock_policy_for_update_in_tx(&mut policy_tx, &policy_id)
        .await
        .expect("target policy locks");
    bump_policy_epoch_in_tx(&mut policy_tx, &policy_id, Some(&fixture.account_id))
        .await
        .expect("target policy epoch bumps");

    let mut apply = spawn_weave_apply(&fixture, writer_headers, weave_id);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), &mut apply)
            .await
            .is_err(),
        "Apply should wait for the target policy row"
    );
    policy_tx.commit().await.expect("policy drift commits");
    let error = apply
        .await
        .expect("Apply task joins")
        .expect_err("policy drift rejects Apply");
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert!(error.message.contains("access policy changed"));
}

#[tokio::test]
async fn weave_apply_observes_role_change_committed_while_waiting() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (writer_id, writer_headers) =
        create_test_workspace_member_headers(&fixture, "member").await;
    let source_layer_id = create_weave_test_layer(&fixture, "Role Race Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "role-race.txt",
        b"role race\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let weave_id = create_planned_weave(
        &fixture,
        writer_headers.clone(),
        &source_layer_id,
        "Role race",
    )
    .await;
    let mut role_tx = fixture.pool.begin().await.expect("role tx begins");
    sqlx::query(
        "UPDATE workspace_memberships SET role = 'viewer' WHERE workspace_id = $1 AND account_id = $2",
    )
    .bind(&fixture.workspace_id)
    .bind(&writer_id)
    .execute(&mut *role_tx)
    .await
    .expect("role change is staged");

    let mut apply = spawn_weave_apply(&fixture, writer_headers, weave_id);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), &mut apply)
            .await
            .is_err(),
        "Apply should wait for the actor role row"
    );
    role_tx.commit().await.expect("role downgrade commits");
    let error = apply
        .await
        .expect("Apply task joins")
        .expect_err("role downgrade rejects Apply");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert!(error.message.contains("no longer writable"));
}

#[tokio::test]
async fn weave_apply_observes_team_change_committed_while_waiting() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (viewer_id, viewer_headers) =
        create_test_workspace_member_headers(&fixture, "viewer").await;
    let source_layer_id = create_weave_test_layer(&fixture, "Team Race Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "team-race.txt",
        b"team race\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let team_id = prefixed_id("team");
    sqlx::query("INSERT INTO teams (team_id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(&team_id)
        .bind(&fixture.workspace_id)
        .bind(format!("Weave Team {}", Uuid::new_v4().simple()))
        .execute(&fixture.pool)
        .await
        .expect("team inserts");
    sqlx::query(
        "INSERT INTO team_memberships (team_id, account_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(&team_id)
    .bind(&viewer_id)
    .execute(&fixture.pool)
    .await
    .expect("team membership inserts");
    for layer_id in [&source_layer_id, &fixture.layer_id] {
        sqlx::query(
            "INSERT INTO layer_memberships (membership_id, layer_id, team_id, role) VALUES ($1, $2, $3, 'writer')",
        )
        .bind(prefixed_id("layer_membership"))
        .bind(layer_id)
        .bind(&team_id)
        .execute(&fixture.pool)
        .await
        .expect("team Layer writer role inserts");
    }
    let weave_id = create_planned_weave(
        &fixture,
        viewer_headers.clone(),
        &source_layer_id,
        "Team race",
    )
    .await;
    let mut team_tx = fixture.pool.begin().await.expect("team tx begins");
    sqlx::query("DELETE FROM team_memberships WHERE team_id = $1 AND account_id = $2")
        .bind(&team_id)
        .bind(&viewer_id)
        .execute(&mut *team_tx)
        .await
        .expect("team removal is staged");

    let mut apply = spawn_weave_apply(&fixture, viewer_headers, weave_id);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), &mut apply)
            .await
            .is_err(),
        "Apply should wait for the actor team row"
    );
    team_tx.commit().await.expect("team removal commits");
    let error = apply
        .await
        .expect("Apply task joins")
        .expect_err("team removal rejects Apply");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert!(error.message.contains("no longer writable"));
}

#[tokio::test]
async fn weave_snapshot_lock_order_avoids_reverse_layer_deadlock() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let other_layer_id = create_weave_test_layer(&fixture, "Lock Order Source").await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));

    let first = spawn_snapshot_lock(
        &fixture,
        vec![other_layer_id.clone(), fixture.layer_id.clone()],
        barrier.clone(),
    );
    let second = spawn_snapshot_lock(
        &fixture,
        vec![fixture.layer_id.clone(), other_layer_id],
        barrier.clone(),
    );
    barrier.wait().await;
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(first, second)
    })
    .await
    .expect("reverse Layer snapshots must not deadlock");
    first
        .expect("first snapshot task joins")
        .expect("first snapshot succeeds");
    second
        .expect("second snapshot task joins")
        .expect("second snapshot succeeds");
}

#[tokio::test]
async fn weave_snapshot_does_not_lock_policies_in_other_workspaces() {
    let Some(first) = SyncTestFixture::create().await else {
        return;
    };
    let Some(second) = SyncTestFixture::create().await else {
        return;
    };
    let mut first_tx = first.pool.begin().await.expect("first snapshot tx begins");
    load_weave_authorization_snapshot_in_tx(
        &mut first_tx,
        &first.workspace_id,
        &first.space_id,
        std::slice::from_ref(&first.layer_id),
        &first.account_id,
    )
    .await
    .expect("first workspace snapshot locks");
    let second_policy_id = policy_id_for_layer(
        &second.pool,
        &second.workspace_id,
        &second.space_id,
        &second.layer_id,
    )
    .await
    .expect("second policy id");
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        insert_layer_access_rule(
            &second.pool,
            &second_policy_id,
            &prefixed_id("access_rule"),
            &unrestricted_weave_rule("other-workspace.txt", &second.account_id),
            Some(&second.account_id),
        ),
    )
    .await
    .expect("other workspace policy write must not wait")
    .expect("other workspace policy write succeeds");
    first_tx
        .rollback()
        .await
        .expect("first snapshot rolls back");
}

async fn create_planned_weave(
    fixture: &SyncTestFixture,
    headers: HeaderMap,
    source_layer_id: &str,
    title: &str,
) -> String {
    let Json(created) = create_weave_request(
        State(test_state(fixture.pool.clone())),
        headers,
        Path((fixture.workspace_id.clone(), fixture.space_id.clone())),
        Json(CreateWeaveRequestBody {
            source_layer_id: source_layer_id.to_string(),
            target_layer_id: fixture.layer_id.clone(),
            title: Some(title.to_string()),
            body: None,
        }),
    )
    .await
    .expect("planned Weave creates");
    created
        .get("weaveId")
        .and_then(Value::as_str)
        .expect("Weave id")
        .to_string()
}

fn spawn_weave_apply(
    fixture: &SyncTestFixture,
    headers: HeaderMap,
    weave_id: String,
) -> tokio::task::JoinHandle<Result<Json<Value>, ApiError>> {
    let pool = fixture.pool.clone();
    let workspace_id = fixture.workspace_id.clone();
    let space_id = fixture.space_id.clone();
    tokio::spawn(async move {
        apply_weave_request(
            State(test_state(pool)),
            headers,
            Path((workspace_id, space_id, weave_id)),
        )
        .await
    })
}

fn spawn_snapshot_lock(
    fixture: &SyncTestFixture,
    layer_ids: Vec<String>,
    barrier: std::sync::Arc<tokio::sync::Barrier>,
) -> tokio::task::JoinHandle<Result<(), ApiError>> {
    let pool = fixture.pool.clone();
    let workspace_id = fixture.workspace_id.clone();
    let space_id = fixture.space_id.clone();
    let account_id = fixture.account_id.clone();
    tokio::spawn(async move {
        let mut tx = pool.begin().await?;
        barrier.wait().await;
        load_weave_authorization_snapshot_in_tx(
            &mut tx,
            &workspace_id,
            &space_id,
            &layer_ids,
            &account_id,
        )
        .await?;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        tx.commit().await?;
        Ok(())
    })
}

async fn weave_test_artifact_id(fixture: &SyncTestFixture, layer_id: &str, path: &str) -> String {
    sqlx::query_scalar(
        r#"
        SELECT artifact_id
        FROM artifacts
        WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = $4
        "#,
    )
    .bind(&fixture.workspace_id)
    .bind(&fixture.space_id)
    .bind(layer_id)
    .bind(path)
    .fetch_one(&fixture.pool)
    .await
    .expect("artifact id loads")
}

async fn restrict_weave_artifact(
    fixture: &SyncTestFixture,
    layer_id: &str,
    artifact_id: &str,
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
            path: format!("artifact-only/{artifact_id}/**"),
            artifact_id: Some(artifact_id.to_string()),
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
    .expect("artifact access restriction installs");
}

fn unrestricted_weave_rule(path: &str, account_id: &str) -> LayerAccessRuleBody {
    LayerAccessRuleBody {
        id: None,
        path: path.to_string(),
        artifact_id: None,
        mode: "restricted".to_string(),
        visibility: "stub".to_string(),
        permissions: LayerAccessPermissionsBody {
            read: LayerAccessPrincipalsBody {
                accounts: vec![account_id.to_string()],
                teams: Vec::new(),
            },
            write: LayerAccessPrincipalsBody {
                accounts: vec![account_id.to_string()],
                teams: Vec::new(),
            },
            admin: LayerAccessPrincipalsBody::default(),
        },
    }
}
