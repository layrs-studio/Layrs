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
    let download_codec = CapabilityCodec::new(&settings.signing_key).unwrap();
    let app = router(V3HttpState::new(Arc::new(facade), V3HttpLimits::default()))
        .merge(identity.as_ref().clone().router())
        .layer(middleware::from_fn(move |request: Request, next: Next| {
            let download_codec = download_codec.clone();
            async move {
                let transaction = request.uri().path().ends_with("/transactions");
                let download = download_probe(&request, &download_codec, phase);
                if transaction && phase == 1 {
                    event(serde_json::json!({"event":"before_dispatch"}));
                    // Parent kills this actual server process, never releases this request.
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    panic!("server restart fixture gate expired");
                }
                let response = next.run(request).await;
                if let Some((offset, mut observation)) = download {
                    assert_eq!(response.status(), axum::http::StatusCode::PARTIAL_CONTENT);
                    if offset > 0 {
                        observation["event"] = serde_json::json!("download_range");
                        event(observation);
                        if phase == 3 {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            panic!("download server death gate expired");
                        }
                    } else if phase == 3 {
                        let (parts, body) = response.into_parts();
                        let bytes = axum::body::to_bytes(body, 2 * 1024 * 1024).await.unwrap();
                        let prefix = bytes.slice(..bytes.len() / 2);
                        // Several actual body frames, followed by a transport error.
                        // No elapsed delay is evidence: the next authorized Range
                        // and the durable record below establish consumed bytes.
                        let frames = prefix
                            .chunks(4096)
                            .map(bytes::Bytes::copy_from_slice)
                            .map(Ok::<_, std::io::Error>)
                            .chain(std::iter::once(Err(std::io::Error::other(
                                "fixture partial object transport cut",
                            ))))
                            .collect::<Vec<_>>();
                        return axum::response::Response::from_parts(
                            parts,
                            axum::body::Body::from_stream(futures_util::stream::iter(frames)),
                        );
                    }
                }
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
            }
        }));
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
    download_restart(&mut settings, &token, root, &workspace, &provision.space_id);
    eprintln!(
        "Server process restart PASS: before dispatch / after commit, exact pending and receipt, one View advance"
    );
}

fn download_probe(
    request: &Request,
    codec: &CapabilityCodec,
    phase: u8,
) -> Option<(u64, serde_json::Value)> {
    if !matches!(phase, 3 | 4) || request.uri().path() != "/v3/transfers/downloads" {
        return None;
    }
    let token = request
        .headers()
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let grant = codec.authenticate_download(token, now).unwrap();
    if grant.descriptor.kind != layrs_sync::v3::ObjectKind::Chunk
        || grant.descriptor.encoded_byte_len < 16384
    {
        return None;
    }
    assert_eq!(
        grant.descriptor.compression,
        layrs_sync::v3::Compression::Identity
    );
    let range = request.headers().get("range")?.to_str().ok()?;
    let start = range
        .strip_prefix("bytes=")?
        .split('-')
        .next()?
        .parse::<u64>()
        .unwrap();
    Some((
        start,
        serde_json::json!({
            "offset": start, "descriptor": grant.descriptor, "claim":grant.claim_digest,
            "principal":grant.actor.principal_id, "space":grant.actor.space_id,
            "epoch":grant.actor.authorization_epoch, "plan":grant.actor.plan_id,
            "request":grant.actor.request_id, "tokenDigest":blake3::hash(token.as_bytes()).to_hex().to_string()
        }),
    ))
}

fn cache_parts(cache: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(cache).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with("entry-") {
            for part in std::fs::read_dir(entry.path()).unwrap() {
                let part = part.unwrap();
                if part.file_name().to_string_lossy().starts_with("part-") {
                    paths.push(part.path());
                }
            }
        }
    }
    paths.sort();
    paths
}

