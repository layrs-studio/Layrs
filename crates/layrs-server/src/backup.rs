//! Conservative single-filesystem-CAS backup. All catalog objects are retained.
//! A completed manifest is written last. Incomplete uploads fail closed.
use crate::{config::ServerConfig, installation};
use layrs_core::scm::{
    FileManifest, ObjectId, RepositoryView, Step, StoreId, Tombstone, Tree, TreeEntryTarget,
};
use layrs_store_server::v3::{
    CasLimits, CatalogedFsCas, FsServerCas, ServerCas, conflict_object_references,
    read_canonical_object, schema,
};
use layrs_sync::v3::{ObjectDescriptor, ObjectKind, PolicySet};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    str::FromStr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub type BackupResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
mod references;
const MANIFEST: &str = "manifest.json";
pub const RESTORE_MARKER: &str = ".layrs-restore-in-progress";
const MAX_OBJECTS: usize = 1_000_000;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupManifest {
    pub format: String,
    pub baseline: String,
    pub deployment_id: String,
    pub store_id: StoreId,
    pub writer_role: String,
    pub snapshot: String,
    pub database_watermark_unix: u64,
    pub cas_completed_unix: u64,
    pub database_dump_sha256: String,
    pub event_watermarks: BTreeMap<String, i64>,
    pub objects: Vec<ObjectDescriptor>,
}

/// Tests may invoke real PostgreSQL tools in an isolated container. Production uses local binaries.
pub trait PgBackupTools: Send + Sync {
    fn dump(&self, database_url: &str, snapshot: &str, destination: &Path) -> BackupResult<()>;
    fn restore(&self, database_url: &str, source: &Path) -> BackupResult<()>;
}
pub struct LocalPgBackupTools;
impl PgBackupTools for LocalPgBackupTools {
    fn dump(&self, url: &str, snapshot: &str, destination: &Path) -> BackupResult<()> {
        let output = private_file(destination)?;
        let status = pg_command("pg_dump", url)?
            .args(["--format=custom", "--no-owner", "--snapshot", snapshot])
            .stdin(Stdio::null())
            .stdout(Stdio::from(output))
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err("pg_dump failed; verify maintenance credentials and matching PostgreSQL client version".into());
        }
        Ok(())
    }
    fn restore(&self, url: &str, source: &Path) -> BackupResult<()> {
        let input = File::open(source)?;
        let status = pg_command("pg_restore", url)?
            .args([
                "--no-owner",
                "--exit-on-error",
                "--single-transaction",
                "--dbname",
                "",
            ])
            .stdin(Stdio::from(input))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err("pg_restore failed; target remains non-serving".into());
        }
        Ok(())
    }
}

fn pg_command(program: &str, value: &str) -> BackupResult<Command> {
    let options = sqlx::postgres::PgConnectOptions::from_str(value)?;
    let url = url::Url::parse(value)?;
    let mut command = Command::new(program);
    command
        .env("PGHOST", options.get_host())
        .env("PGPORT", options.get_port().to_string())
        .env("PGUSER", options.get_username())
        .env(
            "PGDATABASE",
            options.get_database().unwrap_or(options.get_username()),
        )
        .env("PGCONNECT_TIMEOUT", "15")
        .env_remove("PGPASSWORD");
    command.env(
        "PGSSLMODE",
        match options.get_ssl_mode() {
            sqlx::postgres::PgSslMode::Disable => "disable",
            sqlx::postgres::PgSslMode::Allow => "allow",
            sqlx::postgres::PgSslMode::Prefer => "prefer",
            sqlx::postgres::PgSslMode::Require => "require",
            sqlx::postgres::PgSslMode::VerifyCa => "verify-ca",
            sqlx::postgres::PgSslMode::VerifyFull => "verify-full",
        },
    );
    if let Some(password) = url.password() {
        command.env("PGPASSWORD", percent_decode(password)?);
    }
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "sslmode" | "host" | "port" | "user" | "dbname" => {}
            "sslrootcert" => {
                command.env("PGSSLROOTCERT", value.as_ref());
            }
            "sslcert" => {
                command.env("PGSSLCERT", value.as_ref());
            }
            "sslkey" => {
                command.env("PGSSLKEY", value.as_ref());
            }
            _ => return Err("backup does not support this PostgreSQL URL option".into()),
        }
    }
    Ok(command)
}
fn percent_decode(value: &str) -> BackupResult<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err("invalid database password encoding".into());
            }
            let high = (bytes[i + 1] as char)
                .to_digit(16)
                .ok_or("invalid database password encoding")?;
            let low = (bytes[i + 2] as char)
                .to_digit(16)
                .ok_or("invalid database password encoding")?;
            out.push(((high << 4) | low) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Ok(String::from_utf8(out)?)
}

