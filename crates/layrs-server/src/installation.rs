//! Explicit fresh installation and checksummed identity migration.
//! The migration ledger lives outside the immutable native catalog fingerprint.
use crate::{config::ServerConfig, identity::token_digest};
use layrs_store_server::v3::schema;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

const IDENTITY_SQL: &str = include_str!("../../layrs-store-server/migrations/0002_v3_identity.sql");
const WRITER_SQL: &str =
    include_str!("../../layrs-store-server/migrations/0003_v3_writer_access.sql");
const DEVICE_SQL: &str = include_str!("../../layrs-store-server/migrations/0004_v3_devices.sql");
const COLLABORATION_SQL: &str =
    include_str!("../../layrs-store-server/migrations/0005_v3_collaboration.sql");
const MIGRATIONS: &[(i64, &str)] = &[
    // Extensions remain append-only and are applied in ascending order below.
    (1, schema::BASELINE_SQL),
    (2, IDENTITY_SQL),
    (3, WRITER_SQL),
    (4, DEVICE_SQL),
    (5, COLLABORATION_SQL),
    (
        6,
        include_str!("../../layrs-store-server/migrations/0006_v3_webhooks.sql"),
    ),
    (
        7,
        include_str!("../../layrs-store-server/migrations/0007_v3_release_publications.sql"),
    ),
    (
        8,
        include_str!("../../layrs-store-server/migrations/0008_v3_review_integrations.sql"),
    ),
    (
        9,
        include_str!("../../layrs-store-server/migrations/0009_v3_run_pagination.sql"),
    ),
    (
        10,
        include_str!("../../layrs-store-server/migrations/0010_v3_space_invitations.sql"),
    ),
    (
        11,
        include_str!("../../layrs-store-server/migrations/0011_v3_run_cancellations.sql"),
    ),
    (
        12,
        include_str!("../../layrs-store-server/migrations/0012_v3_comment_corrections.sql"),
    ),
    (
        13,
        include_str!("../../layrs-store-server/migrations/0013_v3_space_teams.sql"),
    ),
    (
        14,
        include_str!("../../layrs-store-server/migrations/0014_v3_run_execution_state.sql"),
    ),
    (
        15,
        include_str!("../../layrs-store-server/migrations/0015_v3_published_observations.sql"),
    ),
    (
        16,
        include_str!("../../layrs-store-server/migrations/0016_v3_client_observations.sql"),
    ),
    (
        17,
        include_str!("../../layrs-store-server/migrations/0017_v3_scoped_patch_sessions.sql"),
    ),
    (
        18,
        include_str!("../../layrs-store-server/migrations/0018_v3_scoped_reviews.sql"),
    ),
    (
        19,
        include_str!(
            "../../layrs-store-server/migrations/0019_v3_scoped_patch_catalog_namespace.sql"
        ),
    ),
];
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Requires a maintenance login able to create the baseline's NOLOGIN writer role.
pub async fn initialize(pool: &PgPool, config: &ServerConfig) -> Result<()> {
    use sqlx::Connection;
    let mut connection = pool.acquire().await?.detach();
    // Session advisory lock also spans the baseline's explicit transaction.
    sqlx::query("SELECT pg_advisory_lock(1818323315, 8)")
        .execute(&mut connection)
        .await?;
    let result = initialize_locked(&mut connection, config).await;
    // Always close: even cancellation cannot return a session holding the lock to the pool.
    connection.close().await?;
    result
}

