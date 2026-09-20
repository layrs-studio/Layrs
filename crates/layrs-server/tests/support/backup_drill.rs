//! Real pg_dump exported-snapshot/CAS/pg_restore exercise, using the fixture's Docker PG.
use layrs_server::{
    backup::{self, BackupResult, PgBackupTools},
    config::ServerConfig,
};
use layrs_sync::v3::ObjectKind;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    fs::File,
    path::Path,
    process::{Command, Stdio},
    str::FromStr,
    sync::Arc,
};

struct DockerPgTools;
impl DockerPgTools {
    fn command(&self, url: &str, program: &str) -> BackupResult<Command> {
        let options = PgConnectOptions::from_str(url)?;
        let mut command = Command::new("docker");
        if let Ok(config) = std::env::var("LAYRS_TEST_DOCKER_CONFIG") {
            command.args(["--config", &config]);
        }
        if let Ok(host) = std::env::var("LAYRS_TEST_DOCKER_HOST") {
            command.args(["-H", &host]);
        }
        command.args([
            "exec",
            "-i",
            "-e",
            "PGUSER=postgres",
            "-e",
            &format!(
                "PGDATABASE={}",
                options.get_database().ok_or("missing test database")?
            ),
            &std::env::var("LAYRS_TEST_PG_CONTAINER")?,
            program,
        ]);
        Ok(command)
    }
}
impl PgBackupTools for DockerPgTools {
    fn dump(&self, url: &str, snapshot: &str, path: &Path) -> BackupResult<()> {
        let output = File::create_new(path)?;
        let status = self
            .command(url, "pg_dump")?
            .args(["--format=custom", "--no-owner", "--snapshot", snapshot])
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            return Err("container pg_dump failed".into());
        }
        Ok(())
    }
    fn restore(&self, url: &str, path: &Path) -> BackupResult<()> {
        let status = self
            .command(url, "pg_restore")?
            .args([
                "--no-owner",
                "--exit-on-error",
                "--single-transaction",
                "--dbname",
                "",
            ])
            .stdin(File::open(path)?)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            return Err("container pg_restore failed".into());
        }
        Ok(())
    }
}
fn config_for(original: &ServerConfig, url: String, root: std::path::PathBuf) -> ServerConfig {
    ServerConfig {
        addr: original.addr,
        public_url: original.public_url.clone(),
        database_url: url,
        cas_root: root,
        deployment_id: original.deployment_id.clone(),
        store_id: original.store_id.clone(),
        signing_key: original.signing_key.clone(),
        bootstrap_secret: None,
        cas_quotas: original.cas_quotas,
    }
}

