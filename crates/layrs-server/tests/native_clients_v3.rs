//! Two independent native Stores through the real HTTP production service.
#[path = "support/backup_drill.rs"]
mod backup_drill;
#[path = "support/cancellation_drill.rs"]
mod cancellation_drill;
#[path = "support/unchanged_drill.rs"]
mod unchanged_drill;
#[path = "support/choose_client_drill.rs"]
mod choose_client_drill;
#[path = "support/compare_drill.rs"]
mod compare_drill;
#[path = "support/concurrent_clients.rs"]
mod concurrent_clients;
#[path = "support/concurrent_publications.rs"]
mod concurrent_publications;
#[path = "support/conflict_drill.rs"]
mod conflict_drill;
#[path = "support/durable_download_drill.rs"]
mod durable_download_drill;

#[test]
#[ignore = "child process invoked by durable spool recipe"]
fn durable_download_child() {
    durable_download_drill::child();
}
#[path = "support/fetch_pagination_drill.rs"]
mod fetch_pagination_drill;
#[path = "support/policy_client_drill.rs"]
mod policy_client_drill;
#[path = "support/content_acl_drill.rs"]
mod content_acl_drill;
#[path = "support/product_drill.rs"]
mod product_drill;
#[path = "support/publication_cancellation_drill.rs"]
mod publication_cancellation_drill;
#[path = "support/release_drill.rs"]
mod release_drill;
#[path = "support/rename_conflict_drill.rs"]
mod rename_conflict_drill;
#[path = "support/rewrite_client_drill.rs"]
mod rewrite_client_drill;
#[path = "support/runner_drill.rs"]
mod runner_drill;
#[path = "support/semantic_conflict_drill.rs"]
mod semantic_conflict_drill;
#[path = "support/sync_volume_drill.rs"]
mod sync_volume_drill;
#[path = "support/streaming_volume_drill.rs"]
mod streaming_volume_drill;
#[path = "support/transfer_drill.rs"]
mod transfer_drill;
#[path = "support/transfer_expiry_drill.rs"]
mod transfer_expiry_drill;
#[path = "support/publication_restart_drill.rs"]
mod publication_restart_drill;

