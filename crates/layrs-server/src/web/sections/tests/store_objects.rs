use super::*;

mod chunks_immutable;
mod delete_shared_ownership;
mod file_tree_objects;
mod permissions_redaction_content;

pub(super) struct SyncTestFixture {
    pub(super) pool: PgPool,
    pub(super) account_id: String,
    pub(super) workspace_id: String,
    pub(super) space_id: String,
    pub(super) layer_id: String,
    pub(super) bearer_token: String,
}

impl SyncTestFixture {
    pub(super) async fn create() -> Option<Self> {
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

    pub(super) fn bearer_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.bearer_token))
                .expect("bearer header is valid"),
        );
        headers
    }
}

pub(super) async fn create_test_workspace_member_headers(
    fixture: &SyncTestFixture,
    role: &str,
) -> (String, HeaderMap) {
    let suffix = Uuid::new_v4().simple().to_string();
    let account_id = format!("account_{suffix}");
    let device_id = format!("device_{suffix}");
    let bearer_token = token("desktop_test_member");
    sqlx::query(
        "INSERT INTO accounts (account_id, email, display_name) VALUES ($1, $2, 'Test Member')",
    )
    .bind(&account_id)
    .bind(format!("member-{suffix}@example.com"))
    .execute(&fixture.pool)
    .await
    .expect("test member account inserted");
    sqlx::query(
        "INSERT INTO workspace_memberships (membership_id, workspace_id, account_id, role) VALUES ($1, $2, $3, $4)",
    )
    .bind(prefixed_id("membership"))
    .bind(&fixture.workspace_id)
    .bind(&account_id)
    .bind(role)
    .execute(&fixture.pool)
    .await
    .expect("test workspace membership inserted");
    sqlx::query(
        "INSERT INTO desktop_devices (device_id, account_id, display_name, public_key_thumbprint, last_seen_at) VALUES ($1, $2, 'Test Member Desktop', $3, now())",
    )
    .bind(&device_id)
    .bind(&account_id)
    .bind(format!("thumbprint-{suffix}"))
    .execute(&fixture.pool)
    .await
    .expect("test member device inserted");
    sqlx::query(
        r#"
        INSERT INTO desktop_device_tokens
            (token_id, device_id, account_id, access_token_digest,
             refresh_token_digest, expires_at)
        VALUES ($1, $2, $3, $4, $5, now() + interval '1 day')
        "#,
    )
    .bind(prefixed_id("desktop_token"))
    .bind(&device_id)
    .bind(&account_id)
    .bind(digest_secret(&bearer_token))
    .bind(digest_secret(&token("desktop_refresh_member")))
    .execute(&fixture.pool)
    .await
    .expect("test member token inserted");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {bearer_token}"))
            .expect("test member bearer header"),
    );
    (account_id, headers)
}

fn test_database_url() -> Option<String> {
    let database_url = std::env::var("LAYRS_TEST_DATABASE_URL")
        .or_else(|_| std::env::var("LAYRS_DATABASE_URL"))
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok();
    if database_url.is_none() && std::env::var("LAYRS_REQUIRE_TEST_DATABASE").as_deref() == Ok("1")
    {
        panic!(
            "Postgres integration tests are required: set LAYRS_TEST_DATABASE_URL to a reachable test database"
        );
    }
    database_url
}

pub(super) fn test_tree_id(entries: &[(&str, &str, usize)]) -> String {
    let mut entries = entries.to_vec();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let mut material = String::new();
    for (path, file_object_id, size) in entries {
        material.push_str(path);
        material.push('\0');
        material.push_str(file_object_id);
        material.push('\0');
        material.push_str(&size.to_string());
        material.push('\n');
    }
    blake3_digest_for_bytes(material.as_bytes())
}

pub(super) fn test_state(pool: PgPool) -> AppState {
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

pub(super) async fn dedicated_test_pool(fixture: &SyncTestFixture) -> (PgPool, i32) {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(fixture.pool.connect_options().as_ref().clone())
        .await
        .expect("dedicated test connection opens");
    let pid = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .expect("dedicated backend pid loads");
    (pool, pid)
}

pub(super) async fn wait_for_backend_lock(pool: &PgPool, pid: i32) {
    wait_for_backend_blocker(pool, pid, None).await;
}

pub(super) async fn wait_for_backend_blocked_by(pool: &PgPool, pid: i32, blocker_pid: i32) {
    wait_for_backend_blocker(pool, pid, Some(blocker_pid)).await;
}

async fn wait_for_backend_blocker(pool: &PgPool, pid: i32, blocker_pid: Option<i32>) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                r#"
                SELECT wait_event_type = 'Lock'
                  AND cardinality(pg_blocking_pids(pid)) > 0
                  AND ($2::int IS NULL OR $2 = ANY(pg_blocking_pids(pid)))
                FROM pg_stat_activity
                WHERE pid = $1
                "#,
            )
            .bind(pid)
            .bind(blocker_pid)
            .fetch_one(pool)
            .await
            .expect("backend lock state is observable");
            if blocked {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("backend reaches the expected observable lock wait");
}

