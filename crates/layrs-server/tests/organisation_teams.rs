//! Real PostgreSQL tests of shared directory teams and project-specific grants.
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
    v3::{ProductionV3Facade, V3HttpLimits, V3HttpState},
};
use layrs_store_server::v3::*;
use serde_json::{Value, json};
use sqlx::{
    Executor,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{collections::BTreeMap, str::FromStr, sync::Arc};
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(if body.is_null() {
            String::new()
        } else {
            body.to_string()
        }))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn ok(app: &Router, method: &str, path: &str, token: &str, body: Value) -> Value {
    let (status, value) = call(app, method, path, token, body).await;
    assert!(status.is_success(), "{method} {path}: {status} {value}");
    value
}
fn visible_spaces(value: &Value) -> Vec<String> {
    value["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["spaceId"].as_str().unwrap().to_owned())
        .collect()
}

async fn team_authority_snapshot(pool: &sqlx::PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('spaces',(SELECT jsonb_agg(to_jsonb(s) ORDER BY space_id) FROM spaces s),'directory',(SELECT jsonb_agg(to_jsonb(t) ORDER BY team_id,version) FROM layrs_collaboration.organisation_team_versions t),'assignments',(SELECT jsonb_agg(to_jsonb(a) ORDER BY team_id,space_id,version) FROM layrs_collaboration.organisation_team_assignment_versions a),'currentTeams',(SELECT jsonb_agg(to_jsonb(t) ORDER BY team_id) FROM layrs_collaboration.organisation_teams t),'members',(SELECT jsonb_agg(to_jsonb(m) ORDER BY team_id,principal_id) FROM layrs_collaboration.organisation_team_members m),'currentAssignments',(SELECT jsonb_agg(to_jsonb(a) ORDER BY team_id,space_id) FROM layrs_collaboration.organisation_team_assignments a),'roots',(SELECT jsonb_agg(to_jsonb(h) ORDER BY space_id,authorization_epoch) FROM policy_root_history h),'policies',(SELECT count(*) FROM policy_sets))")
        .fetch_one(pool).await.unwrap()
}

/// Inject a prior valid policy root on the last project to exercise rollback
/// after earlier project policies have already been prepared. This is a fault
/// injection into authority state, not evidence of the custom-policy UI flow.
async fn linked_policy_failure_is_atomic(
    app: &Router,
    pool: &sqlx::PgPool,
    token: &str,
    path: &str,
    team: &str,
    later_space: &str,
) {
    let current: String =
        sqlx::query_scalar("SELECT current_policy_set_id FROM spaces WHERE space_id=$1")
            .bind(later_space)
            .fetch_one(pool)
            .await
            .unwrap();
    let original: String = sqlx::query_scalar("SELECT policy_set_id FROM policy_root_history WHERE space_id=$1 ORDER BY authorization_epoch LIMIT 1").bind(later_space).fetch_one(pool).await.unwrap();
    assert_ne!(current, original);
    sqlx::query("UPDATE spaces SET current_policy_set_id=$2 WHERE space_id=$1")
        .bind(later_space)
        .bind(&original)
        .execute(pool)
        .await
        .unwrap();
    let before = team_authority_snapshot(pool).await;
    for (method, suffix, request) in [
        (
            "PUT",
            "",
            json!({"idempotencyKey":"orgteam-atomic-update-0001","expectedVersion":1,"name":"Engineering","memberPrincipalIds":[]}),
        ),
        (
            "POST",
            "/archive",
            json!({"idempotencyKey":"orgteam-atomic-archive-0001","expectedVersion":1,"reason":"Atomic rollback test"}),
        ),
    ] {
        let (status, body) = call(
            app,
            method,
            &format!("{path}/{team}{suffix}"),
            token,
            request,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(before, team_authority_snapshot(pool).await);
    }
    sqlx::query("UPDATE spaces SET current_policy_set_id=$2 WHERE space_id=$1")
        .bind(later_space)
        .bind(current)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires explicit isolated LAYRS_TEST_DATABASE_URL"]
async fn directory_membership_updates_all_assigned_projects_without_changing_legacy_teams() {
    let options =
        PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let database = format!("layrs_org_teams_{suffix}");
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
            "https://org-teams.example".into(),
        ),
        ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
        ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
        ("LAYRS_DEPLOYMENT_ID".into(), "deployment:org-teams".into()),
        ("LAYRS_STORE_ID".into(), "store:org-teams".into()),
        ("LAYRS_SIGNING_KEY".into(), random_secret()),
        ("LAYRS_BOOTSTRAP_SECRET".into(), random_secret()),
        ("LAYRS_MULTI_ORGANISATION_ENABLED".into(), "true".into()),
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
    let runtime = format!("org_team_runtime_{suffix}");
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
                .clone()
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
            login: "alice-orgteams@example.com".into(),
            password: "Alice team password 2026".into(),
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
    let app = layrs_server::v3::router(V3HttpState::new(Arc::new(facade), V3HttpLimits::default()))
        .merge(identity.as_ref().clone().router())
        .merge(collaboration::router(CollaborationHttpState::new(
            repository.clone(),
            identity.clone(),
        )));
    let bob=ok(&app,"POST","/v3/auth/accounts",&alice.access_token,json!({"login":"bob-orgteams@example.com","password":"Bob team password 2026","displayName":"Bob"})).await;
    let bob_session = identity
        .login(Credentials {
            login: "bob-orgteams@example.com".into(),
            password: "Bob team password 2026".into(),
        })
        .await
        .unwrap();
    let alice_ctx = load_principal_context(&pool, &alice.principal_id)
        .await
        .unwrap();
    let company = repository
        .project_contexts(&alice_ctx)
        .await
        .unwrap()
        .default_context_id;
    let p1 = repository
        .provision_space(
            &alice_ctx,
            &ProvisionSpaceRequest {
                context_id: Some(company.clone()),
                workspace_slug: "ignored".into(),
                space_slug: "first".into(),
                name: "First".into(),
                initial_layer: None,
            },
        )
        .await
        .unwrap()
        .space_id;
    let p2 = repository
        .provision_space(
            &alice_ctx,
            &ProvisionSpaceRequest {
                context_id: Some(company.clone()),
                workspace_slug: "ignored".into(),
                space_slug: "second".into(),
                name: "Second".into(),
                initial_layer: None,
            },
        )
        .await
        .unwrap()
        .space_id;
    let organisation = repository
        .create_organisation(
            &alice_ctx,
            &CreateOrganisationRequest {
                slug: "other".into(),
                name: "Other".into(),
            },
        )
        .await
        .unwrap();
    let p3 = repository
        .provision_space(
            &alice_ctx,
            &ProvisionSpaceRequest {
                context_id: Some(organisation.id),
                workspace_slug: "ignored".into(),
                space_slug: "third".into(),
                name: "Third".into(),
                initial_layer: None,
            },
        )
        .await
        .unwrap()
        .space_id;
    // Bob is a company member but no project is visible before an explicit assignment.
    ok(
        &app,
        "GET",
        "/v3/contexts",
        &bob_session.access_token,
        Value::Null,
    )
    .await;
    assert!(
        visible_spaces(
            &ok(
                &app,
                "GET",
                "/v3/spaces",
                &bob_session.access_token,
                Value::Null
            )
            .await
        )
        .is_empty()
    );
    let legacy=ok(&app,"POST",&format!("/v3/spaces/{p1}/teams"),&alice.access_token,json!({"idempotencyKey":"legacy-organisation-preservation-0001","expectedVersion":0,"name":"Legacy","role":"auditor","memberPrincipalIds":[]})).await;
    let legacy_id = legacy["team"]["teamId"].clone();
    let path = format!("/v3/spaces/{p1}/organisation-teams");
    let created=ok(&app,"POST",&path,&alice.access_token,json!({"idempotencyKey":"orgteam-create-0001","expectedVersion":0,"name":"Engineering","memberPrincipalIds":[bob["principalId"]]})).await;
    let team = created["team"]["teamId"].as_str().unwrap();
    assert!(team.starts_with("org-team:"));
    let assign = |space: &str| format!("/v3/spaces/{space}/organisation-teams/{team}/assignment");
    let first = json!({"idempotencyKey":"orgteam-assignment-first-0001","expectedVersion":0,"role":"member"});
    let result = ok(
        &app,
        "PUT",
        &assign(p1.as_str()),
        &alice.access_token,
        first.clone(),
    )
    .await;
    assert_eq!(result["version"], 1);
    let replay = ok(
        &app,
        "PUT",
        &assign(p1.as_str()),
        &alice.access_token,
        first,
    )
    .await;
    assert_eq!(replay, result);
    ok(&app,"PUT",&assign(p2.as_str()),&alice.access_token,json!({"idempotencyKey":"orgteam-assignment-second-0001","expectedVersion":0,"role":"auditor"})).await;
    let (cross_status, _) = call(
        &app,
        "PUT",
        &assign(p3.as_str()),
        &alice.access_token,
        json!({"idempotencyKey":"orgteam-cross-org-0001","expectedVersion":0,"role":"member"}),
    )
    .await;
    assert!(cross_status == StatusCode::FORBIDDEN || cross_status == StatusCode::NOT_FOUND);
    let before = visible_spaces(
        &ok(
            &app,
            "GET",
            "/v3/spaces",
            &bob_session.access_token,
            Value::Null,
        )
        .await,
    );
    assert_eq!(before.len(), 2);
    assert!(before.contains(&p1.to_string()) && before.contains(&p2.to_string()));
    linked_policy_failure_is_atomic(
        &app,
        &maintenance,
        &alice.access_token,
        &path,
        team,
        std::cmp::max(p1.as_str(), p2.as_str()),
    )
    .await;
    let update=ok(&app,"PUT",&format!("{path}/{team}"),&alice.access_token,json!({"idempotencyKey":"orgteam-update-0001","expectedVersion":1,"name":"Engineering","memberPrincipalIds":[]})).await;
    assert_eq!(update["team"]["version"], 2);
    assert!(
        visible_spaces(
            &ok(
                &app,
                "GET",
                "/v3/spaces",
                &bob_session.access_token,
                Value::Null
            )
            .await
        )
        .is_empty()
    );
    let historical = ok(
        &app,
        "GET",
        &format!("/v3/spaces/{p1}/teams"),
        &alice.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(historical["teams"][0]["teamId"], legacy_id);
    // A stale concurrent editor cannot add back an old member or alter either project.
    let (status,_)=call(&app,"PUT",&format!("{path}/{team}"),&alice.access_token,json!({"idempotencyKey":"orgteam-stale-0001","expectedVersion":1,"name":"Engineering","memberPrincipalIds":[bob["principalId"]]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        visible_spaces(
            &ok(
                &app,
                "GET",
                "/v3/spaces",
                &bob_session.access_token,
                Value::Null
            )
            .await
        )
        .is_empty()
    );
    let restored=ok(&app,"PUT",&format!("{path}/{team}"),&alice.access_token,json!({"idempotencyKey":"orgteam-restore-0001","expectedVersion":2,"name":"Engineering","memberPrincipalIds":[bob["principalId"]]})).await;
    assert_eq!(restored["team"]["version"], 3);
    assert_eq!(
        visible_spaces(
            &ok(
                &app,
                "GET",
                "/v3/spaces",
                &bob_session.access_token,
                Value::Null
            )
            .await
        )
        .len(),
        2
    );
    let archive=ok(&app,"POST",&format!("{path}/{team}/archive"),&alice.access_token,json!({"idempotencyKey":"orgteam-archive-0001","expectedVersion":3,"reason":"Team no longer needed"})).await;
    assert_eq!(archive["team"]["state"], "archived");
    assert!(
        visible_spaces(
            &ok(
                &app,
                "GET",
                "/v3/spaces",
                &bob_session.access_token,
                Value::Null
            )
            .await
        )
        .is_empty()
    );
    let assignment_versions:i64=sqlx::query_scalar("SELECT count(*) FROM layrs_collaboration.organisation_team_assignment_versions WHERE team_id=$1")
        .bind(team).fetch_one(&pool).await.unwrap();
    assert_eq!(
        assignment_versions, 4,
        "creation and archive of each project grant are audited"
    );
    let directory_versions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM layrs_collaboration.organisation_team_versions WHERE team_id=$1",
    )
    .bind(team)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(directory_versions, 4);

    // Reserve the final version for revocation. Seed the boundary instead of
    // performing a thousand unrelated HTTP mutations.
    let boundary=ok(&app,"POST",&path,&alice.access_token,json!({"idempotencyKey":"orgteam-boundary-create","expectedVersion":0,"name":"Boundary","memberPrincipalIds":[bob["principalId"]]})).await;
    let boundary_id = boundary["team"]["teamId"].as_str().unwrap();
    for (index, space) in [&p1, &p2].into_iter().enumerate() {
        ok(&app,"PUT",&format!("/v3/spaces/{space}/organisation-teams/{boundary_id}/assignment"),&alice.access_token,json!({"idempotencyKey":format!("boundary-assign-{index}"),"expectedVersion":0,"role":"member"})).await;
    }
    let mut record = boundary["team"].clone();
    record["version"] = json!(999);
    sqlx::query("INSERT INTO layrs_collaboration.organisation_team_versions(team_id,version,mutation_id,request_digest,actor_principal_id,canonical_document_jcs) VALUES($1,999,'fixture-boundary-version','fixture-boundary',$2,$3)")
        .bind(boundary_id).bind(alice.principal_id.as_str()).bind(serde_jcs::to_string(&record).unwrap()).execute(&maintenance).await.unwrap();
    sqlx::query(
        "UPDATE layrs_collaboration.organisation_teams SET current_version=999 WHERE team_id=$1",
    )
    .bind(boundary_id)
    .execute(&maintenance)
    .await
    .unwrap();
    sqlx::query("UPDATE layrs_collaboration.organisation_team_assignments SET current_version=999 WHERE team_id=$1").bind(boundary_id).execute(&maintenance).await.unwrap();
    let (status,_)=call(&app,"PUT",&format!("{path}/{boundary_id}"),&alice.access_token,json!({"idempotencyKey":"boundary-edit-0001","expectedVersion":999,"name":"Boundary","memberPrincipalIds":[]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        "PUT",
        &format!("/v3/spaces/{p2}/organisation-teams/{boundary_id}/assignment"),
        &alice.access_token,
        json!({"idempotencyKey":"boundary-regrant-0001","expectedVersion":999,"role":"auditor"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let removed = ok(
        &app,
        "PUT",
        &format!("/v3/spaces/{p2}/organisation-teams/{boundary_id}/assignment"),
        &alice.access_token,
        json!({"idempotencyKey":"boundary-remove-0001","expectedVersion":999,"role":null}),
    )
    .await;
    assert_eq!(removed["version"], 1000);
    let archived=ok(&app,"POST",&format!("{path}/{boundary_id}/archive"),&alice.access_token,json!({"idempotencyKey":"boundary-archive-0001","expectedVersion":999,"reason":"Revoke at version limit"})).await;
    assert_eq!(archived["team"]["version"], 1000);
    assert_eq!(archived["team"]["state"], "archived");
    let active:i64=sqlx::query_scalar("SELECT count(*) FROM layrs_collaboration.organisation_team_assignments WHERE team_id=$1 AND role IS NOT NULL").bind(boundary_id).fetch_one(&maintenance).await.unwrap();
    assert_eq!(active, 0);
    assert!(
        visible_spaces(
            &ok(
                &app,
                "GET",
                "/v3/spaces",
                &bob_session.access_token,
                Value::Null
            )
            .await
        )
        .is_empty()
    );
    pool.close().await;
    maintenance.close().await;
    admin
        .execute(format!("DROP DATABASE {database} WITH (FORCE)").as_str())
        .await
        .unwrap();
    admin
        .execute(format!("DROP ROLE {runtime}").as_str())
        .await
        .unwrap();
}
