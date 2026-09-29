//! Real native authority replacement: old authority, exact CAS and managed-mode fences.
use super::*;
use layrs_store_server::v3::{CompareStepsRequest, StepHistoryRequest};
#[path = "policy_version_drill.rs"]
mod policy_version_drill;

fn bind(value: &mut SyncTransactionRequest, label: &str) {
    value.idempotency_key = IdempotencyKey::parse(format!("policy-{label}-00000000")).unwrap();
    value.known_event_cursor = None;
    value.request_digest = value.computed_request_digest().unwrap();
}
fn mutation(
    base: &SpaceBootstrap,
    seed: &Step,
    set: &PolicySet,
    label: &str,
) -> SyncTransactionRequest {
    let mut value = request(base, seed, label, LayerMutationMode::Advance);
    value.steps.clear();
    value.view_mutation = None;
    value.policy_root_mutation = Some(PolicyRootMutation {
        mutation_id: MutationId::parse(format!("mutation:{label}")).unwrap(),
        expected_policy_set_id: base.current_policy_set_id.clone(),
        expected_authorization_epoch: base.authorization_epoch,
        proposed_policy_set: ProposedPolicySet::Inline {
            policy_set: set.clone(),
        },
    });
    bind(&mut value, label);
    value
}
async fn policy(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    base: &SpaceBootstrap,
    rules: Value,
    label: &str,
) -> (ProductEnvelope, PolicySet) {
    let bytes = serde_jcs::to_vec(&rules).unwrap();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-evidence-v1\0");
    hasher.update(&bytes);
    let evidence = object_id(&format!("blake3:{}", hasher.finalize()));
    upload(
        facade,
        principal,
        &base.space_id,
        ObjectKind::Evidence,
        evidence.as_str(),
        bytes,
        label,
    )
    .await;
    let id = PolicyId::parse(format!("policy:{}", blake3::hash(label.as_bytes()))).unwrap();
    let mut product = ProductEnvelope::Policy {
        product_id: id.clone(),
        product_object_id: evidence.clone(),
        depends_on_step_ids: vec![],
        depends_on_product_ids: vec![],
        previous_product_object_id: Nullable::null(),
        record: PolicyRecord {
            schema_version: PolicySchemaVersion::V1,
            policy_id: id.clone(),
            version_id: PolicyVersionId::parse(format!(
                "policy-version:{}",
                blake3::hash(label.as_bytes())
            ))
            .unwrap(),
            space_id: base.space_id.clone(),
            rules_object_id: evidence.clone(),
            created_at: Timestamp::parse("2026-09-20T00:00:00Z").unwrap(),
        },
    };
    let object = product.computed_object_id().unwrap();
    let ProductEnvelope::Policy {
        product_object_id,
        record,
        ..
    } = &mut product
    else {
        unreachable!()
    };
    *product_object_id = object.clone();
    let mut set = base.current_policy_set.record.clone();
    set.policy_versions.push(PolicyVersionEntry {
        policy_id: id,
        policy_version_id: record.version_id.clone(),
        policy_product_object_id: object,
        dependency_object_ids: vec![evidence],
    });
    set.policy_versions
        .sort_by(|a, b| a.policy_id.cmp(&b.policy_id));
    set.policy_set_id = set.computed_id().unwrap();
    (product, set)
}
async fn denied(
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    value: SyncTransactionRequest,
) {
    let before = facade.bootstrap(principal, &value.space_id).await.unwrap();
    let receipt = facade.transact(principal, value.clone()).await.unwrap();
    assert_eq!(
        receipt.receipt().status,
        ReceiptStatus::Rejected,
        "{:?}",
        receipt.receipt()
    );
    assert_eq!(
        facade.transact(principal, value).await.unwrap().receipt(),
        receipt.receipt()
    );
    let after = facade.bootstrap(principal, &before.space_id).await.unwrap();
    assert_eq!(before.current_policy_set_id, after.current_policy_set_id);
    assert_eq!(before.authorization_epoch, after.authorization_epoch);
    assert_eq!(
        before.current_repository_view,
        after.current_repository_view
    );
}
fn fetch(base: &SpaceBootstrap, seed: &Step, label: &str) -> FetchPlanRequest {
    FetchPlanRequest {
        protocol: SyncV3Protocol::V3,
        message_type: FetchPlanRequestType::Request,
        space_id: base.space_id.clone(),
        client_id: ClientId::parse("client:policy-test").unwrap(),
        request_id: RequestId::parse(format!("request:{label}")).unwrap(),
        claims: vec![FetchClaim::Object {
            object: ObjectReference {
                kind: ObjectKind::Tree,
                object_id: object_id(seed.root_tree_id().as_str()),
            },
            root: AuthorizationRoot::Step {
                step_id: seed.id().clone(),
            },
            repo_path: Nullable::null(),
        }],
        page_cursor: Nullable::null(),
        max_objects: 10,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    app: &Router,
    admin: &str,
    identity: &PgIdentity,
    repository: &PgV3Repository,
    facade: &ProductionV3Facade,
    principal: &AuthenticatedPrincipal,
    cas_root: &std::path::Path,
    maintenance: &sqlx::PgPool,
) {
    let provision = facade
        .provision_space(
            principal,
            ProvisionSpaceRequest {
                workspace_slug: "team".into(),
                space_slug: "policy-roots".into(),
                name: "Policy roots".into(),
                initial_layer: None,
            },
        )
        .await
        .unwrap();
    let space = &provision.space_id;
    let base_url = format!("/v3/spaces/{space}");
    let account=ok(app,"POST","/v3/auth/accounts",admin,json!({"login":"policy-reader@example.com","password":"policy reader strong password phrase","displayName":"Policy Reader"})).await;
    let session = identity
        .login(layrs_server::identity::Credentials {
            login: "policy-reader@example.com".into(),
            password: "policy reader strong password phrase".into(),
        })
        .await
        .unwrap();
    let member = facade
        .authenticate_bearer(&session.access_token)
        .await
        .unwrap();
    ok(
        app,
        "POST",
        &format!("{base_url}/members"),
        admin,
        json!({"principalId":account["principalId"],"role":"member"}),
    )
    .await;
    let base = facade.bootstrap(principal, space).await.unwrap();
    let tree = Tree::new(vec![]).unwrap();
    upload(
        facade,
        principal,
        space,
        ObjectKind::Tree,
        tree.id().as_str(),
        serde_jcs::to_vec(&tree).unwrap(),
        "policy-tree",
    )
    .await;
    let seed = Step::new(vec![], tree.id().clone()).unwrap();
    assert_eq!(
        facade
            .transact(
                principal,
                request(&base, &seed, "policy-seed", LayerMutationMode::Advance)
            )
            .await
            .unwrap()
            .receipt()
            .status,
        ReceiptStatus::Applied
    );
    let base = facade.bootstrap(principal, space).await.unwrap();
    let managed = base.current_policy_set.record.clone();
    let management = ok(
        app,
        "GET",
        &format!("{base_url}/policy-management"),
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(management["mode"], "managed");
    assert_eq!(
        call(
            app,
            "GET",
            &format!("{base_url}/policy-management"),
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let forbidden_actions = vec![
        "object.fetch",
        "transaction.apply",
        "review.read",
        "object.plan_upload",
    ];
    let (product,set)=policy(facade,principal,&base,json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[{"effect":"deny","principalIds":[member.principal_id],"actions":forbidden_actions}]}),"policy-deny-member").await;
    let mut active = mutation(&base, &seed, &set, "policy-activate");
    active.products = vec![product.clone()];
    bind(&mut active, "policy-activate");

    // Member cannot authorize a root or sibling action with its own submitted grant.
    let mut own = active.clone();
    bind(&mut own, "policy-self-authorize");
    denied(facade, &member, own).await;
    let (self_grant,self_set)=policy(facade,principal,&base,json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[{"effect":"allow","principalIds":[member.principal_id],"actions":["policy.administer","transaction.apply"]}]}),"policy-self-grant").await;
    let mut self_authorized = mutation(&base, &seed, &self_set, "policy-forged-self-grant");
    self_authorized.products = vec![self_grant];
    let child = Step::new(vec![seed.id().clone()], seed.root_tree_id().clone()).unwrap();
    self_authorized.steps = vec![child.clone()];
    bind(&mut self_authorized, "policy-forged-self-grant");
    denied(facade, &member, self_authorized).await;
    let absent: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM space_steps WHERE space_id=$1 AND step_id=$2)",
    )
    .bind(space.as_str())
    .bind(child.id().as_str())
    .fetch_one(repository.pool())
    .await
    .unwrap();
    assert!(absent, "rejected root cannot admit its sibling Step");
    // Admission alone is inert.
    let mut inert = active.clone();
    inert.policy_root_mutation = None;
    bind(&mut inert, "policy-inert");
    assert_eq!(
        facade
            .transact(principal, inert)
            .await
            .unwrap()
            .receipt()
            .status,
        ReceiptStatus::Applied
    );
    assert_eq!(
        facade
            .bootstrap(principal, space)
            .await
            .unwrap()
            .current_policy_set_id,
        managed.policy_set_id
    );
    // An existing logical policy version is immutable even if the new envelope
    // hashes correctly and its referenced Evidence is still valid.
    let mut conflicting_version = product.clone();
    if let ProductEnvelope::Policy { record, .. } = &mut conflicting_version {
        record.created_at = Timestamp::parse("2026-09-20T00:00:01Z").unwrap();
    }
    let changed_id = conflicting_version.computed_object_id().unwrap();
    if let ProductEnvelope::Policy {
        product_object_id, ..
    } = &mut conflicting_version
    {
        *product_object_id = changed_id;
    }
    let mut duplicate_version = active.clone();
    duplicate_version.policy_root_mutation = None;
    duplicate_version.products = vec![conflicting_version];
    bind(&mut duplicate_version, "policy-version-rebinding");
    denied(facade, principal, duplicate_version).await;
    let allowed = facade
        .fetch_plan(&member, fetch(&base, &seed, "policy-before"))
        .await
        .unwrap();
    assert_eq!(allowed.download_capabilities.len(), 1);
    let old_capability = allowed.download_capabilities[0].token.clone();
    assert_eq!(
        call(
            app,
            "GET",
            &format!("{base_url}/reviews"),
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    // Two independent exact CAS requests: one winner, no rebase or second epoch increment.
    let advance = request(
        &base,
        &child,
        "policy-atomic-advance",
        LayerMutationMode::Advance,
    );
    active.steps = advance.steps;
    active.view_mutation = advance.view_mutation;
    bind(&mut active, "policy-activate");
    let mut rival = active.clone();
    bind(&mut rival, "policy-rival");
    let (a, b) = tokio::join!(
        facade.transact(principal, active.clone()),
        facade.transact(principal, rival.clone())
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.receipt().status, b.receipt().status);
    let winner = if a.receipt().status == ReceiptStatus::Applied {
        &a
    } else {
        &b
    };
    assert_eq!(
        winner.receipt().authorization_epoch,
        base.authorization_epoch
    );
    assert_eq!(
        winner.receipt().resulting_authorization_epoch,
        base.authorization_epoch + 1
    );
    assert_eq!(winner.receipt().current_policy_set_id, set.policy_set_id);
    assert_eq!(
        facade
            .transact(principal, active.clone())
            .await
            .unwrap()
            .receipt(),
        a.receipt()
    );
    assert_eq!(
        facade
            .transact(principal, rival.clone())
            .await
            .unwrap()
            .receipt(),
        b.receipt()
    );
    assert!(facade.download(&old_capability, None).await.is_err());
    let now = facade.bootstrap(principal, space).await.unwrap();
    assert_eq!(
        now.layers[0].head_step_ids,
        vec![child.id().clone()],
        "View and PolicyRoot commit together"
    );
    let snapshot:String=sqlx::query_scalar("SELECT canonical_record_jcs FROM authorization_snapshots WHERE authorization_snapshot_id=$1").bind(winner.receipt().authorization_snapshot_id.as_str()).fetch_one(repository.pool()).await.unwrap();
    let snapshot: AuthorizationSnapshot = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(snapshot.policy_set_id, managed.policy_set_id);
    assert!(
        snapshot
            .decisions
            .iter()
            .any(|d| d.action == "policy.administer" && d.allowed)
    );
    let denied_plan = facade
        .fetch_plan(&member, fetch(&now, &seed, "policy-after"))
        .await
        .unwrap();
    assert!(denied_plan.download_capabilities.is_empty());
    assert_eq!(denied_plan.denied_claims.len(), 1);
    assert_eq!(
        call(
            app,
            "GET",
            &format!("{base_url}/reviews"),
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        facade
            .step_history(
                &member,
                space,
                StepHistoryRequest {
                    anchor: seed.id().clone(),
                    cursor: None,
                    limit: 10,
                    path: None,
                    filter: Default::default()
                }
            )
            .await
            .is_err()
    );
    assert!(
        facade
            .compare_steps(
                &member,
                space,
                CompareStepsRequest {
                    before: seed.id().clone(),
                    after: seed.id().clone()
                }
            )
            .await
            .is_err()
    );
    let mut member_request = request(
        &now,
        &seed,
        "policy-denied-transaction",
        LayerMutationMode::Advance,
    );
    bind(&mut member_request, "policy-denied-transaction");
    denied(facade, &member, member_request).await;
    let management = ok(
        app,
        "GET",
        &format!("{base_url}/policy-management"),
        admin,
        Value::Null,
    )
    .await;
    assert_eq!(management["mode"], "custom");
    assert_eq!(
        management["managedPolicySetId"],
        managed.policy_set_id.as_str()
    );
    let teams = ok(app, "GET", &format!("{base_url}/teams"), admin, Value::Null).await;
    assert_eq!(teams["policyManagement"], management);
    let grant = call(
        app,
        "POST",
        &format!("{base_url}/members"),
        admin,
        json!({"principalId":member.principal_id,"role":"auditor"}),
    )
    .await;
    assert_eq!(grant.0, StatusCode::CONFLICT);
    assert_eq!(grant.1["error"]["code"], "custom_policy_root_active");
    let team=call(app,"POST",&format!("{base_url}/teams"),admin,json!({"idempotencyKey":"policy-team-00000000","expectedVersion":0,"name":"Blocked team","role":"member","memberPrincipalIds":[member.principal_id]})).await;
    assert_eq!(team.0, StatusCode::CONFLICT, "{:?}", team);
    assert_eq!(team.1["error"]["code"], "custom_policy_root_active");
    let invitation = call(app, "POST", &format!("{base_url}/invitations"), admin,
        json!({"idempotencyKey":"policy-invitation-blocked-00000000", "recipientPrincipalId":member.principal_id, "role":"auditor", "expiresInSeconds":3600})).await;
    assert_eq!(invitation.0, StatusCode::CONFLICT);
    assert_eq!(invitation.1["error"]["code"], "custom_policy_root_active");
    let noop = mutation(&now, &seed, &set, "policy-noop");
    let noop = facade.transact(principal, noop).await.unwrap();
    assert_eq!(noop.receipt().status, ReceiptStatus::Applied);
    assert!(noop.receipt().event_ids.is_empty());
    assert_eq!(
        noop.receipt().resulting_authorization_epoch,
        now.authorization_epoch
    );
    // A correct stored old root restores exactly the managed mode, without reinterpreting users.
    let mut restore = mutation(&now, &seed, &managed, "policy-restore");
    restore
        .policy_root_mutation
        .as_mut()
        .unwrap()
        .proposed_policy_set = ProposedPolicySet::Stored {
        policy_set_id: managed.policy_set_id.clone(),
    };
    bind(&mut restore, "policy-restore");
    let restored = facade.transact(principal, restore.clone()).await.unwrap();
    assert_eq!(
        restored.receipt().status,
        ReceiptStatus::Applied,
        "{:?}",
        restored.receipt()
    );
    assert_eq!(
        facade.transact(principal, restore).await.unwrap().receipt(),
        restored.receipt()
    );
    assert_eq!(
        ok(
            app,
            "GET",
            &format!("{base_url}/policy-management"),
            admin,
            Value::Null
        )
        .await["mode"],
        "managed"
    );
    assert_eq!(
        call(
            app,
            "GET",
            &format!("{base_url}/reviews"),
            &session.access_token,
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
    ok(
        app,
        "POST",
        &format!("{base_url}/members"),
        admin,
        json!({"principalId":member.principal_id,"role":"member"}),
    )
    .await;
    let current = facade.bootstrap(principal, space).await.unwrap();
    let mut stale = mutation(&current, &seed, &set, "policy-stale-id");
    stale
        .policy_root_mutation
        .as_mut()
        .unwrap()
        .expected_policy_set_id = set.policy_set_id.clone();
    bind(&mut stale, "policy-stale-id");
    denied(facade, principal, stale).await;
    let mut stale = mutation(&current, &seed, &set, "policy-stale-epoch");
    stale
        .policy_root_mutation
        .as_mut()
        .unwrap()
        .expected_authorization_epoch += 1;
    bind(&mut stale, "policy-stale-epoch");
    denied(facade, principal, stale).await;
    for (label, id) in [
        (
            "policy-missing-set",
            object_id(&format!("blake3:{}", "e".repeat(64))),
        ),
        (
            "policy-wrong-set-kind",
            object_id(seed.root_tree_id().as_str()),
        ),
    ] {
        let mut wrong = mutation(&current, &seed, &set, label);
        wrong
            .policy_root_mutation
            .as_mut()
            .unwrap()
            .proposed_policy_set = ProposedPolicySet::Stored { policy_set_id: id };
        bind(&mut wrong, label);
        denied(facade, principal, wrong).await;
    }
    let mut wrong_space = set.clone();
    wrong_space.space_id = SpaceId::parse("space:unrelated-policy").unwrap();
    wrong_space.policy_set_id = wrong_space.computed_id().unwrap();
    denied(
        facade,
        principal,
        mutation(&current, &seed, &wrong_space, "policy-wrong-space"),
    )
    .await;
    let mut wrong_dependencies = set.clone();
    let entry = wrong_dependencies
        .policy_versions
        .iter_mut()
        .find(|e| e.policy_product_object_id == *product.object_id())
        .unwrap();
    entry.dependency_object_ids = vec![object_id(seed.root_tree_id().as_str())];
    wrong_dependencies.policy_set_id = wrong_dependencies.computed_id().unwrap();
    denied(
        facade,
        principal,
        mutation(
            &current,
            &seed,
            &wrong_dependencies,
            "policy-wrong-dependencies",
        ),
    )
    .await;
    sqlx::query("UPDATE workspace_memberships m SET role='member' FROM spaces s WHERE s.workspace_id=m.workspace_id AND s.space_id=$1 AND m.principal_id=$2").bind(space.as_str()).bind(principal.principal_id.as_str()).execute(maintenance).await.unwrap();
    denied(
        facade,
        principal,
        mutation(&current, &seed, &set, "policy-admin-role-required"),
    )
    .await;
    sqlx::query("UPDATE workspace_memberships m SET role='owner' FROM spaces s WHERE s.workspace_id=m.workspace_id AND s.space_id=$1 AND m.principal_id=$2").bind(space.as_str()).bind(principal.principal_id.as_str()).execute(maintenance).await.unwrap();
    // Unsupported rule schemas/actions/fields are not accepted as inert policy roots.
    for (index,rules) in [json!({"schemaVersion":"future","rules":[]}),json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[{"effect":"allow","principalIds":[member.principal_id],"actions":["*"]}]}),json!({"schemaVersion":"layrs.space-policy-rules.v1","rules":[],"paths":["private"]})].into_iter().enumerate() {
        let label=format!("policy-invalid-{index}"); let (p,s)=policy(facade,principal,&current,rules,&label).await;
        let mut value=mutation(&current,&seed,&s,&label); value.products=vec![p]; bind(&mut value,&label); denied(facade,principal,value).await;
    }
    // Referenced rules must exist and verify in CAS, including an already admitted Product.
    let ProductEnvelope::Policy { record, .. } = &product else {
        unreachable!()
    };
    let hex = &record.rules_object_id.as_str()[7..];
    let path = cas_root.join("objects/evidence").join(&hex[..2]).join(hex);
    let bytes = std::fs::read(&path).unwrap();
    let mut corrupt = bytes.clone();
    corrupt[0] ^= 1;
    std::fs::write(&path, &corrupt).unwrap();
    denied(
        facade,
        principal,
        mutation(&current, &seed, &set, "policy-corrupt-rules"),
    )
    .await;
    std::fs::write(&path, bytes).unwrap();
    let original = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let missing = facade
        .transact(
            principal,
            mutation(&current, &seed, &set, "policy-missing-cas"),
        )
        .await;
    assert!(missing.is_err() || missing.unwrap().receipt().status == ReceiptStatus::Rejected);
    std::fs::write(&path, original).unwrap();
    // Leave a custom root in the fixture so restoration must retain mode and exact managed root.
    let value = mutation(&current, &seed, &set, "policy-final-custom");
    assert_eq!(
        facade
            .transact(principal, value)
            .await
            .unwrap()
            .receipt()
            .status,
        ReceiptStatus::Applied
    );
    let row: (i64,i64)=sqlx::query_as("SELECT authorization_epoch,resulting_authorization_epoch FROM sync_transactions WHERE space_id=$1 AND authorization_epoch<>resulting_authorization_epoch ORDER BY resulting_authorization_epoch DESC LIMIT 1").bind(space.as_str()).fetch_one(repository.pool()).await.unwrap();
    assert_eq!(row.1, row.0 + 1);
    let saved:(String,String,String)=sqlx::query_as("SELECT transaction_id,receipt_canonical_jcs,receipt_digest FROM sync_transactions WHERE space_id=$1 AND resulting_authorization_epoch=$2").bind(space.as_str()).bind(row.1).fetch_one(maintenance).await.unwrap();
    sqlx::query("UPDATE sync_transactions SET receipt_digest=$2 WHERE transaction_id=$1")
        .bind(&saved.0)
        .bind(format!("blake3:{}", "0".repeat(64)))
        .execute(maintenance)
        .await
        .unwrap();
    assert!(
        !call(
            app,
            "GET",
            &format!("{base_url}/policy-management"),
            admin,
            Value::Null
        )
        .await
        .0
        .is_success(),
        "corrupt receipt cannot reclassify custom policy as managed"
    );
    sqlx::query("UPDATE sync_transactions SET receipt_digest=$2 WHERE transaction_id=$1")
        .bind(&saved.0)
        .bind(&saved.2)
        .execute(maintenance)
        .await
        .unwrap();
    let event:(String,String)=sqlx::query_as("SELECT event_id,payload_canonical_jcs FROM space_events WHERE transaction_id=$1 AND event_kind='policy.root_changed'").bind(&saved.0).fetch_one(maintenance).await.unwrap();
    let mut wrong_event: Value = serde_json::from_str(&event.1).unwrap();
    wrong_event["payload"]["newPolicySetId"] = json!(managed.policy_set_id);
    let wrong_event = serde_jcs::to_string(&wrong_event).unwrap();
    sqlx::query("UPDATE space_events SET payload_canonical_jcs=$2 WHERE event_id=$1")
        .bind(&event.0)
        .bind(wrong_event)
        .execute(maintenance)
        .await
        .unwrap();
    assert!(
        !call(
            app,
            "GET",
            &format!("{base_url}/policy-management"),
            admin,
            Value::Null
        )
        .await
        .0
        .is_success(),
        "mismatched canonical event cannot reclassify custom policy"
    );
    sqlx::query("UPDATE space_events SET payload_canonical_jcs=$2 WHERE event_id=$1")
        .bind(&event.0)
        .bind(event.1)
        .execute(maintenance)
        .await
        .unwrap();
    assert_eq!(
        ok(
            app,
            "GET",
            &format!("{base_url}/policy-management"),
            admin,
            Value::Null
        )
        .await["mode"],
        "custom"
    );
    eprintln!(
        "PolicyRoot v1: real enforcement, inert admission, exact CAS/race/no-op/replay, managed API fences/restore and invalid/corrupt rules passed"
    );
    policy_version_drill::run(facade, principal, repository, &seed, &product).await;
}
