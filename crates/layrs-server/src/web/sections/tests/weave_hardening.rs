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

    let (apply, pid) = spawn_weave_apply(&fixture, writer_headers, weave_id).await;
    wait_for_backend_lock(&fixture.pool, pid).await;
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

    let (apply, pid) = spawn_weave_apply(&fixture, writer_headers, weave_id).await;
    wait_for_backend_lock(&fixture.pool, pid).await;
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

    let (apply, pid) = spawn_weave_apply(&fixture, viewer_headers, weave_id).await;
    wait_for_backend_lock(&fixture.pool, pid).await;
    team_tx.commit().await.expect("team removal commits");
    let error = apply
        .await
        .expect("Apply task joins")
        .expect_err("team removal rejects Apply");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    assert!(error.message.contains("no longer writable"));
}

#[tokio::test]
async fn weave_create_revocations_roll_back_without_a_durable_request() {
    for revocation in ["role", "team", "policy"] {
        assert_create_revocation_rolls_back(revocation).await;
    }
}

async fn assert_create_revocation_rolls_back(revocation: &str) {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let initial_role = if revocation == "team" {
        "viewer"
    } else {
        "member"
    };
    let (actor_id, headers) = create_test_workspace_member_headers(&fixture, initial_role).await;
    let source_layer_id = create_weave_test_layer(&fixture, "Create Revocation Source").await;
    publish_weave_test_file(
        &fixture,
        &source_layer_id,
        "create-race.txt",
        b"revocation\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let team_id = prefixed_id("team");
    if revocation == "team" {
        sqlx::query(
            "INSERT INTO teams (team_id, workspace_id, name) VALUES ($1, $2, 'Create race')",
        )
        .bind(&team_id)
        .bind(&fixture.workspace_id)
        .execute(&fixture.pool)
        .await
        .expect("team inserts");
        sqlx::query(
            "INSERT INTO team_memberships (team_id, account_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(&team_id)
        .bind(&actor_id)
        .execute(&fixture.pool)
        .await
        .expect("team member inserts");
        for layer_id in [&source_layer_id, &fixture.layer_id] {
            sqlx::query("INSERT INTO layer_memberships (membership_id, layer_id, team_id, role) VALUES ($1, $2, $3, 'writer')")
                .bind(prefixed_id("layer_membership")).bind(layer_id).bind(&team_id)
                .execute(&fixture.pool).await.expect("team writer role inserts");
        }
    }
    let mut revoke = fixture.pool.begin().await.expect("revocation tx begins");
    match revocation {
        "role" => {
            sqlx::query("UPDATE workspace_memberships SET role = 'viewer' WHERE workspace_id = $1 AND account_id = $2")
                .bind(&fixture.workspace_id).bind(&actor_id).execute(&mut *revoke).await.expect("role revocation stages");
        }
        "team" => {
            sqlx::query("DELETE FROM team_memberships WHERE team_id = $1 AND account_id = $2")
                .bind(&team_id)
                .bind(&actor_id)
                .execute(&mut *revoke)
                .await
                .expect("team revocation stages");
        }
        "policy" => {
            let policy_id = policy_id_for_layer(
                &fixture.pool,
                &fixture.workspace_id,
                &fixture.space_id,
                &fixture.layer_id,
            )
            .await
            .expect("target policy id");
            sqlx::query("UPDATE layer_access_policies SET policy_epoch = policy_epoch + 1 WHERE policy_id = $1")
                .bind(&policy_id).execute(&mut *revoke).await.expect("policy revocation locks");
            sqlx::query("INSERT INTO layer_access_policy_rules (rule_id, policy_id, path, mode, visibility, read_account_ids, write_account_ids) VALUES ($1, $2, 'create-race.txt', 'restricted', 'stub', $3, $4)")
                .bind(prefixed_id("access_rule")).bind(policy_id)
                .bind(vec![fixture.account_id.clone(), actor_id.clone()]).bind(vec![fixture.account_id.clone()])
                .execute(&mut *revoke).await.expect("restrictive rule stages");
        }
        _ => unreachable!(),
    }
    let title = format!("Create {revocation} revocation");
    let (pool, pid) = dedicated_test_pool(&fixture).await;
    let workspace_id = fixture.workspace_id.clone();
    let space_id = fixture.space_id.clone();
    let target_layer_id = fixture.layer_id.clone();
    let source = source_layer_id.clone();
    let request_title = title.clone();
    let create = tokio::spawn(async move {
        create_weave_request(
            State(test_state(pool)),
            headers,
            Path((workspace_id, space_id)),
            Json(CreateWeaveRequestBody {
                source_layer_id: source,
                target_layer_id,
                title: Some(request_title),
                body: None,
            }),
        )
        .await
    });
    wait_for_backend_lock(&fixture.pool, pid).await;
    revoke.commit().await.expect("revocation commits");
    let error = create
        .await
        .expect("Create task joins")
        .expect_err("revocation rejects Create");
    assert_eq!(error.status, StatusCode::FORBIDDEN);
    let durable: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM weave_requests WHERE workspace_id = $1 AND title = $2",
    )
    .bind(&fixture.workspace_id)
    .bind(title)
    .fetch_one(&fixture.pool)
    .await
    .expect("durable request count loads");
    assert_eq!(durable, 0, "{revocation} revocation must roll Create back");
}

#[tokio::test]
async fn weave_apply_delete_layer_and_space_races_do_not_deadlock() {
    for delete_space_target in [false, true] {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let source = create_weave_test_layer(&fixture, "Delete Race Source").await;
        publish_weave_test_file(
            &fixture,
            &source,
            "delete-race.txt",
            b"race\n",
            "text/plain",
            None,
            0,
        )
        .await;
        let weave_id =
            create_planned_weave(&fixture, fixture.bearer_headers(), &source, "Delete race").await;
        assert_apply_delete_contention(&fixture, weave_id, delete_space_target).await;
    }
}

#[tokio::test]
async fn weave_apply_waits_for_v2_publish_of_missing_target_path() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let source = create_weave_test_layer(&fixture, "Missing Target Source").await;
    let path = "new-target.txt";
    let bytes = b"published while Apply waits\n";
    publish_weave_test_file(
        &fixture,
        &source,
        path,
        bytes,
        "text/plain",
        None,
        0,
    )
    .await;
    let weave_id = create_planned_weave(
        &fixture,
        fixture.bearer_headers(),
        &source,
        "Missing target publish race",
    )
    .await;
    let target_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM artifacts WHERE workspace_id = $1 AND space_id = $2 AND layer_id = $3 AND logical_path = $4",
    )
    .bind(&fixture.workspace_id)
    .bind(&fixture.space_id)
    .bind(&fixture.layer_id)
    .bind(path)
    .fetch_one(&fixture.pool)
    .await
    .expect("missing target artifact count loads");
    assert_eq!(target_count, 0);

    assert_apply_waits_for_paused_publish(
        &fixture,
        weave_id,
        path,
        &blake3_digest_for_bytes(bytes),
    )
    .await;
}

