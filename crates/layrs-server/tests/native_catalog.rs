//! Real PostgreSQL/runtime-role/CAS coverage of the UI navigation contracts:
//! authorized Space catalog, capability hints and `whoami` administration flag.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use layrs_core::scm::SpaceId;
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

async fn call(router: &Router, method: &str, path: &str, token: &str, body: Value) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(
            request
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
    let cache = response
        .headers()
        .get("cache-control")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    Response {
        status,
        cache,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

struct Response {
    status: StatusCode,
    cache: Option<String>,
    body: Value,
}

async fn ok(router: &Router, method: &str, path: &str, token: &str, body: Value) -> Value {
    let response = call(router, method, path, token, body).await;
    assert!(
        response.status.is_success(),
        "{method} {path}: {} {}",
        response.status,
        response.body
    );
    response.body
}

/// Follows every cursor and returns the visible Space names in server order.
async fn catalog_names(router: &Router, token: &str, limit: u16) -> Vec<String> {
    let mut names = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..32 {
        let path = match &cursor {
            Some(cursor) => format!("/v3/spaces?limit={limit}&cursor={cursor}"),
            None => format!("/v3/spaces?limit={limit}"),
        };
        let page = call(router, "GET", &path, token, Value::Null).await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.body);
        assert_eq!(page.cache.as_deref(), Some("no-store"));
        // No totals or hidden counts: exactly these members.
        let mut keys: Vec<_> = page.body.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["hasMore", "nextCursor", "schemaVersion", "spaces"]);
        assert_eq!(page.body["schemaVersion"], "layrs.space-catalog.v1");
        let spaces = page.body["spaces"].as_array().unwrap();
        assert!(spaces.len() <= usize::from(limit));
        names.extend(
            spaces
                .iter()
                .map(|space| space["name"].as_str().unwrap().to_owned()),
        );
        if page.body["hasMore"] == json!(false) {
            assert_eq!(page.body["nextCursor"], Value::Null);
            return names;
        }
        cursor = Some(page.body["nextCursor"].as_str().unwrap().to_owned());
    }
    panic!("catalog pagination did not terminate");
}

/// Test double for a content-restricted principal: keeps the real policy
/// decision (allowed or not) and only marks it restricted, so the catalog and
/// capability mapping of `limited` access is observed without a policy root.
struct Restricting {
    inner: PgPolicyAuthorizer,
    principal: layrs_sync::v3::PrincipalId,
}
#[async_trait::async_trait]
impl PolicyAuthorizer for Restricting {
    async fn authorize(&self, check: &AuthorizationCheck) -> V3StoreResult<AuthorizationDecision> {
        let mut decision = self.inner.authorize(check).await?;
        if check.principal.principal_id == self.principal {
            decision.content_restricted = true;
        }
        Ok(decision)
    }
}

fn flags(capabilities: &Value) -> BTreeMap<String, bool> {
    capabilities["capabilities"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, value)| (name.clone(), value.as_bool().unwrap()))
        .collect()
}

