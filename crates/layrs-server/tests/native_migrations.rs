//! Fresh V3 and native extensions only: legacy stores are never converted.
use layrs_server::{config::ServerConfig, identity::random_secret, installation};
use layrs_store_server::v3::schema;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{collections::BTreeMap, str::FromStr};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
#[ignore = "requires an explicitly isolated PostgreSQL administrator endpoint"]
async fn fresh_native_extensions_and_legacy_refusal() -> Result<()> {
    assert_eq!(
        std::env::var("LAYRS_TEST_DATABASE_ISOLATED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("LAYRS_TEST_DATABASE_URL")?;
    let options = PgConnectOptions::from_str(&url)?;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await?;
    for case in ["fresh", "native_baseline", "legacy"] {
        let database = format!("layrs_migrate_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {database}"))
            .execute(&admin)
            .await?;
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect_with(options.clone().database(&database))
            .await?;
        let config = ServerConfig::parse(&BTreeMap::from([
            ("LAYRS_PUBLIC_URL".into(), "https://layrs.example".into()),
            ("LAYRS_DATABASE_URL".into(), url.clone()),
            (
                "LAYRS_CAS_ROOT".into(),
                std::env::temp_dir().join(&database).display().to_string(),
            ),
            (
                "LAYRS_DEPLOYMENT_ID".into(),
                "deployment:migration-test".into(),
            ),
            ("LAYRS_STORE_ID".into(), "store:migration-test".into()),
            ("LAYRS_SIGNING_KEY".into(), random_secret()),
            ("LAYRS_BOOTSTRAP_SECRET".into(), random_secret()),
        ]))?;
        let result = exercise(&pool, &config, case).await;
        let native: bool = sqlx::query_scalar("SELECT to_regclass('schema_metadata') IS NOT NULL")
            .fetch_one(&pool)
            .await?;
        let role: Option<String> = if native {
            sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
                .fetch_optional(&pool)
                .await?
        } else {
            None
        };
        pool.close().await;
        sqlx::query(&format!("DROP DATABASE {database}"))
            .execute(&admin)
            .await?;
        if let Some(role) = role {
            assert!(
                role.starts_with("layrs_v3_w_")
                    && role.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            );
            sqlx::query(&format!("DROP ROLE {role}"))
                .execute(&admin)
                .await?;
        }
        result?;
    }
    Ok(())
}

async fn exercise(pool: &PgPool, config: &ServerConfig, case: &str) -> Result<()> {
    if case == "legacy" {
        sqlx::raw_sql("CREATE TABLE accounts (sentinel TEXT NOT NULL); INSERT INTO accounts VALUES('preserve legacy data');").execute(pool).await?;
        if installation::initialize(pool, config).await.is_ok() {
            return Err("legacy database was accepted".into());
        }
        let value: String = sqlx::query_scalar("SELECT sentinel FROM accounts")
            .fetch_one(pool)
            .await?;
        if value != "preserve legacy data" {
            return Err("legacy data changed".into());
        }
        let native: bool = sqlx::query_scalar("SELECT to_regclass('schema_metadata') IS NOT NULL")
            .fetch_one(pool)
            .await?;
        if native {
            return Err("legacy refusal left a partial native schema".into());
        }
        return Ok(());
    }
    if case == "native_baseline" {
        sqlx::raw_sql(schema::BASELINE_SQL).execute(pool).await?;
        sqlx::query("INSERT INTO stores(store_id,backend_kind,backend_config_jcs) VALUES('store:older','filesystem','{}')").execute(pool).await?;
    }
    installation::initialize(pool, config).await?;
    installation::verify_identity_schema(pool).await?;
    let before: Vec<(i64, String)> =
        sqlx::query_as("SELECT version,checksum FROM layrs_migrations.applied ORDER BY version")
            .fetch_all(pool)
            .await?;
    installation::initialize(pool, config).await?;
    let after: Vec<(i64, String)> =
        sqlx::query_as("SELECT version,checksum FROM layrs_migrations.applied ORDER BY version")
            .fetch_all(pool)
            .await?;
    if before != after {
        return Err("idempotent initialization changed migration ledger".into());
    }
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../../../scripts/migrations.lock.json"))?;
    let expected = lock["migrations"]
        .as_array()
        .ok_or("invalid migration lock")?;
    if before.len() != expected.len() {
        return Err("runner and migration lock length differ".into());
    }
    for ((version, checksum), migration) in before.iter().zip(expected) {
        if Some(*version) != migration["version"].as_i64()
            || Some(checksum.as_str()) != migration["sha256"].as_str()
        {
            return Err("runner differs from locked SQL checksums".into());
        }
    }
    if case == "native_baseline" {
        let preserved: String = sqlx::query_scalar(
            "SELECT backend_config_jcs FROM stores WHERE store_id='store:older'",
        )
        .fetch_one(pool)
        .await?;
        if preserved != "{}" {
            return Err("native extension changed existing data".into());
        }
    }
    sqlx::query(
        "UPDATE layrs_migrations.applied SET checksum='corrupt-test-checksum' WHERE version=1",
    )
    .execute(pool)
    .await?;
    if installation::verify_identity_schema(pool).await.is_ok()
        || installation::initialize(pool, config).await.is_ok()
    {
        return Err("corrupt migration checksum accepted".into());
    }
    sqlx::query("UPDATE layrs_migrations.applied SET checksum=$1 WHERE version=1")
        .bind(&before[0].1)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO layrs_migrations.applied(version,checksum) VALUES(9223372036854775807,'future-test-version')").execute(pool).await?;
    if installation::verify_identity_schema(pool).await.is_ok()
        || installation::initialize(pool, config).await.is_ok()
    {
        return Err("future migration accepted".into());
    }
    Ok(())
}
