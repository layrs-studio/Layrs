//! Disposable TLS/PostgreSQL fixture for real Desktop/Web UI harnesses.
//! Credentials go only to the explicit private fixture file, never stdout.
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_server::{
    collaboration::{self, CollaborationHttpState},
    config::ServerConfig,
    identity::{BootstrapRequest, PgIdentity, random_secret},
    installation,
    v3::*,
    webhooks::{WebhookPolicy, WebhookService},
};
use layrs_store_server::v3::*;
use layrs_sync::v3::{LayerMutationMode, PrincipalId, ReceiptStatus};
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{collections::BTreeMap, io::Write, path::PathBuf, str::FromStr, sync::Arc};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
mod native_ui_resources;
mod native_ui_transfers;
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--cleanup-fixture") {
        let path = std::env::args_os()
            .nth(2)
            .ok_or("missing resource ledger")?;
        return native_ui_resources::cleanup(&PathBuf::from(path)).await;
    }
    let fixture_file = std::env::var_os("LAYRS_TEST_UI_FIXTURE_FILE")
        .map(PathBuf::from)
        .ok_or("set LAYRS_TEST_UI_FIXTURE_FILE to a new private absolute filename")?;
    if !fixture_file.is_absolute() || fixture_file.exists() {
        return Err("fixture filename must be new and absolute".into());
    }
    let options = PgConnectOptions::from_str(
        &std::env::var("LAYRS_TEST_DATABASE_URL")
            .map_err(|_| "set disposable test PostgreSQL endpoint")?,
    )?;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let database = format!("layrs_ui_test_{suffix}");
    let runtime = format!("layrs_ui_runtime_{suffix}");
    native_ui_resources::ledger(&fixture_file, &database, &runtime)?;
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&admin)
        .await?;
    let maintenance = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options.clone().database(&database))
        .await?;
    let root = fixture_file
        .parent()
        .ok_or("fixture parent missing")?
        .join(format!("native-ui-{suffix}"));
    std::fs::create_dir(&root)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let origin = format!("https://{address}");
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()])?;
    let ca = cert.cert.pem();
    let ca_file = root.join("test-ca.pem");
    std::fs::write(&ca_file, &ca)?;
    let key_file = root.join("test-key.pem");
    native_ui_resources::private(&key_file, cert.key_pair.serialize_pem().as_bytes())?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        ca.clone().into_bytes(),
        cert.key_pair.serialize_pem().into_bytes(),
    )
    .await?;
    let bootstrap_secret = random_secret();
    let config = ServerConfig::parse(&BTreeMap::from([
        ("LAYRS_PUBLIC_URL".into(), origin.clone()),
        ("LAYRS_SERVER_ADDR".into(), address.to_string()),
        ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
        (
            "LAYRS_CAS_ROOT".into(),
            root.join("cas").display().to_string(),
        ),
        (
            "LAYRS_DEPLOYMENT_ID".into(),
            format!("deployment:ui-{suffix}"),
        ),
        ("LAYRS_STORE_ID".into(), format!("store:ui-{suffix}")),
        ("LAYRS_SIGNING_KEY".into(), random_secret()),
        ("LAYRS_BOOTSTRAP_SECRET".into(), bootstrap_secret.clone()),
    ]))?;
    installation::initialize(&maintenance, &config).await?;
    let runtime_password = random_secret();
    sqlx::query(&format!(
        "CREATE ROLE {runtime} LOGIN PASSWORD '{runtime_password}'"
    ))
    .execute(&admin)
    .await?;
    let writer: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(&maintenance)
            .await?;
    sqlx::query(&format!("GRANT {writer} TO {runtime}"))
        .execute(&admin)
        .await?;
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .connect_with(
            options
                .database(&database)
                .username(&runtime)
                .password(&runtime_password),
        )
        .await?;
    let identity = Arc::new(
        PgIdentity::new(pool.clone(), config.deployment_id.clone(), origin.clone()).await?,
    );
    let password = format!("ui-test-{}", random_secret());
    let bob_password = format!("ui-test-{}", random_secret());
    let session = identity
        .bootstrap(BootstrapRequest {
            secret: bootstrap_secret,
            login: "alice-ui@example.test".into(),
            password: password.clone(),
            display_name: "Alice — administratrice test".into(),
        })
        .await?;
    let cas = Arc::new(CatalogedFsCas::new(
        config.store_id.clone(),
        FsServerCas::open(&config.cas_root, CasLimits::default()).await?,
    ));
    let repository = PgV3Repository::new(
        pool.clone(),
        cas.clone(),
        CapabilityCodec::new(&config.signing_key)?,
        Arc::new(PgPolicyAuthorizer::new(pool.clone(), cas)),
    );
    let facade = Arc::new(ProductionV3Facade::new(
        repository.clone(),
        identity.clone(),
        EventCursorCodec::new(&config.signing_key)?,
        config.public_url.clone(),
    ));
    let webhook_policy = WebhookPolicy {
        allowed_origins: std::collections::BTreeSet::from([origin.clone()]),
        private_origins: std::collections::BTreeSet::from([origin.clone()]),
        ca_pem: Some(ca.clone().into_bytes()),
    };
    let webhooks = WebhookService::new(
        repository.clone(),
        identity.clone(),
        webhook_policy,
        config.signing_key.clone(),
    )?;
    let telemetry = layrs_server::telemetry::Telemetry::default();
    let transfer_control = native_ui_transfers::Control::new(&session.access_token);
    let app = router(V3HttpState::new(facade, V3HttpLimits::default()))
        .merge(layrs_server::telemetry::router(
            identity.as_ref().clone(),
            pool.clone(),
            telemetry.clone(),
        ))
        .merge(identity.as_ref().clone().router())
        .merge(layrs_server::storage_budget::router(
            identity.as_ref().clone(),
            layrs_server::storage_budget::StorageMonitor::new(
                pool.clone(),
                config.cas_root.clone(),
                config.cas_quotas,
                Default::default(),
                config.deployment_id.clone(),
            )?,
        ))
        .merge(collaboration::router(CollaborationHttpState::new(
            repository.clone(),
            identity.clone(),
        )))
        .merge(webhooks.clone().router())
        .merge(transfer_control.router())
        .route(
            "/test-webhook",
            axum::routing::post(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .layer(axum::middleware::from_fn_with_state(
            telemetry,
            layrs_server::telemetry::observe,
        ))
        .layer(axum::middleware::from_fn_with_state(
            transfer_control,
            native_ui_transfers::pause_download,
        ));
    let server = tokio::spawn(
        axum_server::from_tcp_rustls(listener.into_std()?, tls).serve(app.into_make_service()),
    );
    tokio::spawn(webhooks.run());
    let fixture_root = root.clone();
    let fixture_origin = origin.clone();
    let fixture_ca = ca.clone();
    let fixture_token = session.access_token.clone();
    let fixture_bob_password = bob_password.clone();
    let (provisioned,first,current,bob,release)=tokio::task::spawn_blocking(move||->Result<_>{
        let checkout=fixture_root.join("project");std::fs::create_dir(&checkout)?;std::fs::create_dir(checkout.join("docs"))?;std::fs::write(checkout.join("docs/readme.txt"),"Première capture partagée.\n")?;
        let workspace=NativeWorkspace::initialize(&checkout,"Projet de test UI")?;
        let remote=NativeRemote::with_trusted_ca(&fixture_origin,&fixture_token,fixture_ca.as_bytes())?;
        let provisioned=remote.provision(&workspace,"ui-team","project")?;
        let first=remote.publish(&workspace,LayerMutationMode::Advance)?;if first.status!=ReceiptStatus::Applied{return Err("fixture first publication rejected".into())}
        std::fs::write(checkout.join("docs/readme.txt"),"Première capture partagée.\nModification proposée pour la revue.\n")?;workspace.capture()?;
        let release=workspace.create_release("0.1.0-ui", "Version de recette UI")?;
        let current=remote.publish(&workspace,LayerMutationMode::Advance)?;if current.status!=ReceiptStatus::Applied{return Err("fixture second publication rejected".into())}
        let assertion=remote.publish_observation(&workspace,&current.accepted_step_ids[0],&layrs_core::scm::Timestamp::parse("2020-01-02T03:04:05Z")?,layrs_core::scm::CaptureReason::ManualSnapshot,&layrs_sync::v3::IdempotencyKey::parse("ui-fixture-observation-00001")?)?;
        if assertion.status!=ReceiptStatus::Applied{return Err("fixture client observation rejected".into())}
        let client=reqwest::blocking::Client::builder().add_root_certificate(reqwest::Certificate::from_pem(fixture_ca.as_bytes())?).redirect(reqwest::redirect::Policy::none()).build()?;
        let response=client.post(format!("{fixture_origin}/v3/auth/accounts")).bearer_auth(&fixture_token).json(&json!({"login":"bob-ui@example.test","password":fixture_bob_password,"displayName":"Bob — relecteur test"})).send()?;
        if !response.status().is_success(){return Err("fixture account creation failed".into())}let bob:serde_json::Value=response.json()?;
        Ok((provisioned,first.accepted_step_ids[0].clone(),current.accepted_step_ids[0].clone(),PrincipalId::parse(bob["principalId"].as_str().ok_or("missing account ID")?)?,release.id().clone()))
    }).await??;
    let principal = load_principal_context(&pool, &session.principal_id).await?;
    let collaboration = PgCollaborationStore::new(repository);
    let initial_policy = collaboration
        .policy_management(&principal, &provisioned.space_id)
        .await?;
    collaboration
        .grant_member(
            &principal,
            &provisioned.space_id,
            GrantMemberRequest {
                principal_id: bob.clone(),
                role: MemberRole::Member,
            },
        )
        .await?;
    let review = collaboration
        .propose_review(
            &principal,
            &provisioned.space_id,
            None,
            ReviewProposalRequest {
                idempotency_key: "ui-fixture-review-00001".into(),
                layer_id: provisioned.layer_id.clone(),
                proposed_step_id: current.clone(),
                title: "Vérifier la modification du guide".into(),
                participants: vec![bob.clone()],
                expected_version: 0,
            },
        )
        .await?;
    let document = json!({"origin":origin,"webhookUrl":format!("{origin}/test-webhook"),"caFile":ca_file,"tlsKeyFile":key_file,"caPem":ca,"root":root,"workspaceRoot":root.join("project"),"spaceId":provisioned.space_id,"layerId":provisioned.layer_id,"releaseId":release,"initialPolicySetId":initial_policy.current_policy_set_id,"firstStepId":first,"stepId":current,"reviewId":review.review_id,"databaseName":database,"runtimeRole":runtime,"writerRole":writer,"accounts":{"alice":{"login":"alice-ui@example.test","password":password,"principalId":session.principal_id},"bob":{"login":"bob-ui@example.test","password":bob_password,"principalId":bob}},"adminAccessToken":session.access_token});
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&fixture_file)?;
    file.write_all(&serde_json::to_vec_pretty(&document)?)?;
    file.sync_all()?;
    println!(
        "Native UI TLS fixture ready; private configuration written to {}",
        fixture_file.display()
    );
    // The UI harness owns this disposable service's lifetime and named database.
    // No timeout silently terminates an in-progress browser/IPC workflow.
    server.await??;
    Ok(())
}
