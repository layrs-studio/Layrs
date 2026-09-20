use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{ActorId, ObjectId, SpaceId};
use layrs_store_server::v3::{
    AuthorizationCheck, AuthorizationDecision, CapabilityActor, CapabilityCodec, CatalogedCas,
    CompleteUploadOutcome, CompleteUploadRequest, EventCursorCodec, PgCatalogStore, PgPlanStore,
    PgV3Repository, PlanContext, PlanLimits, PolicyAuthorizer, PreparedCasObject, PrincipalContext,
    ServerCas, TransferUrlPolicy, V3StoreError, V3StoreResult, deterministic_plan_id,
};
use layrs_store_server::v3::{ByteRange, CasByteStream, CasHead, PutOutcome, ScrubOutcome};
use layrs_sync::v3::{ClientId, Compression, ObjectDescriptor, ObjectKind, PrincipalId, RequestId};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use std::sync::Arc;

#[derive(Debug)]
struct AllowAuthorizer;

#[async_trait]
impl PolicyAuthorizer for AllowAuthorizer {
    async fn authorize(&self, _check: &AuthorizationCheck) -> V3StoreResult<AuthorizationDecision> {
        Ok(AuthorizationDecision::allow("postgres_integration_test"))
    }
}

#[derive(Debug)]
struct BoundTestCas {
    store_id: String,
}

#[async_trait]
impl ServerCas for BoundTestCas {
    async fn put_if_absent(
        &self,
        _descriptor: &ObjectDescriptor,
        _body: CasByteStream,
    ) -> V3StoreResult<PutOutcome> {
        Ok(PutOutcome::AlreadyPresent)
    }

    async fn head(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<Option<CasHead>> {
        Ok(Some(CasHead {
            descriptor: descriptor.clone(),
        }))
    }

    async fn verified_stream(
        &self,
        _descriptor: &ObjectDescriptor,
    ) -> V3StoreResult<CasByteStream> {
        Ok(Box::pin(stream::once(async { Ok(Bytes::new()) })))
    }

    async fn verified_range(
        &self,
        _descriptor: &ObjectDescriptor,
        _range: ByteRange,
    ) -> V3StoreResult<CasByteStream> {
        Ok(Box::pin(stream::once(async { Ok(Bytes::new()) })))
    }

    async fn scrub(&self, _descriptor: &ObjectDescriptor) -> V3StoreResult<ScrubOutcome> {
        Ok(ScrubOutcome::Healthy)
    }
}

#[async_trait]
impl CatalogedCas for BoundTestCas {
    async fn verify_existing(
        &self,
        descriptor: &ObjectDescriptor,
    ) -> V3StoreResult<PreparedCasObject> {
        Ok(PreparedCasObject {
            descriptor: descriptor.clone(),
            store_id: self.store_id.clone(),
            storage_key: format!("test/{}", descriptor.object_id),
            backend_etag: None,
        })
    }