pub async fn run(source: &PgPool, admin: &PgPool, config: &ServerConfig, source_db: &str) {
    let started = std::time::Instant::now();
    let mut url = url::Url::parse(&std::env::var("LAYRS_TEST_DATABASE_URL").unwrap()).unwrap();
    url.set_path(source_db);
    let source_config = config_for(config, url.to_string(), config.cas_root.clone());
    let archive =
        std::env::temp_dir().join(format!("layrs_backup_{}", uuid::Uuid::new_v4().simple()));
    let tools: Arc<dyn PgBackupTools> = Arc::new(DockerPgTools);
    let backup_started = std::time::Instant::now();
    let manifest = backup::create_backup(source, &source_config, &archive, tools.clone())
        .await
        .unwrap();
    let backup_ms = backup_started.elapsed().as_millis();
    assert!(
        manifest.objects.len() > 10,
        "nonempty project with files and history must be backed up"
    );
    let chunk = manifest
        .objects
        .iter()
        .find(|o| o.kind == ObjectKind::Chunk)
        .unwrap();
    let hex = &chunk.object_id.as_str()[7..];
    let chunk_path = archive.join("cas/objects/chunk").join(&hex[..2]).join(hex);
    let original = std::fs::read(&chunk_path).unwrap();
    let mut corrupt = original.clone();
    corrupt[0] ^= 1;
    std::fs::write(&chunk_path, &corrupt).unwrap();
    assert!(
        backup::verify_backup(&archive).await.is_err(),
        "corrupt CAS must fail verification"
    );
    std::fs::write(&chunk_path, &original).unwrap();
    let hidden = archive.join("temporarily-missing-chunk");
    std::fs::rename(&chunk_path, &hidden).unwrap();
    assert!(
        backup::verify_backup(&archive).await.is_err(),
        "missing CAS must fail verification"
    );
    std::fs::rename(&hidden, &chunk_path).unwrap();
    let original_manifest = std::fs::read(archive.join("manifest.json")).unwrap();
    let mut incomplete: backup::BackupManifest =
        serde_json::from_slice(&original_manifest).unwrap();
    incomplete
        .objects
        .retain(|o| o.object_id != chunk.object_id);
    std::fs::write(
        archive.join("manifest.json"),
        serde_jcs::to_vec(&incomplete).unwrap(),
    )
    .unwrap();
    assert!(
        backup::verify_backup(&archive).await.is_err(),
        "incomplete file closure must fail verification"
    );
    std::fs::write(archive.join("manifest.json"), &original_manifest).unwrap();
    incomplete = serde_json::from_slice(&original_manifest).unwrap();
    incomplete.cas_completed_unix = incomplete.database_watermark_unix - 1;
    std::fs::write(
        archive.join("manifest.json"),
        serde_jcs::to_vec(&incomplete).unwrap(),
    )
    .unwrap();
    assert!(
        backup::verify_backup(&archive).await.is_err(),
        "older CAS snapshot must fail verification"
    );
    std::fs::write(archive.join("manifest.json"), &original_manifest).unwrap();
    backup::verify_backup(&archive).await.unwrap();
    let target_db = format!("layrs_restore_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {target_db}"))
        .execute(admin)
        .await
        .unwrap();
    url.set_path(&target_db);
    let options = PgConnectOptions::from_str(url.as_str()).unwrap();
    let target = PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    let target_root = std::env::temp_dir().join(&target_db);
    let target_config = config_for(config, url.to_string(), target_root.clone());
    let restore_started = std::time::Instant::now();
    backup::restore_backup(&target, &target_config, &archive, tools)
        .await
        .unwrap();
    let restore_ms = restore_started.elapsed().as_millis();
    assert!(!target_root.join(backup::RESTORE_MARKER).exists());
    let expected:Vec<(String,i64)>=sqlx::query_as("SELECT current_repository_view_id,current_repository_view_generation FROM spaces ORDER BY space_id").fetch_all(source).await.unwrap();
    let actual:Vec<(String,i64)>=sqlx::query_as("SELECT current_repository_view_id,current_repository_view_generation FROM spaces ORDER BY space_id").fetch_all(&target).await.unwrap();
    assert_eq!(
        actual, expected,
        "history pointers and generations must survive restoration"
    );
    let expected_steps: i64 = sqlx::query_scalar("SELECT count(*) FROM steps")
        .fetch_one(source)
        .await
        .unwrap();
    let actual_steps: i64 = sqlx::query_scalar("SELECT count(*) FROM steps")
        .fetch_one(&target)
        .await
        .unwrap();
    assert!(actual_steps > 1);
    assert_eq!(actual_steps, expected_steps);
    let release_events: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT event_object_id,release_id,canonical_event_jcs FROM layrs_collaboration.release_events ORDER BY event_object_id"
    ).fetch_all(source).await.unwrap();
    let restored_events: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT event_object_id,release_id,canonical_event_jcs FROM layrs_collaboration.release_events ORDER BY event_object_id"
    ).fetch_all(&target).await.unwrap();
    assert_eq!(
        restored_events, release_events,
        "immutable release lifecycle survives restoration"
    );
    for (object_id, _, _) in &release_events {
        assert!(
            manifest
                .objects
                .iter()
                .any(|object| object.object_id.as_str() == object_id),
            "release lifecycle Evidence must be in the retained CAS manifest"
        );
    }
    let cancellations:Vec<(String,String,String)>=sqlx::query_as("SELECT run_id,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.run_cancellations ORDER BY run_id").fetch_all(source).await.unwrap();
    let restored_cancellations:Vec<(String,String,String)>=sqlx::query_as("SELECT run_id,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.run_cancellations ORDER BY run_id").fetch_all(&target).await.unwrap();
    assert_eq!(
        restored_cancellations, cancellations,
        "immutable cancellation records must survive restoration"
    );
    for (run, object, _) in &cancellations {
        assert!(
            manifest
                .objects
                .iter()
                .any(|item| item.object_id.as_str() == object),
            "cancellation Evidence must be retained in CAS"
        );
        let no_verdict:bool=sqlx::query_scalar("SELECT proof_object_id IS NULL AND outcome IS NULL AND completed_at IS NULL FROM layrs_collaboration.runs WHERE run_id=$1").bind(run).fetch_one(&target).await.unwrap();
        assert!(
            no_verdict,
            "restore must not transform cancellation into a verdict"
        );
    }
    let observation_query = "SELECT row_to_json(record)::text FROM (SELECT p.*,o.canonical_record_jcs FROM layrs_collaboration.published_step_observations p JOIN step_observations o ON o.observation_id=p.observation_id ORDER BY p.space_id,p.ordinal) record";
    let observations: Vec<String> = sqlx::query_scalar(observation_query)
        .fetch_all(source)
        .await
        .unwrap();
    let restored_observations: Vec<String> = sqlx::query_scalar(observation_query)
        .fetch_all(&target)
        .await
        .unwrap();
    assert_eq!(
        restored_observations, observations,
        "restore preserves exact immutable observation claims, provenance, receive dates and per-Space order"
    );
    restored_observation_pages(source, &target, config, &target_config).await;
    restored_policy_management(source, &target, config, &target_config).await;
    let corrections:Vec<(String,String,String)>=sqlx::query_as("SELECT correction_id,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.comment_corrections ORDER BY correction_id").fetch_all(source).await.unwrap();
    let execution_query = "SELECT run_id,execution_requested,execution_started_at::text,proof_object_id FROM layrs_collaboration.runs ORDER BY run_id";
    let execution: Vec<(String, bool, Option<String>, Option<String>)> =
        sqlx::query_as(execution_query)
            .fetch_all(source)
            .await
            .unwrap();
    let restored_execution: Vec<(String, bool, Option<String>, Option<String>)> =
        sqlx::query_as(execution_query)
            .fetch_all(&target)
            .await
            .unwrap();
    assert_eq!(
        restored_execution, execution,
        "restore must preserve pending versus declared running and completed attempts"
    );
    let restored_corrections:Vec<(String,String,String)>=sqlx::query_as("SELECT correction_id,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.comment_corrections ORDER BY correction_id").fetch_all(&target).await.unwrap();
    assert_eq!(
        restored_corrections, corrections,
        "comment correction audit trail must survive restoration exactly"
    );
    for (_, object, raw) in &corrections {
        assert!(
            manifest
                .objects
                .iter()
                .any(|item| item.object_id.as_str() == object)
        );
        let record: serde_json::Value = serde_json::from_str(raw).unwrap();
        let original = record["originalActionObjectId"].as_str().unwrap();
        assert!(
            manifest
                .objects
                .iter()
                .any(|item| item.object_id.as_str() == original),
            "original comment remains available alongside its revisions"
        );
    }
    let source_actions:Vec<(String,String,String)>=sqlx::query_as("SELECT action_id,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.review_actions ORDER BY action_id").fetch_all(source).await.unwrap();
    let team_versions:Vec<(String,i32,String,String)>=sqlx::query_as("SELECT team_id,version,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.team_versions ORDER BY team_id,version").fetch_all(source).await.unwrap();
    let restored_teams:Vec<(String,i32,String,String)>=sqlx::query_as("SELECT team_id,version,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.team_versions ORDER BY team_id,version").fetch_all(&target).await.unwrap();
    assert_eq!(
        restored_teams, team_versions,
        "all immutable Team versions survive restoration"
    );
    for (_, _, object, raw) in &team_versions {
        let record: serde_json::Value = serde_json::from_str(raw).unwrap();
        for dependency in [object.as_str(), record["policySetId"].as_str().unwrap()] {
            assert!(
                manifest
                    .objects
                    .iter()
                    .any(|item| item.object_id.as_str() == dependency),
                "Team event and policy roots retained in CAS"
            );
        }
    }
    let team_heads:Vec<(String,String,i32)>=sqlx::query_as("SELECT team_id,space_id,current_version FROM layrs_collaboration.space_teams ORDER BY team_id").fetch_all(source).await.unwrap();
    let restored_heads:Vec<(String,String,i32)>=sqlx::query_as("SELECT team_id,space_id,current_version FROM layrs_collaboration.space_teams ORDER BY team_id").fetch_all(&target).await.unwrap();
    assert_eq!(restored_heads, team_heads);
    let team_members:Vec<(String,String,String)>=sqlx::query_as("SELECT team_id,space_id,principal_id FROM layrs_collaboration.team_members ORDER BY team_id,principal_id").fetch_all(source).await.unwrap();
    let restored_members:Vec<(String,String,String)>=sqlx::query_as("SELECT team_id,space_id,principal_id FROM layrs_collaboration.team_members ORDER BY team_id,principal_id").fetch_all(&target).await.unwrap();
    assert_eq!(
        restored_members, team_members,
        "current exact Team memberships survive restoration"
    );
    team_authorizations(source, &target, config, &target_config, &team_members).await;
    let target_actions:Vec<(String,String,String)>=sqlx::query_as("SELECT action_id,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.review_actions ORDER BY action_id").fetch_all(&target).await.unwrap();
    assert_eq!(
        target_actions, source_actions,
        "restoration does not rewrite original comments"
    );
    let restored_chunk = target_root.join("objects/chunk").join(&hex[..2]).join(hex);
    assert_eq!(std::fs::read(restored_chunk).unwrap(), original);
    assert!(
        backup::restore_backup(&target, &target_config, &archive, Arc::new(DockerPgTools))
            .await
            .is_err(),
        "restoration refuses nonempty target"
    );
    target.close().await;
    // SQLx background connection shutdown can lag close() under concurrent
    // suites. This database was generated by this drill, never supplied by users.
    assert!(
        target_db.starts_with("layrs_restore_test_")
            && target_db
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    );
    sqlx::query(&format!("DROP DATABASE {target_db} WITH (FORCE)"))
        .execute(admin)
        .await
        .unwrap();
    let temp = std::env::temp_dir();
    assert!(target_root.starts_with(&temp) && archive.starts_with(&temp));
    std::fs::remove_dir_all(target_root).unwrap();
    std::fs::remove_dir_all(archive).unwrap();
    eprintln!(
        "Backup measurement: objects={}, canonical_bytes={}, backup_ms={}, restore_ms={}, cas_copy_window_seconds={}",
        manifest.objects.len(),
        manifest
            .objects
            .iter()
            .map(|object| object.canonical_byte_len)
            .sum::<u64>(),
        backup_ms,
        restore_ms,
        manifest
            .cas_completed_unix
            .saturating_sub(manifest.database_watermark_unix)
    );
    eprintln!(
        "Backup/restore drill: {} objects, {} Steps, {} release events, {} cancelled runs, {} comment corrections, {} ms",
        manifest.objects.len(),
        actual_steps,
        release_events.len(),
        cancellations.len(),
        corrections.len(),
        started.elapsed().as_millis()
    );
}

