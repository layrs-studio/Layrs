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
            context_id: None,
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
    assert!(
        entry["workspace"]["slug"]
            .as_str()
            .unwrap()
            .starts_with("company-")
    );
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
            (json!(notes), json!("member"))
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
    assert_eq!(
        call(
            &router,
            "GET",
            "/v3/spaces",
            &alice.access_token,
            Value::Null
        )
        .await
        .status,
        StatusCode::FORBIDDEN,
        "a suspended sole company membership hides its catalog"
    );

    // Company members may create their own projects, while auditors cannot.
    // Retrying the same request returns the same ready project.
    let bob_workspaces = ok(
        &router,
        "GET",
        "/v3/workspaces",
        &bob.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(bob_workspaces["canCreateOrganisation"], false);
    assert!(
        bob_workspaces["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["slug"]
                .as_str()
                .is_some_and(|slug| slug.starts_with("company-"))
                && item["role"] == "member")
    );
    let acme_id: String = sqlx::query_scalar("SELECT workspace_id FROM spaces WHERE space_id=$1")
        .bind(alpha.as_str())
        .fetch_one(&maintenance)
        .await
        .unwrap();
    let existing_request =
        json!({"workspaceId":acme_id,"spaceSlug":"web-created","name":"Web created"});
    sqlx::query(
        "UPDATE workspace_memberships SET role='auditor' WHERE workspace_id=$1 AND principal_id=$2",
    )
    .bind(&acme_id)
    .bind(bob.principal_id.as_str())
    .execute(&maintenance)
    .await
    .unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            "/v3/projects",
            &bob.access_token,
            existing_request.clone()
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    sqlx::query(
        "UPDATE workspace_memberships SET role='member' WHERE workspace_id=$1 AND principal_id=$2",
    )
    .bind(&acme_id)
    .bind(bob.principal_id.as_str())
    .execute(&maintenance)
    .await
    .unwrap();
    let listed = ok(
        &router,
        "GET",
        "/v3/workspaces",
        &bob.access_token,
        Value::Null,
    )
    .await;
    assert!(
        listed["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["workspaceId"] == acme_id && item["role"] == "member")
    );
    let created = ok(
        &router,
        "POST",
        "/v3/projects",
        &bob.access_token,
        existing_request.clone(),
    )
    .await;
    assert_eq!(
        ok(
            &router,
            "POST",
            "/v3/projects",
            &bob.access_token,
            existing_request
        )
        .await,
        created
    );
    assert_eq!(
        ok(
            &router,
            "GET",
            &format!(
                "/v3/spaces/{}/bootstrap",
                created["spaceId"].as_str().unwrap()
            ),
            &bob.access_token,
            Value::Null
        )
        .await["spaceId"],
        created["spaceId"]
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/v3/projects",
            &bob.access_token,
            json!({"workspaceId":acme_id,"spaceSlug":"web-created","name":"Different"})
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    sqlx::query("UPDATE workspace_memberships SET state='suspended' WHERE workspace_id=$1 AND principal_id=$2")
        .bind(&acme_id).bind(bob.principal_id.as_str()).execute(&maintenance).await.unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            "/v3/projects",
            &bob.access_token,
            json!({"workspaceId":acme_id,"spaceSlug":"blocked","name":"Blocked"})
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let new_organisation = json!({"workspaceSlug":"erin-web","workspaceName":"Erin Web","spaceSlug":"first","name":"First project"});
    assert_eq!(
        call(
            &router,
            "POST",
            "/v3/projects",
            &erin.access_token,
            new_organisation
        )
        .await
        .status,
        StatusCode::BAD_REQUEST,
        "inline organisation creation is disabled"
    );
    let first_request = json!({"spaceSlug":"first","name":"First project"});
    let first = ok(
        &router,
        "POST",
        "/v3/projects",
        &erin.access_token,
        first_request.clone(),
    )
    .await;
    assert_eq!(
        ok(
            &router,
            "POST",
            "/v3/projects",
            &erin.access_token,
            first_request
        )
        .await,
        first
    );
    assert_eq!(call(&router, "POST", "/v3/projects", &erin.access_token, json!({"workspaceSlug":"erin-web","workspaceName":"Other name","spaceSlug":"second","name":"Second project"})).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        ok(
            &router,
            "GET",
            &format!(
                "/v3/spaces/{}/bootstrap",
                first["spaceId"].as_str().unwrap()
            ),
            &erin.access_token,
            Value::Null
        )
        .await["spaceId"],
        first["spaceId"]
    );

    let lens_catalog = ok(
        &router,
        "GET",
        "/v3/lenses/catalog",
        &erin.access_token,
        Value::Null,
    )
    .await;
    let items = lens_catalog
        .as_array()
        .or_else(|| lens_catalog["items"].as_array())
        .unwrap();
    let text_lens = items
        .iter()
        .find(|item| {
            item["manifest"]["analyzer"]["fileExtensions"]
                .as_array()
                .is_some_and(|extensions| extensions.iter().any(|extension| extension == "txt"))
        })
        .expect("installed text Lens");
    let lens_id = text_lens["pin"]["lensId"].as_str().unwrap();
    let configured_request = json!({
        "workspaceId": first["workspaceId"], "spaceSlug":"with-text", "name":"With Text",
        "lensConfiguration":{"pins":[text_lens["pin"].clone()],"associations":[{"extension":".txt","lensId":lens_id}]}
    });
    let configured = ok(
        &router,
        "POST",
        "/v3/projects",
        &erin.access_token,
        configured_request.clone(),
    )
    .await;
    assert_eq!(
        ok(
            &router,
            "POST",
            "/v3/projects",
            &erin.access_token,
            configured_request.clone()
        )
        .await,
        configured
    );
    let pinned = ok(
        &router,
        "GET",
        &format!(
            "/v3/spaces/{}/lenses",
            configured["spaceId"].as_str().unwrap()
        ),
        &erin.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(pinned["revision"], 1);
    assert_eq!(pinned["pins"], json!([text_lens["pin"].clone()]));
    assert_eq!(
        pinned["associations"],
        configured_request["lensConfiguration"]["associations"]
    );
    let different_config = json!({
        "workspaceId": first["workspaceId"], "spaceSlug":"with-text", "name":"With Text",
        "lensConfiguration":{"pins":[],"associations":[]}
    });
    assert_eq!(
        call(
            &router,
            "POST",
            "/v3/projects",
            &erin.access_token,
            different_config
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );

    let project = configured["spaceId"].as_str().unwrap();
    let text_pin = text_lens["pin"].clone();
    let artifact = format!(
        "/v3/spaces/{project}/lenses/{}/{}/{}",
        text_pin["lensId"].as_str().unwrap(),
        text_pin["version"].as_str().unwrap(),
        text_pin["digest"].as_str().unwrap()
    );
    let typed_text_pin: layrs_lens_sdk::LensPin = serde_json::from_value(text_pin.clone()).unwrap();
    let erin_context = load_principal_context(&pool, &erin.principal_id)
        .await
        .unwrap();
    let direct_packet = PgCollaborationStore::new(repo.clone())
        .fetch_lens_packet(
            &erin_context,
            &SpaceId::parse(project.to_owned()).unwrap(),
            &typed_text_pin,
        )
        .await;
    assert!(
        direct_packet.is_ok(),
        "direct Lens packet fetch: {:?}",
        direct_packet.err()
    );
    assert_eq!(
        ok(&router, "GET", &artifact, &erin.access_token, Value::Null).await["manifest"]["id"],
        text_pin["lensId"]
    );
    assert_eq!(
        call(&router, "GET", &artifact, &bob.access_token, Value::Null)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &router,
            "GET",
            "/v3/lenses",
            &erin.access_token,
            Value::Null
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert!(
        text_lens.get("usageCount").is_none(),
        "public catalog must not reveal other projects' usage"
    );
    let admin_lenses = ok(
        &router,
        "GET",
        "/v3/lenses",
        &alice.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(admin_lenses["errors"], json!([]));
    assert!(
        admin_lenses["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["pin"] == text_pin && item["usageCount"] == 1)
    );

    let mut example: Value = serde_json::from_str(include_str!(
        "../../../deploy/lenses/example-markdown.layrslens.json"
    ))
    .unwrap();
    example["approveUnsigned"] = json!(false);
    assert_eq!(
        call(
            &router,
            "POST",
            "/v3/lenses",
            &alice.access_token,
            example.clone()
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    example["approveUnsigned"] = json!(true);
    let installed = ok(&router, "POST", "/v3/lenses", &alice.access_token, example).await;
    assert_eq!(installed["state"], "active");
    assert_eq!(installed["source"], "installed");

    let config_path = format!("/v3/spaces/{project}/lenses");
    let updated = ok(
        &router,
        "PUT",
        &config_path,
        &erin.access_token,
        json!({
            "expectedRevision":1,"pins":[text_pin.clone(),installed["pin"].clone()],
            "associations":[{"extension":".txt","lensId":lens_id}]
        }),
    )
    .await;
    assert_eq!(updated["revision"], 2);
    assert_eq!(
        call(
            &router,
            "PUT",
            &config_path,
            &erin.access_token,
            json!({
                "expectedRevision":1,"pins":[],"associations":[]
            })
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &router,
            "PUT",
            &config_path,
            &bob.access_token,
            json!({
                "expectedRevision":2,"pins":[],"associations":[]
            })
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );

    let state_path = format!(
        "/v3/lenses/{}/{}/{}",
        text_pin["lensId"].as_str().unwrap(),
        text_pin["version"].as_str().unwrap(),
        text_pin["digest"].as_str().unwrap()
    );
    let retired = ok(
        &router,
        "POST",
        &format!("{state_path}/retire"),
        &alice.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(retired["state"], "retired");
    assert_eq!(
        ok(&router, "GET", &artifact, &erin.access_token, Value::Null).await["manifest"]["id"],
        text_pin["lensId"]
    );
    assert!(
        ok(
            &router,
            "GET",
            &config_path,
            &erin.access_token,
            Value::Null
        )
        .await["availableCatalog"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["pin"] == text_pin && item["available"] == true)
    );
    let revoked = ok(
        &router,
        "POST",
        &format!("{state_path}/revoke"),
        &alice.access_token,
        Value::Null,
    )
    .await;
    assert_eq!(revoked["state"], "revoked");
    assert_eq!(
        call(&router, "GET", &artifact, &erin.access_token, Value::Null)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert!(
        ok(
            &router,
            "GET",
            &config_path,
            &erin.access_token,
            Value::Null
        )
        .await["availableCatalog"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["pin"] == text_pin && item["available"] == false)
    );
    let unpinned = ok(
        &router,
        "PUT",
        &config_path,
        &erin.access_token,
        json!({
            "expectedRevision":2,"pins":[installed["pin"].clone()],"associations":[]
        }),
    )
    .await;
    assert_eq!(unpinned["revision"], 3);
    let historical: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.space_lens_history WHERE space_id=$1 AND lens_id=$2 AND version=$3 AND digest=$4)")
        .bind(project).bind(text_pin["lensId"].as_str().unwrap()).bind(text_pin["version"].as_str().unwrap()).bind(text_pin["digest"].as_str().unwrap())
        .fetch_one(&pool).await.unwrap();
    assert!(historical, "removed pin keeps its authorization history");
    assert_eq!(
        call(
            &router,
            "PUT",
            &config_path,
            &erin.access_token,
            json!({
                "expectedRevision":3,"pins":[text_pin],"associations":[]
            })
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );

    // A pre-catalogue client may only submit its old built-in execution pin
    // while this project has no explicit configuration. An empty configuration
    // is a decision to disable lenses, not an invitation to use legacy pins.
    let legacy_space = SpaceId::parse(first["spaceId"].as_str().unwrap()).unwrap();
    let configured_space = SpaceId::parse(configured["spaceId"].as_str().unwrap()).unwrap();
    assert!(repo.validate_legacy_execution(&legacy_space).await.is_ok());
    assert!(
        repo.validate_legacy_execution(&configured_space)
            .await
            .is_err()
    );
    let empty = ok(
        &router,
        "PUT",
        &format!("/v3/spaces/{legacy_space}/lenses"),
        &erin.access_token,
        json!({"expectedRevision":0,"pins":[],"associations":[]}),
    )
    .await;
    assert_eq!(empty["pins"], json!([]));
    assert!(repo.validate_legacy_execution(&legacy_space).await.is_err());
    // The prepare observation above was valid. Commit must observe the new
    // explicit configuration under the same Space lock used by configuration.
    let mut admission = pool.begin().await.unwrap();
    sqlx::query("SELECT space_id FROM spaces WHERE space_id=$1 FOR UPDATE")
        .bind(legacy_space.as_str())
        .fetch_one(&mut *admission)
        .await
        .unwrap();
    assert!(
        repo.validate_legacy_execution_in(&mut admission, &legacy_space)
            .await
            .is_err()
    );
    admission.rollback().await.unwrap();

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