pub async fn create_backup(
    pool: &PgPool,
    config: &ServerConfig,
    destination: &Path,
    tools: Arc<dyn PgBackupTools>,
) -> BackupResult<BackupManifest> {
    installation::verify_identity_schema(pool).await?;
    // Acquire CAS participation before SQL snapshot locks and keep it through
    // manifest completion. Maintenance cannot enter while pg_dump is running.
    let source = FsServerCas::open(&config.cas_root, CasLimits::default()).await?;
    create_private_directory(destination)?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    // Freeze mutable roots and catalog removals for the snapshot/CAS copy. The current
    // server performs no CAS GC; future GC must acquire these fences before unlinking.
    sqlx::query("LOCK TABLE spaces,canonical_objects,object_leases IN SHARE MODE")
        .execute(&mut *tx)
        .await?;
    let stores: i64 = sqlx::query_scalar("SELECT count(*) FROM stores")
        .fetch_one(&mut *tx)
        .await?;
    if stores != 1 {
        return Err("backup currently requires exactly one filesystem CAS store".into());
    }
    let dangling:i64=sqlx::query_scalar("SELECT count(*) FROM object_leases l LEFT JOIN canonical_objects o ON o.object_id=l.object_id WHERE l.released_at IS NULL AND l.expires_at>clock_timestamp() AND (o.object_id IS NULL OR o.verification_state<>'verified')").fetch_one(&mut *tx).await?;
    if dangling != 0 {
        return Err(
            "backup refused: unfinished leased uploads; finish or expire transfers first".into(),
        );
    }
    let unverified: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM canonical_objects WHERE verification_state<>'verified'",
    )
    .fetch_one(&mut *tx)
    .await?;
    if unverified != 0 {
        return Err(
            "backup refused: unverified or quarantined catalog objects require diagnosis".into(),
        );
    }
    let snapshot: String = sqlx::query_scalar("SELECT pg_export_snapshot()")
        .fetch_one(&mut *tx)
        .await?;
    let watermark = now()?;
    let rows=sqlx::query("SELECT object_id,object_kind,canonical_byte_length,encoded_byte_length,compression,store_id FROM canonical_objects ORDER BY object_id LIMIT 1000001").fetch_all(&mut *tx).await?;
    if rows.len() > MAX_OBJECTS {
        return Err("backup inventory exceeds supported object bound".into());
    }
    let mut objects = Vec::with_capacity(rows.len());
    for row in rows {
        if row.get::<String, _>("store_id") != config.store_id.as_str() {
            return Err("backup store does not match configuration".into());
        }
        objects.push(descriptor(&row)?);
    }
    let installed: Option<String> =
        sqlx::query_scalar("SELECT store_id FROM deployments WHERE deployment_id=$1")
            .bind(&config.deployment_id)
            .fetch_optional(&mut *tx)
            .await?;
    if installed.as_deref() != Some(config.store_id.as_str()) {
        return Err("backup deployment does not match configuration".into());
    }
    let writer: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(&mut *tx)
            .await?;
    let mut event_watermarks = BTreeMap::new();
    for row in sqlx::query(
        "SELECT space_id,next_event_sequence-1 AS watermark FROM spaces ORDER BY space_id",
    )
    .fetch_all(&mut *tx)
    .await?
    {
        event_watermarks.insert(row.get("space_id"), row.get("watermark"));
    }
    let dump = destination.join("database.dump");
    let dump_path = dump.clone();
    let url = config.database_url.clone();
    let snapshot_arg = snapshot.clone();
    tokio::task::spawn_blocking(move || tools.dump(&url, &snapshot_arg, &dump_path)).await??;
    OpenOptions::new().write(true).open(&dump)?.sync_all()?;
    let target = FsServerCas::open(destination.join("cas"), CasLimits::default()).await?;
    for object in &objects {
        target
            .put_if_absent(object, source.verified_stream(object).await?)
            .await?;
    }
    let manifest = BackupManifest {
        format: "layrs.backup.v1".into(),
        baseline: schema::SCHEMA_VERSION.into(),
        deployment_id: config.deployment_id.clone(),
        store_id: config.store_id.clone(),
        writer_role: writer,
        snapshot,
        database_watermark_unix: watermark,
        cas_completed_unix: now()?,
        database_dump_sha256: hash_file(&dump)?,
        event_watermarks,
        objects,
    };
    verify_inventory(destination, &manifest).await?;
    tx.commit().await?;
    let bytes = serde_jcs::to_vec(&manifest)?;
    let mut file = private_file(&destination.join(MANIFEST))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    sync_dir(destination)?;
    Ok(manifest)
}