async fn team_authorizations(
    source: &PgPool,
    target: &PgPool,
    source_config: &ServerConfig,
    target_config: &ServerConfig,
    members: &[(String, String, String)],
) {
    use layrs_core::scm::{ActorId, ObjectId, SpaceId};
    use layrs_store_server::v3::{
        AuthorizationCheck, AuthorizationTarget, CasLimits, CatalogedFsCas, FsServerCas,
        PgPolicyAuthorizer, PolicyAuthorizer, PrincipalContext, V3Action,
    };
    use layrs_sync::v3::PrincipalId;
    let mut decisions = Vec::new();
    for (pool, config) in [(source, source_config), (target, target_config)] {
        let cas = Arc::new(CatalogedFsCas::new(
            config.store_id.clone(),
            FsServerCas::open(&config.cas_root, CasLimits::default())
                .await
                .unwrap(),
        ));
        let authorizer = PgPolicyAuthorizer::new(pool.clone(), cas);
        let mut current = Vec::new();
        for (_, space, member) in members {
            let (actor,deployment,subject):(String,String,String)=sqlx::query_as("SELECT actor_id,deployment_id,subject_digest FROM principals WHERE principal_id=$1").bind(member).fetch_one(pool).await.unwrap();
            let (policy, epoch): (String, i64) = sqlx::query_as(
                "SELECT current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1",
            )
            .bind(space)
            .fetch_one(pool)
            .await
            .unwrap();
            for action in [
                V3Action::Bootstrap,
                V3Action::ApplyTransaction,
                V3Action::AdministerPolicy,
            ] {
                let check = AuthorizationCheck {
                    principal: PrincipalContext::new(
                        PrincipalId::parse(member).unwrap(),
                        ActorId::parse(&actor).unwrap(),
                        deployment.clone(),
                        subject.clone(),
                    )
                    .unwrap(),
                    action,
                    target: AuthorizationTarget::Space(SpaceId::parse(space).unwrap()),
                    policy_set_id: ObjectId::parse(&policy).unwrap(),
                    authorization_epoch: epoch as u64,
                };
                current.push(authorizer.authorize(&check).await.unwrap().allowed);
            }
        }
        decisions.push(current);
    }
    assert_eq!(
        decisions[0], decisions[1],
        "restored Team policy authorization reads and verifies restored CAS, not only SQL projections"
    );
    if !members.is_empty() {
        assert_eq!(&decisions[1][..3], &[true, true, false]);
    }
}

