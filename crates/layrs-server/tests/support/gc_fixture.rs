use bytes::Bytes;
use futures_util::{StreamExt, stream};
use layrs_core::scm::*;
use layrs_server::{
    backup::{self, LocalPgBackupTools},
    config::ServerConfig,
    identity::{BootstrapRequest, PgIdentity, random_secret},
    installation,
    retention::gc::*,
    v3::*,
};
use layrs_store_server::v3::*;
use layrs_sync::v3::*;
use sqlx::{
    PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

pub struct Clock(pub u64);
impl GcClock for Clock {
    fn now(&self) -> GcResult<u64> {
        Ok(self.0)
    }
}
pub struct Fixture {
    pub pool: PgPool,
    pub config: ServerConfig,
    pub base: PathBuf,
    pub now: Clock,
    admin: PgPool,
    options: PgConnectOptions,
    database: String,
    writer: String,
    retained: Vec<(PathBuf, Vec<u8>)>,
    old: Vec<PathBuf>,
}
impl Fixture {
    pub async fn new() -> Self {
        assert_eq!(
            std::env::var("LAYRS_TEST_DATABASE_ISOLATED").as_deref(),
            Ok("1")
        );
        let options =
            PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .unwrap();
        let database = format!("layrs_gc_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {database}"))
            .execute(&admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect_with(options.clone().database(&database))
            .await
            .unwrap();
        let base = std::env::temp_dir().join(&database);
        fs::create_dir(&base).unwrap();
        let root = base.join("cas");
        fs::create_dir(&root).unwrap();
        let mut url = url::Url::parse(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
        url.set_path(&database);
        let secret = random_secret();
        let config = ServerConfig::parse(&BTreeMap::from([
            ("LAYRS_PUBLIC_URL".into(), "https://gc.example".into()),
            ("LAYRS_DATABASE_URL".into(), url.to_string()),
            ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
            ("LAYRS_DEPLOYMENT_ID".into(), "deployment:gc-test".into()),
            ("LAYRS_STORE_ID".into(), "store:gc-test".into()),
            ("LAYRS_SIGNING_KEY".into(), random_secret()),
            ("LAYRS_BOOTSTRAP_SECRET".into(), secret.clone()),
        ]))
        .unwrap();
        installation::initialize(&pool, &config).await.unwrap();
        let writer: String =
            sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
                .fetch_one(&pool)
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
                login: "gc@example.com".into(),
                password: "correct horse battery staple".into(),
                display_name: "GC fixture".into(),
            })
            .await
            .unwrap();
        let fs_cas = FsServerCas::open(&root, CasLimits::default())
            .await
            .unwrap();
        let cas = Arc::new(CatalogedFsCas::new(config.store_id.clone(), fs_cas.clone()));
        let repository = PgV3Repository::new(
            pool.clone(),
            cas.clone(),
            CapabilityCodec::new(&config.signing_key).unwrap(),
            Arc::new(PgPolicyAuthorizer::new(pool.clone(), cas)),
        );
        let facade = ProductionV3Facade::new(
            repository.clone(),
            identity,
            EventCursorCodec::new(&config.signing_key).unwrap(),
            config.public_url.clone(),
        );
        let principal = facade
            .authenticate_bearer(&session.access_token)
            .await
            .unwrap();
        let provision = facade
            .provision_space(
                &principal,
                ProvisionSpaceRequest {
                    workspace_slug: "gc".into(),
                    space_slug: "project".into(),
                    name: "GC roots".into(),
                    initial_layer: None,
                },
            )
            .await
            .unwrap();
        let initial = facade
            .bootstrap(&principal, &provision.space_id)
            .await
            .unwrap();
        let tree = Tree::new(vec![]).unwrap();
        let tree_bytes = serde_jcs::to_vec(&tree).unwrap();
        upload(
            &facade,
            &principal,
            &provision.space_id,
            ObjectDescriptor {
                kind: ObjectKind::Tree,
                object_id: ObjectId::parse(tree.id().as_str()).unwrap(),
                canonical_byte_len: tree_bytes.len() as u64,
                encoded_byte_len: tree_bytes.len() as u64,
                compression: Compression::Identity,
            },
            tree_bytes,
            "tree",
        )
        .await;
        publish_roots(&facade, &principal, &initial, &tree).await;
        let now = SystemGcClock.now().unwrap();
        let mut old = vec![];
        let mut retained = vec![];
        for (index, label) in ["old-a", "old-b", "old-c", "reserved", "leased", "fresh"]
            .iter()
            .enumerate()
        {
            let bytes = label.as_bytes().to_vec();
            let chunk = Chunk::new(bytes.clone()).unwrap();
            let descriptor = ObjectDescriptor {
                kind: ObjectKind::Chunk,
                object_id: ObjectId::parse(chunk.id().as_str()).unwrap(),
                canonical_byte_len: bytes.len() as u64,
                encoded_byte_len: bytes.len() as u64,
                compression: Compression::Identity,
            };
            fs_cas
                .put_if_absent(
                    &descriptor,
                    Box::pin(stream::once({
                        let bytes = bytes.clone();
                        async move { Ok(Bytes::from(bytes)) }
                    })),
                )
                .await
                .unwrap();
            let hex = &descriptor.object_id.as_str()[7..];
            let path = root.join(format!("objects/chunk/{}/{hex}", &hex[..2]));
            if index < 5 {
                let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
                file.set_times(fs::FileTimes::new().set_modified(
                    std::time::UNIX_EPOCH
                        + std::time::Duration::from_secs(now - 3 * MIN_RETENTION_SECONDS),
                ))
                .unwrap();
            }
            if index < 3 {
                old.push(path)
            } else {
                retained.push((path, bytes));
            }
            if *label == "reserved" {
                let principal_context = load_principal_context(&pool, &principal.principal_id)
                    .await
                    .unwrap();
                let client_id = ClientId::parse("client:gc").unwrap();
                let request_id = RequestId::parse("request:reserved").unwrap();
                // Trusted issuance time is injected before creation: the immutable
                // finalized plan is genuinely expired, never edited afterwards.
                let context = PlanContext {
                    actor: CapabilityActor {
                        plan_id: deterministic_plan_id(
                            &provision.space_id,
                            &client_id,
                            &request_id,
                        )
                        .unwrap(),
                        space_id: provision.space_id.clone(),
                        client_id,
                        request_id,
                        principal_id: principal.principal_id.clone(),
                        authorization_epoch: initial.authorization_epoch,
                    },
                    now_unix_seconds: now - 2 * MIN_RETENTION_SECONDS,
                    expires_at_unix_seconds: now - MIN_RETENTION_SECONDS,
                    transfer_base_url: config.public_url.clone(),
                };
                PgPlanStore::new(repository.clone(), PlanLimits::default())
                    .create_object_plan(&principal_context, &context, vec![descriptor.clone()])
                    .await
                    .unwrap();
            }
            if *label == "leased" {
                sqlx::query("INSERT INTO object_leases(lease_id,space_id,object_id,lease_purpose,owner_client_id,fencing_generation,expires_at,heartbeat_at,released_at) VALUES('lease:gc-expired',$1,$2,'upload','client:gc',1,clock_timestamp()-interval '1 day',clock_timestamp()-interval '2 days',clock_timestamp()-interval '1 day')").bind(provision.space_id.as_str()).bind(descriptor.object_id.as_str()).execute(&pool).await.unwrap();
            }
        }
        fs::write(
            root.join("staging/abandoned.upload"),
            b"retain unknown staging",
        )
        .unwrap();
        retained.push((
            root.join("staging/abandoned.upload"),
            b"retain unknown staging".to_vec(),
        ));
        drop(facade);
        drop(repository);
        drop(fs_cas);
        Self {
            pool,
            config,
            base,
            now: Clock(now),
            admin,
            options,
            database,
            writer,
            retained,
            old,
        }
    }
    pub fn old_orphans(&self) -> Vec<PathBuf> {
        self.old.clone()
    }
    pub async fn authorities(&self) -> BTreeMap<String, String> {
        authorities(&self.pool).await
    }
    pub async fn assert_retained(&self) {
        for (path, bytes) in &self.retained {
            assert_eq!(&fs::read(path).unwrap(), bytes)
        }
        let backend = FsServerCas::open(&self.config.cas_root, CasLimits::default())
            .await
            .unwrap();
        let rows=sqlx::query("SELECT object_id,object_kind,canonical_byte_length,encoded_byte_length,compression FROM canonical_objects").fetch_all(&self.pool).await.unwrap();
        for row in rows {
            let object = ObjectDescriptor {
                object_id: ObjectId::parse(row.get::<String, _>("object_id")).unwrap(),
                kind: serde_json::from_value(serde_json::json!(
                    row.get::<String, _>("object_kind")
                ))
                .unwrap(),
                canonical_byte_len: row.get::<i64, _>("canonical_byte_length") as u64,
                encoded_byte_len: row.get::<i64, _>("encoded_byte_length") as u64,
                compression: serde_json::from_value(serde_json::json!(
                    row.get::<String, _>("compression")
                ))
                .unwrap(),
            };
            let mut stream = backend.verified_stream(&object).await.unwrap();
            let mut bytes = 0_u64;
            while let Some(chunk) = stream.next().await {
                bytes += chunk.unwrap().len() as u64;
            }
            assert_eq!(bytes, object.encoded_byte_len);
        }
    }
    pub fn cli(&self, args: &[&str]) -> std::process::Output {
        std::process::Command::new(env!("CARGO_BIN_EXE_layrs-server"))
            .args(args)
            .env("LAYRS_DATABASE_URL", &self.config.database_url)
            .env("LAYRS_PUBLIC_URL", self.config.public_url.as_str())
            .env("LAYRS_CAS_ROOT", &self.config.cas_root)
            .env("LAYRS_DEPLOYMENT_ID", &self.config.deployment_id)
            .env("LAYRS_STORE_ID", self.config.store_id.as_str())
            .env("LAYRS_SIGNING_KEY", random_secret())
            .output()
            .unwrap()
    }
    pub async fn restore_and_compare(&self, backup: &Path, expected: &BTreeMap<String, String>) {
        let db = format!("{}_restore", self.database);
        sqlx::query(&format!("CREATE DATABASE {db}"))
            .execute(&self.admin)
            .await
            .unwrap();
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .connect_with(self.options.clone().database(&db))
            .await
            .unwrap();
        let mut config = ServerConfig {
            addr: self.config.addr,
            public_url: self.config.public_url.clone(),
            database_url: self.config.database_url.clone(),
            cas_root: self.config.cas_root.clone(),
            deployment_id: self.config.deployment_id.clone(),
            store_id: self.config.store_id.clone(),
            signing_key: self.config.signing_key.clone(),
            bootstrap_secret: None,
            cas_quotas: self.config.cas_quotas,
        };
        config.cas_root = self.base.join("restored");
        let mut url = url::Url::parse(&config.database_url).unwrap();
        url.set_path(&db);
        config.database_url = url.to_string();
        backup::restore_backup(&pool, &config, backup, Arc::new(LocalPgBackupTools))
            .await
            .unwrap();
        assert_eq!(&authorities(&pool).await, expected);
        pool.close().await;
        sqlx::query(&format!("DROP DATABASE {db}"))
            .execute(&self.admin)
            .await
            .unwrap();
    }
    pub async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP DATABASE {}", self.database))
            .execute(&self.admin)
            .await
            .unwrap();
        sqlx::query(&format!("DROP ROLE {}", self.writer))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
        assert_eq!(self.base.parent(), Some(std::env::temp_dir().as_path()));
        fs::remove_dir_all(self.base).unwrap();
    }
}
async fn authorities(pool: &PgPool) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for table in [
        "steps",
        "repository_views",
        "products",
        "spaces",
        "object_plan_items",
        "object_leases",
    ] {
        let query = format!(
            "SELECT md5(COALESCE(string_agg(row_to_json(t)::text,E'\\n' ORDER BY row_to_json(t)::text),'')) FROM {table} t"
        );
        result.insert(
            table.into(),
            sqlx::query_scalar(&query).fetch_one(pool).await.unwrap(),
        );
    }
    result
}
async fn upload(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
    descriptor: ObjectDescriptor,
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
                client_id: ClientId::parse("client:gc").unwrap(),
                request_id: RequestId::parse(format!("request:{label}")).unwrap(),
                objects: vec![descriptor],
            },
        )
        .await
        .unwrap();
    facade
        .upload(
            &plan.upload_capabilities[0].token,
            bytes.len() as u64,
            Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) })),
        )
        .await
        .unwrap();
}
async fn publish_roots(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    initial: &SpaceBootstrap,
    tree: &Tree,
) {
    let step = Step::new(vec![], tree.id().clone()).unwrap();
    let timestamp = Timestamp::parse("2026-09-20T00:00:00Z").unwrap();
    let marker = Marker::new(
        MarkerId::parse("marker:gc").unwrap(),
        initial.space_id.clone(),
        step.id().clone(),
        MarkerKind::Checkpoint,
        "kept",
        "",
        "",
        None,
        timestamp.clone(),
        vec![],
        Metadata::empty(),
    )
    .unwrap();
    let release = Release::new(
        ReleaseId::parse("release:gc").unwrap(),
        initial.space_id.clone(),
        step.id().clone(),
        "1.0.0",
        "kept",
        "",
        None,
        timestamp,
        vec![],
        Metadata::empty(),
    )
    .unwrap();
    let placeholder = ObjectId::parse(format!("blake3:{}", "0".repeat(64))).unwrap();
    let mut products = vec![
        ProductEnvelope::Marker {
            product_id: marker.id().clone(),
            product_object_id: placeholder.clone(),
            depends_on_step_ids: vec![step.id().clone()],
            depends_on_product_ids: vec![],
            record: marker.clone(),
        },
        ProductEnvelope::Release {
            product_id: release.id().clone(),
            product_object_id: placeholder.clone(),
            depends_on_step_ids: vec![step.id().clone()],
            depends_on_product_ids: vec![],
            record: release.clone(),
        },
    ];
    for product in &mut products {
        let id = product.computed_object_id().unwrap();
        match product {
            ProductEnvelope::Marker {
                product_object_id, ..
            }
            | ProductEnvelope::Release {
                product_object_id, ..
            } => *product_object_id = id,
            _ => unreachable!(),
        }
    }
    let layer = initial.layers[0].record.id().clone();
    let view = RepositoryView::new(
        initial.space_id.clone(),
        vec![LayerHeadSet::new(layer.clone(), vec![step.id().clone()]).unwrap()],
        vec![marker.id().clone()],
        vec![release.id().clone()],
        vec![],
    )
    .unwrap();
    let mut request = SyncTransactionRequest {
        protocol: SyncV3Protocol::V3,
        message_type: TransactionRequestType::Request,
        space_id: initial.space_id.clone(),
        client_id: ClientId::parse("client:gc").unwrap(),
        idempotency_key: IdempotencyKey::parse("idempotency-gc-roots-00000000").unwrap(),
        request_digest: placeholder,
        known_event_cursor: None,
        steps: vec![step],
        observations: vec![],
        products,
        view_mutation: Some(RepositoryViewMutation {
            mutation_id: MutationId::parse("mutation:gc").unwrap(),
            expected_current_view_id: initial.current_repository_view.repository_view_id.clone(),
            expected_generation: initial.current_repository_view_generation,
            proposed_view: ProposedRepositoryView::Inline { view },
            layer_mutations: vec![LayerMutationIntent {
                layer_id: layer,
                mode: LayerMutationMode::Advance,
                decision_product_object_id: None,
            }],
            expected_policy_version_object_ids: captured_policy_version_object_ids(
                &initial.current_policy_set.record,
            )
            .unwrap(),
        }),
        policy_root_mutation: None,
    };
    request.request_digest = request.computed_request_digest().unwrap();
    let receipt = facade.transact(principal, request).await.unwrap();
    assert_eq!(receipt.receipt().status, ReceiptStatus::Applied);
    let bootstrap = facade
        .bootstrap(principal, &initial.space_id)
        .await
        .unwrap();
    assert_eq!(bootstrap.markers.len(), 1);
    assert_eq!(bootstrap.releases.len(), 1);
}