pub(super) async fn spawn_weave_apply(
    fixture: &SyncTestFixture,
    headers: HeaderMap,
    weave_id: String,
) -> (tokio::task::JoinHandle<Result<Json<Value>, ApiError>>, i32) {
    let (pool, pid) = dedicated_test_pool(fixture).await;
    let path = (
        fixture.workspace_id.clone(),
        fixture.space_id.clone(),
        weave_id,
    );
    let task = tokio::spawn(async move {
        apply_weave_request(State(test_state(pool)), headers, Path(path)).await
    });
    (task, pid)
}

pub(super) async fn weave_durable_state(fixture: &SyncTestFixture, weave_id: &str) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('request', to_jsonb(wr), 'session', to_jsonb(ws), 'conflicts', COALESCE((SELECT jsonb_agg(to_jsonb(wc) ORDER BY wc.conflict_id) FROM weave_conflicts wc WHERE wc.weave_id = wr.weave_id), '[]'::jsonb), 'resolutions', COALESCE((SELECT jsonb_agg(to_jsonb(r) ORDER BY r.resolution_id) FROM weave_resolutions r JOIN weave_conflicts wc ON wc.conflict_id = r.conflict_id WHERE wc.weave_id = wr.weave_id), '[]'::jsonb)) FROM weave_requests wr JOIN weave_sessions ws ON ws.weave_id = wr.weave_id WHERE wr.weave_id = $1")
        .bind(weave_id).fetch_one(&fixture.pool).await.expect("durable Weave state loads")
}

pub(super) enum TerminalWeaveLoser {
    Apply,
    Abort,
    Resolve(String),
}

pub(super) async fn assert_terminal_transition_wins(
    fixture: &SyncTestFixture,
    weave_id: String,
    loser: TerminalWeaveLoser,
) {
    let (terminal_pool, terminal_pid) = dedicated_test_pool(fixture).await;
    let mut terminal = terminal_pool.begin().await.expect("terminal tx begins");
    let transitioned = sqlx::query(
        "UPDATE weave_requests SET status = 'aborted', updated_at = now() WHERE weave_id = $1 AND status IN ('open', 'preview', 'conflicted', 'resolved')",
    )
    .bind(&weave_id)
    .execute(&mut *terminal)
    .await
    .expect("terminal request transition stages");
    assert_eq!(transitioned.rows_affected(), 1);
    sqlx::query("UPDATE weave_sessions SET status = 'aborted', updated_at = now() WHERE weave_id = $1")
        .bind(&weave_id)
        .execute(&mut *terminal)
        .await
        .expect("terminal session transition stages");

    let (loser_pool, loser_pid) = dedicated_test_pool(fixture).await;
    let headers = fixture.bearer_headers();
    let path = (
        fixture.workspace_id.clone(),
        fixture.space_id.clone(),
        weave_id.clone(),
    );
    let task = tokio::spawn(async move {
        match loser {
            TerminalWeaveLoser::Apply => {
                apply_weave_request(State(test_state(loser_pool)), headers, Path(path)).await
            }
            TerminalWeaveLoser::Abort => {
                abort_weave_request(State(test_state(loser_pool)), headers, Path(path)).await
            }
            TerminalWeaveLoser::Resolve(conflict_id) => {
                resolve_weave_conflict(
                    State(test_state(loser_pool)),
                    headers,
                    Path((path.0, path.1, path.2, conflict_id)),
                    Json(ResolveWeaveConflictBody {
                        method: "incoming".to_string(),
                        block_id: None,
                        manual_text: None,
                    }),
                )
                .await
            }
        }
    });
    wait_for_backend_blocked_by(&fixture.pool, loser_pid, terminal_pid).await;
    terminal.commit().await.expect("terminal transition commits");
    let terminal_snapshot = weave_durable_state(fixture, &weave_id).await;
    let error = task
        .await
        .expect("competing mutation joins")
        .expect_err("terminal request rejects competing mutation");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(weave_durable_state(fixture, &weave_id).await, terminal_snapshot);
}

