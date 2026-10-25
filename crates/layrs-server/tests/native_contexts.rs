//! Real PostgreSQL coverage for independently configured project contexts.
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

#[tokio::test]
#[ignore = "requires explicit isolated LAYRS_TEST_DATABASE_URL"]
async fn company_organisation_and_personal_options_are_independent() {
    for (multi, personal) in [(false, false), (false, true), (true, false), (true, true)] {
        let options =
            PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .unwrap();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let database = format!("layrs_context_{suffix}");
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
        let mut config = ServerConfig::parse(&BTreeMap::from([
            ("LAYRS_PUBLIC_URL".into(), "https://contexts.example".into()),
            ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
            ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
            ("LAYRS_DEPLOYMENT_ID".into(), "deployment:contexts".into()),
            ("LAYRS_STORE_ID".into(), "store:contexts".into()),
            ("LAYRS_SIGNING_KEY".into(), random_secret()),
            ("LAYRS_BOOTSTRAP_SECRET".into(), random_secret()),
            ("LAYRS_MULTI_ORGANISATION_ENABLED".into(), multi.to_string()),
            (
                "LAYRS_PERSONAL_PROJECTS_ENABLED".into(),
                personal.to_string(),
            ),
        ]))
        .unwrap();
        installation::initialize(&pool, &config).await.unwrap();
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
                password: "Context test password 2026".into(),
                display_name: "Alice".into(),
            })
            .await
            .unwrap();
        let alice_context = load_principal_context(&pool, &alice.principal_id)
            .await
            .unwrap();
        // A normal account asks first: company ownership still belongs to the
        // installation administrator, not the first HTTP caller.
        let bob_id = format!("principal:{}", uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO principals(principal_id,actor_id,deployment_id,principal_kind,subject_digest,display_name) VALUES($1,$2,$3,'account',$4,'Bob')")
            .bind(&bob_id).bind(format!("actor:{}",uuid::Uuid::new_v4())).bind(&config.deployment_id)
            .bind(format!("subject:{}",uuid::Uuid::new_v4())).execute(&pool).await.unwrap();
        let bob_context =
            load_principal_context(&pool, &layrs_sync::v3::PrincipalId::parse(bob_id).unwrap())
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
        let bob_contexts = repo.project_contexts(&bob_context).await.unwrap();
        assert_eq!(bob_contexts.multi_organisation_enabled, multi);
        assert_eq!(bob_contexts.personal_projects_enabled, personal);
        assert!(
            bob_contexts
                .contexts
                .iter()
                .any(|item| item.id == bob_contexts.default_context_id
                    && item.kind == "company"
                    && item.role == "member")
        );
        let company_id = bob_contexts.default_context_id.clone();
        let company_owner: String = sqlx::query_scalar(
            "SELECT created_by_principal_id FROM workspaces WHERE workspace_id=$1",
        )
        .bind(&company_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(company_owner, alice.principal_id.as_str());
        let alice_contexts = repo.project_contexts(&alice_context).await.unwrap();
        assert_eq!(
            alice_contexts
                .contexts
                .iter()
                .find(|item| item.id == company_id)
                .unwrap()
                .role,
            "owner"
        );
        assert_eq!(alice_contexts.can_create_organisation, multi);
        let company_project = repo
            .create_project(
                &bob_context,
                &CreateProjectRequest {
                    context_id: Some(company_id.clone()),
                    workspace_id: None,
                    workspace_slug: None,
                    workspace_name: None,
                    space_slug: "company-project".into(),
                    name: "Company project".into(),
                    lens_configuration: None,
                },
            )
            .await
            .unwrap();
        let capability = PgCollaborationStore::new(repo.clone())
            .space_capabilities(&bob_context, &company_project.space_id)
            .await
            .unwrap();
        assert_eq!(capability.context_id, company_id);
        assert_eq!(capability.context_kind, "company");
        assert!(
            capability.capabilities.manage_members,
            "company project creator can manage own project"
        );
        let company_page = PgCollaborationStore::new(repo.clone())
            .space_catalog(
                &bob_context,
                SpaceCatalogQuery {
                    limit: None,
                    cursor: None,
                    context_id: Some(company_id.clone()),
                },
            )
            .await
            .unwrap();
        assert_eq!(company_page.spaces.len(), 1);
        assert_eq!(company_page.spaces[0].space_id, company_project.space_id);
        if personal {
            // Merely having visited the personal context does not prevent
            // disabling it while it holds no projects.
            config.personal_projects_enabled = false;
            installation::initialize(&pool, &config).await.unwrap();
            config.personal_projects_enabled = true;
            installation::initialize(&pool, &config).await.unwrap();
            let personal_id = bob_contexts
                .contexts
                .iter()
                .find(|item| item.kind == "personal")
                .unwrap()
                .id
                .clone();
            let private = repo
                .create_project(
                    &bob_context,
                    &CreateProjectRequest {
                        context_id: Some(personal_id.clone()),
                        workspace_id: None,
                        workspace_slug: None,
                        workspace_name: None,
                        space_slug: "private".into(),
                        name: "Private".into(),
                        lens_configuration: None,
                    },
                )
                .await
                .unwrap();
            let private_page = PgCollaborationStore::new(repo.clone())
                .space_catalog(
                    &bob_context,
                    SpaceCatalogQuery {
                        limit: None,
                        cursor: None,
                        context_id: Some(personal_id.clone()),
                    },
                )
                .await
                .unwrap();
            assert_eq!(private_page.spaces.len(), 1);
            assert_eq!(private_page.spaces[0].space_id, private.space_id);
            assert!(matches!(
                PgCollaborationStore::new(repo.clone())
                    .space_catalog(
                        &alice_context,
                        SpaceCatalogQuery {
                            limit: None,
                            cursor: None,
                            context_id: Some(personal_id.clone())
                        }
                    )
                    .await,
                Err(V3StoreError::InvalidCapability)
            ));
            assert!(matches!(
                PgCollaborationStore::new(repo.clone())
                    .space_capabilities(&alice_context, &private.space_id)
                    .await,
                Err(V3StoreError::NotFound)
            ));
            let caps = PgCollaborationStore::new(repo.clone())
                .space_capabilities(&bob_context, &private.space_id)
                .await
                .unwrap();
            assert!(
                !caps.capabilities.manage_members
                    && !caps.capabilities.manage_teams
                    && !caps.capabilities.manage_invitations
            );
            // Even a maintenance-level manual membership cannot confer
            // personal access, including if a policy grants the principal.
            sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role) VALUES($1,$2,'member')")
                .bind(&personal_id).bind(alice.principal_id.as_str()).execute(&pool).await.unwrap();
            let authority = PgBootstrapStore::new(
                repo.clone(),
                EventCursorCodec::new(&config.signing_key).unwrap(),
            )
            .load_authority(&private.space_id)
            .await
            .unwrap();
            let decision = repo
                .authorizer()
                .authorize(&AuthorizationCheck {
                    principal: alice_context.clone(),
                    action: V3Action::Bootstrap,
                    target: AuthorizationTarget::Space(private.space_id.clone()),
                    policy_set_id: authority.policy_set_id,
                    authorization_epoch: authority.authorization_epoch,
                })
                .await
                .unwrap();
            assert_eq!(decision.reason_code, "personal_owner_only");
            assert!(!decision.allowed);
            assert!(matches!(
                PgCollaborationStore::new(repo.clone())
                    .space_capabilities(&alice_context, &private.space_id)
                    .await,
                Err(V3StoreError::NotFound)
            ));
            config.personal_projects_enabled = false;
            assert!(
                installation::initialize(&pool, &config).await.is_err(),
                "cannot hide existing private project"
            );
            let still_enabled:bool=sqlx::query_scalar("SELECT personal_projects_enabled FROM layrs_collaboration.deployment_project_contexts WHERE deployment_id=$1")
                .bind(&config.deployment_id).fetch_one(&pool).await.unwrap();
            assert!(still_enabled, "failed reconfiguration rolls back");
            config.personal_projects_enabled = true;
        } else {
            assert!(
                bob_contexts
                    .contexts
                    .iter()
                    .all(|item| item.kind != "personal")
            );
        }
        if multi {
            let org = repo
                .create_organisation(
                    &alice_context,
                    &CreateOrganisationRequest {
                        slug: "design".into(),
                        name: "Design".into(),
                    },
                )
                .await
                .unwrap();
            assert_eq!(org.kind, "organisation");
            assert_eq!(
                repo.create_organisation(
                    &alice_context,
                    &CreateOrganisationRequest {
                        slug: "design".into(),
                        name: "Design".into()
                    }
                )
                .await
                .unwrap()
                .id,
                org.id,
                "exact organisation replay is idempotent"
            );
            assert!(matches!(
                repo.create_project(
                    &bob_context,
                    &CreateProjectRequest {
                        context_id: Some(org.id.clone()),
                        workspace_id: None,
                        workspace_slug: None,
                        workspace_name: None,
                        space_slug: "blocked".into(),
                        name: "Blocked".into(),
                        lens_configuration: None
                    }
                )
                .await,
                Err(V3StoreError::InvalidCapability)
            ));
            assert!(matches!(
                repo.create_organisation(
                    &bob_context,
                    &CreateOrganisationRequest {
                        slug: "unauthorized".into(),
                        name: "Unauthorized".into()
                    }
                )
                .await,
                Err(V3StoreError::InvalidCapability)
            ));
        } else {
            assert!(matches!(
                repo.create_organisation(
                    &alice_context,
                    &CreateOrganisationRequest {
                        slug: "design".into(),
                        name: "Design".into()
                    }
                )
                .await,
                Err(V3StoreError::InvalidCapability)
            ));
        }
        pool.close().await;
        admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires explicit isolated LAYRS_TEST_DATABASE_URL"]
async fn legacy_workspaces_keep_their_memberships_and_project_authority() {
    for count in [1, 2] {
        let options =
            PgConnectOptions::from_str(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .unwrap();
        let database = format!("layrs_context_upgrade_{}", uuid::Uuid::new_v4().simple());
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
        let mut config = ServerConfig::parse(&BTreeMap::from([
            ("LAYRS_PUBLIC_URL".into(), "https://contexts.example".into()),
            ("LAYRS_DATABASE_URL".into(), "postgres://unused".into()),
            ("LAYRS_CAS_ROOT".into(), root.display().to_string()),
            (
                "LAYRS_DEPLOYMENT_ID".into(),
                "deployment:contexts-upgrade".into(),
            ),
            ("LAYRS_STORE_ID".into(), "store:contexts-upgrade".into()),
            ("LAYRS_SIGNING_KEY".into(), random_secret()),
            ("LAYRS_BOOTSTRAP_SECRET".into(), random_secret()),
            ("LAYRS_MULTI_ORGANISATION_ENABLED".into(), "true".into()),
        ]))
        .unwrap();
        installation::initialize(&pool, &config).await.unwrap();
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
                password: "Context upgrade password 2026".into(),
                display_name: "Alice".into(),
            })
            .await
            .unwrap();
        let principal = load_principal_context(&pool, &alice.principal_id)
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
        let mut legacy = Vec::new();
        for index in 0..count {
            let slug = if index == 0 {
                let digest = blake3::hash(format!("{}/company", config.deployment_id).as_bytes())
                    .to_hex()
                    .to_string();
                format!("company-{}", &digest[..32])
            } else {
                format!("legacy-{index}")
            };
            let org = repo
                .create_organisation(
                    &principal,
                    &CreateOrganisationRequest {
                        slug,
                        name: format!("Legacy {index}"),
                    },
                )
                .await
                .unwrap();
            let project = repo
                .create_project(
                    &principal,
                    &CreateProjectRequest {
                        context_id: Some(org.id.clone()),
                        workspace_id: None,
                        workspace_slug: None,
                        workspace_name: None,
                        space_slug: format!("project-{index}"),
                        name: format!("Project {index}"),
                        lens_configuration: None,
                    },
                )
                .await
                .unwrap();
            let authority: (String, i64) = sqlx::query_as(
                "SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1",
            )
            .bind(project.space_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
            let member:(String,String)=sqlx::query_as("SELECT role,state FROM workspace_memberships WHERE workspace_id=$1 AND principal_id=$2")
                .bind(&org.id).bind(alice.principal_id.as_str()).fetch_one(&pool).await.unwrap();
            // A legacy workspace predates metadata and is classified as an
            // organisation by the absence of this extension row.
            sqlx::query("DELETE FROM layrs_collaboration.workspace_contexts WHERE workspace_id=$1")
                .bind(&org.id)
                .execute(&pool)
                .await
                .unwrap();
            legacy.push((org.id, project.space_id, authority, member));
        }
        let company_id:String=sqlx::query_scalar("SELECT workspace_id FROM layrs_collaboration.workspace_contexts WHERE deployment_id=$1 AND kind='company'")
            .bind(&config.deployment_id).fetch_one(&pool).await.unwrap();
        sqlx::query("DELETE FROM workspaces WHERE workspace_id=$1 AND NOT EXISTS(SELECT 1 FROM spaces WHERE workspace_id=$1)")
            .bind(&company_id).execute(&pool).await.unwrap();
        config.multi_organisation_enabled = false;
        assert!(
            installation::initialize(&pool, &config).await.is_err(),
            "legacy organisations require the multi-organisation option even when only one exists"
        );
        let still_multi:bool=sqlx::query_scalar("SELECT multi_organisation_enabled FROM layrs_collaboration.deployment_project_contexts WHERE deployment_id=$1")
            .bind(&config.deployment_id).fetch_one(&pool).await.unwrap();
        assert!(still_multi, "failed reconfiguration rolls back");
        config.multi_organisation_enabled = true;
        installation::initialize(&pool, &config).await.unwrap();
        let contexts = repo.project_contexts(&principal).await.unwrap();
        assert_eq!(
            contexts
                .contexts
                .iter()
                .filter(|context| context.kind == "organisation")
                .count(),
            count
        );
        assert!(
            contexts
                .contexts
                .iter()
                .any(|context| context.kind == "company")
        );
        assert!(
            legacy
                .iter()
                .all(|(id, _, _, _)| id != &contexts.default_context_id),
            "fresh company is separate"
        );
        let company_slug: String =
            sqlx::query_scalar("SELECT slug FROM workspaces WHERE workspace_id=$1")
                .bind(&contexts.default_context_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(company_slug.starts_with("company-"));
        assert!(
            company_slug.ends_with("-1"),
            "colliding legacy slug is preserved and company chooses a free suffix"
        );
        for (id, space, authority, member) in legacy {
            assert!(
                contexts
                    .contexts
                    .iter()
                    .any(|context| context.id == id && context.kind == "organisation")
            );
            assert_eq!(
                sqlx::query_as::<_, (String, i64)>(
                    "SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1"
                )
                .bind(space.as_str())
                .fetch_one(&pool)
                .await
                .unwrap(),
                authority
            );
            assert_eq!(sqlx::query_as::<_,(String,String)>("SELECT role,state FROM workspace_memberships WHERE workspace_id=$1 AND principal_id=$2")
                .bind(&id).bind(alice.principal_id.as_str()).fetch_one(&pool).await.unwrap(),member);
        }
        pool.close().await;
        admin.close().await;
    }
}
