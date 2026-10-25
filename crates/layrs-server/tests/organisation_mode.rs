//! Reversible organisation visibility and administrator ownership of the mode.
use layrs_server::{
    config::ServerConfig,
    identity::{BootstrapRequest, PgIdentity, random_secret},
    installation,
};
use layrs_store_server::v3::*;
use sqlx::{
    Executor,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{collections::BTreeMap, str::FromStr, sync::Arc};

fn project(context_id: &str, slug: &str) -> CreateProjectRequest {
    CreateProjectRequest {
        context_id: Some(context_id.to_owned()),
        workspace_id: None,
        workspace_slug: None,
        workspace_name: None,
        space_slug: slug.into(),
        name: slug.into(),
        lens_configuration: None,
    }
}

fn update(
    settings: &OrganisationModeSettings,
    multi: bool,
    primary: &str,
    confirm: bool,
) -> UpdateOrganisationModeRequest {
    UpdateOrganisationModeRequest {
        expected_revision: settings.revision,
        expected_state_token: settings.state_token.clone(),
        multi_organisation_enabled: multi,
        primary_organisation_id: primary.into(),
        confirm_danger: confirm,
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated LAYRS_TEST_DATABASE_URL"]
async fn reversible_mode_preserves_projects_roles_and_personal_access() {
    let options =
        PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let database = format!("layrs_org_mode_{}", uuid::Uuid::new_v4().simple());
    admin
        .execute(format!("CREATE DATABASE {database}").as_str())
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect_with(options.database(&database))
        .await
        .unwrap();
    let root = std::env::temp_dir().join(&database);
    std::fs::create_dir(&root).unwrap();
    let config = ServerConfig::parse(&BTreeMap::from([
        ("LAYRS_PUBLIC_URL".into(), "https://mode.example".into()),
        ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
        ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
        (
            "LAYRS_DEPLOYMENT_ID".into(),
            "deployment:organisation-mode".into(),
        ),
        ("LAYRS_STORE_ID".into(), "store:organisation-mode".into()),
        ("LAYRS_SIGNING_KEY".into(), random_secret()),
        ("LAYRS_BOOTSTRAP_SECRET".into(), random_secret()),
        ("LAYRS_MULTI_ORGANISATION_ENABLED".into(), "true".into()),
        ("LAYRS_PERSONAL_PROJECTS_ENABLED".into(), "true".into()),
    ]))
    .unwrap();
    installation::initialize(&pool, &config).await.unwrap();
    // Runtime uses a restricted writer role; the new admin setting must be
    // writable through that role, not only by the migration superuser.
    let writer: String =
        sqlx::query_scalar("SELECT writer_role_name FROM schema_metadata WHERE singleton_id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        writer.starts_with("layrs_v3_w_")
            && writer
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
    let mut writer_tx = pool.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE \"{writer}\""))
        .execute(&mut *writer_tx)
        .await
        .unwrap();
    sqlx::query("UPDATE layrs_collaboration.deployment_project_contexts SET mode_revision=mode_revision WHERE deployment_id=$1")
        .bind(&config.deployment_id).execute(&mut *writer_tx).await.unwrap();
    writer_tx.rollback().await.unwrap();
    let identity = PgIdentity::new(
        pool.clone(),
        config.deployment_id.clone(),
        config.public_url.as_str().into(),
    )
    .await
    .unwrap();
    let alice = identity
        .bootstrap(BootstrapRequest {
            secret: config.bootstrap_secret.clone().unwrap(),
            login: "alice@example.com".into(),
            password: "Mode test password 2026".into(),
            display_name: "Alice".into(),
        })
        .await
        .unwrap();
    let alice = load_principal_context(&pool, &alice.principal_id)
        .await
        .unwrap();
    let bob_id = format!("principal:{}", uuid::Uuid::new_v4());
    sqlx::query("INSERT INTO principals(principal_id,actor_id,deployment_id,principal_kind,subject_digest,display_name) VALUES($1,$2,$3,'account',$4,'Bob')")
        .bind(&bob_id).bind(format!("actor:{}",uuid::Uuid::new_v4()))
        .bind(&config.deployment_id).bind(format!("subject:{}",uuid::Uuid::new_v4()))
        .execute(&pool).await.unwrap();
    let bob = load_principal_context(&pool, &layrs_sync::v3::PrincipalId::parse(bob_id).unwrap())
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
    let company = repo
        .project_contexts(&alice)
        .await
        .unwrap()
        .default_context_id;
    let bob_personal = repo
        .project_contexts(&bob)
        .await
        .unwrap()
        .contexts
        .into_iter()
        .find(|c| c.kind == "personal")
        .unwrap()
        .id;
    let company_project = repo
        .create_project(&alice, &project(&company, "company-project"))
        .await
        .unwrap();
    let org = repo
        .create_organisation(
            &alice,
            &CreateOrganisationRequest {
                slug: "design".into(),
                name: "Design".into(),
            },
        )
        .await
        .unwrap();
    let org_project = repo
        .create_project(&alice, &project(&org.id, "design-project"))
        .await
        .unwrap();
    let sibling = repo
        .create_organisation(
            &alice,
            &CreateOrganisationRequest {
                slug: "operations".into(),
                name: "Operations".into(),
            },
        )
        .await
        .unwrap();
    let sibling_project = repo
        .create_project(&alice, &project(&sibling.id, "operations-project"))
        .await
        .unwrap();
    let sibling_authority = PgBootstrapStore::new(
        repo.clone(),
        EventCursorCodec::new(&config.signing_key).unwrap(),
    )
    .load_authority(&sibling_project.space_id)
    .await
    .unwrap();
    let memberships_before: Vec<(String,String,String)> = sqlx::query_as("SELECT workspace_id,principal_id,role FROM workspace_memberships WHERE workspace_id=ANY($1) ORDER BY workspace_id,principal_id")
        .bind(vec![company.as_str(),org.id.as_str(),sibling.id.as_str()]).fetch_all(&pool).await.unwrap();
    let before = repo.organisation_mode_settings(&alice).await.unwrap();
    assert_eq!(before.organisations.len(), 3);
    assert!(matches!(
        repo.organisation_mode_settings(&bob).await,
        Err(V3StoreError::InvalidCapability)
    ));
    assert!(matches!(
        repo.update_organisation_mode(&bob, &update(&before, false, &org.id, true))
            .await,
        Err(V3StoreError::InvalidCapability)
    ));
    assert!(matches!(
        repo.update_organisation_mode(&alice, &update(&before, false, &org.id, false))
            .await,
        Err(V3StoreError::InvalidDescriptor(_))
    ));
    assert!(matches!(
        repo.update_organisation_mode(&alice, &update(&before, false, &bob_personal, true))
            .await,
        Err(V3StoreError::InvalidDescriptor(_))
    ));
    repo.create_project(&alice, &project(&company, "count-changed"))
        .await
        .unwrap();
    assert!(matches!(
        repo.update_organisation_mode(&alice, &update(&before, false, &org.id, true))
            .await,
        Err(V3StoreError::OrganisationModeConflict)
    ));
    let fresh = repo.organisation_mode_settings(&alice).await.unwrap();
    let single = repo
        .update_organisation_mode(&alice, &update(&fresh, false, &org.id, true))
        .await
        .unwrap();
    assert!(!single.multi_organisation_enabled);
    assert_eq!(single.primary_organisation_id, org.id);
    // No organisation was converted, and Bob received no membership in Design.
    let bob_contexts = repo.project_contexts(&bob).await.unwrap();
    assert_eq!(bob_contexts.contexts.len(), 1);
    assert_eq!(bob_contexts.default_context_id, bob_personal);
    assert_eq!(
        repo.project_contexts(&alice)
            .await
            .unwrap()
            .default_context_id,
        org.id
    );
    let company_authority = PgBootstrapStore::new(
        repo.clone(),
        EventCursorCodec::new(&config.signing_key).unwrap(),
    )
    .load_authority(&company_project.space_id)
    .await
    .unwrap();
    let hidden_decision = repo
        .authorizer()
        .authorize(&AuthorizationCheck {
            principal: alice.clone(),
            action: V3Action::Bootstrap,
            target: AuthorizationTarget::Space(company_project.space_id.clone()),
            policy_set_id: company_authority.policy_set_id.clone(),
            authorization_epoch: company_authority.authorization_epoch,
        })
        .await
        .unwrap();
    assert!(!hidden_decision.allowed);
    assert_eq!(hidden_decision.reason_code, "organisation_hidden");
    let sibling_hidden = repo
        .authorizer()
        .authorize(&AuthorizationCheck {
            principal: alice.clone(),
            action: V3Action::Bootstrap,
            target: AuthorizationTarget::Space(sibling_project.space_id.clone()),
            policy_set_id: sibling_authority.policy_set_id.clone(),
            authorization_epoch: sibling_authority.authorization_epoch,
        })
        .await
        .unwrap();
    assert_eq!(sibling_hidden.reason_code, "organisation_hidden");
    assert!(matches!(
        repo.create_project(&alice, &project(&company, "hidden-write"))
            .await,
        Err(V3StoreError::InvalidCapability)
    ));
    assert!(matches!(
        repo.create_project(&bob, &project(&org.id, "foreign-write"))
            .await,
        Err(V3StoreError::InvalidCapability)
    ));
    repo.create_project(&bob, &project(&bob_personal, "personal-still-works"))
        .await
        .unwrap();
    let role: String = sqlx::query_scalar(
        "SELECT role FROM workspace_memberships WHERE workspace_id=$1 AND principal_id=$2",
    )
    .bind(&org.id)
    .bind(alice.principal_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(role, "owner");
    let foreign_members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_memberships WHERE workspace_id=$1 AND principal_id=$2",
    )
    .bind(&org.id)
    .bind(bob.principal_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(foreign_members, 0);
    installation::initialize(&pool, &config).await.unwrap();
    assert!(
        !repo
            .organisation_mode_settings(&alice)
            .await
            .unwrap()
            .multi_organisation_enabled,
        "admin mode survives restart with multi env=true"
    );
    let single = repo.organisation_mode_settings(&alice).await.unwrap();
    let restored = repo
        .update_organisation_mode(&alice, &update(&single, true, &company, false))
        .await
        .unwrap();
    assert!(restored.multi_organisation_enabled);
    let restored_decision = repo
        .authorizer()
        .authorize(&AuthorizationCheck {
            principal: alice.clone(),
            action: V3Action::Bootstrap,
            target: AuthorizationTarget::Space(company_project.space_id.clone()),
            policy_set_id: company_authority.policy_set_id,
            authorization_epoch: company_authority.authorization_epoch,
        })
        .await
        .unwrap();
    assert!(restored_decision.allowed);
    let sibling_restored = repo
        .authorizer()
        .authorize(&AuthorizationCheck {
            principal: alice.clone(),
            action: V3Action::Bootstrap,
            target: AuthorizationTarget::Space(sibling_project.space_id.clone()),
            policy_set_id: sibling_authority.policy_set_id.clone(),
            authorization_epoch: sibling_authority.authorization_epoch,
        })
        .await
        .unwrap();
    assert!(sibling_restored.allowed);
    let sibling_authority_after = PgBootstrapStore::new(
        repo.clone(),
        EventCursorCodec::new(&config.signing_key).unwrap(),
    )
    .load_authority(&sibling_project.space_id)
    .await
    .unwrap();
    assert_eq!(
        sibling_authority_after.policy_set_id,
        sibling_authority.policy_set_id
    );
    assert_eq!(
        sibling_authority_after.authorization_epoch,
        sibling_authority.authorization_epoch
    );
    let memberships_after: Vec<(String,String,String)> = sqlx::query_as("SELECT workspace_id,principal_id,role FROM workspace_memberships WHERE workspace_id=ANY($1) ORDER BY workspace_id,principal_id")
        .bind(vec![company.as_str(),org.id.as_str(),sibling.id.as_str()]).fetch_all(&pool).await.unwrap();
    assert_eq!(memberships_after, memberships_before);
    assert!(
        repo.project_contexts(&alice)
            .await
            .unwrap()
            .contexts
            .iter()
            .any(|c| c.id == company)
    );
    assert!(
        repo.create_project(&alice, &project(&company, "restored-write"))
            .await
            .is_ok()
    );
    let projects: i64 = sqlx::query_scalar("SELECT count(*) FROM spaces WHERE space_id=ANY($1)")
        .bind(vec![
            company_project.space_id.as_str(),
            org_project.space_id.as_str(),
            sibling_project.space_id.as_str(),
        ])
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(projects, 3);
    pool.close().await;
    admin.close().await;
}
