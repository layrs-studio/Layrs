//! Private resource ledger for CI cleanup, including failed fixture startups.
use super::*;
use std::path::Path;

pub fn ledger(path: &Path, database: &str, runtime: &str) -> Result<()> {
    if let Some(path) = std::env::var_os("LAYRS_TEST_UI_RESOURCE_FILE") {
        let resource = PathBuf::from(path);
        if !resource.is_absolute() || resource.exists() {
            return Err("resource ledger must be a new absolute filename".into());
        }
        private(
            &resource,
            &serde_json::to_vec(&json!({"database":database,"runtime":runtime}))?,
        )?;
    }
    if !path.parent().is_some_and(Path::is_dir) {
        return Err("private fixture directory missing".into());
    }
    Ok(())
}

pub fn private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub async fn cleanup(path: &Path) -> Result<()> {
    let bytes = std::fs::read(path)?;
    if bytes.len() > 4096 {
        return Err("invalid resource ledger".into());
    }
    let ledger: serde_json::Value = serde_json::from_slice(&bytes)?;
    let database = ledger["database"]
        .as_str()
        .ok_or("missing fixture database")?;
    let runtime = ledger["runtime"]
        .as_str()
        .ok_or("missing fixture runtime role")?;
    let suffix = database
        .strip_prefix("layrs_ui_test_")
        .ok_or("invalid fixture database")?;
    if suffix.len() != 32
        || !suffix.bytes().all(|b| b.is_ascii_hexdigit())
        || runtime != format!("layrs_ui_runtime_{suffix}")
    {
        return Err("resource ledger does not describe an isolated fixture".into());
    }
    let options = PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL")?)?;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname=$1)")
            .bind(database)
            .fetch_one(&admin)
            .await?;
    let mut writer = None;
    if exists {
        let target = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options.database(database))
            .await?;
        let metadata: bool =
            sqlx::query_scalar("SELECT to_regclass('public.schema_metadata') IS NOT NULL")
                .fetch_one(&target)
                .await?;
        if metadata {
            writer = sqlx::query_scalar::<_, String>(
                "SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1",
            )
            .fetch_optional(&target)
            .await?;
        }
        target.close().await;
        sqlx::query(&format!("DROP DATABASE {database} WITH (FORCE)"))
            .execute(&admin)
            .await?;
    }
    sqlx::query(&format!("DROP ROLE IF EXISTS {runtime}"))
        .execute(&admin)
        .await?;
    if let Some(writer) = writer {
        if writer.len() != 35
            || !writer.starts_with("layrs_v3_w_")
            || !writer
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("invalid fixture writer role".into());
        }
        sqlx::query(&format!("DROP ROLE IF EXISTS {writer}"))
            .execute(&admin)
            .await?;
    }
    admin.close().await;
    Ok(())
}
