//! Real PostgreSQL/runtime-role/CAS identities and adversarial invitation API.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use layrs_server::{
    collaboration::{self, CollaborationHttpState},
    config::ServerConfig,
    identity::{BootstrapRequest, Credentials, PgIdentity, random_secret},
    installation,
};
use layrs_store_server::v3::*;
use layrs_sync::v3::PrincipalId;
use serde_json::{Value, json};
use sqlx::{
    Executor,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{collections::BTreeMap, str::FromStr, sync::Arc};
use tower::ServiceExt;

async fn call(
    router: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                }))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
#[ignore = "requires explicit isolated LAYRS_TEST_DATABASE_URL"]
async fn invitations_are_nominative_atomic_expiring_and_never_regrant_on_replay() {
    let options =
        PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let database = format!("layrs_invites_{suffix}");
    admin
        .execute(format!("CREATE DATABASE {database}").as_str())
        .await
        .unwrap();
    let maintenance = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options.clone().database(&database))
        .await
        .unwrap();
    let root = std::env::temp_dir().join(&database);
    std::fs::create_dir(&root).unwrap();
    let config = ServerConfig::parse(&BTreeMap::from([
        (
            "LAYRS_PUBLIC_URL".into(),
            "https://invitations.example".into(),
        ),
        ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
        ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
        (
            "LAYRS_DEPLOYMENT_ID".into(),
            "deployment:invitations".into(),
        ),
        ("LAYRS_STORE_ID".into(), "store:invitations".into()),
        ("LAYRS_SIGNING_KEY".into(), random_secret()),
        ("LAYRS_BOOTSTRAP_SECRET".into(), random_secret()),
    ]))
    .unwrap();
    installation::initialize(&maintenance, &config)
        .await
        .unwrap();
    let writer: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(&maintenance)
            .await
            .unwrap();
    let runtime = format!("invites_runtime_{suffix}");
    let password = random_secret();
    admin
        .execute(format!("CREATE ROLE {runtime} LOGIN PASSWORD '{password}'").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("GRANT {writer} TO {runtime}").as_str())
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect_with(
            options
                .database(&database)
                .username(&runtime)
                .password(&password),
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
    let alice = identity
        .bootstrap(BootstrapRequest {
            secret: config.bootstrap_secret.clone().unwrap(),
            login: "alice@example.com".into(),
            password: "Alice invitation password 2026".into(),
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
    let repo = PgV3Repository::new(
        pool.clone(),
        cas.clone(),
        CapabilityCodec::new(&config.signing_key).unwrap(),
        Arc::new(PgPolicyAuthorizer::new(pool.clone(), cas)),
    );
    let alice_context = load_principal_context(&pool, &alice.principal_id)
        .await
        .unwrap();
    let space = repo
        .provision_space(
            &alice_context,
            &ProvisionSpaceRequest {
                workspace_slug: "invites".into(),
                space_slug: "project".into(),
                name: "Invitations".into(),
                initial_layer: None,
            },
        )
        .await
        .unwrap()
        .space_id;
    let telemetry = layrs_server::telemetry::Telemetry::default();
    let router = identity
        .as_ref()
        .clone()
        .router()
        .merge(layrs_server::telemetry::router(
            identity.as_ref().clone(),
            pool.clone(),
            telemetry.clone(),
        ))
        .merge(collaboration::router(CollaborationHttpState::new(
            repo.clone(),
            identity.clone(),
        )))
        .layer(axum::middleware::from_fn_with_state(
            telemetry,
            layrs_server::telemetry::observe,
        ));
    let mut accounts = vec![];
    for login in ["bob@example.com", "charlie@example.com", "dana@example.com"] {
        let status=call(&router,"POST","/v3/auth/accounts",&alice.access_token,json!({"login":login,"password":"Recipient invitation password 2026","displayName":login})).await.0;
        assert_eq!(status, StatusCode::CREATED);
        accounts.push(
            identity
                .login(Credentials {
                    login: login.into(),
                    password: "Recipient invitation password 2026".into(),
                })
                .await
                .unwrap(),
        );
    }
    let bob = &accounts[0];
    assert_eq!(
        call(&router, "GET", "/v3/admin/metrics", "", json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &router,
            "GET",
            "/v3/admin/metrics",
            &bob.access_token,
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, metrics) = call(
        &router,
        "GET",
        "/v3/admin/metrics",
        &alice.access_token,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(metrics["schemaVersion"], "layrs.telemetry.v1");
    assert!(metrics["http"]["completed"].as_u64().unwrap() >= 2);
    let charlie = &accounts[1];
    let dana = &accounts[2];
    let path = format!("/v3/spaces/{space}/invitations");
    let create = |recipient: &PrincipalId, key: &str| json!({"idempotencyKey":key,"recipientPrincipalId":recipient,"role":"member","expiresInSeconds":3600});
    let request = create(&bob.principal_id, "invite-bob-00000001");
    assert_eq!(
        call(
            &router,
            "POST",
            &path,
            &charlie.access_token,
            request.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, invite) = call(&router, "POST", &path, &alice.access_token, request.clone()).await;
    assert_eq!(status, StatusCode::OK, "{invite}");
    let invitation = invite["invitationId"].as_str().unwrap();
    let action = format!("/v3/invitations/{invitation}/actions");
    assert_eq!(
        call(&router, "POST", &path, &alice.access_token, request.clone())
            .await
            .1,
        invite
    );
    let mut collision = request.clone();
    collision["role"] = json!("auditor");
    assert_eq!(
        call(&router, "POST", &path, &alice.access_token, collision)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &router,
            "GET",
            "/v3/invitations",
            &charlie.access_token,
            Value::Null
        )
        .await
        .1["invitations"],
        json!([])
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &action,
            &charlie.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let membership: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_memberships WHERE principal_id=$1")
            .bind(bob.principal_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(membership, 0, "creation cannot grant rights");
    let before: i64 =
        sqlx::query_scalar("SELECT authorization_epoch FROM spaces WHERE space_id=$1")
            .bind(space.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    let (a, b) = tokio::join!(
        call(
            &router,
            "POST",
            &action,
            &bob.access_token,
            json!({"action":"accept"})
        ),
        call(
            &router,
            "POST",
            &action,
            &bob.access_token,
            json!({"action":"accept"})
        )
    );
    assert!(a.0 == StatusCode::OK || b.0 == StatusCode::OK);
    assert!([StatusCode::OK, StatusCode::CONFLICT].contains(&a.0));
    assert!([StatusCode::OK, StatusCode::CONFLICT].contains(&b.0));
    let after: i64 = sqlx::query_scalar("SELECT authorization_epoch FROM spaces WHERE space_id=$1")
        .bind(space.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, before + 1);
    let accepted = call(
        &router,
        "POST",
        &action,
        &bob.access_token,
        json!({"action":"accept"}),
    )
    .await;
    assert_eq!(accepted.0, StatusCode::OK);
    assert_eq!(accepted.1["state"], "accepted");
    let context = load_principal_context(&pool, &bob.principal_id)
        .await
        .unwrap();
    let collaboration = PgCollaborationStore::new(repo.clone());
    assert!(collaboration.list_reviews(&context, &space).await.is_ok());
    collaboration
        .grant_member(
            &alice_context,
            &space,
            GrantMemberRequest {
                principal_id: bob.principal_id.clone(),
                role: MemberRole::Revoked,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            &action,
            &bob.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        collaboration.list_reviews(&context, &space).await.is_err(),
        "replay must not grant again"
    );
    let rejected = call(
        &router,
        "POST",
        &path,
        &alice.access_token,
        create(&charlie.principal_id, "invite-charlie-00001"),
    )
    .await
    .1;
    let rejected_action = format!(
        "/v3/invitations/{}/actions",
        rejected["invitationId"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &rejected_action,
            &charlie.access_token,
            json!({"action":"reject"})
        )
        .await
        .1["state"],
        "rejected"
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &rejected_action,
            &charlie.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let stale = call(
        &router,
        "POST",
        &path,
        &alice.access_token,
        create(&charlie.principal_id, "invite-charlie-stale1"),
    )
    .await
    .1;
    collaboration
        .grant_member(
            &alice_context,
            &space,
            GrantMemberRequest {
                principal_id: bob.principal_id.clone(),
                role: MemberRole::Auditor,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            &format!(
                "/v3/invitations/{}/actions",
                stale["invitationId"].as_str().unwrap()
            ),
            &charlie.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let expired = call(
        &router,
        "POST",
        &path,
        &alice.access_token,
        create(&charlie.principal_id, "invite-charlie-expire"),
    )
    .await
    .1;
    sqlx::query("UPDATE layrs_collaboration.space_invitations SET created_at=clock_timestamp()-interval '2 hours',expires_at=clock_timestamp()-interval '1 hour' WHERE invitation_id=$1").bind(expired["invitationId"].as_str().unwrap()).execute(&maintenance).await.unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            &format!(
                "/v3/invitations/{}/actions",
                expired["invitationId"].as_str().unwrap()
            ),
            &charlie.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let revoked = call(
        &router,
        "POST",
        &path,
        &alice.access_token,
        create(&charlie.principal_id, "invite-charlie-revoke"),
    )
    .await
    .1;
    let revoked_path = format!(
        "/v3/invitations/{}/actions",
        revoked["invitationId"].as_str().unwrap()
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &revoked_path,
            &alice.access_token,
            json!({"action":"revoke"})
        )
        .await
        .1["state"],
        "revoked"
    );
    assert_eq!(
        call(
            &router,
            "POST",
            &revoked_path,
            &charlie.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let suspended = call(
        &router,
        "POST",
        &path,
        &alice.access_token,
        create(&dana.principal_id, "invite-dana-suspend1"),
    )
    .await
    .1;
    sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role,state) SELECT workspace_id,$2,'member','suspended' FROM spaces WHERE space_id=$1").bind(space.as_str()).bind(dana.principal_id.as_str()).execute(&maintenance).await.unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            &format!(
                "/v3/invitations/{}/actions",
                suspended["invitationId"].as_str().unwrap()
            ),
            &dana.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let state: String = sqlx::query_scalar(
        "SELECT state FROM layrs_collaboration.space_invitations WHERE invitation_id=$1",
    )
    .bind(suspended["invitationId"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "pending");
    let no_access: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_memberships WHERE principal_id=$1")
            .bind(charlie.principal_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(no_access, 0);
    // Fault after policy installation proves the invitation decision and grant
    // roll back together, including the first workspace membership insertion.
    let interrupted = call(
        &router,
        "POST",
        &path,
        &alice.access_token,
        create(&charlie.principal_id, "invite-atomic-fault1"),
    )
    .await
    .1;
    let interrupted_id = interrupted["invitationId"].as_str().unwrap();
    let interrupted_path = format!("/v3/invitations/{interrupted_id}/actions");
    let epoch_before: i64 =
        sqlx::query_scalar("SELECT authorization_epoch FROM spaces WHERE space_id=$1")
            .bind(space.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    maintenance.execute("CREATE FUNCTION layrs_collaboration.test_invitation_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected decision failure'; END $$; CREATE TRIGGER test_invitation_fault BEFORE UPDATE ON layrs_collaboration.space_invitations FOR EACH ROW EXECUTE FUNCTION layrs_collaboration.test_invitation_fault() ").await.unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            &interrupted_path,
            &charlie.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    maintenance.execute("DROP TRIGGER test_invitation_fault ON layrs_collaboration.space_invitations; DROP FUNCTION layrs_collaboration.test_invitation_fault()").await.unwrap();
    let epoch_after: i64 =
        sqlx::query_scalar("SELECT authorization_epoch FROM spaces WHERE space_id=$1")
            .bind(space.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(epoch_before, epoch_after);
    let state: String = sqlx::query_scalar(
        "SELECT state FROM layrs_collaboration.space_invitations WHERE invitation_id=$1",
    )
    .bind(interrupted_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "pending");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_memberships WHERE principal_id=$1")
            .bind(charlie.principal_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE principals SET status='suspended' WHERE principal_id=$1")
        .bind(alice.principal_id.as_str())
        .execute(&maintenance)
        .await
        .unwrap();
    assert_ne!(
        call(
            &router,
            "POST",
            &interrupted_path,
            &charlie.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::OK
    );
    sqlx::query("UPDATE principals SET status='active' WHERE principal_id=$1")
        .bind(alice.principal_id.as_str())
        .execute(&maintenance)
        .await
        .unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            &interrupted_path,
            &charlie.access_token,
            json!({"action":"accept"})
        )
        .await
        .0,
        StatusCode::OK
    );
    drop(router);
    drop(collaboration);
    drop(repo);
    drop(identity);
    pool.close().await;
    maintenance.close().await;
    admin
        .execute(format!("DROP DATABASE {database}").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("DROP ROLE {runtime}").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("DROP ROLE {writer}").as_str())
        .await
        .unwrap();
    admin.close().await;
    std::fs::remove_dir_all(root).unwrap();
}
