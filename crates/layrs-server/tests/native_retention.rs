//! Real PostgreSQL metadata audit: no business row or filesystem byte changes.
#[path = "support/retention_exclusion.rs"]
mod retention_exclusion;
use layrs_server::{
    config::ServerConfig,
    identity::random_secret,
    installation,
    retention::{AuditLimits, audit},
};
use sqlx::{
    Executor, PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
};

#[tokio::test]
#[ignore = "requires explicitly isolated PostgreSQL administrator endpoint"]
async fn retention_observes_catalogue_missing_and_prepared_files_without_mutation() {
    let options =
        PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let database = format!("layrs_retention_{suffix}");
    let reader = format!("retention_reader_{suffix}");
    admin
        .execute(format!("CREATE DATABASE {database}").as_str())
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options.clone().database(&database))
        .await
        .unwrap();
    #[cfg(windows)]
    let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("tmp");
    #[cfg(not(windows))]
    let parent = std::env::temp_dir();
    let root = parent.join(&database);
    std::fs::create_dir_all(root.join("staging")).unwrap();
    let config = ServerConfig::parse(&BTreeMap::from([
        (
            "LAYRS_PUBLIC_URL".into(),
            "https://retention.example".into(),
        ),
        ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
        ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
        (
            "LAYRS_DEPLOYMENT_ID".into(),
            "deployment:retention-test".into(),
        ),
        ("LAYRS_STORE_ID".into(), "store:retention-test".into()),
        ("LAYRS_SIGNING_KEY".into(), random_secret()),
        ("LAYRS_BOOTSTRAP_SECRET".into(), random_secret()),
    ]))
    .unwrap();
    installation::initialize(&pool, &config).await.unwrap();
    let writer: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    for (hex, expected, contents) in [
        ('a', 4, Some(b"kept".as_slice())),
        ('b', 5, None),
        ('c', 9, Some(b"bad".as_slice())),
    ] {
        let digest = hex.to_string().repeat(64);
        let key = format!("objects/chunk/{}/{digest}", &digest[..2]);
        sqlx::query("INSERT INTO canonical_objects(object_id,store_id,object_kind,canonical_byte_length,encoded_byte_length,compression,storage_key,verification_state,verified_at) VALUES($1,$2,'chunk',$3,$3,'identity',$4,'verified',clock_timestamp())")
            .bind(format!("blake3:{digest}")).bind(config.store_id.as_str()).bind(expected as i64).bind(&key).execute(&pool).await.unwrap();
        if let Some(bytes) = contents {
            let path = root.join(key);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    }
    let pending = root.join(format!("objects/chunk/dd/{}", "d".repeat(64)));
    std::fs::create_dir_all(pending.parent().unwrap()).unwrap();
    std::fs::write(pending, b"possibly-prepared-before-commit").unwrap();
    std::fs::write(root.join("staging/in-progress.upload"), b"pending-upload").unwrap();
    let password = random_secret();
    admin
        .execute(format!("CREATE ROLE {reader} LOGIN PASSWORD '{password}'").as_str())
        .await
        .unwrap();
    for schema in [
        "public",
        "layrs_identity",
        "layrs_collaboration",
        "layrs_migrations",
    ] {
        pool.execute(format!("GRANT USAGE ON SCHEMA {schema} TO {reader}").as_str())
            .await
            .unwrap();
        pool.execute(format!("GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO {reader}").as_str())
            .await
            .unwrap();
    }
    let read_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(
            options
                .database(&database)
                .username(&reader)
                .password(&password),
        )
        .await
        .unwrap();
    let can_write: bool =
        sqlx::query_scalar("SELECT has_table_privilege(current_user,'canonical_objects','INSERT')")
            .fetch_one(&read_pool)
            .await
            .unwrap();
    assert!(!can_write);
    let before_tables = table_fingerprints(&pool).await;
    let before_files = files(&root);
    let report = audit(&read_pool, &config, Default::default())
        .await
        .unwrap();
    assert_eq!(report.schema_version, "layrs.retention-audit.v1");
    assert!(!report.deletion_authorized);
    assert!(!report.content_verified);
    assert_eq!(report.catalogued_objects_retained, 3);
    assert_eq!(report.catalogued_files_present, 2);
    assert_eq!(report.missing_files.len(), 1);
    assert_eq!(report.size_mismatches.len(), 1);
    assert_eq!(report.files_to_examine.len(), 2);
    let mut command_url =
        url::Url::parse(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
    command_url.set_path(&database);
    command_url.set_username(&reader).unwrap();
    command_url.set_password(Some(&password)).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_layrs-server"))
        .arg("retention-audit")
        .env("LAYRS_DATABASE_URL", command_url.as_str())
        .env("LAYRS_PUBLIC_URL", "https://retention.example")
        .env("LAYRS_CAS_ROOT", &root)
        .env("LAYRS_DEPLOYMENT_ID", &config.deployment_id)
        .env("LAYRS_STORE_ID", config.store_id.as_str())
        .env("LAYRS_SIGNING_KEY", random_secret())
        .output()
        .unwrap();
    assert!(output.status.success(), "retention CLI failed");
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["schemaVersion"], "layrs.retention-audit.v1");
    assert_eq!(document["deletionAuthorized"], false);
    assert!(
        report
            .files_to_examine
            .iter()
            .all(|f| f.classification.contains("may_be_"))
    );
    assert_eq!(files(&root), before_files);
    assert_eq!(table_fingerprints(&pool).await, before_tables);
    assert!(
        audit(
            &read_pool,
            &config,
            AuditLimits {
                catalog_objects: 1,
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert!(
        audit(
            &read_pool,
            &config,
            AuditLimits {
                report_bytes: 1,
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    let outside = parent.join(format!("layrs-retention-outside-{suffix}"));
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"untouched").unwrap();
    let link = root.join("redirect");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    #[cfg(windows)]
    assert!(
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        audit(&read_pool, &config, Default::default())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(outside.join("sentinel")).unwrap(),
        b"untouched"
    );
    #[cfg(windows)]
    std::fs::remove_dir(&link).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(&link).unwrap();
    assert_eq!(files(&root), before_files);
    assert_eq!(table_fingerprints(&pool).await, before_tables);
    retention_exclusion::prove_sql_fence_does_not_exclude_cas(&pool, &root).await;
    let cas = layrs_store_server::v3::FsServerCas::open(&root, Default::default())
        .await
        .unwrap();
    let preflight = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_layrs-server"))
            .arg("retention-preflight")
            .env("LAYRS_DATABASE_URL", command_url.as_str())
            .env("LAYRS_PUBLIC_URL", "https://retention.example")
            .env("LAYRS_CAS_ROOT", &root)
            .env("LAYRS_DEPLOYMENT_ID", &config.deployment_id)
            .env("LAYRS_STORE_ID", config.store_id.as_str())
            .env("LAYRS_SIGNING_KEY", random_secret())
            .output()
            .unwrap()
    };
    assert!(
        !preflight().status.success(),
        "live CAS refuses maintenance CLI"
    );
    drop(cas);
    let before_preflight_files = files(&root);
    let before_preflight_tables = table_fingerprints(&pool).await;
    let output = preflight();
    assert!(
        output.status.success(),
        "exclusive preflight CLI succeeds offline"
    );
    let preflight_report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        preflight_report["schemaVersion"],
        "layrs.retention-preflight.v1"
    );
    assert_eq!(preflight_report["cooperativeCasExclusion"], true);
    assert_eq!(preflight_report["deletionAuthorized"], false);
    assert_eq!(files(&root), before_preflight_files);
    assert_eq!(table_fingerprints(&pool).await, before_preflight_tables);
    read_pool.close().await;
    pool.close().await;
    admin
        .execute(format!("DROP DATABASE {database}").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("DROP ROLE {reader}").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("DROP ROLE {writer}").as_str())
        .await
        .unwrap();
    admin.close().await;
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}
async fn table_fingerprints(pool: &PgPool) -> BTreeMap<String, String> {
    let rows=sqlx::query("SELECT schemaname,tablename FROM pg_tables WHERE schemaname IN ('public','layrs_identity','layrs_collaboration','layrs_migrations') ORDER BY schemaname,tablename").fetch_all(pool).await.unwrap();
    let mut out = BTreeMap::new();
    for row in rows {
        let schema: String = row.get("schemaname");
        let table: String = row.get("tablename");
        assert!(
            schema
                .bytes()
                .chain(table.bytes())
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        );
        let query = format!(
            "SELECT md5(COALESCE(string_agg(row_to_json(t)::text,E'\\n' ORDER BY row_to_json(t)::text),'')) FROM {schema}.{table} t"
        );
        let digest: String = sqlx::query_scalar(&query).fetch_one(pool).await.unwrap();
        out.insert(format!("{schema}.{table}"), digest);
    }
    out
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, out)
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