pub(super) async fn assert_apply_waits_for_paused_publish(
    fixture: &SyncTestFixture,
    weave_id: String,
    path: &str,
    file_object_id: &str,
) {
    let policy_epoch = current_policy_epoch(
        &fixture.pool,
        &fixture.workspace_id,
        &fixture.space_id,
        &fixture.layer_id,
    )
    .await
    .expect("target policy epoch loads");
    let (publish_pool, publish_pid) = dedicated_test_pool(fixture).await;
    let mut publish = publish_pool.begin().await.expect("publish tx begins");
    lock_layer_paths_in_tx(
        &mut publish,
        &fixture.workspace_id,
        &fixture.space_id,
        std::slice::from_ref(&fixture.layer_id),
        &[path.to_string()],
    )
    .await
    .expect("publish takes target Layer/path lock");
    let body = serde_json::from_value(json!({
        "path": path,
        "kind": "note",
        "mediaType": "text/plain",
        "fileObjectId": file_object_id
    }))
    .expect("paused publish body deserializes");
    let (_, event_id, _) = publish_artifact_v2_in_tx(
        &fixture.pool,
        &mut publish,
        &fixture.workspace_id,
        &fixture.space_id,
        &fixture.layer_id,
        &fixture.account_id,
        body,
    )
    .await
    .expect("publish inserts previously missing artifact");

    let (apply, apply_pid) = spawn_weave_apply(fixture, fixture.bearer_headers(), weave_id).await;
    wait_for_backend_blocked_by(&fixture.pool, apply_pid, publish_pid).await;
    let root_tree_id = rebuild_layer_tree_in_tx(
        &mut publish,
        &fixture.workspace_id,
        &fixture.space_id,
        &fixture.layer_id,
        &fixture.account_id,
    )
    .await
    .expect("paused publish rebuilds target tree");
    advance_layer_head_in_tx(
        &mut publish,
        &fixture.workspace_id,
        &fixture.space_id,
        &fixture.layer_id,
        root_tree_id.as_deref(),
        policy_epoch,
        Some(&event_id),
        &fixture.account_id,
    )
    .await
    .expect("paused publish advances target head");
    publish.commit().await.expect("paused publish commits");
    let error = apply
        .await
        .expect("Apply task joins")
        .expect_err("committed publish invalidates prepared target generation");
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert!(!error.message.contains("40P01"));
}

pub(super) async fn assert_apply_delete_contention(
    fixture: &SyncTestFixture,
    weave_id: String,
    delete_space_target: bool,
) {
    let policy_id = policy_id_for_layer(
        &fixture.pool,
        &fixture.workspace_id,
        &fixture.space_id,
        &fixture.layer_id,
    )
    .await
    .expect("target policy id loads");
    let (blocker_pool, blocker_pid) = dedicated_test_pool(fixture).await;
    let mut blocker = blocker_pool.begin().await.expect("policy blocker begins");
    sqlx::query("SELECT policy_id FROM layer_access_policies WHERE policy_id = $1 FOR UPDATE")
        .bind(policy_id)
        .fetch_one(&mut *blocker)
        .await
        .expect("target policy row locks");
    let (apply, apply_pid) = spawn_weave_apply(fixture, fixture.bearer_headers(), weave_id).await;
    wait_for_backend_blocked_by(&fixture.pool, apply_pid, blocker_pid).await;

    let (delete_pool, delete_pid) = dedicated_test_pool(fixture).await;
    let headers = fixture.bearer_headers();
    let workspace_id = fixture.workspace_id.clone();
    let space_id = fixture.space_id.clone();
    let layer_id = fixture.layer_id.clone();
    let delete = tokio::spawn(async move {
        if delete_space_target {
            delete_space(State(test_state(delete_pool)), Path((workspace_id, space_id)), headers)
                .await
        } else {
            delete_layer(
                State(test_state(delete_pool)),
                Path((workspace_id, space_id, layer_id)),
                headers,
            )
            .await
        }
    });
    wait_for_backend_blocked_by(&fixture.pool, delete_pid, apply_pid).await;
    blocker.commit().await.expect("policy blocker commits");
    let (apply, delete) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(apply, delete)
    })
    .await
    .expect("contending Apply/Delete operations complete");
    for result in [apply.expect("Apply task joins"), delete.expect("Delete task joins")] {
        if let Err(error) = result {
            assert!(!error.message.contains("40P01") && !error.message.contains("deadlock"));
        }
    }
}

pub(super) fn publish_artifact(
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