async fn initialize_locked(
    connection: &mut sqlx::PgConnection,
    config: &ServerConfig,
) -> Result<()> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('schema_metadata') IS NOT NULL")
        .fetch_one(&mut *connection)
        .await?;
    if !exists {
        sqlx::raw_sql(schema::BASELINE_SQL)
            .execute(&mut *connection)
            .await?;
    }
    let row = sqlx::query(schema::METADATA_PROBE_SQL)
        .fetch_one(&mut *connection)
        .await?;
    schema::verify_metadata(Some(schema::SchemaMetadata {
        schema_version: row.get("schema_version"),
        protocol_version: row.get("protocol_version"),
        baseline_migration: row.get("baseline_migration"),
        schema_contract_checksum: row.get("schema_contract_checksum"),
        catalog_fingerprint: row.get("catalog_fingerprint"),
        actual_catalog_fingerprint: row.get("actual_catalog_fingerprint"),
    }))?;
    sqlx::raw_sql("CREATE SCHEMA IF NOT EXISTS layrs_migrations; REVOKE ALL ON SCHEMA layrs_migrations FROM PUBLIC; CREATE TABLE IF NOT EXISTS layrs_migrations.applied(version BIGINT PRIMARY KEY, checksum TEXT NOT NULL, applied_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp());").execute(&mut *connection).await?;
    let unsupported: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM layrs_migrations.applied WHERE version<1 OR version>$1)",
    )
    .bind(MIGRATIONS.len() as i64)
    .fetch_one(&mut *connection)
    .await?;
    if unsupported {
        return Err("unsupported server migration version".into());
    }
    for &(version, sql) in MIGRATIONS {
        let checksum = hex::encode(Sha256::digest(sql.as_bytes()));
        let stored: Option<String> =
            sqlx::query_scalar("SELECT checksum FROM layrs_migrations.applied WHERE version=$1")
                .bind(version)
                .fetch_optional(&mut *connection)
                .await?;
        match stored {
            Some(value) if value != checksum => {
                return Err("server migration checksum mismatch".into());
            }
            Some(_) => {}
            None => {
                use sqlx::Connection;
                let mut tx = connection.begin().await?;
                if version != 1 {
                    sqlx::raw_sql(sql).execute(&mut *tx).await?;
                }
                sqlx::query("INSERT INTO layrs_migrations.applied(version,checksum) VALUES($1,$2)")
                    .bind(version)
                    .bind(checksum)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
            }
        }
    }
    use sqlx::Connection;
    let mut tx = connection.begin().await?;
    let backend =
        serde_jcs::to_string(&serde_json::json!({"path":config.cas_root.to_string_lossy()}))?;
    sqlx::query("INSERT INTO stores(store_id,backend_kind,backend_config_jcs) VALUES($1,'filesystem',$2) ON CONFLICT DO NOTHING")
        .bind(config.store_id.as_str()).bind(&backend).execute(&mut *tx).await?;
    let row = sqlx::query("SELECT backend_kind,backend_config_jcs FROM stores WHERE store_id=$1")
        .bind(config.store_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
    if row.get::<String, _>("backend_kind") != "filesystem"
        || row.get::<String, _>("backend_config_jcs") != backend
    {
        return Err("configured store differs from installed store".into());
    }
    sqlx::query("INSERT INTO deployments(deployment_id,store_id,display_name) VALUES($1,$2,'Layrs') ON CONFLICT DO NOTHING")
        .bind(&config.deployment_id).bind(config.store_id.as_str()).execute(&mut *tx).await?;
    let installed_store: String =
        sqlx::query_scalar("SELECT store_id FROM deployments WHERE deployment_id=$1")
            .bind(&config.deployment_id)
            .fetch_one(&mut *tx)
            .await?;
    if installed_store != config.store_id.as_str() {
        return Err("configured deployment differs from installed deployment".into());
    }
    let bootstrap_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM layrs_identity.bootstrap WHERE deployment_id=$1)",
    )
    .bind(&config.deployment_id)
    .fetch_one(&mut *tx)
    .await?;
    if !bootstrap_exists {
        let secret = config
            .bootstrap_secret
            .as_ref()
            .ok_or("first installation requires a bootstrap secret")?;
        sqlx::query(
            "INSERT INTO layrs_identity.bootstrap(deployment_id,secret_digest) VALUES($1,$2)",
        )
        .bind(&config.deployment_id)
        .bind(token_digest(secret))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    // Reader access to the separate migration ledger does not alter native schema identity.
    sqlx::raw_sql("DO $grants$ DECLARE writer TEXT; BEGIN SELECT writer_role_name INTO STRICT writer FROM schema_metadata WHERE singleton_id=1; EXECUTE format('GRANT USAGE ON SCHEMA layrs_migrations TO %I',writer); EXECUTE format('GRANT SELECT ON layrs_migrations.applied TO %I',writer); EXECUTE format('GRANT SELECT ON stores, deployments, schema_metadata TO %I',writer); EXECUTE format('GRANT SELECT, INSERT, UPDATE ON principals TO %I',writer); END $grants$;").execute(&mut *connection).await?;
    Ok(())
}

/// Runtime never creates or repairs a missing schema.
pub async fn verify_identity_schema(pool: &PgPool) -> Result<()> {
    let rows =
        sqlx::query("SELECT version,checksum FROM layrs_migrations.applied ORDER BY version")
            .fetch_all(pool)
            .await?;
    if rows.len() != MIGRATIONS.len() {
        return Err("unsupported server migration version".into());
    }
    for (row, (version, sql)) in rows.iter().zip(MIGRATIONS.iter().copied()) {
        if row.get::<i64, _>("version") != version
            || row.get::<String, _>("checksum") != hex::encode(Sha256::digest(sql.as_bytes()))
        {
            return Err("server migration checksum mismatch".into());
        }
    }
    sqlx::query("SELECT principal_id,password_hash FROM layrs_identity.accounts LIMIT 0")
        .execute(pool)
        .await?;
    Ok(())
}
