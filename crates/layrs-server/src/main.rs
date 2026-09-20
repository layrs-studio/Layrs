use axum::{Json, Router, extract::State, http::StatusCode, middleware, routing::get};
use layrs_server::{
    backup::{self, LocalPgBackupTools},
    config::ServerConfig,
    identity::{PgIdentity, random_secret},
    installation,
    storage_budget::{StorageMonitor, StorageThresholds},
    telemetry::{self, Telemetry},
    v3::{self, ProductionV3Facade, V3HttpLimits, V3HttpState},
};
use layrs_store_server::v3::{
    CapabilityCodec, CasLimits, CatalogedFsCas, EventCursorCodec, FsServerCas, PgPolicyAuthorizer,
    PgV3Repository, schema,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{path::PathBuf, sync::Arc, time::Duration};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h") => {
            println!(
                "Layrs native server\nUsage: layrs-server [serve|initialize|new-secret|healthcheck]\n       layrs-server recover-password LOGIN PASSWORD_FILE\n       layrs-server [backup|verify-backup|restore] DIRECTORY\n\nRequired: LAYRS_PUBLIC_URL (HTTPS), LAYRS_DATABASE_URL[_FILE], LAYRS_CAS_ROOT,\nLAYRS_DEPLOYMENT_ID, LAYRS_STORE_ID, LAYRS_SIGNING_KEY[_FILE].\nFirst initialize: LAYRS_BOOTSTRAP_SECRET[_FILE]. See deploy/selfhost/README.md."
            );
            println!("       layrs-server [storage-budget|storage-check] [SPACE_ID]");
            println!("       layrs-server retention-audit (read-only filesystem/CAS inventory)");
            println!("       layrs-server gc-plan BACKUP_DIRECTORY");
            println!(
                "       layrs-server gc-{{apply,resume,restore,purge}} PLAN_ID BACKUP_DIRECTORY"
            );
            println!(
                "       layrs-server retention-preflight (exclusive cooperative CAS fence, no deletion)"
            );
            return Ok(());
        }
        Some("new-secret") if args.len() == 1 => {
            println!("{}", random_secret());
            return Ok(());
        }
        Some("healthcheck") if args.len() == 1 => return healthcheck(),
        Some("verify-backup") if args.len() == 2 => {
            let manifest = backup::verify_backup(std::path::Path::new(&args[1])).await?;
            println!(
                "{}",
                serde_json::json!({"event":"backup_verified","objects":manifest.objects.len()})
            );
            return Ok(());
        }
        None | Some("serve" | "initialize") if args.len() <= 1 => {}
        Some("backup" | "restore") if args.len() == 2 => {}
        Some("recover-password") if args.len() == 3 => {}
        Some("storage-budget" | "storage-check") if args.len() <= 2 => {}
        Some("retention-audit" | "retention-preflight") if args.len() == 1 => {}
        Some("gc-plan") if args.len() == 2 => {}
        Some("gc-apply" | "gc-resume" | "gc-restore" | "gc-purge") if args.len() == 3 => {}
        _ => return Err("unknown or incomplete server command; use --help".into()),
    }
    let alert_check = args.first().is_some_and(|s| s == "storage-check");
    let config = match ServerConfig::from_env() {
        Ok(config) => config,
        Err(_) if alert_check => monitor_unavailable(),
        Err(error) => return Err(error.into()),
    };
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(10))
        .connect(&config.database_url)
        .await
        .map_err(|_| {
            if alert_check {
                monitor_unavailable();
            }
            "cannot connect to configured PostgreSQL database"
        })?;
    if args.first().is_some_and(|s| s == "retention-audit") {
        let report = layrs_server::retention::audit(&pool, &config, Default::default()).await?;
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "retention-preflight") {
        let _exclusive = layrs_store_server::v3::FsCasMaintenance::try_acquire(&config.cas_root)?;
        if config.cas_root.join(backup::RESTORE_MARKER).exists()
            || config
                .cas_root
                .join(layrs_server::retention::gc::ACTIVE)
                .exists()
        {
            return Err("unfinished restore/GC: resume the recorded operation first".into());
        }
        let report = layrs_server::retention::audit(&pool, &config, Default::default()).await?;
        println!(
            "{}",
            serde_json::json!({
                "schemaVersion":"layrs.retention-preflight.v1",
                "cooperativeCasExclusion":true,
                "deletionAuthorized":false,
                "limitations":["Older binaries and external filesystem writers do not participate; all must be stopped.", "No GC plan, unlink, reservation release or catalog deletion is implemented."],
                "audit":report
            })
        );
        return Ok(());
    }
    if args.first().is_some_and(|s| s.starts_with("gc-")) {
        let report = layrs_server::retention::gc::command(&pool, &config, &args).await?;
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    let thresholds =
        StorageThresholds::from_env(&std::env::vars().collect()).inspect_err(|_| {
            if alert_check {
                monitor_unavailable();
            }
        })?;
    let storage_monitor = StorageMonitor::new(
        pool.clone(),
        config.cas_root.clone(),
        config.cas_quotas,
        thresholds,
        config.deployment_id.clone(),
    )?;
    if args
        .first()
        .is_some_and(|s| s == "storage-budget" || s == "storage-check")
    {
        let space = args
            .get(1)
            .map(|id| layrs_core::scm::SpaceId::parse(id.clone()))
            .transpose()
            .inspect_err(|_| {
                if alert_check {
                    monitor_unavailable();
                }
            })?;
        let report = storage_monitor
            .read(space.as_ref())
            .await
            .inspect_err(|_| {
                if alert_check {
                    monitor_unavailable();
                }
            })?;
        println!("{}", serde_json::to_string(&report)?);
        if alert_check {
            pool.close().await;
            std::process::exit(report.severity.exit_code());
        }
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "backup") {
        let manifest = backup::create_backup(
            &pool,
            &config,
            std::path::Path::new(&args[1]),
            Arc::new(LocalPgBackupTools),
        )
        .await?;
        println!(
            "{}",
            serde_json::json!({"event":"backup_completed","objects":manifest.objects.len()})
        );
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "restore") {
        backup::restore_backup(
            &pool,
            &config,
            std::path::Path::new(&args[1]),
            Arc::new(LocalPgBackupTools),
        )
        .await?;
        if let Ok(role) = std::env::var("LAYRS_RUNTIME_ROLE") {
            grant_runtime_role(&pool, &role).await?;
        }
        println!("{{\"event\":\"restore_verified\"}}");
        return Ok(());
    }
    if config.cas_root.join(backup::RESTORE_MARKER).exists() {
        return Err(
            "unfinished restore: server stays non-serving; retain evidence and diagnose".into(),
        );
    }
    if args.first().is_some_and(|s| s == "initialize") {
        installation::initialize(&pool, &config).await?;
        if let Ok(role) = std::env::var("LAYRS_RUNTIME_ROLE") {
            grant_runtime_role(&pool, &role).await?;
        }
        println!("{{\"event\":\"installation_ready\",\"protocol\":\"layrs.sync.v3\"}}");
        return Ok(());
    }
    installation::verify_identity_schema(&pool).await?;
    let identity = PgIdentity::new(
        pool.clone(),
        config.deployment_id.clone(),
        config.public_url.as_str().into(),
    )
    .await?;
    if args.first().is_some_and(|s| s == "recover-password") {
        let password =
            std::fs::read_to_string(&args[2]).map_err(|_| "cannot read recovery password file")?;
        identity
            .recover_password(&args[1], password.trim_end_matches(['\r', '\n']).to_owned())
            .await?;
        println!("{{\"event\":\"password_recovered\",\"previous_tokens\":\"revoked\"}}");
        return Ok(());
    }
    let privileged: bool = sqlx::query_scalar(
        "SELECT rolsuper OR rolcreaterole OR rolcreatedb FROM pg_roles WHERE rolname=current_user",
    )
    .fetch_one(&pool)
    .await?;
    if privileged {
        return Err("serve requires a non-privileged runtime database login; use initialize with maintenance credentials first".into());
    }
    let backend = FsServerCas::open(&config.cas_root, CasLimits::default()).await?;
    let cas = Arc::new(CatalogedFsCas::new(config.store_id.clone(), backend));
    let authorizer = Arc::new(PgPolicyAuthorizer::new(pool.clone(), cas.clone()));
    let repository = PgV3Repository::new(
        pool.clone(),
        cas,
        CapabilityCodec::new(&config.signing_key)?,
        authorizer,
    );
    let repository = repository.with_quotas(config.cas_quotas)?;
    repository.verify_writer_membership().await?;
    verify_deployment(&pool, &config).await?;
    let collaboration = layrs_server::collaboration::router(
        layrs_server::collaboration::CollaborationHttpState::new(
            repository.clone(),
            Arc::new(identity.clone()),
        ),
    );
    let webhooks = layrs_server::webhooks::WebhookService::new(
        repository.clone(),
        Arc::new(identity.clone()),
        layrs_server::webhooks::WebhookPolicy::from_env()?,
        config.signing_key.clone(),
    )?;
    tokio::spawn(webhooks.clone().run());
    let facade = Arc::new(ProductionV3Facade::new(
        repository,
        Arc::new(identity.clone()),
        EventCursorCodec::new(&config.signing_key)?,
        config.public_url.clone(),
    ));
    let telemetry = Telemetry::default();
    let metrics = telemetry::router(identity.clone(), pool.clone(), telemetry.clone());
    let health = HealthState {
        pool,
        cas_root: config.cas_root,
        deployment_id: config.deployment_id,
        telemetry: telemetry.clone(),
    };
    let app = v3::router(V3HttpState::new(facade, V3HttpLimits::default()))
        .merge(layrs_server::storage_budget::router(
            identity.clone(),
            storage_monitor,
        ))
        .merge(identity.router())
        .merge(metrics)
        .merge(collaboration)
        .merge(webhooks.router())
        .merge(
            Router::new()
                .route("/healthz", get(|| async { StatusCode::OK }))
                .route("/readyz", get(readiness))
                .with_state(health),
        )
        .layer(middleware::from_fn_with_state(
            telemetry,
            telemetry::observe,
        ));
    let listener = tokio::net::TcpListener::bind(config.addr).await?;
    eprintln!("{{\"event\":\"server_started\",\"protocol\":\"layrs.sync.v3\"}}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn monitor_unavailable() -> ! {
    println!(
        "{{\"schemaVersion\":\"layrs.storage-budget.v1\",\"severity\":\"unknown\",\"error\":{{\"code\":\"storage_observation_unavailable\"}}}}"
    );
    std::process::exit(3);
}

async fn grant_runtime_role(pool: &PgPool, role: &str) -> Result<()> {
    if role.is_empty()
        || role.len() > 63
        || !role.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid runtime database role".into());
    }
    let writer: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(pool)
            .await?;
    if !writer.starts_with("layrs_v3_w_")
        || !writer
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid installed writer role".into());
    }
    sqlx::query(&format!("GRANT \"{writer}\" TO \"{role}\""))
        .execute(pool)
        .await?;
    Ok(())
}