#[tokio::test]
#[ignore = "requires explicit isolated LAYRS_TEST_DATABASE_URL"]
async fn catalog_lists_only_openable_spaces_and_capabilities_follow_handler_checks() {
    let options =
        PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let database = format!("layrs_catalog_{suffix}");
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
        ("LAYRS_PUBLIC_URL".into(), "https://catalog.example".into()),
        ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
        ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
        ("LAYRS_DEPLOYMENT_ID".into(), "deployment:catalog".into()),
        ("LAYRS_STORE_ID".into(), "store:catalog".into()),
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
    let runtime = format!("catalog_runtime_{suffix}");
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
            password: "Alice catalog password 2026".into(),
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
    let facade = ProductionV3Facade::new(
        repo.clone(),
        identity.clone(),
        EventCursorCodec::new(&config.signing_key).unwrap(),
        config.public_url.clone(),
    );
    let router =
        layrs_server::v3::router(V3HttpState::new(Arc::new(facade), V3HttpLimits::default()))
            .merge(identity.as_ref().clone().router())
            .merge(collaboration::router(CollaborationHttpState::new(
                repo.clone(),
                identity.clone(),
            )));
    let mut sessions = vec![];
    for (login, name) in [
        ("bob@example.com", "Bob"),
        ("carol@example.com", "Carol"),
        ("dave@example.com", "Dave"),
    ] {
        ok(
            &router,
            "POST",
            "/v3/auth/accounts",
            &alice.access_token,
            json!({"login":login,"password":"Catalog member password 2026","displayName":name}),
        )
        .await;
        sessions.push(
            identity
                .login(Credentials {
                    login: login.into(),
                    password: "Catalog member password 2026".into(),
                })
                .await
                .unwrap(),
        );
    }
    let (bob, carol, dave) = (&sessions[0], &sessions[1], &sessions[2]);

    // whoami: display name, kind and the shared instance-administrator rule.
    let me = ok(
        &router,
        "GET",
        "/v3/auth/whoami",
        &alice.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(me["display_name"], "Alice");
    assert_eq!(me["kind"], "bootstrap_admin");
    assert_eq!(me["instance_administrator"], true);
    let me = ok(
        &router,
        "GET",
        "/v3/auth/whoami",
        &bob.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(me["kind"], "account");
    assert_eq!(me["instance_administrator"], false);

    let provision = |context: PrincipalContext, workspace: &str, slug: &str, name: &str| {
        let repo = repo.clone();
        let request = ProvisionSpaceRequest {
            workspace_slug: workspace.into(),
            space_slug: slug.into(),
            name: name.into(),
            initial_layer: None,
        };
        async move {
            repo.provision_space(&context, &request)
                .await
                .unwrap()
                .space_id
        }
    };
    let alice_context = load_principal_context(&pool, &alice.principal_id)
        .await
        .unwrap();
    let bob_context = load_principal_context(&pool, &bob.principal_id)
        .await
        .unwrap();
    let zeta = provision(alice_context.clone(), "acme", "zeta", "Zeta site").await;
    let alpha = provision(alice_context.clone(), "acme", "alpha", "Alpha app").await;
    let beta = provision(alice_context.clone(), "acme", "beta", "beta docs").await;
    let hidden = provision(alice_context.clone(), "acme", "hidden", "Hidden lab").await;
    let homonym = provision(alice_context.clone(), "acme", "alpha-2", "alpha app").await;
    let notes = provision(bob_context, "bob-notes", "notes", "Bob notes").await;

    // Direct grant: Bob becomes a member of Alpha only (and thus of the acme
    // Workspace, which must not reveal its other Spaces).
    ok(
        &router,
        "POST",
        &format!("/v3/spaces/{alpha}/members"),
        &alice.access_token,
        json!({"principalId":bob.principal_id,"role":"member"}),
    )
    .await;
    // Team grant: Carol holds a plain Workspace membership (explicit fixture,
    // as Teams never create one) and reaches beta docs through a Team only.
    sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role,state) SELECT workspace_id,$2,'member','active' FROM spaces WHERE space_id=$1")
        .bind(beta.as_str())
        .bind(carol.principal_id.as_str())
        .execute(&maintenance)
        .await
        .unwrap();
    assert!(
        catalog_names(&router, &carol.access_token, 10)
            .await
            .is_empty()
    );
    let team = ok(
        &router,
        "POST",
        &format!("/v3/spaces/{beta}/teams"),
        &alice.access_token,
        json!({"idempotencyKey":"catalog-team-create-0001","expectedVersion":0,"name":"Docs","role":"member","memberPrincipalIds":[carol.principal_id]}),
    )
    .await;
    let team_id = team["team"]["teamId"].as_str().unwrap().to_owned();

    // Owner: every acme Space, sorted by case-folded name then id, never the
    // Workspace of another owner, whatever the page size.
    let mut expected = vec!["Alpha app", "alpha app"];
    let ids = [alpha.as_str(), homonym.as_str()];
    if ids[0] > ids[1] {
        expected.swap(0, 1);
    }
    expected.extend(["beta docs", "Hidden lab", "Zeta site"]);
    for limit in [1, 2, 5, 100] {
        assert_eq!(
            catalog_names(&router, &alice.access_token, limit).await,
            expected,
            "limit {limit}"
        );
    }
    let first = ok(
        &router,
        "GET",
        "/v3/spaces?limit=2",
        &alice.access_token,
        Value::Null,
    )
    .await;
    let entry = &first["spaces"][0];
    assert_eq!(entry["workspace"]["slug"], "acme");
    assert_eq!(entry["workspaceRole"], "owner");
    assert_eq!(entry["access"], "full");
    assert_eq!(
        entry["lastActivityAt"],
        Value::Null,
        "no event was accepted yet"
    );
    let default_page = ok(
        &router,
        "GET",
        "/v3/spaces",
        &alice.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(default_page["spaces"].as_array().unwrap().len(), 5);
    assert_eq!(default_page["hasMore"], false);

    // Direct access, access through a Team, no access.
    assert_eq!(
        catalog_names(&router, &bob.access_token, 1).await,
        ["Alpha app", "Bob notes"]
    );
    let bob_page = ok(&router, "GET", "/v3/spaces", &bob.access_token, Value::Null).await;
    let roles: Vec<_> = bob_page["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|space| (space["spaceId"].clone(), space["workspaceRole"].clone()))
        .collect();
    assert_eq!(
        roles,
        [
            (json!(alpha), json!("member")),
            (json!(notes), json!("owner"))
        ]
    );
    assert_eq!(
        catalog_names(&router, &carol.access_token, 1).await,
        ["beta docs"]
    );
    let carol_page = ok(
        &router,
        "GET",
        "/v3/spaces?limit=1",
        &carol.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(
        carol_page["hasMore"], false,
        "hidden acme Spaces are not hinted"
    );
    assert!(
        catalog_names(&router, &dave.access_token, 3)
            .await
            .is_empty()
    );

    // Cursors: opaque, bound to their principal and listing, bounded limits.
    let cursor = first["nextCursor"].as_str().unwrap();
    assert!(!cursor.contains("beta"));
    let reused = call(
        &router,
        "GET",
        &format!("/v3/spaces?limit=2&cursor={cursor}"),
        &bob.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(reused.status, StatusCode::BAD_REQUEST);
    assert_eq!(reused.body["error"]["code"], "invalid_cursor");
    let mut tampered = cursor.to_owned();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    for path in [
        format!("/v3/spaces?cursor={tampered}"),
        "/v3/spaces?cursor=".to_owned(),
        format!(
            "/v3/spaces?cursor={}",
            "A".repeat(SEALED_CURSOR_MAX_CHARS + 4)
        ),
    ] {
        let refused = call(&router, "GET", &path, &alice.access_token, Value::Null).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(refused.body["error"]["code"], "invalid_cursor", "{path}");
    }
    for (path, code) in [
        ("/v3/spaces?limit=0", "limit_exceeded"),
        ("/v3/spaces?limit=101", "limit_exceeded"),
        ("/v3/spaces?limit=many", "invalid_descriptor"),
        ("/v3/spaces?offset=2", "invalid_descriptor"),
    ] {
        let refused = call(&router, "GET", path, &alice.access_token, Value::Null).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(refused.body["error"]["code"], code, "{path}");
    }
    assert_eq!(
        call(&router, "GET", "/v3/spaces", "", Value::Null)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&router, "GET", "/v3/spaces", &random_secret(), Value::Null)
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    // Capabilities: owner, direct member, Team member, outsider.
    let owner = ok(
        &router,
        "GET",
        &format!("/v3/spaces/{alpha}/capabilities"),
        &alice.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(owner["schemaVersion"], "layrs.space-capabilities.v1");
    assert_eq!(owner["spaceId"], json!(alpha));
    assert_eq!(owner["access"], "full");
    assert!(owner["authorizationEpoch"].as_u64().unwrap() >= 1);
    let owner_flags = flags(&owner);
    assert_eq!(owner_flags.len(), 22);
    for (name, value) in &owner_flags {
        assert_eq!(*value, name != "runProofs", "owner {name}");
    }
    let member_expected = |name: &str| {
        !matches!(
            name,
            "manageMembers"
                | "manageTeams"
                | "manageInvitations"
                | "managePolicy"
                | "manageWebhooks"
                | "configureProtection"
                | "publishReleases"
                | "revokeReleases"
                | "runProofs"
        )
    };
    for (token, space) in [(&bob.access_token, &alpha), (&carol.access_token, &beta)] {
        let response = call(
            &router,
            "GET",
            &format!("/v3/spaces/{space}/capabilities"),
            token,
            Value::Null,
        )
        .await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.body);
        assert_eq!(response.cache.as_deref(), Some("no-store"));
        assert_eq!(flags(&response.body).len(), owner_flags.len());
        for (name, value) in flags(&response.body) {
            assert_eq!(value, member_expected(&name), "member {name}");
        }
    }
    // Hints never exceed the guarded endpoints: a member is refused team
    // management, exactly as `manageTeams=false` announced.
    assert_eq!(
        call(
            &router,
            "GET",
            &format!("/v3/spaces/{alpha}/teams/eligible-members"),
            &bob.access_token,
            Value::Null
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let missing = SpaceId::parse(format!("space:{}", "0".repeat(64))).unwrap();
    let absent = call(
        &router,
        "GET",
        &format!("/v3/spaces/{missing}/capabilities"),
        &dave.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(absent.status, StatusCode::NOT_FOUND);
    for (token, space) in [
        (&dave.access_token, &alpha),
        (&bob.access_token, &hidden),
        (&carol.access_token, &zeta),
        (&alice.access_token, &notes),
    ] {
        let refused = call(
            &router,
            "GET",
            &format!("/v3/spaces/{space}/capabilities"),
            token,
            Value::Null,
        )
        .await;
        assert_eq!(refused.status, absent.status, "{space}");
        assert_eq!(refused.body, absent.body, "no existence oracle for {space}");
    }

    // Limited (content-restricted) access: listed as limited without activity
    // time; whole-Space collaboration flags drop, path-scoped ones remain.
    let restricted_repo = PgV3Repository::new(
        pool.clone(),
        repo.cas().clone(),
        CapabilityCodec::new(&config.signing_key).unwrap(),
        Arc::new(Restricting {
            inner: PgPolicyAuthorizer::new(pool.clone(), repo.cas().clone()),
            principal: bob.principal_id.clone(),
        }),
    );
    let restricted = collaboration::router(CollaborationHttpState::new(
        restricted_repo,
        identity.clone(),
    ));
    let limited = ok(
        &restricted,
        "GET",
        "/v3/spaces",
        &bob.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(limited["spaces"][0]["spaceId"], json!(alpha));
    assert_eq!(limited["spaces"][0]["access"], "limited");
    assert_eq!(limited["spaces"][0]["lastActivityAt"], Value::Null);
    let limited = ok(
        &restricted,
        "GET",
        &format!("/v3/spaces/{alpha}/capabilities"),
        &bob.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(limited["access"], "limited");
    assert_eq!(flags(&limited).len(), owner_flags.len());
    for (name, value) in flags(&limited) {
        let scoped = matches!(
            name.as_str(),
            "open"
                | "browseFiles"
                | "readScopedReviews"
                | "proposeScopedReviews"
                | "decideScopedReviews"
                | "integrateScopedReviews"
        );
        assert_eq!(value, scoped, "limited {name}");
    }
    assert_eq!(
        ok(
            &restricted,
            "GET",
            "/v3/spaces",
            &alice.access_token,
            Value::Null
        )
        .await["spaces"][0]["access"],
        "full"
    );

    // Revocation between list and open: the listed Space no longer opens,
    // disappears from the next page and loses its capability summary at once.
    let listed = ok(
        &router,
        "GET",
        "/v3/spaces?limit=1",
        &bob.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(listed["spaces"][0]["spaceId"], json!(alpha));
    let bootstrap = format!("/v3/spaces/{alpha}/bootstrap");
    assert_eq!(
        call(&router, "GET", &bootstrap, &bob.access_token, Value::Null)
            .await
            .status,
        StatusCode::OK
    );
    ok(
        &router,
        "POST",
        &format!("/v3/spaces/{alpha}/members"),
        &alice.access_token,
        json!({"principalId":bob.principal_id,"role":"revoked"}),
    )
    .await;
    assert_ne!(
        call(&router, "GET", &bootstrap, &bob.access_token, Value::Null)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &router,
            "GET",
            &format!("/v3/spaces/{alpha}/capabilities"),
            &bob.access_token,
            Value::Null
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let resumed = ok(
        &router,
        "GET",
        &format!(
            "/v3/spaces?limit=1&cursor={}",
            listed["nextCursor"].as_str().unwrap()
        ),
        &bob.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(resumed["spaces"][0]["name"], "Bob notes");
    assert_eq!(
        catalog_names(&router, &bob.access_token, 5).await,
        ["Bob notes"]
    );

    // Removing the only Team grant hides the Space the same way.
    ok(
        &router,
        "PUT",
        &format!("/v3/spaces/{beta}/teams/{team_id}"),
        &alice.access_token,
        json!({"idempotencyKey":"catalog-team-empty-0001","expectedVersion":1,"name":"Docs","role":"member","memberPrincipalIds":[]}),
    )
    .await;
    assert!(
        catalog_names(&router, &carol.access_token, 10)
            .await
            .is_empty()
    );
    assert_eq!(
        call(
            &router,
            "GET",
            &format!("/v3/spaces/{beta}/capabilities"),
            &carol.access_token,
            Value::Null
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    // Bounded scan: a hidden stretch longer than one request's evaluation
    // budget yields an empty page with a sealed cursor that still progresses.
    ok(
        &router,
        "POST",
        "/v3/auth/accounts",
        &alice.access_token,
        json!({"login":"erin@example.com","password":"Catalog member password 2026","displayName":"Erin"}),
    )
    .await;
    let erin = identity
        .login(Credentials {
            login: "erin@example.com".into(),
            password: "Catalog member password 2026".into(),
        })
        .await
        .unwrap();
    for index in 0..260 {
        provision(
            alice_context.clone(),
            "bulk",
            &format!("bulk-{index:03}"),
            &format!("Bulk {index:03}"),
        )
        .await;
    }
    let visible = provision(alice_context.clone(), "bulk", "visible", "Zz visible").await;
    ok(
        &router,
        "POST",
        &format!("/v3/spaces/{visible}/members"),
        &alice.access_token,
        json!({"principalId":erin.principal_id,"role":"member"}),
    )
    .await;
    let scanned = ok(
        &router,
        "GET",
        "/v3/spaces?limit=5",
        &erin.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(scanned["spaces"], json!([]));
    assert_eq!(scanned["hasMore"], true);
    let cursor = scanned["nextCursor"].as_str().unwrap();
    assert!(!cursor.to_ascii_lowercase().contains("bulk"));
    let found = ok(
        &router,
        "GET",
        &format!("/v3/spaces?limit=5&cursor={cursor}"),
        &erin.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(found["spaces"][0]["name"], "Zz visible");
    assert_eq!(found["hasMore"], false);

    // A suspended Workspace membership hides every Space of that Workspace
    // even though policy grants remain.
    sqlx::query("UPDATE workspace_memberships SET state='suspended' WHERE principal_id=$1 AND workspace_id=(SELECT workspace_id FROM spaces WHERE space_id=$2)")
        .bind(alice.principal_id.as_str())
        .bind(alpha.as_str())
        .execute(&maintenance)
        .await
        .unwrap();
    let remaining = catalog_names(&router, &alice.access_token, 100).await;
    assert_eq!(remaining.len(), 261);
    assert!(
        remaining
            .iter()
            .all(|name| name.starts_with("Bulk ") || name == "Zz visible"),
        "no acme Space remains listed"
    );

    drop(router);
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
