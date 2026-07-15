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
    if database_url.is_none() && std::env::var("LAYRS_REQUIRE_POSTGRES_TESTS").as_deref() == Ok("1")
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
