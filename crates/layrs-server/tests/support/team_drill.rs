use super::*;

fn change(key: &str, version: u32, role: &str, members: Value) -> Value {
    json!({"idempotencyKey":key,"expectedVersion":version,"name":"Engineering","role":role,"memberPrincipalIds":members})
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: &Router,
    base: &str,
    admin: &str,
    identity: &PgIdentity,
    repository: &PgV3Repository,
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    space: &SpaceId,
) {
    let notice_start: i64 = sqlx::query_scalar(
        "SELECT COALESCE(max(sequence),0) FROM layrs_collaboration.notifications WHERE space_id=$1",
    )
    .bind(space.as_str())
    .fetch_one(repository.pool())
    .await
    .unwrap();
    let eve=ok(app,"POST","/v3/auth/accounts",admin,json!({"login":"teams-eve@example.com","password":"teams eve strong password phrase","displayName":"Eve"})).await;
    let outsider=ok(app,"POST","/v3/auth/accounts",admin,json!({"login":"teams-outsider@example.com","password":"teams outsider strong password phrase","displayName":"Outsider"})).await;
    let session = identity
        .login(layrs_server::identity::Credentials {
            login: "teams-eve@example.com".into(),
            password: "teams eve strong password phrase".into(),
        })
        .await
        .unwrap();
    let token = &session.access_token;
    let path = format!("{base}/teams");
    // Explicit fixture membership only: the production Teams endpoint must never
    // create or reactivate Workspace membership as a side effect.
    sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role,state) SELECT workspace_id,$2,'member','active' FROM spaces WHERE space_id=$1").bind(space.as_str()).bind(eve["principalId"].as_str().unwrap()).execute(repository.pool()).await.unwrap();
    assert_eq!(
        call(app, "GET", &path, token, Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    let eligible = ok(
        app,
        "GET",
        &format!("{path}/eligible-members"),
        admin,
        Value::Null,
    )
    .await;
    assert!(
        eligible["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["principalId"] == eve["principalId"])
    );
    assert!(
        !eligible["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["principalId"] == outsider["principalId"])
    );
    let outside = change(
        "teams-outside-refused-001",
        0,
        "member",
        json!([outsider["principalId"]]),
    );
    assert_eq!(
        call(app, "POST", &path, admin, outside).await.0,
        StatusCode::BAD_REQUEST
    );
    let outside_memberships: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_memberships WHERE principal_id=$1")
            .bind(outsider["principalId"].as_str().unwrap())
            .fetch_one(repository.pool())
            .await
            .unwrap();
    assert_eq!(outside_memberships, 0);
    sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role,state) SELECT workspace_id,$2,'member','revoked' FROM spaces WHERE space_id=$1").bind(space.as_str()).bind(outsider["principalId"].as_str().unwrap()).execute(repository.pool()).await.unwrap();
    assert_eq!(
        call(
            app,
            "POST",
            &path,
            admin,
            change(
                "teams-revoked-workspace-001",
                0,
                "member",
                json!([outsider["principalId"]])
            )
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let outsider_state: String =
        sqlx::query_scalar("SELECT state FROM workspace_memberships WHERE principal_id=$1")
            .bind(outsider["principalId"].as_str().unwrap())
            .fetch_one(repository.pool())
            .await
            .unwrap();
    assert_eq!(outsider_state, "revoked");
    let service = ok(
        app,
        "POST",
        "/v3/auth/service-accounts",
        admin,
        json!({"displayName":"Teams refusal fixture","label":"team-service-test","expiresIn":3600}),
    )
    .await;
    sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role,state) SELECT workspace_id,$2,'member','active' FROM spaces WHERE space_id=$1").bind(space.as_str()).bind(service["principal_id"].as_str().unwrap()).execute(repository.pool()).await.unwrap();
    assert_eq!(
        call(
            app,
            "POST",
            &path,
            admin,
            change(
                "teams-service-refused-001",
                0,
                "member",
                json!([service["principal_id"]])
            )
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let mut invalid_name = change("teams-long-name-refused-001", 0, "member", json!([]));
    invalid_name["name"] = json!("n".repeat(129));
    assert_eq!(
        call(app, "POST", &path, admin, invalid_name).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            app,
            "POST",
            &path,
            admin,
            change(
                "teams-admin-role-refused-001",
                0,
                "administrator",
                json!([])
            )
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let create = change(
        "teams-create-engineering-001",
        0,
        "member",
        json!([eve["principalId"]]),
    );
    let first = ok(app, "POST", &path, admin, create.clone()).await;
    let team = first["team"]["teamId"].as_str().unwrap();
    let item = format!("{path}/{team}");
    assert_eq!(
        ok(app, "POST", &path, admin, create.clone()).await,
        first,
        "retry must retain original event/epoch"
    );
    let mut changed = create;
    changed["name"] = json!("Different payload");
    assert_eq!(
        call(app, "POST", &path, admin, changed).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        ok(app, "GET", &path, token, Value::Null).await["teams"][0]["teamId"],
        team
    );
    assert_eq!(
        call(
            app,
            "GET",
            &format!("{path}/eligible-members"),
            token,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            app,
            "PUT",
            &item,
            token,
            change("teams-member-cannot-admin-001", 1, "auditor", json!([]))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let member = facade.authenticate_bearer(token).await.unwrap();
    let watermark = facade
        .bootstrap(&member, space)
        .await
        .unwrap()
        .watermark_cursor;
    let chunk = Chunk::new(b"team epoch capability".to_vec()).unwrap();
    let plan_request = ObjectPlanRequest {
        protocol: SyncV3Protocol::V3,
        message_type: ObjectPlanRequestType::Request,
        space_id: space.clone(),
        client_id: ClientId::parse("client:team-test").unwrap(),
        request_id: RequestId::parse("request:team-epoch").unwrap(),
        objects: vec![descriptor(
            ObjectKind::Chunk,
            chunk.id().as_str(),
            chunk.bytes(),
        )],
    };
    let old_plan = facade
        .object_plan(&member, plan_request.clone())
        .await
        .unwrap();
    let second = ok(
        app,
        "PUT",
        &item,
        admin,
        change(
            "teams-downgrade-engineering-001",
            1,
            "auditor",
            json!([eve["principalId"]]),
        ),
    )
    .await;
    assert_eq!(second["team"]["version"], 2);
    assert_ne!(first["policySetId"], second["policySetId"]);
    assert!(
        facade
            .events(
                &member,
                SpaceEventPageRequest {
                    client_id: ClientId::parse("client:team-test").unwrap(),
                    protocol: SyncV3Protocol::V3,
                    message_type: EventPageRequestType::Request,
                    space_id: space.clone(),
                    cursor: watermark,
                    max_events: 10
                }
            )
            .await
            .is_err(),
        "old epoch event cursor must not survive Team mutation"
    );
    assert!(
        facade.object_plan(&member, plan_request).await.is_err(),
        "auditor cannot upload"
    );
    let bytes = chunk.bytes().to_vec();
    assert!(
        facade
            .upload(
                &old_plan.upload_capabilities[0].token,
                bytes.len() as u64,
                Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) }))
            )
            .await
            .is_err(),
        "Team change invalidates previously issued capabilities"
    );
    assert!(
        call(app, "GET", &path, token, Value::Null)
            .await
            .0
            .is_success()
    );
    let page = ok(
        app,
        "GET",
        &format!("{item}/history?limit=1"),
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(page["anchorVersion"], 2);
    assert_eq!(page["hasMore"], true);
    let left = change(
        "teams-concurrent-left-001",
        2,
        "member",
        json!([eve["principalId"]]),
    );
    let right = change(
        "teams-concurrent-right-001",
        2,
        "release_manager",
        json!([eve["principalId"]]),
    );
    let (a, b) = tokio::join!(
        call(app, "PUT", &item, admin, left),
        call(app, "PUT", &item, admin, right)
    );
    assert_ne!(
        a.0.is_success(),
        b.0.is_success(),
        "exactly one CAS update wins"
    );
    assert!(
        [StatusCode::CONFLICT, StatusCode::BAD_REQUEST].contains(&if a.0.is_success() {
            b.0
        } else {
            a.0
        })
    );
    let anchored = ok(
        app,
        "GET",
        &format!("{item}/history?limit=1&after=1&anchor=2"),
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(anchored["events"].as_array().unwrap().len(), 1);
    assert_eq!(anchored["events"][0]["record"]["version"], 2);
    assert_eq!(anchored["hasMore"], false);
    let extra = ok(
        app,
        "POST",
        &path,
        admin,
        change(
            "teams-independent-grant-001",
            0,
            "member",
            json!([eve["principalId"]]),
        ),
    )
    .await;
    ok(
        app,
        "PUT",
        &item,
        admin,
        change("teams-remove-member-001", 3, "member", json!([])),
    )
    .await;
    assert!(
        call(app, "GET", &path, token, Value::Null)
            .await
            .0
            .is_success(),
        "independent Team grant survives removal"
    );
    let archive = format!(
        "{path}/{}/archive",
        extra["team"]["teamId"].as_str().unwrap()
    );
    let archive_request = json!({"idempotencyKey":"teams-archive-extra-001","expectedVersion":1,"reason":"Project completed"});
    let archived = ok(app, "POST", &archive, admin, archive_request.clone()).await;
    assert_eq!(archived["team"]["state"], "archived");
    assert_eq!(
        ok(app, "POST", &archive, admin, archive_request).await,
        archived
    );
    assert_eq!(
        call(app, "GET", &path, token, Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    ok(
        app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":eve["principalId"],"role":"member"}),
    )
    .await;
    ok(
        app,
        "PUT",
        &item,
        admin,
        change(
            "teams-readd-member-001",
            4,
            "member",
            json!([eve["principalId"]]),
        ),
    )
    .await;
    ok(
        app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":eve["principalId"],"role":"revoked"}),
    )
    .await;
    assert_eq!(
        call(app, "GET", &path, token, Value::Null).await.0,
        StatusCode::FORBIDDEN,
        "explicit individual deny overrides Team allow"
    );
    ok(
        app,
        "POST",
        &format!("{base}/members"),
        admin,
        json!({"principalId":eve["principalId"],"role":"member"}),
    )
    .await;
    ok(app,"POST",&format!("{item}/archive"),admin,json!({"idempotencyKey":"teams-archive-main-001","expectedVersion":5,"reason":"Keep individual grant"})).await;
    assert!(
        call(app, "GET", &path, token, Value::Null)
            .await
            .0
            .is_success(),
        "archiving does not remove individual grants"
    );
    assert_eq!(
        call(
            app,
            "PUT",
            &item,
            admin,
            change("teams-no-unarchive-001", 6, "member", json!([]))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    for (key, members) in [
        (
            "teams-duplicates-refused-001",
            json!([eve["principalId"], eve["principalId"]]),
        ),
        (
            "teams-too-many-refused-001",
            Value::Array(vec![eve["principalId"].clone(); 65]),
        ),
    ] {
        assert_eq!(
            call(app, "POST", &path, admin, change(key, 0, "member", members))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let other = facade
        .provision_space(
            principal,
            ProvisionSpaceRequest {
                workspace_slug: "team".into(),
                space_slug: "teams-isolated".into(),
                name: "Isolated Teams".into(),
                initial_layer: None,
            },
        )
        .await
        .unwrap();
    let other_path = format!("/v3/spaces/{}/teams/{team}/history", other.space_id);
    assert_eq!(
        call(app, "GET", &other_path, admin, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(
        sqlx::query("UPDATE layrs_collaboration.team_versions SET version=version WHERE false")
            .execute(repository.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM layrs_collaboration.team_versions WHERE false")
            .execute(repository.pool())
            .await
            .is_err()
    );
    limits(app, base, admin, repository, space, &eve["principalId"]).await;
    let history = ok(app, "GET", &format!("{item}/history"), admin, Value::Null).await;
    let records = history["events"].as_array().unwrap();
    assert_eq!(records.len(), 6);
    for pair in records.windows(2) {
        assert_eq!(
            pair[1]["record"]["previousEventObjectId"],
            pair[0]["eventObjectId"]
        );
    }
    let notices = ok(
        app,
        "GET",
        &format!("{base}/notifications?after={notice_start}"),
        admin,
        Value::Null,
    )
    .await;
    let team_notices: Vec<_> = notices["notifications"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "team.changed")
        .collect();
    assert!(!team_notices.is_empty());
    for notice in team_notices {
        assert!(notice["reviewContext"].is_null());
    }
    eprintln!(
        "Teams: exact rights, epoch revocation, idempotence, concurrent CAS, immutable audit and bounds passed"
    );
}

async fn limits(
    app: &Router,
    base: &str,
    admin: &str,
    repository: &PgV3Repository,
    space: &SpaceId,
    member: &Value,
) {
    let path = format!("{base}/teams");
    let mut added = Vec::new();
    let mut rejected = false;
    for index in 0..32 {
        let before: (String, i64) = sqlx::query_as(
            "SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1",
        )
        .bind(space.as_str())
        .fetch_one(repository.pool())
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM canonical_objects")
            .fetch_one(repository.pool())
            .await
            .unwrap();
        let (status, result) = call(
            app,
            "POST",
            &path,
            admin,
            change(
                &format!("teams-policy-capacity-{index:03}"),
                0,
                "member",
                json!([member]),
            ),
        )
        .await;
        if status.is_success() {
            added.push(result);
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(result["error"]["code"], "invalid_descriptor");
            let policies:i64=sqlx::query_scalar("SELECT count(*) FROM policy_set_entries e JOIN spaces s ON s.space_id=e.space_id AND s.current_policy_set_id=e.policy_set_id WHERE s.space_id=$1").bind(space.as_str()).fetch_one(repository.pool()).await.unwrap();
            assert_eq!(policies, 32);
            let after: (String, i64) = sqlx::query_as(
                "SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1",
            )
            .bind(space.as_str())
            .fetch_one(repository.pool())
            .await
            .unwrap();
            assert_eq!(after, before);
            let after_count: i64 = sqlx::query_scalar("SELECT count(*) FROM canonical_objects")
                .fetch_one(repository.pool())
                .await
                .unwrap();
            assert_eq!(after_count, count, "failed grant rolls back SQL admission");
            rejected = true;
            break;
        }
    }
    assert!(rejected);
    for (index, team) in added.iter().enumerate() {
        ok(app,"POST",&format!("{path}/{}/archive",team["team"]["teamId"].as_str().unwrap()),admin,json!({"idempotencyKey":format!("teams-capacity-archive-{index:03}"),"expectedVersion":1,"reason":"Release policy capacity"})).await;
    }
    let count = ok(app, "GET", &path, admin, Value::Null).await["teams"]
        .as_array()
        .unwrap()
        .len();
    for index in count..32 {
        ok(
            app,
            "POST",
            &path,
            admin,
            change(
                &format!("teams-empty-capacity-{index:03}"),
                0,
                "auditor",
                json!([]),
            ),
        )
        .await;
    }
    let overflow = call(
        app,
        "POST",
        &path,
        admin,
        change("teams-thirty-third-refused", 0, "member", json!([])),
    )
    .await;
    assert_eq!(overflow.0, StatusCode::BAD_REQUEST);
    assert_eq!(overflow.1["error"]["code"], "invalid_descriptor");
    assert_eq!(
        ok(app, "GET", &path, admin, Value::Null).await["teams"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    // Leave a real active grant for the restoration drill, not only archives.
    let teams = ok(app, "GET", &path, admin, Value::Null).await;
    let active = teams["teams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["state"] == "active")
        .unwrap();
    let item = format!("{path}/{}", active["teamId"].as_str().unwrap());
    let active = ok(
        app,
        "PUT",
        &item,
        admin,
        change(
            "teams-retained-active-grant-001",
            active["version"].as_u64().unwrap() as u32,
            "member",
            json!([member]),
        ),
    )
    .await;
    let before = ok(app, "GET", &path, admin, Value::Null).await;
    let id = active["team"]["teamId"].as_str().unwrap();
    // Rebuild a deliberately stale projection from the immutable ordered ledger.
    sqlx::query("UPDATE layrs_collaboration.space_teams SET current_version=1 WHERE team_id=$1")
        .bind(id)
        .execute(repository.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM layrs_collaboration.team_members WHERE team_id=$1")
        .bind(id)
        .execute(repository.pool())
        .await
        .unwrap();
    let rows:Vec<(i32,String)>=sqlx::query_as("SELECT version,canonical_document_jcs FROM layrs_collaboration.team_versions WHERE team_id=$1 ORDER BY version").bind(id).fetch_all(repository.pool()).await.unwrap();
    let (version, raw) = rows.last().unwrap();
    let record: Value = serde_json::from_str(raw).unwrap();
    let mut tx = repository.pool().begin().await.unwrap();
    sqlx::query("UPDATE layrs_collaboration.space_teams SET current_version=$2 WHERE team_id=$1")
        .bind(id)
        .bind(version)
        .execute(&mut *tx)
        .await
        .unwrap();
    for principal in record["memberPrincipalIds"].as_array().unwrap() {
        sqlx::query("INSERT INTO layrs_collaboration.team_members(team_id,space_id,principal_id) VALUES($1,$2,$3)").bind(id).bind(space.as_str()).bind(principal.as_str().unwrap()).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(
        ok(app, "GET", &path, admin, Value::Null).await,
        before,
        "reconstructible display projection; current PolicySet remains authority"
    );
}