pub async fn verify_backup(directory: &Path) -> BackupResult<BackupManifest> {
    let metadata = std::fs::symlink_metadata(directory.join(MANIFEST))?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 * 1024 {
        return Err("invalid or oversized backup manifest".into());
    }
    let bytes = std::fs::read(directory.join(MANIFEST))?;
    let manifest: BackupManifest = serde_json::from_slice(&bytes)?;
    if serde_jcs::to_vec(&manifest)? != bytes {
        return Err("backup manifest must be canonical and complete".into());
    }
    verify_inventory(directory, &manifest).await?;
    Ok(manifest)
}

async fn verify_inventory(directory: &Path, manifest: &BackupManifest) -> BackupResult<()> {
    if manifest.format != "layrs.backup.v1"
        || manifest.baseline != schema::SCHEMA_VERSION
        || manifest.cas_completed_unix < manifest.database_watermark_unix
        || manifest.objects.len() > MAX_OBJECTS
    {
        return Err("unsupported or inconsistent backup manifest".into());
    }
    if hash_file(&directory.join("database.dump"))? != manifest.database_dump_sha256 {
        return Err("database dump checksum mismatch".into());
    }
    let cas_root = directory.join("cas");
    for path in [
        &cas_root,
        &cas_root.join("objects"),
        &cas_root.join("staging"),
    ] {
        if !std::fs::symlink_metadata(path)?.is_dir() {
            return Err("backup CAS directory is missing or linked".into());
        }
    }
    let cas = CatalogedFsCas::new(
        manifest.store_id.clone(),
        FsServerCas::open(cas_root, CasLimits::default()).await?,
    );
    let mut objects = BTreeMap::new();
    for object in &manifest.objects {
        if objects.insert(object.object_id.as_str(), object).is_some() {
            return Err("duplicate object in backup inventory".into());
        }
    }
    let mut files = BTreeSet::new();
    let mut products = BTreeSet::new();
    let mut product_dependencies = BTreeSet::new();
    let mut references = Vec::<(String, Option<ObjectKind>)>::new();
    for object in &manifest.objects {
        let bytes = read_canonical_object(&cas, object, 64 * 1024 * 1024).await?;
        match object.kind {
            ObjectKind::FileManifest => {
                let file: FileManifest = serde_json::from_slice(&bytes)?;
                let mut hash = blake3::Hasher::new();
                hash.update(b"layrs-file-v1\0");
                for chunk in file.chunks() {
                    let descriptor = objects
                        .get(chunk.chunk_id().as_str())
                        .ok_or("backup file closure misses a chunk")?;
                    if descriptor.kind != ObjectKind::Chunk
                        || descriptor.canonical_byte_len != chunk.byte_length()
                    {
                        return Err("backup chunk kind or size mismatch".into());
                    }
                    hash.update(&read_canonical_object(&cas, descriptor, 64 * 1024 * 1024).await?);
                }
                if format!("blake3:{}", hash.finalize()) != file.file_id().as_str() {
                    return Err("backup FileManifest reconstructs the wrong FileId".into());
                }
                files.insert(file.file_id().as_str().to_owned());
            }
            ObjectKind::Tree => {
                let tree: Tree = serde_json::from_slice(&bytes)?;
                for entry in tree.entries() {
                    match entry.target() {
                        TreeEntryTarget::File { file_id } => {
                            references.push((file_id.as_str().into(), None))
                        }
                        TreeEntryTarget::Tree { tree_id } => {
                            references.push((tree_id.as_str().into(), Some(ObjectKind::Tree)))
                        }
                        TreeEntryTarget::Conflict { conflict_id } => references
                            .push((conflict_id.as_str().into(), Some(ObjectKind::Conflict))),
                        TreeEntryTarget::Symlink { symlink_id } => {
                            references.push((symlink_id.as_str().into(), Some(ObjectKind::Symlink)))
                        }
                        TreeEntryTarget::Tombstone { tombstone_id } => references
                            .push((tombstone_id.as_str().into(), Some(ObjectKind::Tombstone))),
                    }
                }
            }
            ObjectKind::Step => {
                let step: Step = serde_json::from_slice(&bytes)?;
                references.push((step.root_tree_id().as_str().into(), Some(ObjectKind::Tree)));
                references.extend(
                    step.parent_step_ids()
                        .iter()
                        .map(|id| (id.as_str().into(), Some(ObjectKind::Step))),
                );
            }
            ObjectKind::RepositoryView => {
                let view: RepositoryView = serde_json::from_slice(&bytes)?;
                references.extend(
                    view.layer_heads()
                        .iter()
                        .flat_map(|heads| heads.step_ids().iter())
                        .map(|id| (id.as_str().into(), Some(ObjectKind::Step))),
                );
            }
            ObjectKind::PolicySet => {
                let set: PolicySet = serde_json::from_slice(&bytes)?;
                for version in set.policy_versions {
                    references.push((
                        version.policy_product_object_id.as_str().into(),
                        Some(ObjectKind::Product),
                    ));
                    references.extend(
                        version
                            .dependency_object_ids
                            .into_iter()
                            .map(|id| (id.as_str().into(), None)),
                    );
                }
            }
            ObjectKind::Tombstone => {
                let tombstone: Tombstone = serde_json::from_slice(&bytes)?;
                if let Some(id) = tombstone.previous_object_id() {
                    references.push((id.as_str().into(), None));
                }
            }
            ObjectKind::Product => {
                let product: layrs_sync::v3::ProductEnvelope = serde_json::from_slice(&bytes)?;
                products.insert(product.logical_id().to_owned());
                product_dependencies.extend(
                    product
                        .dependencies()
                        .1
                        .iter()
                        .map(|id| id.as_str().to_owned()),
                );
                references.extend(references::product_references(&product));
            }
            ObjectKind::Conflict => {
                references.extend(
                    conflict_object_references(&bytes)?
                        .into_iter()
                        .map(|(id, kind)| (id.as_str().to_owned(), kind)),
                );
            }
            _ => {}
        }
    }
    if !product_dependencies.is_subset(&products) {
        return Err("backup Product logical dependency closure is incomplete".into());
    }
    for (id, kind) in references {
        match objects.get(id.as_str()) {
            Some(value) if kind.is_none_or(|expected| expected == value.kind) => {}
            None if kind.is_none() && files.contains(&id) => {}
            _ => return Err("backup canonical object closure is incomplete".into()),
        }
    }
    Ok(())
}