fn download_restart(
    settings: &mut Settings,
    token: &str,
    root: &Path,
    workspace: &NativeWorkspace,
    space: &layrs_core::scm::SpaceId,
) {
    let bytes = (0..512 * 1024)
        .map(|index| ((index * 17 + index / 251) % 256) as u8)
        .collect::<Vec<_>>();
    std::fs::write(root.join("asset.bin"), &bytes).unwrap();
    workspace.capture().unwrap();
    settings.phase = 0;
    let server = Server::start(settings);
    let remote =
        NativeRemote::with_trusted_ca(&settings.origin, token, settings.ca.as_bytes()).unwrap();
    assert_eq!(
        remote
            .publish(workspace, LayerMutationMode::Advance)
            .unwrap()
            .status,
        ReceiptStatus::Applied
    );
    drop(server);
    let target = root.with_extension("download-target");
    let cache = root.with_extension("download-cache");
    // Keep every directory under the harness-owned clients root for cleanup.
    let target = root.join(target.file_name().unwrap());
    let cache = root.join(cache.file_name().unwrap());
    std::fs::create_dir(&target).unwrap();
    settings.phase = 3;
    let server = Server::start(settings);
    let (tx, rx) = mpsc::channel();
    let (origin, ca, token_copy, space_copy, target_copy, cache_copy) = (
        settings.origin.clone(),
        settings.ca.clone(),
        token.to_owned(),
        space.clone(),
        target.clone(),
        cache.clone(),
    );
    let downloader = std::thread::spawn(move || {
        let remote = NativeRemote::with_trusted_ca(&origin, &token_copy, ca.as_bytes())
            .unwrap()
            .with_download_cache(&cache_copy);
        tx.send(
            remote
                .clone_into(&target_copy, &space_copy, "Interrupted server")
                .is_err(),
        )
        .unwrap();
    });
    let boundary = server.event();
    assert_eq!(boundary["event"], "download_range");
    let offset = boundary["offset"].as_u64().unwrap();
    assert!(offset > 0 && offset < boundary["descriptor"]["encodedByteLen"].as_u64().unwrap());
    // Range cannot be emitted before resume.save; inspect the actual immutable
    // checkpoint now, not after an arbitrary sleep or after the server kill.
    let parts = cache_parts(&cache);
    assert_eq!(parts.len(), 1);
    let record = std::fs::read(&parts[0]).unwrap();
    let header_length = u32::from_be_bytes(record[..4].try_into().unwrap()) as usize;
    let header: serde_json::Value = serde_json::from_slice(&record[4..4 + header_length]).unwrap();
    let saved = &record[4 + header_length..];
    assert_eq!(header["start"], 0);
    assert_eq!(header["end"], offset);
    assert_eq!(saved.len() as u64, offset);
    assert_eq!(header["digest"], blake3::hash(saved).to_hex().to_string());
    assert_eq!(header["capability_digest"], boundary["tokenDigest"]);
    assert!(
        !record
            .windows(token.len())
            .any(|slice| slice == token.as_bytes())
    );
    assert_eq!(
        std::fs::read_dir(&target).unwrap().count(),
        0,
        "no local Store before verified closure"
    );
    drop(server);
    assert!(
        rx.recv_timeout(Duration::from_secs(35))
            .expect("download must observe killed server")
    );
    downloader.join().unwrap();
    assert_eq!(std::fs::read(&parts[0]).unwrap(), record);
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
    settings.phase = 4;
    let server = Server::start(settings);
    let remote = NativeRemote::with_trusted_ca(&settings.origin, token, settings.ca.as_bytes())
        .unwrap()
        .with_download_cache(&cache);
    remote
        .clone_into(&target, space, "After server restart")
        .unwrap();
    let resumed = server.event();
    for field in [
        "event",
        "offset",
        "descriptor",
        "claim",
        "principal",
        "space",
        "epoch",
    ] {
        assert_eq!(
            resumed[field], boundary[field],
            "renewed exact binding {field}"
        );
    }
    for field in ["tokenDigest", "plan", "request"] {
        assert_ne!(
            resumed[field], boundary[field],
            "fresh authorization {field}"
        );
    }
    assert_eq!(std::fs::read(target.join("asset.bin")).unwrap(), bytes);
    assert_eq!(
        std::fs::read(target.join("content")).unwrap(),
        std::fs::read(root.join("content")).unwrap()
    );
    assert!(!NativeWorkspace::open(&target).unwrap().is_dirty().unwrap());
    assert!(cache_parts(&cache).is_empty());
    drop(server);
    eprintln!(
        "Server object restart PASS: durable prefix before kill, fresh exact claim/capability, nonzero Range, complete bytes"
    );
}