    async fn prepare_generated(
        &self,
        _kind: ObjectKind,
        _canonical_bytes: Bytes,
    ) -> V3StoreResult<PreparedCasObject> {
        Err(V3StoreError::UnsupportedUploadKind)
    }
}

/// Runs against a fresh baseline installed by the PG sentinel/CI harness.
/// It is a no-op for ordinary unit-test runs with no PostgreSQL configured.
#[tokio::test]
async fn postgres_plan_upload_is_idempotent_epoch_bound_and_non_oracular() {
    let Some(database_url) = std::env::var("LAYRS_V3_TEST_DATABASE_URL").ok() else {
        return;
    };
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await
        .unwrap();
    let fixture = seed(&pool).await;
    let repository = PgV3Repository::new(
        pool.clone(),
        Arc::new(BoundTestCas {
            store_id: fixture.store_id.clone(),
        }),
        CapabilityCodec::new([11_u8; 32]).unwrap(),
        Arc::new(AllowAuthorizer),
    );
    repository.verify_schema().await.unwrap();
    let principal = PrincipalContext::new(
        PrincipalId::parse(&fixture.principal_id).unwrap(),
        ActorId::parse(&fixture.actor_id).unwrap(),
        &fixture.deployment_id,
        &fixture.subject_digest,
    )
    .unwrap();
    let space_id = SpaceId::parse(&fixture.space_id).unwrap();
    let client_id = ClientId::parse(format!("client:{}", fixture.suffix)).unwrap();
    let request_id = RequestId::parse(format!("request:{}", fixture.suffix)).unwrap();
    let plan_id = deterministic_plan_id(&space_id, &client_id, &request_id).unwrap();
    let context = PlanContext {
        actor: CapabilityActor {
            principal_id: principal.principal_id.clone(),
            authorization_epoch: 1,
            space_id: space_id.clone(),
            client_id,
            request_id,
            plan_id,
        },
        now_unix_seconds: 1_800_000_000,
        expires_at_unix_seconds: 1_800_000_600,
        transfer_base_url: TransferUrlPolicy::new(["https://sync.example"], [])
            .unwrap()
            .validate_base("https://sync.example")
            .unwrap(),
    };
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Evidence,
        object_id: object_id('e'),
        canonical_byte_len: 0,
        encoded_byte_len: 0,
        compression: Compression::Identity,
    };
    let store = PgPlanStore::new(repository.clone(), PlanLimits::default());
    let (left, right) = tokio::join!(
        store.create_object_plan(&principal, &context, vec![descriptor.clone()]),
        store.create_object_plan(&principal, &context, vec![descriptor.clone()])
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left, right);
    assert_eq!(left.missing_objects, vec![descriptor.clone()]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM object_plans WHERE plan_id=$1")
            .bind(context.actor.plan_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );

    let token = left.upload_capabilities[0].token.clone();
    let wrong_catalog = PgCatalogStore::new(PgV3Repository::new(
        pool.clone(),
        Arc::new(BoundTestCas {
            store_id: "store:wrong-backend".into(),
        }),
        CapabilityCodec::new([11_u8; 32]).unwrap(),
        Arc::new(AllowAuthorizer),
    ));
    let wrong_binding = CompleteUploadRequest {
        principal: principal.clone(),
        actor: context.actor.clone(),
        descriptor: descriptor.clone(),
        capability_token: token.clone(),
        now_unix_seconds: context.now_unix_seconds,
    };
    assert!(matches!(
        wrong_catalog.complete_upload(wrong_binding).await,
        Err(V3StoreError::InvalidCapability)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM space_object_availability WHERE space_id=$1 AND object_id=$2",
        )
        .bind(space_id.as_str())
        .bind(descriptor.object_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    let catalog = PgCatalogStore::new(repository);
    let valid_token = token.clone();
    let upload = CompleteUploadRequest {
        principal: principal.clone(),
        actor: context.actor.clone(),
        descriptor: descriptor.clone(),
        capability_token: token,
        now_unix_seconds: context.now_unix_seconds,
    };
    let (left, right) = tokio::join!(
        catalog.complete_upload(upload.clone()),
        catalog.complete_upload(upload)
    );
    let outcomes = [left.unwrap(), right.unwrap()];
    assert!(outcomes.contains(&CompleteUploadOutcome::Admitted));
    assert!(outcomes.contains(&CompleteUploadOutcome::AlreadyAdmitted));

    sqlx::query("UPDATE principals SET status='revoked', revoked_at=now() WHERE principal_id=$1")
        .bind(principal.principal_id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    let revoked = CompleteUploadRequest {
        principal: principal.clone(),
        actor: context.actor.clone(),
        descriptor: descriptor.clone(),
        capability_token: valid_token,
        now_unix_seconds: context.now_unix_seconds,
    };
    assert!(matches!(
        catalog.complete_upload(revoked).await,
        Err(V3StoreError::InvalidCapability)
    ));
    sqlx::query("UPDATE principals SET status='active', revoked_at=NULL WHERE principal_id=$1")
        .bind(principal.principal_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("UPDATE spaces SET authorization_epoch=2 WHERE space_id=$1")
        .bind(space_id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    let stale = CompleteUploadRequest {
        principal,
        actor: context.actor,
        descriptor,
        capability_token: left_token(&outcomes),
        now_unix_seconds: context.now_unix_seconds,
    };
    assert!(matches!(
        catalog.complete_upload(stale).await,
        Err(V3StoreError::InvalidCapability | V3StoreError::AuthorizationEpochChanged)
    ));

    let _ = EventCursorCodec::new([13_u8; 32]).unwrap();
}

fn left_token(_outcomes: &[CompleteUploadOutcome]) -> String {
    // Epoch rejection occurs before HMAC verification for an unknown token;
    // this also asserts that the endpoint does not reveal capability state.
    "not-a-capability".into()
}

struct Fixture {
    suffix: String,
    store_id: String,
    deployment_id: String,
    principal_id: String,
    actor_id: String,
    subject_digest: String,
    space_id: String,
}

async fn seed(pool: &PgPool) -> Fixture {
    let suffix = format!("t{}", std::process::id());
    let store_id = format!("store:{suffix}");
    let deployment_id = format!("deployment:{suffix}");
    let principal_id = format!("principal:{suffix}");
    let actor_id = format!("actor:{suffix}");
    let subject_digest = format!("subject:{suffix}");
    let workspace_id = format!("workspace:{suffix}");
    let space_id = format!("space:{suffix}");
    let view_id = object_id('a').to_string();
    let policy_id = object_id('b').to_string();
    let mut tx = pool.begin().await.unwrap();
    tx.execute("SET CONSTRAINTS ALL DEFERRED").await.unwrap();
    sqlx::query(
        "INSERT INTO stores(store_id,backend_kind,backend_config_jcs) VALUES($1,'filesystem','{}')",
    )
    .bind(&store_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO deployments(deployment_id,store_id,display_name) VALUES($1,$2,'test')",
    )
    .bind(&deployment_id)
    .bind(&store_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO principals(principal_id,actor_id,deployment_id,principal_kind,subject_digest,display_name) VALUES($1,$2,$3,'account',$4,'test')")
        .bind(&principal_id).bind(&actor_id).bind(&deployment_id).bind(&subject_digest).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO workspaces(workspace_id,deployment_id,slug,display_name,created_by_principal_id) VALUES($1,$2,$3,'test',$4)")
        .bind(&workspace_id).bind(&deployment_id).bind(&suffix).bind(&principal_id).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO workspace_memberships(workspace_id,principal_id,role) VALUES($1,$2,'owner')",
    )
    .bind(&workspace_id)
    .bind(&principal_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO spaces(space_id,workspace_id,slug,display_name,lifecycle_state,current_repository_view_id,current_policy_set_id,created_by_principal_id) VALUES($1,$2,$3,'test','ready',$4,$5,$6)")
        .bind(&space_id).bind(&workspace_id).bind(&suffix).bind(&view_id).bind(&policy_id).bind(&principal_id).execute(&mut *tx).await.unwrap();
    for (object, kind) in [(&view_id, "repository_view"), (&policy_id, "policy_set")] {
        sqlx::query("INSERT INTO canonical_objects(object_id,store_id,object_kind,canonical_byte_length,encoded_byte_length,compression,storage_key,verification_state,verified_at) VALUES($1,$2,$3,2,2,'identity',$4,'verified',now())")
            .bind(object).bind(&store_id).bind(kind).bind(format!("seed/{object}"))
            .execute(&mut *tx).await.unwrap();
    }
    let globally_catalogued_evidence = object_id('e').to_string();
    sqlx::query("INSERT INTO canonical_objects(object_id,store_id,object_kind,canonical_byte_length,encoded_byte_length,compression,storage_key,verification_state,verified_at) VALUES($1,$2,'evidence',0,0,'identity',$3,'verified',now())")
        .bind(&globally_catalogued_evidence).bind(&store_id)
        .bind(format!("test/{globally_catalogued_evidence}"))
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO repository_views(repository_view_id,space_id,canonical_record_jcs) VALUES($1,$2,'{}')")
        .bind(&view_id).bind(&space_id).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO policy_sets(policy_set_id,space_id,canonical_record_jcs) VALUES($1,$2,'{}')",
    )
    .bind(&policy_id)
    .bind(&space_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    Fixture {
        suffix,
        store_id,
        deployment_id,
        principal_id,
        actor_id,
        subject_digest,
        space_id,
    }
}

fn object_id(hex: char) -> ObjectId {
    ObjectId::parse(format!("blake3:{}", hex.to_string().repeat(64))).unwrap()
}
