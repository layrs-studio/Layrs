//! Real PostgreSQL + filesystem CAS + password identity + policy + V3 facade.
#[path = "support/backup_drill.rs"]
mod backup_drill;
#[path = "support/client_observations.rs"]
mod client_observations;
#[path = "support/collaboration_drill.rs"]
mod collaboration_drill;
#[path = "support/history_pages.rs"]
mod history_pages;
#[path = "support/published_observations.rs"]
mod published_observations;
#[path = "support/webhook_drill.rs"]
mod webhook_drill;
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use layrs_core::scm::*;
use layrs_server::{
    config::ServerConfig,
    identity::{BootstrapRequest, PgIdentity, random_secret},
    installation,
    v3::*,
};
use layrs_store_server::v3::{
    CapabilityCodec, CasLimits, CatalogedFsCas, EventCursorCodec, FsServerCas, PgPolicyAuthorizer,
    PgV3Repository, ProvisionSpaceRequest,
};
use layrs_sync::v3::*;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{collections::BTreeMap, str::FromStr, sync::Arc};

fn object_id(value: &str) -> ObjectId {
    ObjectId::parse(value).unwrap()
}
fn descriptor(kind: ObjectKind, id: &str, bytes: &[u8]) -> ObjectDescriptor {
    ObjectDescriptor {
        kind,
        object_id: object_id(id),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    }
}
async fn upload(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    kind: ObjectKind,
    id: &str,
    bytes: Vec<u8>,
    label: &str,
) {
    let plan = facade
        .object_plan(
            principal,
            ObjectPlanRequest {
                protocol: SyncV3Protocol::V3,
                message_type: ObjectPlanRequestType::Request,
                space_id: space.clone(),
                client_id: ClientId::parse("client:desktop-a").unwrap(),
                request_id: RequestId::parse(format!("request:{label}")).unwrap(),
                objects: vec![descriptor(kind, id, &bytes)],
            },
        )
        .await
        .unwrap();
    assert_eq!(plan.missing_objects.len(), 1);
    let capability = &plan.upload_capabilities[0];
    let length = bytes.len() as u64;
    facade
        .upload(
            &capability.token,
            length,
            Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) })),
        )
        .await
        .unwrap();
}
fn request(
    bootstrap: &SpaceBootstrap,
    step: &Step,
    key: &str,
    mode: LayerMutationMode,
) -> SyncTransactionRequest {
    let layer = bootstrap.layers[0].record.id().clone();
    let view = RepositoryView::new(
        bootstrap.space_id.clone(),
        vec![LayerHeadSet::new(layer.clone(), vec![step.id().clone()]).unwrap()],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let mut request = SyncTransactionRequest {
        protocol: SyncV3Protocol::V3,
        message_type: TransactionRequestType::Request,
        space_id: bootstrap.space_id.clone(),
        client_id: ClientId::parse("client:desktop-a").unwrap(),
        idempotency_key: IdempotencyKey::parse(format!("idempotency-{key}-00000000")).unwrap(),
        request_digest: object_id(&format!("blake3:{}", "0".repeat(64))),
        known_event_cursor: Some(bootstrap.watermark_cursor.clone()),
        steps: vec![step.clone()],
        observations: vec![],
        products: vec![],
        view_mutation: Some(RepositoryViewMutation {
            mutation_id: MutationId::parse(format!("mutation:{key}")).unwrap(),
            expected_current_view_id: bootstrap.current_repository_view.repository_view_id.clone(),
            expected_generation: bootstrap.current_repository_view_generation,
            proposed_view: ProposedRepositoryView::Inline { view },
            layer_mutations: vec![LayerMutationIntent {
                layer_id: layer,
                mode,
                decision_product_object_id: None,
            }],
            expected_policy_version_object_ids: captured_policy_version_object_ids(
                &bootstrap.current_policy_set.record,
            )
            .unwrap(),
        }),
        policy_root_mutation: None,
    };
    request.request_digest = request.computed_request_digest().unwrap();
    request
}

#[tokio::test]
#[ignore = "requires LAYRS_TEST_DATABASE_URL disposable PostgreSQL administrator endpoint"]
async fn real_service_provision_upload_advance_replay_concurrency_fetch_and_revoke() {
    let options = PgConnectOptions::from_str(
        &std::env::var("LAYRS_TEST_DATABASE_URL").expect("set disposable admin URL"),
    )
    .unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let db = format!("layrs_sync_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {db}"))
        .execute(&admin)
        .await
        .unwrap();
    let maintenance = PgPoolOptions::new()
        .max_connections(12)
        .connect_with(options.clone().database(&db))
        .await
        .unwrap();
    let secret = random_secret();
    let root = std::env::temp_dir().join(&db);
    let config = ServerConfig::parse(&BTreeMap::from([
        ("LAYRS_PUBLIC_URL".into(), "https://layrs.example".into()),
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
    let principal = facade
        .authenticate_bearer(&session.access_token)
        .await
        .unwrap();
    let create = || ProvisionSpaceRequest {
        workspace_slug: "team".into(),
        space_slug: "project".into(),
        name: "Project".into(),
        initial_layer: None,
    };
    let provisioned = facade.provision_space(&principal, create()).await.unwrap();
    assert_eq!(
        facade
            .provision_space(&principal, create())
            .await
            .unwrap()
            .space_id,
        provisioned.space_id
    );
    let initial = facade
        .bootstrap(&principal, &provisioned.space_id)
        .await
        .unwrap();
    initial.validate_v3().unwrap();
    let chunk = Chunk::new(b"first project file".to_vec()).unwrap();
    let manifest =
        FileManifest::from_chunks(ChunkingDescriptor::single(), std::slice::from_ref(&chunk))
            .unwrap();
    let subtree = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("readme.txt").unwrap(),
        TreeEntryTarget::file(manifest.file_id().clone()),
        false,
    )])
    .unwrap();
    let tree = Tree::new(vec![TreeEntry::new(
        RepoPathComponent::parse("docs").unwrap(),
        TreeEntryTarget::tree(subtree.id().clone()),
        false,
    )])
    .unwrap();
    upload(
        &facade,
        &principal,
        &initial.space_id,
        ObjectKind::Chunk,
        chunk.id().as_str(),
        chunk.bytes().to_vec(),
        "chunk",
    )
    .await;
    upload(
        &facade,
        &principal,
        &initial.space_id,
        ObjectKind::FileManifest,
        manifest.id().as_str(),
        serde_jcs::to_vec(&manifest).unwrap(),
        "manifest",
    )
    .await;
    upload(
        &facade,
        &principal,
        &initial.space_id,
        ObjectKind::Tree,
        subtree.id().as_str(),
        serde_jcs::to_vec(&subtree).unwrap(),
        "subtree",
    )
    .await;
    upload(
        &facade,
        &principal,
        &initial.space_id,
        ObjectKind::Tree,
        tree.id().as_str(),
        serde_jcs::to_vec(&tree).unwrap(),
        "tree",
    )
    .await;
    let first = Step::new(vec![], tree.id().clone()).unwrap();
    let first_request = request(&initial, &first, "first", LayerMutationMode::Advance);
    let first_receipt = facade
        .transact(&principal, first_request.clone())
        .await
        .unwrap();
    assert_eq!(first_receipt.receipt().status, ReceiptStatus::Applied);
    let bytes = first_receipt.into_canonical_jcs();
    assert_eq!(
        facade
            .transact(&principal, first_request.clone())
            .await
            .unwrap()
            .into_canonical_jcs(),
        bytes
    );
    let mut different = first_request.clone();
    different.known_event_cursor = None;
    different.request_digest = different.computed_request_digest().unwrap();
    assert!(matches!(
        facade.transact(&principal, different).await,
        Err(V3ServiceError::IdempotencyDigestMismatch)
    ));
    let base = facade
        .bootstrap(&principal, &initial.space_id)
        .await
        .unwrap();
    let mut branches = vec![];
    for label in ["left", "right"] {
        let symlink = Symlink::new(format!("{label}.txt")).unwrap();
        let tree = Tree::new(vec![TreeEntry::new(
            RepoPathComponent::parse("link").unwrap(),
            TreeEntryTarget::symlink(symlink.id().clone()),
            false,
        )])
        .unwrap();
        upload(
            &facade,
            &principal,
            &initial.space_id,
            ObjectKind::Symlink,
            symlink.id().as_str(),
            serde_jcs::to_vec(&symlink).unwrap(),
            &format!("{label}-symlink"),
        )
        .await;
        upload(
            &facade,
            &principal,
            &initial.space_id,
            ObjectKind::Tree,
            tree.id().as_str(),
            serde_jcs::to_vec(&tree).unwrap(),
            label,
        )
        .await;
        branches.push(Step::new(vec![first.id().clone()], tree.id().clone()).unwrap());
    }
    let left = request(&base, &branches[0], "left", LayerMutationMode::Advance);
    let right = request(&base, &branches[1], "right", LayerMutationMode::Advance);
    let (a, b) = tokio::join!(
        facade.transact(&principal, left.clone()),
        facade.transact(&principal, right.clone())
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.receipt().status, b.receipt().status);
    let loser = if a.receipt().status == ReceiptStatus::Rejected {
        &branches[0]
    } else {
        &branches[1]
    };
    let winner = if a.receipt().status == ReceiptStatus::Applied {
        &branches[0]
    } else {
        &branches[1]
    };
    let history_page =
        history_pages::page(&facade, &principal, &initial.space_id, winner.id(), None, 1).await;
    assert_eq!(history_page.steps[0].id(), winner.id());
    let history_cursor = history_page.next.unwrap();
    let merged = facade
        .transact(
            &principal,
            request(&base, loser, "integrate", LayerMutationMode::Integrate),
        )
        .await
        .unwrap();
    assert_eq!(merged.receipt().status, ReceiptStatus::Applied);
    let final_state = facade
        .bootstrap(&principal, &initial.space_id)
        .await
        .unwrap();
    assert_eq!(final_state.layers[0].head_step_ids.len(), 2);
    assert_eq!(final_state.current_repository_view_generation, 3);
    history_pages::verify(
        &facade,
        &principal,
        &initial.space_id,
        winner.id(),
        first.id(),
        history_cursor,
    )
    .await;
    let fetch = facade
        .fetch_plan(
            &principal,
            FetchPlanRequest {
                protocol: SyncV3Protocol::V3,
                message_type: FetchPlanRequestType::Request,
                space_id: initial.space_id.clone(),
                client_id: ClientId::parse("client:desktop-b").unwrap(),
                request_id: RequestId::parse("request:file-fetch").unwrap(),
                claims: vec![FetchClaim::File {
                    file_id: manifest.file_id().clone(),
                    preferred_manifest_ids: vec![],
                    root: AuthorizationRoot::Step {
                        step_id: first.id().clone(),
                    },
                    repo_path: RepoPath::parse("docs/readme.txt").unwrap(),
                }],
                page_cursor: Nullable::null(),
                max_objects: 10,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        fetch.download_capabilities.len(),
        2,
        "{:?}",
        fetch.denied_claims
    );
    assert!(fetch.denied_claims.is_empty());
    for cap in &fetch.download_capabilities {
        let mut response = facade.download(&cap.token, None).await.unwrap();
        let mut received = vec![];
        while let Some(bytes) = response.body.next().await {
            received.extend_from_slice(&bytes.unwrap());
        }
        assert_eq!(received.len() as u64, cap.descriptor.encoded_byte_len);
    }
    let events = facade
        .events(
            &principal,
            SpaceEventPageRequest {
                client_id: ClientId::parse("client:desktop-b").unwrap(),
                protocol: SyncV3Protocol::V3,
                message_type: EventPageRequestType::Request,
                space_id: initial.space_id.clone(),
                cursor: initial.watermark_cursor,
                max_events: 100,
            },
        )
        .await
        .unwrap();
    assert_eq!(events.events.len(), 6);
    published_observations::verify(
        &facade,
        &principal,
        &initial.space_id,
        winner.id(),
        first.id(),
        &maintenance,
    )
    .await;
    client_observations::verify(&facade, &principal, &initial.space_id, winner, &maintenance).await;
    collaboration_drill::run(
        repository.clone(),
        identity.clone(),
        &session.access_token,
        &facade,
        &principal,
        &first,
        &root,
        &maintenance,
    )
    .await;
    webhook_drill::run(
        repository,
        identity.clone(),
        &session.access_token,
        &provisioned.space_id,
        &provisioned.layer_id,
        &first,
    )
    .await;
    if std::env::var_os("LAYRS_TEST_PG_CONTAINER").is_some() {
        backup_drill::run(&maintenance, &admin, &config, &db).await;
    }
    client_observations::epoch_race(&facade, &principal, &initial.space_id, winner, &maintenance)
        .await;
    sqlx::query("UPDATE workspace_memberships SET state='revoked' WHERE workspace_id=$1")
        .bind(&provisioned.workspace_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(facade.transact(&principal, first_request).await.is_err());
    assert!(
        facade
            .step_history(
                &principal,
                &provisioned.space_id,
                layrs_store_server::v3::StepHistoryRequest {
                    filter: Default::default(),
                    path: None,
                    anchor: first.id().clone(),
                    cursor: None,
                    limit: 30
                }
            )
            .await
            .is_err()
    );
    assert!(
        facade
            .download(&fetch.download_capabilities[0].token, None)
            .await
            .is_err()
    );
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
}