#[tokio::test]
async fn weave_terminal_transition_wins_request_row_races() {
    for loser in ["resolve", "abort", "apply"] {
        let Some(fixture) = SyncTestFixture::create().await else {
            return;
        };
        let (weave_id, mutation) = if loser == "resolve" {
            let (weave_id, conflict_id) = create_raw_conflicted_weave(&fixture).await;
            (weave_id, TerminalWeaveLoser::Resolve(conflict_id))
        } else {
            let source = create_weave_test_layer(&fixture, "Terminal Race Source").await;
            publish_weave_test_file(
                &fixture,
                &source,
                "terminal-race.txt",
                b"terminal\n",
                "text/plain",
                None,
                0,
            )
            .await;
            let weave_id =
                create_planned_weave(&fixture, fixture.bearer_headers(), &source, loser).await;
            let mutation = if loser == "abort" {
                TerminalWeaveLoser::Abort
            } else {
                TerminalWeaveLoser::Apply
            };
            (weave_id, mutation)
        };
        assert_terminal_transition_wins(&fixture, weave_id, mutation).await;
    }
}

#[tokio::test]
async fn weave_apply_normalizes_all_inaccessible_states() {
    let Some(fixture) = SyncTestFixture::create().await else {
        return;
    };
    let (_, outsider) = create_test_workspace_member_headers(&fixture, "viewer").await;
    let source = create_weave_test_layer(&fixture, "Inaccessible Source").await;
    publish_weave_test_file(
        &fixture,
        &source,
        "hidden.txt",
        b"hidden\n",
        "text/plain",
        None,
        0,
    )
    .await;
    let open = create_planned_weave(
        &fixture,
        fixture.bearer_headers(),
        &source,
        "Open inaccessible",
    )
    .await;
    let applied = create_planned_weave(
        &fixture,
        fixture.bearer_headers(),
        &source,
        "Applied inaccessible",
    )
    .await;
    let _ = apply_weave_request(
        State(test_state(fixture.pool.clone())),
        fixture.bearer_headers(),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            applied.clone(),
        )),
    )
    .await
    .expect("admin applies");
    let aborted = create_planned_weave(
        &fixture,
        fixture.bearer_headers(),
        &source,
        "Aborted inaccessible",
    )
    .await;
    let _ = abort_weave_request(
        State(test_state(fixture.pool.clone())),
        fixture.bearer_headers(),
        Path((
            fixture.workspace_id.clone(),
            fixture.space_id.clone(),
            aborted.clone(),
        )),
    )
    .await
    .expect("admin aborts");
    let malformed_json = create_planned_weave(
        &fixture,
        fixture.bearer_headers(),
        &source,
        "Malformed replay JSON",
    )
    .await;
    sqlx::query("UPDATE weave_step_replays SET path_replays = '{}'::jsonb WHERE weave_id = $1")
        .bind(&malformed_json)
        .execute(&fixture.pool)
        .await
        .expect("malformed replay JSON stages");
    let malformed_path = create_planned_weave(
        &fixture,
        fixture.bearer_headers(),
        &source,
        "Malformed replay path",
    )
    .await;
    sqlx::query("UPDATE weave_step_replays SET changed_paths = ARRAY['../private'] WHERE weave_id = $1")
        .bind(&malformed_path)
        .execute(&fixture.pool)
        .await
        .expect("malformed replay path stages");
    let mut normalized = None;
    for weave_id in [
        open,
        applied,
        aborted,
        malformed_json,
        malformed_path,
        prefixed_id("missing_weave"),
    ] {
        let error = apply_weave_request(
            State(test_state(fixture.pool.clone())),
            outsider.clone(),
            Path((
                fixture.workspace_id.clone(),
                fixture.space_id.clone(),
                weave_id,
            )),
        )
        .await
        .expect_err("outsider cannot observe Apply state");
        let response = (error.status, error.message);
        assert_eq!(normalized.get_or_insert(response.clone()), &response);
    }
    assert_eq!(
        normalized,
        Some((
            StatusCode::FORBIDDEN,
            "Weave request is inaccessible".to_string()
        ))
    );
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