async fn verify_deployment(pool: &PgPool, config: &ServerConfig) -> Result<()> {
    let expected =
        serde_jcs::to_string(&serde_json::json!({"path":config.cas_root.to_string_lossy()}))?;
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deployments d JOIN stores s USING(store_id) WHERE d.deployment_id=$1 AND d.store_id=$2 AND d.status='ready' AND s.backend_kind='filesystem' AND s.backend_config_jcs=$3)")
        .bind(&config.deployment_id).bind(config.store_id.as_str()).bind(expected).fetch_one(pool).await?;
    if !valid {
        return Err("configured deployment or CAS differs from installation".into());
    }
    Ok(())
}

#[derive(Clone)]
struct HealthState {
    pool: PgPool,
    cas_root: PathBuf,
    deployment_id: String,
    telemetry: Telemetry,
}
async fn readiness(State(state): State<HealthState>) -> (StatusCode, Json<serde_json::Value>) {
    let database = sqlx::query("SELECT 1").execute(&state.pool).await.is_ok();
    let root = state.cas_root;
    let cas = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::Write;
        let path = root
            .join("staging")
            .join(format!(".health-{}", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let result = file
            .write_all(b"layrs-health")
            .and_then(|_| file.sync_all());
        drop(file);
        let cleanup = std::fs::remove_file(path);
        result.and(cleanup)
    })
    .await
    .is_ok_and(|r| r.is_ok());
    state.telemetry.record_readiness(database, cas);
    let healthy = database && cas;
    (
        if healthy {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(
            serde_json::json!({"ready":healthy,"database":database,"cas":cas,"protocol":schema::PROTOCOL_VERSION,"baseline":schema::SCHEMA_VERSION,"deploymentId":state.deployment_id,"authentication":["local-bearer"],"version":env!("CARGO_PKG_VERSION")}),
        ),
    )
}
fn healthcheck() -> Result<()> {
    use std::io::{Read, Write};
    let mut socket =
        std::net::TcpStream::connect_timeout(&"127.0.0.1:8787".parse()?, Duration::from_secs(5))?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut buffer = [0u8; 64];
    let count = socket.read(&mut buffer)?;
    if !buffer[..count].starts_with(b"HTTP/1.1 200 ") {
        return Err("server is not ready".into());
    }
    Ok(())
}
