//! Explicit real-PostgreSQL authorization and monitor CLI recipe.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use layrs_server::{
    config::ServerConfig,
    identity::{BootstrapRequest, Credentials, PgIdentity, random_secret},
    installation,
    storage_budget::{self, StorageMonitor, StorageThresholds},
};
use serde_json::{Value, json};
use sqlx::{
    Executor,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{collections::BTreeMap, str::FromStr};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires LAYRS_TEST_DATABASE_URL; creates isolated database/runtime role"]
async fn administrator_capacity_http_and_monitor_exit_codes() {
    let endpoint = std::env::var("LAYRS_TEST_DATABASE_URL").expect("explicit PostgreSQL recipe");
    let options = PgConnectOptions::from_str(&endpoint).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let database = format!("layrs_capacity_{suffix}");
    admin
        .execute(format!("CREATE DATABASE {database}").as_str())
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options.clone().database(&database))
        .await
        .unwrap();
    let root = std::env::temp_dir().join(&database);
    std::fs::create_dir(&root).unwrap();
    let mut environment = BTreeMap::from([
        ("LAYRS_PUBLIC_URL".into(), "https://capacity.example".into()),
        ("LAYRS_DATABASE_URL".into(), endpoint.clone()),
        ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
        ("LAYRS_DEPLOYMENT_ID".into(), "deployment:capacity".into()),
        ("LAYRS_STORE_ID".into(), "store:capacity".into()),
        ("LAYRS_SIGNING_KEY".into(), random_secret()),
        ("LAYRS_BOOTSTRAP_SECRET".into(), random_secret()),
    ]);
    let config = ServerConfig::parse(&environment).unwrap();
    installation::initialize(&pool, &config).await.unwrap();
    let writer: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let role = format!("capacity_{suffix}");
    let password = random_secret();
    admin
        .execute(format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("GRANT {writer} TO {role}").as_str())
        .await
        .unwrap();
    let runtime = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(
            options
                .database(&database)
                .username(&role)
                .password(&password),
        )
        .await
        .unwrap();
    let identity = PgIdentity::new(
        runtime.clone(),
        config.deployment_id.clone(),
        config.public_url.as_str().into(),
    )
    .await
    .unwrap();
    let session = identity
        .bootstrap(BootstrapRequest {
            secret: config.bootstrap_secret.clone().unwrap(),
            login: "admin@example.com".into(),
            password: "Admin capacity password 2026".into(),
            display_name: "Admin".into(),
        })
        .await
        .unwrap();
    let monitor = StorageMonitor::new(
        runtime.clone(),
        root.clone(),
        config.cas_quotas,
        StorageThresholds::default(),
        config.deployment_id.clone(),
    )
    .unwrap();
    let router = storage_budget::router(identity.clone(), monitor.clone());
    let path = "/v3/admin/storage-budget";
    for token in [None, Some("bogus")] {
        let (status, body) = request(&router, "GET", path, token, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body, json!({"error":{"code":"unauthorized"}}));
    }
    let (status, report) = request(
        &router,
        "GET",
        path,
        Some(&session.access_token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["schemaVersion"], "layrs.storage-budget.v1");
    assert!(report["databaseBytes"].as_u64().unwrap() > 0);
    let volume = &report["casVolume"];
    assert!(volume["totalBytes"].as_u64().unwrap() >= volume["availableBytes"].as_u64().unwrap());
    assert!(volume["availableBytes"].as_u64().unwrap() > 0);
    assert!(report["physicalCasBytes"].is_null());
    assert!(!report.to_string().contains(&root.display().to_string()));
    assert!(!report.to_string().contains(&session.access_token));
    assert!(!report.to_string().contains(&endpoint));
    let (status, body) = request(
        &router,
        "GET",
        path,
        Some(&session.access_token),
        Some("https://elsewhere.example"),
        None,
    )
    .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"unauthorized"}})
        )
    );

    let account = json!({"login":"reader@example.com","password":"Reader capacity password 2026","displayName":"Reader"});
    let (status, _) = request(
        &identity.clone().router(),
        "POST",
        "/v3/auth/accounts",
        Some(&session.access_token),
        None,
        Some(account),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let reader = identity
        .login(Credentials {
            login: "reader@example.com".into(),
            password: "Reader capacity password 2026".into(),
        })
        .await
        .unwrap();
    for uri in [
        path,
        "/v3/admin/storage-budget?spaceId=space:does-not-exist",
    ] {
        let (status, body) =
            request(&router, "GET", uri, Some(&reader.access_token), None, None).await;
        assert_eq!(
            (status, body),
            (
                StatusCode::UNAUTHORIZED,
                json!({"error":{"code":"unauthorized"}})
            )
        );
    }
    let (status, body) = request(
        &router,
        "GET",
        "/v3/admin/storage-budget?spaceId=space:does-not-exist",
        Some(&session.access_token),
        None,
        None,
    )
    .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::BAD_REQUEST,
            json!({"error":{"code":"invalid_identity_request"}})
        )
    );
    identity.revoke(&session.access_token).await.unwrap();
    let (status, body) = request(
        &router,
        "GET",
        path,
        Some(&session.access_token),
        None,
        None,
    )
    .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"unauthorized"}})
        )
    );

    // Same real monitor via the operator CLI, using the non-privileged writer.
    let mut runtime_url = url::Url::parse(&endpoint).unwrap();
    runtime_url.set_path(&database);
    runtime_url.set_username(&role).unwrap();
    runtime_url.set_password(Some(&password)).unwrap();
    environment.insert("LAYRS_DATABASE_URL".into(), runtime_url.to_string());
    environment.insert("LAYRS_STORAGE_DATABASE_CRITICAL_BYTES".into(), "1".into());
    let (code, report) = cli(&environment);
    assert_eq!(code, 2);
    assert_eq!(report["severity"], "critical");
    environment.insert(
        "LAYRS_STORAGE_DATABASE_CRITICAL_BYTES".into(),
        u64::MAX.to_string(),
    );
    environment.insert("LAYRS_STORAGE_DATABASE_WARN_BYTES".into(), "1".into());
    environment.insert("LAYRS_STORAGE_VOLUME_WARN_PERCENT".into(), "99".into());
    environment.insert("LAYRS_STORAGE_VOLUME_CRITICAL_PERCENT".into(), "100".into());
    let (code, report) = cli(&environment);
    assert_eq!(code, 1);
    assert_eq!(report["severity"], "warning");
    environment.insert(
        "LAYRS_CAS_ROOT".into(),
        root.join("missing-volume-probe").display().to_string(),
    );
    let (code, report) = cli(&environment);
    assert_eq!(code, 3);
    assert_eq!(
        report,
        json!({"schemaVersion":"layrs.storage-budget.v1","severity":"unknown","error":{"code":"storage_observation_unavailable"}})
    );
    environment.insert("LAYRS_STORAGE_QUOTA_WARN_PERCENT".into(), "wrong".into());
    assert_eq!(cli(&environment).0, 3);

    drop(router);
    drop(monitor);
    drop(identity);
    runtime.close().await;
    pool.close().await;
    admin
        .execute(format!("DROP DATABASE {database} WITH (FORCE)").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("DROP ROLE {writer}").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("DROP ROLE {role}").as_str())
        .await
        .unwrap();
    admin.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    origin: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let response = router
        .clone()
        .oneshot(
            request
                .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    if path.starts_with("/v3/admin/") {
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    }
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn cli(environment: &BTreeMap<String, String>) -> (i32, Value) {
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_layrs-server"))
        .arg("storage-check")
        .envs(environment)
        .output()
        .unwrap();
    (
        result.status.code().unwrap(),
        serde_json::from_slice(&result.stdout).unwrap(),
    )
}