async fn restored_observation_pages(
    source: &PgPool,
    target: &PgPool,
    source_config: &ServerConfig,
    target_config: &ServerConfig,
) {
    use layrs_core::scm::{ActorId, SpaceId, StepId};
    use layrs_store_server::v3::{
        CapabilityCodec, CasLimits, CatalogedFsCas, EventCursorCodec, FsServerCas,
        PgBootstrapStore, PgPolicyAuthorizer, PgV3Repository, PrincipalContext, StepHistoryRequest,
    };
    use layrs_sync::v3::PrincipalId;
    let candidate:Option<(String,String,String,String,String,String)>=sqlx::query_as("SELECT o.space_id,o.step_id,p.principal_id,p.actor_id,p.deployment_id,p.subject_digest FROM layrs_collaboration.published_step_observations o JOIN spaces s ON s.space_id=o.space_id JOIN workspace_memberships m ON m.workspace_id=s.workspace_id AND m.state='active' AND m.role='owner' JOIN principals p ON p.principal_id=m.principal_id AND p.status='active' ORDER BY (o.provenance='client_assertion') DESC,o.space_id,o.ordinal DESC LIMIT 1").fetch_optional(source).await.unwrap();
    let Some((space, step, principal, actor, deployment, subject)) = candidate else {
        return;
    };
    let space = SpaceId::parse(space).unwrap();
    let anchor = StepId::parse(step).unwrap();
    let has_assertions: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.published_step_observations WHERE provenance='client_assertion')").fetch_one(source).await.unwrap();
    let principal = PrincipalContext::new(
        PrincipalId::parse(principal).unwrap(),
        ActorId::parse(actor).unwrap(),
        deployment,
        subject,
    )
    .unwrap();
    let mut pages = Vec::new();
    let mut continuation = None;
    for (pool, config) in [(source, source_config), (target, target_config)] {
        let cas = Arc::new(CatalogedFsCas::new(
            config.store_id.clone(),
            FsServerCas::open(&config.cas_root, CasLimits::default())
                .await
                .unwrap(),
        ));
        let repository = PgV3Repository::new(
            pool.clone(),
            cas.clone(),
            CapabilityCodec::new(&config.signing_key).unwrap(),
            Arc::new(PgPolicyAuthorizer::new(pool.clone(), cas)),
        );
        let authority = PgBootstrapStore::new(
            repository.clone(),
            EventCursorCodec::new(&config.signing_key).unwrap(),
        )
        .load_authority(&space)
        .await
        .unwrap();
        let page = repository
            .step_history(
                &principal,
                &authority,
                StepHistoryRequest {
                    anchor: anchor.clone(),
                    cursor: None,
                    limit: 1,
                    path: None,
                    filter: Default::default(),
                },
            )
            .await
            .unwrap();
        assert!(!page.observations.is_empty());
        if has_assertions {
            let claims: Vec<_> = page
                .observations
                .values()
                .flatten()
                .filter(|o| {
                    o.provenance
                        == layrs_store_server::v3::PublishedObservationProvenance::ClientAssertion
                })
                .collect();
            assert!(
                !claims.is_empty(),
                "restored history must reopen actual client provenance"
            );
            assert!(
                claims
                    .iter()
                    .all(|o| o.observation.observed_at() != &o.received_at)
            );
        }
        if pages.is_empty() {
            continuation = page.next.clone();
        }
        let next = if let Some(cursor) = &continuation {
            Some(
                repository
                    .step_history(
                        &principal,
                        &authority,
                        StepHistoryRequest {
                            anchor: anchor.clone(),
                            cursor: Some(cursor.clone()),
                            limit: 1,
                            path: None,
                            filter: Default::default(),
                        },
                    )
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        pages.push(serde_json::to_vec(&(page, next)).unwrap());
    }
    assert_eq!(
        pages[0], pages[1],
        "restored real policy/CAS history reproduces observation snapshot and accepts its exact original cursor"
    );
}

async fn restored_policy_management(
    source: &PgPool,
    target: &PgPool,
    source_config: &ServerConfig,
    target_config: &ServerConfig,
) {
    use layrs_core::scm::{ActorId, SpaceId};
    use layrs_store_server::v3::{
        CapabilityCodec, CasLimits, CatalogedFsCas, FsServerCas, PgCollaborationStore,
        PgPolicyAuthorizer, PgV3Repository, PrincipalContext,
    };
    use layrs_sync::v3::PrincipalId;
    let rows: Vec<(String,String,String,String,String)> = sqlx::query_as("SELECT DISTINCT t.space_id,p.principal_id,p.actor_id,p.deployment_id,p.subject_digest FROM sync_transactions t JOIN spaces s ON s.space_id=t.space_id JOIN workspace_memberships m ON m.workspace_id=s.workspace_id AND m.state='active' AND m.role='owner' JOIN principals p ON p.principal_id=m.principal_id AND p.status='active' WHERE t.resulting_authorization_epoch<>t.authorization_epoch ORDER BY t.space_id").fetch_all(source).await.unwrap();
    if rows.is_empty() {
        return;
    }
    let mut reconstructed = Vec::new();
    for (pool, config) in [(source, source_config), (target, target_config)] {
        let cas = Arc::new(CatalogedFsCas::new(
            config.store_id.clone(),
            FsServerCas::open(&config.cas_root, CasLimits::default())
                .await
                .unwrap(),
        ));
        let repo = PgV3Repository::new(
            pool.clone(),
            cas.clone(),
            CapabilityCodec::new(&config.signing_key).unwrap(),
            Arc::new(PgPolicyAuthorizer::new(pool.clone(), cas)),
        );
        let service = PgCollaborationStore::new(repo);
        let mut states = Vec::new();
        for (space, principal, actor, deployment, subject) in &rows {
            let principal = PrincipalContext::new(
                PrincipalId::parse(principal).unwrap(),
                ActorId::parse(actor).unwrap(),
                deployment.clone(),
                subject.clone(),
            )
            .unwrap();
            states.push(
                service
                    .policy_management(&principal, &SpaceId::parse(space).unwrap())
                    .await
                    .unwrap(),
            );
        }
        assert!(
            states.iter().any(|state| state.mode == "custom"),
            "fixture must exercise persisted custom mode"
        );
        reconstructed.push(serde_json::to_vec(&states).unwrap());
    }
    assert_eq!(
        reconstructed[0], reconstructed[1],
        "real restored CAS policies, immutable receipts and events reconstruct exact managed/custom modes"
    );
}
