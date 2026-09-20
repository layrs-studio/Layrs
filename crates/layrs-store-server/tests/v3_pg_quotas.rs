//! Explicit PostgreSQL + real filesystem CAS recipe. Creates its own database.
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{ActorId, ObjectId, SpaceId, StoreId};
use layrs_store_server::v3::*;
use layrs_sync::v3::{ClientId, Compression, ObjectDescriptor, ObjectKind, PrincipalId, RequestId};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use std::sync::Arc;

struct Allow;
#[async_trait]
impl PolicyAuthorizer for Allow {
    async fn authorize(&self, _: &AuthorizationCheck) -> V3StoreResult<AuthorizationDecision> {
        Ok(AuthorizationDecision::allow("quota_fixture"))
    }
}

#[tokio::test]
#[ignore = "requires LAYRS_TEST_DATABASE_URL with CREATE DATABASE; isolated PG + real CAS"]
async fn concurrent_quota_reservations_retry_corruption_and_generated_objects() {
    let admin_url = std::env::var("LAYRS_TEST_DATABASE_URL").expect("explicit PostgreSQL recipe");
    let admin = PgPool::connect(&admin_url).await.unwrap();
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let database = format!("layrs_quota_{suffix}");
    admin
        .execute(format!("CREATE DATABASE {database}").as_str())
        .await
        .unwrap();
    let mut url = url::Url::parse(&admin_url).unwrap();
    url.set_path(&database);
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(url.as_str())
        .await
        .unwrap();
    sqlx::raw_sql(schema::BASELINE_SQL)
        .execute(&pool)
        .await
        .unwrap();
    let principal = seed(&pool).await;
    let root = std::env::temp_dir().join(format!("layrs-quota-{suffix}"));
    let cas = Arc::new(CatalogedFsCas::new(
        StoreId::parse("store:quota").unwrap(),
        FsServerCas::open(&root, CasLimits::default())
            .await
            .unwrap(),
    ));
    let repository = PgV3Repository::new(
        pool.clone(),
        cas.clone(),
        CapabilityCodec::new([19; 32]).unwrap(),
        Arc::new(Allow),
    )
    .with_quotas(CasQuotas {
        max_total_bytes: Some(28),
        max_total_objects: Some(5),
        max_space_bytes: Some(20),
        max_space_objects: Some(1),
    })
    .unwrap();
    let plans = PgPlanStore::new(repository.clone(), PlanLimits::default());
    // A second repository has its own process-local gate. Only PostgreSQL's
    // advisory lock coordinates these independent writers across Spaces.
    let second_repository = PgV3Repository::new(
        pool.clone(),
        cas.clone(),
        CapabilityCodec::new([19; 32]).unwrap(),
        Arc::new(Allow),
    )
    .with_quotas(repository.quotas())
    .unwrap();
    let other_plans = PgPlanStore::new(second_repository, PlanLimits::default());
    let bytes_a = Bytes::from_static(b"aaaaaaaaaaaaaaaaaaaa");
    let bytes_b = Bytes::from_static(b"bbbbbbbbbbbbbbbbbbbb");
    let a = chunk(&bytes_a);
    let b = chunk(&bytes_b);
    let context_a = context(&principal, "a");
    let mut context_b = context(&principal, "b");
    context_b.actor.space_id = SpaceId::parse("space:quota-b").unwrap();
    context_b.actor.plan_id = deterministic_plan_id(
        &context_b.actor.space_id,
        &context_b.actor.client_id,
        &context_b.actor.request_id,
    )
    .unwrap();
    let (left, right) = tokio::join!(
        plans.create_object_plan(&principal, &context_a, vec![a.clone()]),
        other_plans.create_object_plan(&principal, &context_b, vec![b.clone()]),
    );
    assert_ne!(
        left.is_ok(),
        right.is_ok(),
        "only one 20-byte reservation fits"
    );
    let (accepted, denied, context, descriptor, bytes) = if left.is_ok() {
        (left.unwrap(), right.unwrap_err(), context_a, a, bytes_a)
    } else {
        (right.unwrap(), left.unwrap_err(), context_b, b, bytes_b)
    };
    assert_eq!(denied.code(), "limit_exceeded");
    let space = context.actor.space_id.clone();
    let next_context = |key: &str| {
        let mut next = self::context(&principal, key);
        next.actor.space_id = space.clone();
        next.actor.plan_id =
            deterministic_plan_id(&space, &next.actor.client_id, &next.actor.request_id).unwrap();
        next
    };
    let usage = read_cas_budget(&pool, Some(&space)).await.unwrap();
    assert_eq!(
        (
            usage.objects,
            usage.charged_bytes,
            usage.reserved_only_bytes
        ),
        (1, 20, 20)
    );
    assert_eq!(
        read_cas_budget(&pool, None).await.unwrap().charged_bytes,
        28
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM object_plans")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );

    // The historical lease has expired in wall time, but still reserves bytes.
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT expires_at < clock_timestamp() FROM object_plans")
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    let (retry_a, retry_b) = tokio::join!(
        plans.create_object_plan(&principal, &context, vec![descriptor.clone()]),
        plans.create_object_plan(&principal, &context, vec![descriptor.clone()]),
    );
    assert_eq!(retry_a.unwrap(), accepted);
    assert_eq!(retry_b.unwrap(), accepted);
    // A new request for the same identity is also deduplicated.
    plans
        .create_object_plan(&principal, &next_context("dedup"), vec![descriptor.clone()])
        .await
        .unwrap();
    assert_eq!(read_cas_budget(&pool, Some(&space)).await.unwrap(), usage);
    let contexts: Vec<_> = (0..20)
        .map(|i| next_context(&format!("burst-{i}")))
        .collect();
    let burst =
        futures_util::future::join_all(contexts.iter().map(|context| {
            plans.create_object_plan(&principal, context, vec![descriptor.clone()])
        }))
        .await;
    assert!(
        burst.iter().all(Result::is_ok),
        "queued writers do not consume the database connection pool"
    );
    assert_eq!(read_cas_budget(&pool, Some(&space)).await.unwrap(), usage);

    // Different metadata cannot reuse an already reserved ObjectId.
    let mut changed = descriptor.clone();
    changed.canonical_byte_len += 1;
    changed.encoded_byte_len += 1;
    assert!(
        plans
            .create_object_plan(&principal, &next_context("mismatch"), vec![changed])
            .await
            .is_err()
    );
    // Byte and count limits work independently, and count empty objects.
    for quotas in [
        CasQuotas {
            max_space_bytes: Some(20),
            ..Default::default()
        },
        CasQuotas {
            max_space_objects: Some(1),
            ..Default::default()
        },
        CasQuotas {
            max_total_objects: Some(5),
            ..Default::default()
        },
    ] {
        let other = PgPlanStore::new(
            repository.clone().with_quotas(quotas).unwrap(),
            PlanLimits::default(),
        );
        let next = if quotas.max_space_bytes.is_some() {
            chunk(b"x")
        } else {
            chunk(b"")
        };
        assert_eq!(
            other
                .create_object_plan(&principal, &next_context("limit"), vec![next])
                .await
                .unwrap_err()
                .code(),
            "limit_exceeded"
        );
    }

    cas.put_if_absent(
        &descriptor,
        Box::pin(stream::once({
            let bytes = bytes.clone();
            async move { Ok(bytes) }
        })),
    )
    .await
    .unwrap();
    let catalog = PgCatalogStore::new(repository.clone());
    let upload = CompleteUploadRequest {
        principal: principal.clone(),
        actor: context.actor.clone(),
        descriptor: descriptor.clone(),
        capability_token: accepted.upload_capabilities[0].token.clone(),
        now_unix_seconds: 100,
    };
    let (left, right) = tokio::join!(
        catalog.complete_upload(upload.clone()),
        catalog.complete_upload(upload.clone())
    );
    let outcomes = [left.unwrap(), right.unwrap()];
    assert!(outcomes.contains(&CompleteUploadOutcome::Admitted));
    assert!(outcomes.contains(&CompleteUploadOutcome::AlreadyAdmitted));
    let admitted = read_cas_budget(&pool, Some(&space)).await.unwrap();
    assert_eq!(
        (
            admitted.charged_bytes,
            admitted.catalogued_bytes,
            admitted.reserved_only_bytes
        ),
        (20, 20, 0)
    );

    // Lowered quotas allow exact retries without admitting another object.
    let lowered = repository
        .clone()
        .with_quotas(CasQuotas {
            max_total_bytes: Some(1),
            max_space_bytes: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        PgCatalogStore::new(lowered)
            .complete_upload(upload.clone())
            .await
            .unwrap(),
        CompleteUploadOutcome::AlreadyAdmitted
    );
    // Server-generated catalogue admissions obey the same serialized budget.
    let generated = cas
        .prepare_generated(
            ObjectKind::RepositoryView,
            Bytes::from(
                serde_jcs::to_vec(
                    &layrs_core::scm::RepositoryView::new(
                        space.clone(),
                        vec![],
                        vec![],
                        vec![],
                        vec![],
                    )
                    .unwrap(),
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        catalog
            .install_prepared(&mut tx, &generated)
            .await
            .unwrap_err()
            .code(),
        "limit_exceeded"
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        read_cas_budget(&pool, Some(&space)).await.unwrap(),
        admitted
    );
    let space_limited = PgCatalogStore::new(
        repository
            .clone()
            .with_quotas(CasQuotas {
                max_space_bytes: Some(20),
                ..Default::default()
            })
            .unwrap(),
    );
    let mut tx = pool.begin().await.unwrap();
    space_limited
        .install_prepared(&mut tx, &generated)
        .await
        .unwrap();
    assert_eq!(
        space_limited
            .make_available(&mut tx, &space, &generated.descriptor.object_id)
            .await
            .unwrap_err()
            .code(),
        "limit_exceeded"
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        read_cas_budget(&pool, None).await.unwrap().charged_bytes,
        28,
        "whole catalogue transaction rolled back"
    );

    // A corrupted existing CAS object never turns a retry into silent success.
    let prepared = cas.verify_existing(&descriptor).await.unwrap();
    std::fs::write(root.join(prepared.storage_key), b"XXXXXXXXXXXXXXXXXXXX").unwrap();
    assert!(catalog.complete_upload(upload).await.is_err());
    assert_eq!(
        read_cas_budget(&pool, Some(&space)).await.unwrap(),
        admitted
    );
    pool.close().await;
    admin
        .execute(format!("DROP DATABASE {database} WITH (FORCE)").as_str())
        .await
        .unwrap();
    admin.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

fn chunk(bytes: &[u8]) -> ObjectDescriptor {
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-chunk-v1\0");
    hash.update(bytes);
    ObjectDescriptor {
        kind: ObjectKind::Chunk,
        object_id: ObjectId::parse(format!("blake3:{}", hash.finalize())).unwrap(),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    }
}

fn context(principal: &PrincipalContext, key: &str) -> PlanContext {
    let space_id = SpaceId::parse("space:quota").unwrap();
    let client_id = ClientId::parse("client:quota").unwrap();
    let request_id = RequestId::parse(format!("request:quota-{key}")).unwrap();
    PlanContext {
        actor: CapabilityActor {
            plan_id: deterministic_plan_id(&space_id, &client_id, &request_id).unwrap(),
            space_id,
            client_id,
            request_id,
            principal_id: principal.principal_id.clone(),
            authorization_epoch: 1,
        },
        now_unix_seconds: 100,
        expires_at_unix_seconds: 700,
        transfer_base_url: TransferUrlPolicy::new(["https://quota.example"], [])
            .unwrap()
            .validate_base("https://quota.example")
            .unwrap(),
    }
}

async fn seed(pool: &PgPool) -> PrincipalContext {
    let mut tx = pool.begin().await.unwrap();
    tx.execute("SET CONSTRAINTS ALL DEFERRED").await.unwrap();
    tx.execute("INSERT INTO stores(store_id,backend_kind,backend_config_jcs) VALUES('store:quota','filesystem','{}')").await.unwrap();
    tx.execute("INSERT INTO deployments(deployment_id,store_id,display_name) VALUES('deployment:quota','store:quota','test')").await.unwrap();
    tx.execute("INSERT INTO principals(principal_id,actor_id,deployment_id,principal_kind,subject_digest,display_name) VALUES('principal:quota','actor:quota','deployment:quota','account','subject:quota','test')").await.unwrap();
    tx.execute("INSERT INTO workspaces(workspace_id,deployment_id,slug,display_name,created_by_principal_id) VALUES('workspace:quota','deployment:quota','quota','test','principal:quota')").await.unwrap();
    tx.execute("INSERT INTO workspace_memberships(workspace_id,principal_id,role) VALUES('workspace:quota','principal:quota','owner')").await.unwrap();
    for (space, slug, v, p) in [
        ("space:quota", "quota", 'a', 'b'),
        ("space:quota-b", "quota-b", 'c', 'd'),
    ] {
        let view = format!("blake3:{}", v.to_string().repeat(64));
        let policy = format!("blake3:{}", p.to_string().repeat(64));
        sqlx::query("INSERT INTO spaces(space_id,workspace_id,slug,display_name,lifecycle_state,current_repository_view_id,current_policy_set_id,created_by_principal_id) VALUES($3,'workspace:quota',$4,'test','ready',$1,$2,'principal:quota')").bind(&view).bind(&policy).bind(space).bind(slug).execute(&mut *tx).await.unwrap();
        for (id, kind) in [(&view, "repository_view"), (&policy, "policy_set")] {
            sqlx::query("INSERT INTO canonical_objects(object_id,store_id,object_kind,canonical_byte_length,encoded_byte_length,compression,storage_key,verification_state,verified_at) VALUES($1,'store:quota',$2,2,2,'identity',$3,'verified',now())").bind(id).bind(kind).bind(format!("seed/{slug}/{kind}")).execute(&mut *tx).await.unwrap();
        }
        sqlx::query("INSERT INTO repository_views(repository_view_id,space_id,canonical_record_jcs) VALUES($1,$2,'{}')").bind(view).bind(space).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO policy_sets(policy_set_id,space_id,canonical_record_jcs) VALUES($1,$2,'{}')").bind(policy).bind(space).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    PrincipalContext::new(
        PrincipalId::parse("principal:quota").unwrap(),
        ActorId::parse("actor:quota").unwrap(),
        "deployment:quota",
        "subject:quota",
    )
    .unwrap()
}