#[test]
#[ignore = "child process invoked by publication restart recipe"]
fn publication_restart_child() {
    publication_restart_drill::child();
}
use layrs_client_core::native_scm::{NativeDevicePoll, NativeRemote, NativeWorkspace};
use layrs_core::scm::OperationKind;
use layrs_server::{
    config::ServerConfig,
    identity::{BootstrapRequest, PgIdentity, random_secret},
    installation,
    v3::*,
};
use layrs_store_local::scm_v1::materialize::{MaterializationFaults, MaterializationPoint};
use layrs_store_server::v3::{
    CapabilityCodec, CasLimits, CatalogedFsCas, EventCursorCodec, FsServerCas, PgPolicyAuthorizer,
    PgV3Repository,
};
use layrs_sync::v3::{LayerMutationMode, ReceiptStatus};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{collections::BTreeMap, str::FromStr, sync::Arc};
struct FailAt(MaterializationPoint);
impl MaterializationFaults for FailAt {
    fn check(&self, point: MaterializationPoint) -> std::io::Result<()> {
        if self.0 == point {
            Err(std::io::Error::other("injected receive interruption"))
        } else {
            Ok(())
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires LAYRS_TEST_DATABASE_URL disposable PostgreSQL administrator endpoint"]
async fn two_native_clients_publish_clone_receive_divergence_and_local_operations() {
    let options = PgConnectOptions::from_str(
        &std::env::var("LAYRS_TEST_DATABASE_URL").expect("set disposable admin URL"),
    )
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let db = format!("layrs_clients_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {db}"))
        .execute(&admin)
        .await
        .unwrap();
    let maintenance = PgPoolOptions::new()
        .max_connections(12)
        .connect_with(options.clone().database(&db))
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("https://{address}");
    let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let ca = cert.cert.pem();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        ca.clone().into_bytes(),
        cert.key_pair.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let secret = random_secret();
    let root = std::env::temp_dir().join(&db);
    let config = ServerConfig::parse(&BTreeMap::from([
        ("LAYRS_PUBLIC_URL".into(), origin.clone()),
        ("LAYRS_ALLOW_LOOPBACK_HTTP".into(), "true".into()),
        ("LAYRS_SERVER_ADDR".into(), address.to_string()),
        ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
        ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
        ("LAYRS_DEPLOYMENT_ID".into(), "deployment:sync-test".into()),
        ("LAYRS_STORE_ID".into(), "store:sync-test".into()),
        ("LAYRS_SIGNING_KEY".into(), random_secret()),
        ("LAYRS_BOOTSTRAP_SECRET".into(), secret.clone()),
    ]))
    .unwrap();
    installation::initialize(&maintenance, &config)
        .await
        .unwrap();
    let runtime = format!("layrs_runtime_{}", uuid::Uuid::new_v4().simple());
    let runtime_password = random_secret();
    sqlx::query(&format!(
        "CREATE ROLE {runtime} LOGIN PASSWORD '{runtime_password}'"
    ))
    .execute(&admin)
    .await
    .unwrap();
    let writer: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(&maintenance)
            .await
            .unwrap();
    sqlx::query(&format!("GRANT {writer} TO {runtime}"))
        .execute(&admin)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(12)
        .connect_with(
            options
                .database(&db)
                .username(&runtime)
                .password(&runtime_password),
        )
        .await
        .unwrap();
    let identity = Arc::new(
        PgIdentity::new(
            pool.clone(),
            config.deployment_id.clone(),
            config.public_url.as_str().into(),
        )
        .await
        .unwrap(),
    );
    let session = identity
        .bootstrap(BootstrapRequest {
            secret,
            login: "alice@example.com".into(),
            password: "correct horse battery staple".into(),
            display_name: "Alice".into(),
        })
        .await
        .unwrap();
    let cas = Arc::new(CatalogedFsCas::new(
        config.store_id.clone(),
        FsServerCas::open(&root, CasLimits::default())
            .await
            .unwrap(),
    ));
    let repository = PgV3Repository::new(
        pool.clone(),
        cas.clone(),
        CapabilityCodec::new(&config.signing_key).unwrap(),
        Arc::new(PgPolicyAuthorizer::new(pool.clone(), cas)),
    );
    let facade = ProductionV3Facade::new(
        repository.clone(),
        identity.clone(),
        EventCursorCodec::new(&config.signing_key).unwrap(),
        config.public_url.clone(),
    );
    let short_plans = router(V3HttpState::new(Arc::new(ProductionV3Facade::new(
        repository.clone(), identity.clone(), EventCursorCodec::new(&config.signing_key).unwrap(),
        config.public_url.clone(),
    ).with_plan_lifetime_seconds(3).unwrap()), V3HttpLimits::default()));
    let lose_response = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let corrupt_download = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let interrupted_download = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let resumed_download = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let interrupt_min_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let interrupted = interrupted_download.clone();
    let resumed = resumed_download.clone();
    let minimum = interrupt_min_bytes.clone();
    let lose = lose_response.clone();
    let corrupt = corrupt_download.clone();
    let app = router(V3HttpState::new(Arc::new(facade), V3HttpLimits::default()))
        .merge(identity.as_ref().clone().router())
        .merge(layrs_server::collaboration::router(
            layrs_server::collaboration::CollaborationHttpState::new(repository, identity.clone()),
        ))
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let lose = lose.clone();
                let corrupt = corrupt.clone();
                let interrupted = interrupted.clone();
                let resumed = resumed.clone();
                let minimum = minimum.clone();
                async move {
                    let path = request.uri().path().to_owned();
                    if path == "/v3/transfers/downloads"
                        && request
                            .headers()
                            .get("range")
                            .and_then(|v| v.to_str().ok())
                            .is_some_and(|value| !value.starts_with("bytes=0-"))
                    {
                        resumed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                    let response = next.run(request).await;
                    if path == "/v3/transfers/downloads"
                        && response
                            .headers()
                            .get("content-length")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(0)
                            >= minimum.load(std::sync::atomic::Ordering::SeqCst)
                        && interrupted
                            .fetch_update(
                                std::sync::atomic::Ordering::SeqCst,
                                std::sync::atomic::Ordering::SeqCst,
                                |value| value.checked_sub(1),
                            )
                            .is_ok()
                    {
                        let (parts, body) = response.into_parts();
                        let bytes = axum::body::to_bytes(body, 64 * 1024 * 1024).await.unwrap();
                        let prefix = bytes.slice(..(bytes.len() / 2).max(1));
                        let stream =
                            futures_util::stream::unfold(Some(prefix), |value| async move {
                                match value {
                                    Some(bytes) => Some((Ok::<_, std::io::Error>(bytes), None)),
                                    None => {
                                        tokio::time::sleep(std::time::Duration::from_millis(20))
                                            .await;
                                        Some((
                                            Err(std::io::Error::other("injected transport cut")),
                                            None,
                                        ))
                                    }
                                }
                            });
                        return axum::response::Response::from_parts(
                            parts,
                            axum::body::Body::from_stream(stream),
                        );
                    }
                    if response.status().is_server_error() {
                        eprintln!("Native test backend failure: {} {path}", response.status());
                    }
                    if path.ends_with("/transactions")
                        && lose.swap(false, std::sync::atomic::Ordering::SeqCst)
                    {
                        return axum::response::Response::builder()
                            .status(502)
                            .body(axum::body::Body::empty())
                            .unwrap();
                    }
                    if path == "/v3/transfers/downloads"
                        && corrupt.swap(false, std::sync::atomic::Ordering::SeqCst)
                    {
                        let (parts, body) = response.into_parts();
                        let mut bytes = axum::body::to_bytes(body, 64 * 1024 * 1024)
                            .await
                            .unwrap()
                            .to_vec();
                        if !bytes.is_empty() {
                            bytes[0] ^= 0x20;
                        }
                        return axum::response::Response::from_parts(
                            parts,
                            axum::body::Body::from(bytes),
                        );
                    }
                    response
                }
            },
        ));
    let (app, cancellation_probe) = cancellation_drill::wrap(app);
    let (app, publication_probe) = publication_cancellation_drill::wrap(app);
    let (app, rewrite_probe) = rewrite_client_drill::wrap(app);
    let (app, unchanged_probe) = unchanged_drill::wrap(app);
    let (app, expiry_probe) = transfer_expiry_drill::wrap(app, short_plans);
    let (app, restart_probe) = publication_restart_drill::wrap(app);
    let server = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener.into_std().unwrap(), tls)
            .serve(app.into_make_service())
            .await
            .unwrap()
    });
    let clients = root.with_extension("clients");
    std::fs::create_dir_all(clients.join("alice")).unwrap();
    std::fs::create_dir_all(clients.join("bob")).unwrap();
    std::fs::create_dir_all(clients.join("corrupt")).unwrap();
    let client_root = clients.clone();
    let space = tokio::task::spawn_blocking(move || {
        let aroot = client_root.join("alice");
        let broot = client_root.join("bob");
        std::fs::create_dir(aroot.join("docs")).unwrap();
        std::fs::write(aroot.join("docs/readme.txt"), b"initial contents").unwrap();
        let a = NativeWorkspace::initialize(&aroot, "Project").unwrap();
        let logged = NativeRemote::login(
            &origin,
            "alice@example.com",
            "correct horse battery staple",
            Some(ca.as_bytes()),
        )
        .unwrap();
        assert_eq!(logged.principal_id, session.principal_id);
        let authenticated =
            NativeRemote::with_trusted_ca(&origin, &logged.access_token, ca.as_bytes()).unwrap();
        assert_eq!(
            authenticated.whoami().unwrap()["principal_id"].as_str(),
            Some(session.principal_id.as_str())
        );
        let anonymous = NativeRemote::unauthenticated(&origin, Some(ca.as_bytes())).unwrap();
        let device = anonymous
            .device_start("Native TLS integration device")
            .unwrap();
        assert!(matches!(
            anonymous.device_poll(&device.device_code).unwrap(),
            NativeDevicePoll::Pending { interval: 5 }
        ));
        assert!(matches!(
            anonymous.device_poll(&device.device_code).unwrap(),
            NativeDevicePoll::Pending { interval: 10 }
        ));
        authenticated.logout().unwrap();
        assert_eq!(
            authenticated.whoami().unwrap_err().code(),
            "remote_unauthorized"
        );
        let remote =
            NativeRemote::with_trusted_ca(&origin, &session.access_token, ca.as_bytes()).unwrap();
        let provision = remote.provision(&a, "native-team", "project").unwrap();
        let feature = a.create_layer("feature").unwrap();
        let marker = a.create_marker("baseline", "initial publication").unwrap();
        let release = a.create_release("1.0.0", "first release").unwrap();
        assert_eq!(provision.space_id, *a.state().unwrap().space.id());
        assert_eq!(
            remote
                .provision(&a, "native-team", "project")
                .unwrap()
                .space_id,
            provision.space_id
        );
        assert!(
            NativeRemote::new(&origin, &session.access_token)
                .unwrap()
                .bootstrap(&provision.space_id)
                .is_err(),
            "untrusted TLS certificate must be refused"
        );
        assert!(matches!(
            remote.publish(&a, LayerMutationMode::Advance),
            Err(layrs_client_core::native_scm::NativeRemoteError::Http(502))
        ));
        let pending: serde_json::Value = serde_json::from_slice(
            &std::fs::read(a.store_root().join("native-remote.json")).unwrap(),
        )
        .unwrap();
        let first = remote.publish(&a, LayerMutationMode::Advance).unwrap();
        assert_eq!(first.accepted_products.len(), 3, "{:?}", first.rejections);
        assert_eq!(
            pending["pending"]["idempotencyKey"].as_str(),
            Some(first.idempotency_key.as_str())
        );
        assert_eq!(
            first.status,
            ReceiptStatus::Applied,
            "{:?}",
            first.rejections
        );
        if std::env::var_os("LAYRS_TEST_STREAMING_VOLUME_ONLY").is_some() {
            streaming_volume_drill::run(&remote, &client_root);
            return provision.space_id;
        }
        if std::env::var_os("LAYRS_TEST_SYNC_VOLUMES_ONLY").is_some() {
            sync_volume_drill::run(&remote, &client_root);
            return provision.space_id;
        }
        if std::env::var_os("LAYRS_TEST_DURABLE_DOWNLOAD_ONLY").is_some() {
            durable_download_drill::run(
                &client_root,
                &origin,
                &session.access_token,
                &ca,
                &provision.space_id,
                &interrupted_download,
                &resumed_download,
            );
            return provision.space_id;
        }
        if std::env::var_os("LAYRS_TEST_CONTENT_ACL_ONLY").is_some() {
            content_acl_drill::run(&remote,&client_root,&origin,&session.access_token,&ca);
            return provision.space_id;
        }
        if std::env::var_os("LAYRS_TEST_POLICY_ONLY").is_none() {
            transfer_expiry_drill::run(&remote, &client_root, &origin, &session.access_token, &ca, &expiry_probe);
            publication_restart_drill::run(&remote, &client_root, &origin, &session.access_token, &ca, &restart_probe);
        }
        if std::env::var_os("LAYRS_TEST_TRANSFER_RESTART_ONLY").is_some() {
            return provision.space_id;
        }
        unchanged_drill::run(&remote, &client_root, &origin, &session.access_token, &ca, &unchanged_probe, &pending["pending"]);
        if std::env::var_os("LAYRS_TEST_UNCHANGED_ONLY").is_some() { return provision.space_id; }
        policy_client_drill::run(&remote, &client_root, &origin, &session.access_token, &ca, &lose_response);
        if std::env::var_os("LAYRS_TEST_POLICY_ONLY").is_some() {
            return provision.space_id;
        }
        content_acl_drill::run(&remote, &client_root, &origin, &session.access_token, &ca);
        fetch_pagination_drill::run(&remote, &client_root, &origin, &session.access_token, &ca);
        if std::env::var_os("LAYRS_TEST_FETCH_PAGINATION_ONLY").is_some() {
            return provision.space_id;
        }
        semantic_conflict_drill::run(
            &remote,
            &client_root,
            &origin,
            &session.access_token,
            &ca,
            &lose_response,
        );
        choose_client_drill::run(&remote, &client_root);
        rename_conflict_drill::run(
            &remote,
            &client_root,
            &origin,
            &session.access_token,
            &ca,
            &lose_response,
        );
        rewrite_client_drill::run(
            &remote,
            &client_root,
            &rewrite_probe,
            &origin,
            &session.access_token,
            &ca,
        );
        cancellation_drill::run(&remote, &client_root, &cancellation_probe);
        publication_cancellation_drill::run(
            &remote,
            &client_root,
            &publication_probe,
            &lose_response,
        );
        transfer_drill::run(
            &remote,
            &provision.space_id,
            &first.accepted_step_ids[0],
            &interrupted_download,
            &resumed_download,
        );
        transfer_drill::large_file(
            &remote,
            &client_root,
            &interrupted_download,
            &interrupt_min_bytes,
        );
        concurrent_clients::run(
            &remote,
            &provision.space_id,
            &first.accepted_step_ids[0],
            &client_root,
        );
        concurrent_publications::run(&remote, &client_root, &lose_response);
        product_drill::run(&origin, &session.access_token, &ca, &pending["pending"]);
        corrupt_download.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            remote
                .clone_into(client_root.join("corrupt"), &provision.space_id, "Project")
                .is_err()
        );
        assert!(
            !client_root.join("corrupt/.layrs").exists(),
            "corrupt download must fail before Store creation"
        );
        let b = remote
            .clone_into(&broot, &provision.space_id, "Project")
            .unwrap();
        assert_ne!(a.worktree_id(), b.worktree_id());
        assert!(
            b.state()
                .unwrap()
                .layers
                .iter()
                .any(|layer| layer.id() == feature.id())
        );
        assert_eq!(b.markers().unwrap(), vec![marker]);
        assert_eq!(b.releases().unwrap(), vec![release.clone()]);
        assert_eq!(
            a.state().unwrap().view.layer_heads(),
            b.state().unwrap().view.layer_heads()
        );
        assert_eq!(
            std::fs::read(broot.join("docs/readme.txt")).unwrap(),
            b"initial contents"
        );
        assert!(
            b.operations(100)
                .unwrap()
                .iter()
                .any(|op| op.kind() == OperationKind::ReceiveRemoteState)
        );
        let a_ops: std::collections::BTreeSet<_> = a
            .operations(100)
            .unwrap()
            .into_iter()
            .map(|op| op.id().clone())
            .collect();
        assert!(
            b.operations(100)
                .unwrap()
                .iter()
                .all(|op| !a_ops.contains(op.id()))
        );
        std::fs::write(aroot.join("docs/readme.txt"), b"alice update").unwrap();
        assert_eq!(
            remote
                .publish(&a, LayerMutationMode::Advance)
                .unwrap()
                .status,
            ReceiptStatus::Applied
        );
        assert!(
            remote
                .receive_with_faults(&b, &FailAt(MaterializationPoint::AfterPath))
                .is_err()
        );
        assert!(b.needs_repair().unwrap());
        assert!(b.capture().is_err());
        assert_eq!(b.recover().unwrap(), 1);
        assert_eq!(
            std::fs::read(broot.join("docs/readme.txt")).unwrap(),
            b"initial contents"
        );
        let received = remote.receive(&b).unwrap();
        assert!(received.materialized && !received.divergent);
        assert_eq!(
            std::fs::read(broot.join("docs/readme.txt")).unwrap(),
            b"alice update"
        );
        std::fs::write(aroot.join("docs/readme.txt"), b"alice committed receive").unwrap();
        assert_eq!(
            remote
                .publish(&a, LayerMutationMode::Advance)
                .unwrap()
                .status,
            ReceiptStatus::Applied
        );
        assert!(
            remote
                .receive_with_faults(&b, &FailAt(MaterializationPoint::AfterMetadata))
                .is_err()
        );
        assert!(b.needs_repair().unwrap());
        let committed = b.state().unwrap().operation.id().clone();
        assert_eq!(b.recover().unwrap(), 1);
        assert_eq!(
            std::fs::read(broot.join("docs/readme.txt")).unwrap(),
            b"alice committed receive"
        );
        let resumed = remote.receive(&b).unwrap();
        assert_eq!(
            resumed.operation_id, committed,
            "retry must not duplicate local Operation"
        );
        assert!(!resumed.materialized);
        std::fs::write(aroot.join("alice.txt"), b"alice concurrent").unwrap();
        std::fs::write(broot.join("bob.txt"), b"bob concurrent").unwrap();
        a.capture().unwrap();
        b.capture().unwrap();
        assert_eq!(
            remote
                .publish(&a, LayerMutationMode::Advance)
                .unwrap()
                .status,
            ReceiptStatus::Applied
        );
        let rejected = remote.publish(&b, LayerMutationMode::Advance).unwrap();
        assert_eq!(rejected.status, ReceiptStatus::Rejected);
        let integrated = remote.publish(&b, LayerMutationMode::Integrate).unwrap();
        assert_eq!(
            integrated.status,
            ReceiptStatus::Applied,
            "{:?}",
            integrated.rejections
        );
        let received = remote.receive(&b).unwrap();
        assert!(received.divergent && !received.materialized);
        assert_eq!(
            b.state()
                .unwrap()
                .view
                .layer_heads()
                .iter()
                .find(|h| h.layer_id() == &provision.layer_id)
                .unwrap()
                .step_ids()
                .len(),
            2
        );
        assert_eq!(
            std::fs::read(broot.join("bob.txt")).unwrap(),
            b"bob concurrent"
        );
        let received = remote.receive(&a).unwrap();
        assert!(received.divergent && !received.materialized);
        assert_eq!(
            std::fs::read(aroot.join("alice.txt")).unwrap(),
            b"alice concurrent"
        );
        assert_eq!(
            a.state().unwrap().view.layer_heads(),
            b.state().unwrap().view.layer_heads()
        );
        let repeated = remote.receive(&b).unwrap();
        assert!(repeated.divergent && !repeated.materialized);
        let heads = a
            .state()
            .unwrap()
            .view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == &provision.layer_id)
            .unwrap()
            .step_ids()
            .to_vec();
        let preview = a
            .preview_structural_merge(
                &heads,
                &provision.layer_id,
                layrs_weave::reconciliation::FileMergePolicy {
                    unsupported: layrs_weave::reconciliation::FailureAction::StructuralFallback,
                    quota: layrs_weave::reconciliation::FailureAction::Reject,
                    limits: Default::default(),
                },
                Default::default(),
            )
            .unwrap();
        let merged = a.accept_merge(&preview).unwrap();
        assert!(merged.materialized && merged.conflict_ids.is_empty());
        let published = remote.publish(&a, LayerMutationMode::Advance).unwrap();
        assert_eq!(
            published.status,
            ReceiptStatus::Applied,
            "{:?}",
            published.rejections
        );
        assert!(
            published
                .accepted_products
                .iter()
                .any(|p| p.object_id() == &merged.product_object_id)
        );
        let received = remote.receive(&b).unwrap();
        assert!(received.materialized && !received.divergent);
        for checkout in [&aroot, &broot] {
            assert_eq!(
                std::fs::read(checkout.join("alice.txt")).unwrap(),
                b"alice concurrent"
            );
            assert_eq!(
                std::fs::read(checkout.join("bob.txt")).unwrap(),
                b"bob concurrent"
            );
        }
        std::fs::create_dir(client_root.join("other")).unwrap();
        let other = NativeWorkspace::initialize(client_root.join("other"), "Other").unwrap();
        let other_provision = remote.provision(&other, "native-team", "other").unwrap();
        compare_drill::run(
            &origin,
            &session.access_token,
            &ca,
            &provision.space_id,
            &other_provision.space_id,
            &first.accepted_step_ids[0],
            &merged.step_id,
        );
        conflict_drill::run(&remote, &a, &b, &aroot, &broot, &provision.layer_id);
        conflict_drill::shared_revert(&remote, &a, &b, &aroot, &broot);
        let old_heads = a.state().unwrap().view.layer_heads().to_vec();
        let later_marker = a
            .create_marker("after-merge", "reference-only publication")
            .unwrap();
        let later_release = a
            .create_release("2.0.0", "reference-only publication")
            .unwrap();
        let references = remote.publish(&a, LayerMutationMode::Advance).unwrap();
        assert_eq!(
            references.status,
            ReceiptStatus::Applied,
            "{:?}",
            references.rejections
        );
        assert_eq!(references.accepted_products.len(), 2);
        assert!(references.accepted_step_ids.is_empty());
        assert!(
            references
                .view_result
                .as_ref()
                .unwrap()
                .layer_results
                .is_empty()
        );
        assert_eq!(old_heads, a.state().unwrap().view.layer_heads());
        assert!(!remote.receive(&b).unwrap().materialized);
        assert!(b.markers().unwrap().contains(&later_marker));
        assert!(b.releases().unwrap().contains(&later_release));
        release_drill::run(
            &remote,
            &origin,
            &session.access_token,
            &ca,
            &provision.space_id,
            &other_provision.space_id,
            &later_release,
        );
        let journal = std::fs::read_to_string(a.store_root().join("native-remote.json")).unwrap();
        assert!(!journal.contains(&session.access_token));
        if std::env::var_os("LAYRS_TEST_RUNNER_IMAGE").is_some() {
            runner_drill::run(
                &origin,
                &session.access_token,
                &ca,
                &provision.space_id,
                &provision.layer_id,
                &first.accepted_step_ids[0],
                &client_root,
            );
            let obsolete = remote.publish(&a, LayerMutationMode::Advance).unwrap();
            assert_eq!(obsolete.status, ReceiptStatus::Rejected);
            assert!(
                obsolete
                    .rejections
                    .iter()
                    .any(|r| r.code == layrs_sync::v3::RejectionCode::AuthorizationEpochChanged)
            );
            let journal: serde_json::Value = serde_json::from_slice(
                &std::fs::read(a.store_root().join("native-remote.json")).unwrap(),
            )
            .unwrap();
            assert!(
                journal["pending"].is_null(),
                "terminal epoch rejection must release pending publication"
            );
            remote.receive(&a).unwrap();
            assert_eq!(
                remote
                    .publish(&a, LayerMutationMode::Advance)
                    .unwrap()
                    .status,
                ReceiptStatus::Applied
            );
            product_drill::protected_releases(
                &origin,
                &session.access_token,
                &ca,
                &pending["pending"],
                &first.accepted_step_ids[0],
                &merged.step_id,
            );
            release_drill::protected(
                &remote,
                &provision.space_id,
                &later_release,
                &release,
                &origin,
                &session.access_token,
                &ca,
                &provision.layer_id,
            );
        }
        provision.space_id
    })
    .await;
    let space = match space {
        Ok(space) => space,
        Err(failure) => {
            // Release only this UUID fixture, even when a volume/adversarial
            // scenario panics. Never leave its database writer role behind.
            server.abort();
            pool.close().await;
            maintenance.close().await;
            sqlx::query(&format!("DROP DATABASE {db} WITH (FORCE)"))
                .execute(&admin)
                .await
                .unwrap();
            sqlx::query(&format!("DROP ROLE {runtime}"))
                .execute(&admin)
                .await
                .unwrap();
            admin.close().await;
            assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
            assert_eq!(clients, root.with_extension("clients"));
            std::fs::remove_dir_all(&root).unwrap();
            std::fs::remove_dir_all(&clients).unwrap();
            panic!("native client fixture failed: {failure}");
        }
    };
    let op_count:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.tables WHERE table_schema='public' AND table_name='operations'").fetch_one(&pool).await.unwrap();
    assert_eq!(
        op_count, 0,
        "local Operations must not be synchronization records"
    );
    let head_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM current_layer_heads_projection WHERE space_id=$1")
            .bind(space.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(head_count, 2);
    assert!(
        sqlx::query("UPDATE layrs_collaboration.release_events SET sequence=sequence WHERE false")
            .execute(&pool)
            .await
            .is_err(),
        "runtime writer cannot rewrite release audit events"
    );
    assert!(
        sqlx::query("DELETE FROM layrs_collaboration.release_events WHERE false")
            .execute(&pool)
            .await
            .is_err(),
        "runtime writer cannot erase release audit events"
    );
    if std::env::var_os("LAYRS_TEST_PG_CONTAINER").is_some() {
        backup_drill::run(&maintenance, &admin, &config, &db).await;
    }
    server.abort();
    pool.close().await;
    maintenance.close().await;
    sqlx::query(&format!("DROP DATABASE {db} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {runtime}"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(clients).unwrap();
}
