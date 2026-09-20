//! Real server process death, with persistent PostgreSQL/CAS and exact receipt replay.
//! Gates belong only to this fixture; production transaction code is unchanged.
use axum::{
    extract::Request,
    middleware::{self, Next},
};
use layrs_client_core::native_scm::{NativeRemote, NativeWorkspace};
use layrs_server::{identity::PgIdentity, v3::*};
use layrs_store_server::v3::{
    CapabilityCodec, CasLimits, CatalogedFsCas, EventCursorCodec, FsServerCas, PgPolicyAuthorizer,
    PgV3Repository,
};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, mpsc},
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub database_url: String,
    pub origin: String,
    pub address: String,
    pub cas_root: PathBuf,
    pub deployment: String,
    pub store: layrs_core::scm::StoreId,
    pub signing_key: Vec<u8>,
    pub ca: String,
    pub tls_key: String,
    pub phase: u8,
}

struct Server {
    child: Child,
    events: mpsc::Receiver<serde_json::Value>,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Server {
    fn start(settings: &Settings) -> Self {
        let encoded = serde_json::to_vec(settings).unwrap();
        assert!(encoded.len() < 64 * 1024);
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "server_restart_child",
                "--ignored",
                "--nocapture",
            ])
            .env("LAYRS_SERVER_RESTART_CHILD", "1")
            .env_remove("LAYRS_TEST_DATABASE_URL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let (tx, events) = mpsc::channel();
        let mut server = Self { child, events };
        // Install the kill/wait guard before any fallible pipe work.
        // Only an inherited private pipe carries credentials and TLS private material.
        server
            .child
            .stdin
            .take()
            .unwrap()
            .write_all(&encoded)
            .unwrap();
        let stdout = server.child.stdout.take().unwrap();
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let count = reader
                    .by_ref()
                    .take(2 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut line)
                    .unwrap_or(0);
                if count == 0 || line.len() > 2 * 1024 * 1024 {
                    break;
                }
                if let Some(position) = line.windows(7).position(|bytes| bytes == b"EVENT: ") {
                    if let Ok(value) = serde_json::from_slice(&line[position + 7..]) {
                        if tx.send(value).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        assert_eq!(
            server.event()["event"],
            "ready",
            "server child readiness; output withheld"
        );
        server
    }
    fn event(&self) -> serde_json::Value {
        self.events
            .recv_timeout(Duration::from_secs(30))
            .expect("server boundary timeout; child output withheld")
    }
}
fn event(value: serde_json::Value) {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "EVENT: {value}").unwrap();
    stdout.flush().unwrap();
}

pub async fn child() {
    if std::env::var_os("LAYRS_SERVER_RESTART_CHILD").is_none() {
        return;
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() <= 64 * 1024);
    let settings: Settings =
        serde_json::from_slice(&bytes).expect("invalid private child settings");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .connect(&settings.database_url)
        .await
        .expect("fixture database connection failed");
    let privileged: bool = sqlx::query_scalar(
        "SELECT rolsuper OR rolcreaterole OR rolcreatedb FROM pg_roles WHERE rolname=current_user",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!privileged);
    let identity = Arc::new(
        PgIdentity::new(pool.clone(), settings.deployment, settings.origin.clone())
            .await
            .unwrap(),
    );
    let cas = Arc::new(CatalogedFsCas::new(
        settings.store,
        FsServerCas::open(&settings.cas_root, CasLimits::default())
            .await
            .unwrap(),
    ));
    let repository = PgV3Repository::new(
        pool.clone(),
        cas.clone(),
        CapabilityCodec::new(&settings.signing_key).unwrap(),
        Arc::new(PgPolicyAuthorizer::new(pool, cas)),
    );
    repository.verify_writer_membership().await.unwrap();
    let policy = layrs_store_server::v3::TransferUrlPolicy::new([&settings.origin], []).unwrap();
    let facade = ProductionV3Facade::new(
        repository,
        identity.clone(),
        EventCursorCodec::new(&settings.signing_key).unwrap(),
        policy.validate_base(&settings.origin).unwrap(),
    );
    let phase = settings.phase;
    let app = router(V3HttpState::new(Arc::new(facade), V3HttpLimits::default()))
        .merge(identity.as_ref().clone().router())
        .layer(middleware::from_fn(
            move |request: Request, next: Next| async move {
                let transaction = request.uri().path().ends_with("/transactions");
                if transaction && phase == 1 {
                    event(serde_json::json!({"event":"before_dispatch"}));
                    // Parent kills this actual server process, never releases this request.
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    panic!("server restart fixture gate expired");
                }
                let response = next.run(request).await;
                if transaction && phase == 2 {
                    assert!(response.status().is_success());
                    let (_, body) = response.into_parts();
                    let bytes = axum::body::to_bytes(body, 2 * 1024 * 1024).await.unwrap();
                    let receipt: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    event(serde_json::json!({"event":"after_commit", "receipt":receipt}));
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    panic!("server restart fixture gate expired");
                }
                response
            },
        ));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        settings.ca.into_bytes(),
        settings.tls_key.into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind(&settings.address).unwrap();
    listener.set_nonblocking(true).unwrap();
    event(serde_json::json!({"event":"ready"}));
    axum_server::from_tcp_rustls(listener, tls)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

fn journal(workspace: &NativeWorkspace) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(workspace.store_root().join("native-remote.json")).unwrap(),
    )
    .unwrap()
}
pub fn run(mut settings: Settings, token: String, root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("content"), b"initial").unwrap();
    let workspace = NativeWorkspace::initialize(root, "Server restart").unwrap();
    let make_remote =
        || NativeRemote::with_trusted_ca(&settings.origin, &token, settings.ca.as_bytes()).unwrap();
    let remote = make_remote();
    let server = Server::start(&settings);
    let provision = remote
        .provision(&workspace, "server-restart", "server-restart")
        .unwrap();
    assert_eq!(
        remote
            .publish(&workspace, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    let mut baseline = remote
        .bootstrap(&provision.space_id)
        .unwrap()
        .current_repository_view_generation;
    drop(server);
    for phase in [1, 2] {
        let bytes = format!("server death phase {phase}, exact work");
        std::fs::write(root.join("content"), bytes.as_bytes()).unwrap();
        workspace.capture().unwrap();
        let operation = workspace.state().unwrap().operation.id().clone();
        settings.phase = phase;
        let server = Server::start(&settings);
        let publisher_root = root.to_owned();
        let publisher_origin = settings.origin.clone();
        let publisher_ca = settings.ca.clone();
        let publisher_token = token.clone();
        let (tx, rx) = mpsc::channel();
        let publisher = std::thread::spawn(move || {
            let remote = NativeRemote::with_trusted_ca(
                &publisher_origin,
                &publisher_token,
                publisher_ca.as_bytes(),
            )
            .unwrap();
            let workspace = NativeWorkspace::open(publisher_root).unwrap();
            tx.send(
                remote
                    .publish(&workspace, LayerMutationMode::Advance)
                    .is_err(),
            )
            .unwrap();
        });
        let boundary = server.event();
        assert_eq!(
            boundary["event"],
            if phase == 1 {
                "before_dispatch"
            } else {
                "after_commit"
            }
        );
        let pending = journal(&workspace)["pending"].clone();
        assert!(pending.is_object());
        drop(server); // Hard stop + wait: connections and in-process state are gone.
        assert!(
            rx.recv_timeout(Duration::from_secs(35))
                .expect("publication did not observe server death")
        );
        publisher.join().unwrap();
        assert_eq!(workspace.state().unwrap().operation.id(), &operation);
        assert_eq!(
            std::fs::read(root.join("content")).unwrap(),
            bytes.as_bytes()
        );
        assert_eq!(journal(&workspace)["pending"], pending);
        settings.phase = 0;
        let restarted = Server::start(&settings);
        let remote =
            NativeRemote::with_trusted_ca(&settings.origin, &token, settings.ca.as_bytes())
                .unwrap();
        assert_eq!(
            remote
                .bootstrap(&provision.space_id)
                .unwrap()
                .current_repository_view_generation,
            baseline + u64::from(phase == 2)
        );
        let reopened = NativeWorkspace::open(root).unwrap();
        let receipt = remote
            .publish(&reopened, LayerMutationMode::Advance)
            .unwrap();
        assert_eq!(receipt.status, ReceiptStatus::Applied);
        let persisted = journal(&reopened);
        assert!(persisted["pending"].is_null());
        assert_eq!(
            persisted["lastReceipt"]["idempotencyKey"],
            pending["idempotencyKey"]
        );
        if phase == 2 {
            assert_eq!(persisted["lastReceipt"], boundary["receipt"]);
        }
        baseline += 1;
        assert_eq!(
            remote
                .bootstrap(&provision.space_id)
                .unwrap()
                .current_repository_view_generation,
            baseline
        );
        assert_eq!(
            std::fs::read(root.join("content")).unwrap(),
            bytes.as_bytes()
        );
        assert!(!reopened.is_dirty().unwrap());
        drop(restarted);
    }
    eprintln!(
        "Server process restart PASS: before dispatch / after commit, exact pending and receipt, one View advance"
    );
}
