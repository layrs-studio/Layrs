//! Run explicitly against a disposable PostgreSQL administrator endpoint.
use layrs_server::{
    config::ServerConfig,
    identity::{BearerVerifier, BootstrapRequest, Credentials, PgIdentity, random_secret},
    installation,
};
use sqlx::{
    Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{collections::BTreeMap, str::FromStr};

#[tokio::test]
#[ignore = "requires LAYRS_TEST_DATABASE_URL and CREATE DATABASE/ROLE privileges"]
async fn native_bootstrap_sessions_revocation_and_recovery() {
    let url = std::env::var("LAYRS_TEST_DATABASE_URL")
        .expect("set disposable administrator PostgreSQL URL");
    let options = PgConnectOptions::from_str(&url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let name = format!("layrs_identity_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect_with(options.database(&name))
        .await
        .unwrap();
    let secret = random_secret();
    let mut env = BTreeMap::from([
        ("LAYRS_PUBLIC_URL".into(), "https://layrs.example".into()),
        ("LAYRS_DATABASE_URL".into(), url),
        (
            "LAYRS_CAS_ROOT".into(),
            std::env::temp_dir().join(&name).display().to_string(),
        ),
        ("LAYRS_DEPLOYMENT_ID".into(), "deployment:test".into()),
        ("LAYRS_STORE_ID".into(), "store:test".into()),
        ("LAYRS_SIGNING_KEY".into(), random_secret()),
        ("LAYRS_BOOTSTRAP_SECRET".into(), secret.clone()),
    ]);
    let config = ServerConfig::parse(&env).unwrap();
    installation::initialize(&pool, &config).await.unwrap();
    // Idempotent rerun neither rotates nor re-arms bootstrap.
    installation::initialize(&pool, &config).await.unwrap();
    installation::verify_identity_schema(&pool).await.unwrap();
    let role: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let runtime_role = format!("rt_{}", uuid::Uuid::new_v4().simple());
    let runtime_password = random_secret();
    sqlx::query(&format!(
        "CREATE ROLE {runtime_role} LOGIN PASSWORD '{runtime_password}'"
    ))
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(&format!("GRANT {role} TO {runtime_role}"))
        .execute(&admin)
        .await
        .unwrap();
    let runtime_options = PgConnectOptions::from_str(&config.database_url)
        .unwrap()
        .database(&name)
        .username(&runtime_role)
        .password(&runtime_password);
    let runtime_pool = PgPoolOptions::new()
        .max_connections(8)
        .connect_with(runtime_options)
        .await
        .unwrap();
    installation::verify_identity_schema(&runtime_pool)
        .await
        .unwrap();
    let identity = PgIdentity::new(
        runtime_pool.clone(),
        "deployment:test".into(),
        "https://layrs.example".into(),
    )
    .await
    .unwrap();
    let request = || BootstrapRequest {
        secret: secret.clone(),
        login: "alice@example.com".into(),
        password: "correct horse battery staple".into(),
        display_name: "Alice".into(),
    };
    let (a, b) = tokio::join!(identity.bootstrap(request()), identity.bootstrap(request()));
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "exactly one concurrent bootstrap succeeds"
    );
    let session = a.or(b).unwrap();
    assert_eq!(
        identity.verify_bearer(&session.access_token).await.unwrap(),
        session.principal_id
    );
    assert!(identity.bootstrap(request()).await.is_err());
    identity.revoke(&session.access_token).await.unwrap();
    assert!(identity.verify_bearer(&session.access_token).await.is_err());
    let login = || Credentials {
        login: "alice@example.com".into(),
        password: "correct horse battery staple".into(),
    };
    let session = identity.login(login()).await.unwrap();
    exercise_http_identity(&identity, &pool, &session.access_token).await;
    identity
        .recover_password("alice@example.com", "replacement password 2026".into())
        .await
        .unwrap();
    assert!(identity.verify_bearer(&session.access_token).await.is_err());
    assert!(identity.login(login()).await.is_err());
    let session = identity
        .login(Credentials {
            login: "alice@example.com".into(),
            password: "replacement password 2026".into(),
        })
        .await
        .unwrap();
    sqlx::query("UPDATE layrs_identity.tokens SET created_at=clock_timestamp()-interval '2 days',expires_at=clock_timestamp()-interval '1 day'").execute(&pool).await.unwrap();
    assert!(identity.verify_bearer(&session.access_token).await.is_err());
    env.remove("LAYRS_BOOTSTRAP_SECRET");
    installation::initialize(&pool, &ServerConfig::parse(&env).unwrap())
        .await
        .unwrap();
    assert!(identity.bootstrap(request()).await.is_err());
    let row = sqlx::query("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let role: String = row.get("writer_role_name");
    assert!(
        role.starts_with("layrs_v3_w_")
            && role.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    );
    pool.close().await;
    runtime_pool.close().await;
    sqlx::query(&format!("DROP DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {role}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {runtime_role}"))
        .execute(&admin)
        .await
        .unwrap();
}

async fn post(
    identity: &PgIdentity,
    path: &str,
    token: Option<&str>,
    value: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let mut builder = axum::http::Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = identity
        .clone()
        .router()
        .oneshot(
            builder
                .body(axum::body::Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    (
        status,
        if body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        },
    )
}

async fn exercise_http_identity(identity: &PgIdentity, pool: &sqlx::PgPool, admin: &str) {
    use axum::http::StatusCode;
    use serde_json::json;
    let account = json!({"login":"bob@example.com","password":"Bob correct password 2026","displayName":"Bob"});
    let (status, bob) = post(identity, "/v3/auth/accounts", Some(admin), account.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{bob}");
    let bob_session = identity
        .login(Credentials {
            login: "bob@example.com".into(),
            password: "Bob correct password 2026".into(),
        })
        .await
        .unwrap();
    let (status, _) = post(
        identity,
        "/v3/auth/accounts",
        Some(&bob_session.access_token),
        account,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, service) = post(
        identity,
        "/v3/auth/service-accounts",
        Some(admin),
        json!({"displayName":"CI","label":"runner","expiresIn":3600}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{service}");
    let service_token = service["access_token"].as_str().unwrap();
    assert!(identity.verify_bearer(service_token).await.is_ok());
    let memberships: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_memberships WHERE principal_id=$1")
            .bind(service["principal_id"].as_str().unwrap())
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        memberships, 0,
        "service principal inherits no administrator rights"
    );
    let (status, rotated) = post(
        identity,
        "/v3/auth/service-accounts/rotate",
        Some(admin),
        json!({"principalId":service["principal_id"],"label":"rotated runner","expiresIn":3600}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    assert!(identity.verify_bearer(service_token).await.is_err());
    assert!(
        identity
            .verify_bearer(rotated["access_token"].as_str().unwrap())
            .await
            .is_ok()
    );
    let (status, start) = post(
        identity,
        "/v3/auth/devices/start",
        None,
        json!({"label":"Alice laptop"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{start}");
    let poll = json!({"deviceCode":start["deviceCode"]});
    let (status, _) = post(identity, "/v3/auth/devices/poll", None, poll.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, _) = post(identity, "/v3/auth/devices/poll", None, poll.clone()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, preview) = post(
        identity,
        "/v3/auth/devices/inspect",
        Some(admin),
        json!({"userCode":start["userCode"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(preview["label"], "Alice laptop");
    let (status, _) = post(
        identity,
        "/v3/auth/devices/approve",
        Some(rotated["access_token"].as_str().unwrap()),
        json!({"userCode":start["userCode"]}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "service token cannot mint a longer lived device token"
    );
    let (status, _) = post(
        identity,
        "/v3/auth/devices/approve",
        Some(admin),
        json!({"userCode":start["userCode"]}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    sqlx::query("UPDATE layrs_identity.device_requests SET next_poll_at=clock_timestamp()-interval '1 second'").execute(pool).await.unwrap();
    let (status, device) = post(identity, "/v3/auth/devices/poll", None, poll.clone()).await;
    assert_eq!(status, StatusCode::OK, "{device}");
    assert!(
        identity
            .verify_bearer(device["access_token"].as_str().unwrap())
            .await
            .is_ok()
    );
    let (status, _) = post(identity, "/v3/auth/devices/poll", None, poll).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "token emitted once");
    let (status, _) = post(
        identity,
        "/v3/auth/accounts/revoke",
        Some(admin),
        json!({"principalId":bob["principalId"]}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        identity
            .verify_bearer(&bob_session.access_token)
            .await
            .is_err()
    );
}