pub async fn restore_backup(
    pool: &PgPool,
    config: &ServerConfig,
    directory: &Path,
    tools: Arc<dyn PgBackupTools>,
) -> BackupResult<()> {
    let manifest = verify_backup(directory).await?;
    if manifest.store_id != config.store_id || manifest.deployment_id != config.deployment_id {
        return Err("restore requires the backup's stable deployment/store IDs".into());
    }
    let tables:i64=sqlx::query_scalar("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' AND c.relkind IN ('r','p','v','m','S','f')").fetch_one(pool).await?;
    if tables != 0 {
        return Err("restore target database must be empty and non-serving".into());
    }
    create_private_directory(&config.cas_root)?;
    let target = FsServerCas::open(&config.cas_root, CasLimits::default()).await?;
    let marker = config.cas_root.join(RESTORE_MARKER);
    let file = private_file(&marker)?;
    file.sync_all()?;
    sync_dir(&config.cas_root)?;
    let writer = &manifest.writer_role;
    if !writer.starts_with("layrs_v3_w_")
        || writer.len() != 35
        || !writer
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid backup writer role".into());
    }
    let existing:Option<bool>=sqlx::query_scalar("SELECT NOT(rolcanlogin OR rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls) FROM pg_roles WHERE rolname=$1").bind(writer).fetch_optional(pool).await?;
    match existing {
        Some(false) => return Err("restore writer role has unsafe privileges".into()),
        Some(true) => {}
        None => {
            sqlx::query(&format!("CREATE ROLE {writer} NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS")).execute(pool).await?;
        }
    }
    let source = FsServerCas::open(directory.join("cas"), CasLimits::default()).await?;
    for object in &manifest.objects {
        target
            .put_if_absent(object, source.verified_stream(object).await?)
            .await?;
    }
    let dump = directory.join("database.dump");
    let url = config.database_url.clone();
    tokio::task::spawn_blocking(move || tools.restore(&url, &dump)).await??;
    installation::verify_identity_schema(pool).await?;
    let row = sqlx::query(schema::METADATA_PROBE_SQL)
        .fetch_one(pool)
        .await?;
    schema::verify_metadata(Some(schema::SchemaMetadata {
        schema_version: row.get("schema_version"),
        protocol_version: row.get("protocol_version"),
        baseline_migration: row.get("baseline_migration"),
        schema_contract_checksum: row.get("schema_contract_checksum"),
        catalog_fingerprint: row.get("catalog_fingerprint"),
        actual_catalog_fingerprint: row.get("actual_catalog_fingerprint"),
    }))?;
    let rows=sqlx::query("SELECT object_id,object_kind,canonical_byte_length,encoded_byte_length,compression FROM canonical_objects ORDER BY object_id").fetch_all(pool).await?;
    let restored: Vec<_> = rows.iter().map(descriptor).collect::<BackupResult<_>>()?;
    if restored != manifest.objects {
        return Err("restored database inventory differs from manifest".into());
    }
    let backend =
        serde_jcs::to_string(&serde_json::json!({"path":config.cas_root.to_string_lossy()}))?;
    sqlx::query(
        "UPDATE stores SET backend_config_jcs=$2 WHERE store_id=$1 AND backend_kind='filesystem'",
    )
    .bind(config.store_id.as_str())
    .bind(backend)
    .execute(pool)
    .await?;
    std::fs::remove_file(marker)?;
    sync_dir(&config.cas_root)?;
    Ok(())
}

fn descriptor(row: &sqlx::postgres::PgRow) -> BackupResult<ObjectDescriptor> {
    Ok(ObjectDescriptor {
        object_id: ObjectId::parse(row.get::<String, _>("object_id"))?,
        kind: serde_json::from_value(serde_json::Value::String(row.get("object_kind")))?,
        canonical_byte_len: u64::try_from(row.get::<i64, _>("canonical_byte_length"))?,
        encoded_byte_len: u64::try_from(row.get::<i64, _>("encoded_byte_length"))?,
        compression: serde_json::from_value(serde_json::Value::String(row.get("compression")))?,
    })
}
fn now() -> BackupResult<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
fn hash_file(path: &Path) -> BackupResult<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}
fn private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::DirBuilder::new().create(path)
    }
}
fn sync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.join(".layrs-backup-fsync"))?
            .sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pg_secrets_are_decoded_into_environment_only() {
        let command = pg_command(
            "pg_dump",
            "postgres://operator:p%40ss%3Aword@localhost:5433/project?sslmode=require",
        )
        .unwrap();
        assert_eq!(command.get_args().count(), 0);
        let environment: BTreeMap<_, _> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|s| s.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(environment["PGPASSWORD"].as_deref(), Some("p@ss:word"));
        assert_eq!(environment["PGSSLMODE"].as_deref(), Some("require"));
        assert_eq!(environment["PGDATABASE"].as_deref(), Some("project"));
    }
    #[test]
    fn malformed_password_escape_never_panics() {
        for invalid in ["%", "%x", "%xz", "%é"] {
            assert!(percent_decode(invalid).is_err());
        }
    }
}
