//! Workspace-owned directory teams and explicit per-project grants.
//! Existing space_teams remain independent; no migration broadens their grants.
use super::*;
use layrs_sync::v3::PrincipalId;
use serde::Deserialize;
use sqlx::postgres::PgRow;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganisationTeamRequest {
    pub idempotency_key: String,
    pub expected_version: u32,
    pub name: String,
    pub member_principal_ids: Vec<PrincipalId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganisationTeamAssignmentRequest {
    pub idempotency_key: String,
    pub expected_version: u32,
    pub role: Option<TeamRole>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DirectoryRecord {
    team_id: String,
    workspace_id: String,
    version: u32,
    name: String,
    member_principal_ids: Vec<PrincipalId>,
    state: String,
    changed_by: PrincipalId,
    updated_at: Timestamp,
    reason: Option<String>,
}

fn invalid(code: &str) -> V3StoreError {
    V3StoreError::InvalidDescriptor(code.to_owned())
}

fn team_id(id: &str) -> V3StoreResult<()> {
    if id.len() != 73
        || !id.starts_with("org-team:")
        || !id[9..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("invalid_organisation_team_id"));
    }
    Ok(())
}

fn team_role(role: TeamRole) -> &'static str {
    match role {
        TeamRole::Member => "member",
        TeamRole::Auditor => "auditor",
        TeamRole::ReleaseManager => "release_manager",
    }
}

fn parse_role(value: &str) -> V3StoreResult<TeamRole> {
    match value {
        "member" => Ok(TeamRole::Member),
        "auditor" => Ok(TeamRole::Auditor),
        "release_manager" => Ok(TeamRole::ReleaseManager),
        _ => Err(invalid("invalid_organisation_team_role")),
    }
}

fn mutation_id(principal: &PrincipalContext, workspace: &str, key: &str) -> String {
    format!(
        "org-team-mutation:{}",
        blake3::hash(format!("{}\0{workspace}\0{key}", principal.principal_id).as_bytes()).to_hex()
    )
}

fn assignment_mutation_id(principal: &PrincipalContext, space: &SpaceId, key: &str) -> String {
    format!(
        "org-team-assignment:{}",
        blake3::hash(format!("{}\0{}\0{key}", principal.principal_id, space).as_bytes()).to_hex()
    )
}

fn validate_name_members(request: &mut OrganisationTeamRequest) -> V3StoreResult<()> {
    super::types::validate_key(&request.idempotency_key)?;
    request.name = request.name.trim().to_owned();
    request
        .member_principal_ids
        .sort_by(|a, b| a.as_str().cmp(b.as_str()));
    if request.name.is_empty()
        || request.name.len() > 128
        || request.name.chars().any(char::is_control)
        || request.member_principal_ids.len() > 64
        // Version 1000 is reserved for archival so a busy team remains revocable.
        || request.expected_version >= 999
        || request
            .member_principal_ids
            .windows(2)
            .any(|pair| pair[0] == pair[1])
    {
        return Err(invalid("invalid_organisation_team_request"));
    }
    Ok(())
}

fn parse_record(row: &PgRow) -> V3StoreResult<DirectoryRecord> {
    serde_json::from_str(&row.get::<String, _>("canonical_document_jcs")).map_err(bad)
}

impl PgCollaborationStore {
    async fn organisation_scope(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        space: &SpaceId,
        directory_admin: bool,
    ) -> V3StoreResult<String> {
        super::super::contexts::require_visible_space(tx, principal, space).await?;
        let row = sqlx::query("SELECT s.workspace_id,s.created_by_principal_id,COALESCE(c.kind,'organisation') AS kind,m.role,m.state,p.principal_kind FROM spaces s JOIN workspaces w USING(workspace_id) LEFT JOIN layrs_collaboration.workspace_contexts c ON c.workspace_id=w.workspace_id JOIN workspace_memberships m ON m.workspace_id=w.workspace_id AND m.principal_id=$2 JOIN principals p ON p.principal_id=m.principal_id WHERE s.space_id=$1 AND s.lifecycle_state='ready' AND w.deployment_id=$3 FOR SHARE OF m,p")
            .bind(space.as_str()).bind(principal.principal_id.as_str()).bind(&principal.deployment_id)
            .fetch_optional(&mut **tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        if row.get::<String, _>("kind") == "personal"
            || !matches!(
                row.get::<String, _>("principal_kind").as_str(),
                "account" | "bootstrap_admin"
            )
            || row.get::<String, _>("state") != "active"
            || !(super::super::space_access::administers_workspace(&row.get::<String, _>("role"))
                || (!directory_admin
                    && row.get::<String, _>("created_by_principal_id")
                        == principal.principal_id.as_str()))
        {
            return Err(V3StoreError::InvalidCapability);
        }
        Ok(row.get("workspace_id"))
    }

    async fn validate_directory_members(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &str,
        members: &[PrincipalId],
    ) -> V3StoreResult<()> {
        let ids: Vec<&str> = members.iter().map(|id| id.as_str()).collect();
        let rows = sqlx::query("SELECT p.principal_id,p.status,p.principal_kind,m.state FROM workspace_memberships m JOIN principals p USING(principal_id) WHERE m.workspace_id=$1 AND p.principal_id=ANY($2) ORDER BY p.principal_id FOR SHARE OF p,m")
            .bind(workspace).bind(ids).fetch_all(&mut **tx).await.map_err(pg_error)?;
        if rows.len() != members.len()
            || rows.iter().any(|row| {
                row.get::<String, _>("status") != "active"
                    || row.get::<String, _>("state") != "active"
                    || !matches!(
                        row.get::<String, _>("principal_kind").as_str(),
                        "account" | "bootstrap_admin"
                    )
            })
        {
            return Err(invalid("organisation_team_member_not_active"));
        }
        Ok(())
    }

    async fn lock_workspace_mutations(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &str,
    ) -> V3StoreResult<()> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("layrs-org-teams:{workspace}"))
            .execute(&mut **tx)
            .await
            .map_err(pg_error)?;
        Ok(())
    }

    async fn apply_directory_policy(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        workspace: &str,
        space: &SpaceId,
        team: &str,
        grant: (TeamRole, &[PrincipalId]),
    ) -> V3StoreResult<(SpaceAuthority, ObjectId)> {
        let (role, members) = grant;
        let authority = self.current_authority(space).await?;
        let row = sqlx::query("SELECT workspace_id,current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 AND lifecycle_state='ready' FOR UPDATE")
            .bind(space.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        if row.get::<String, _>("workspace_id") != workspace {
            return Err(V3StoreError::InvalidCapability);
        }
        if row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let (policy_set_id, _) = self
            .install_team_policy(tx, &authority, team, role, members)
            .await?;
        self.notify_directory_change(
            tx,
            principal,
            space,
            team,
            &policy_set_id,
            authority.authorization_epoch + 1,
        )
        .await?;
        Ok((authority, policy_set_id))
    }

    async fn notify_directory_change(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        space: &SpaceId,
        team: &str,
        policy: &ObjectId,
        epoch: u64,
    ) -> V3StoreResult<()> {
        let evidence=self.evidence(&serde_json::json!({"schemaVersion":"layrs.organisation-team-policy.v1","teamId":team,"spaceId":space,"policySetId":policy,"authorizationEpoch":epoch})).await?;
        self.install(tx, space, &evidence).await?;
        self.notify(
            tx,
            principal,
            space,
            "team.changed",
            team,
            &evidence.descriptor.object_id,
        )
        .await
    }

    pub async fn list_organisation_teams(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let workspace = self
            .organisation_scope(&mut tx, principal, space, false)
            .await?;
        let role: String = sqlx::query_scalar(
            "SELECT role FROM workspace_memberships WHERE workspace_id=$1 AND principal_id=$2",
        )
        .bind(&workspace)
        .bind(principal.principal_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(pg_error)?;
        let can_manage_directory = super::super::space_access::administers_workspace(&role);
        let rows = sqlx::query("SELECT v.canonical_document_jcs,a.current_version AS assignment_version,a.role AS assignment_role FROM layrs_collaboration.organisation_teams t JOIN layrs_collaboration.organisation_team_versions v ON v.team_id=t.team_id AND v.version=t.current_version LEFT JOIN layrs_collaboration.organisation_team_assignments a ON a.team_id=t.team_id AND a.space_id=$2 WHERE t.workspace_id=$1 ORDER BY t.name,t.team_id LIMIT 65")
            .bind(&workspace).bind(space.as_str()).fetch_all(&mut *tx).await.map_err(pg_error)?;
        if rows.len() > 64 {
            return Err(invalid("organisation_team_limit"));
        }
        let teams = rows.iter().map(|row| {
            let mut value = serde_json::to_value(parse_record(row)?).map_err(bad)?;
            value["assignment"] = match row.try_get::<Option<String>, _>("assignment_role").map_err(pg_error)? {
                Some(role) => serde_json::json!({"role":role,"version":row.get::<i32,_>("assignment_version")}),
                None => serde_json::Value::Null,
            };
            value["assignmentVersion"] = serde_json::json!(row.try_get::<Option<i32>, _>("assignment_version").map_err(pg_error)?.unwrap_or(0));
            Ok(value)
        }).collect::<V3StoreResult<Vec<_>>>()?;
        let members=sqlx::query("SELECT p.principal_id,p.display_name FROM workspace_memberships m JOIN principals p USING(principal_id) WHERE m.workspace_id=$1 AND m.state='active' AND p.status='active' AND p.principal_kind IN ('account','bootstrap_admin') ORDER BY p.principal_id LIMIT 257")
            .bind(&workspace).fetch_all(&mut *tx).await.map_err(pg_error)?;
        if members.len() > 256 {
            return Err(invalid("organisation_member_limit"));
        }
        Ok(
            serde_json::json!({"teams":teams,"eligibleMembers":members.iter().map(|r|serde_json::json!({"principalId":r.get::<String,_>("principal_id"),"displayName":r.get::<String,_>("display_name")})).collect::<Vec<_>>(),"workspaceId":workspace,"canManageDirectory":can_manage_directory}),
        )
    }

    pub async fn change_organisation_team(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        existing: Option<&str>,
        mut request: OrganisationTeamRequest,
    ) -> V3StoreResult<serde_json::Value> {
        validate_name_members(&mut request)?;
        if existing.is_none() != (request.expected_version == 0) {
            return Err(invalid("organisation_team_version_conflict"));
        }
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        super::super::transaction_store::lock_identity(&mut tx, principal, space).await?;
        let workspace = self
            .organisation_scope(&mut tx, principal, space, true)
            .await?;
        self.lock_workspace_mutations(&mut tx, &workspace).await?;
        let id = existing.map(str::to_owned).unwrap_or_else(|| {
            format!(
                "org-team:{}",
                blake3::hash(
                    format!(
                        "{}\0{}\0{}",
                        workspace, principal.principal_id, request.idempotency_key
                    )
                    .as_bytes()
                )
                .to_hex()
            )
        });
        team_id(&id)?;
        let mutation = mutation_id(principal, &workspace, &request.idempotency_key);
        let request_digest = digest(&(existing, &request))?;
        if let Some(row)=sqlx::query("SELECT request_digest,canonical_document_jcs FROM layrs_collaboration.organisation_team_versions WHERE mutation_id=$1")
            .bind(&mutation).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("request_digest")!=request_digest {return Err(invalid("idempotency_digest_mismatch"));}
            let record=parse_record(&row)?;
            if record.workspace_id!=workspace {return Err(V3StoreError::InvalidCapability);}
            return Ok(serde_json::json!({"team":record}));
        }
        let old = self.lock_directory_team(&mut tx, &workspace, &id).await?;
        if old.as_ref().map_or(0, |o| o.version) != request.expected_version
            || old.as_ref().is_some_and(|o| o.state != "active")
        {
            return Err(invalid("organisation_team_version_conflict"));
        }
        if old.is_none() {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM layrs_collaboration.organisation_teams WHERE workspace_id=$1",
            )
            .bind(&workspace)
            .fetch_one(&mut *tx)
            .await
            .map_err(pg_error)?;
            if count >= 64 {
                return Err(invalid("organisation_team_limit"));
            }
        }
        self.validate_directory_members(&mut tx, &workspace, &request.member_principal_ids)
            .await?;
        if old.is_some() {
            self.update_linked_policies(
                &mut tx,
                principal,
                &workspace,
                &id,
                &request.member_principal_ids,
            )
            .await?;
        }
        let record = DirectoryRecord {
            team_id: id.clone(),
            workspace_id: workspace.clone(),
            version: request.expected_version + 1,
            name: request.name,
            member_principal_ids: request.member_principal_ids,
            state: "active".into(),
            changed_by: principal.principal_id.clone(),
            updated_at: now()?,
            reason: None,
        };
        self.persist_directory_team(&mut tx, &record, old.is_none(), &mutation, &request_digest)
            .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(serde_json::json!({"team":record}))
    }

    pub async fn archive_organisation_team(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
        request: ArchiveTeamRequest,
    ) -> V3StoreResult<serde_json::Value> {
        team_id(id)?;
        super::types::validate_key(&request.idempotency_key)?;
        if request.reason.trim().is_empty()
            || request.reason.len() > 4096
            || !(1..1000).contains(&request.expected_version)
        {
            return Err(invalid("invalid_organisation_team_archive"));
        }
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        super::super::transaction_store::lock_identity(&mut tx, principal, space).await?;
        let workspace = self
            .organisation_scope(&mut tx, principal, space, true)
            .await?;
        self.lock_workspace_mutations(&mut tx, &workspace).await?;
        let mutation = mutation_id(principal, &workspace, &request.idempotency_key);
        let request_digest = digest(&(id, &request))?;
        if let Some(row)=sqlx::query("SELECT request_digest,canonical_document_jcs FROM layrs_collaboration.organisation_team_versions WHERE mutation_id=$1")
            .bind(&mutation).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("request_digest")!=request_digest{return Err(invalid("idempotency_digest_mismatch"));}
            let record=parse_record(&row)?;
            if record.workspace_id!=workspace{return Err(V3StoreError::InvalidCapability);}
            return Ok(serde_json::json!({"team":record}));
        }
        let old = self
            .lock_directory_team(&mut tx, &workspace, id)
            .await?
            .ok_or(V3StoreError::NotFound)?;
        if old.version != request.expected_version || old.state != "active" {
            return Err(invalid("organisation_team_version_conflict"));
        }
        self.archive_linked_assignments(&mut tx, &workspace, id, principal, &mutation)
            .await?;
        let record = DirectoryRecord {
            version: old.version + 1,
            member_principal_ids: vec![],
            state: "archived".into(),
            changed_by: principal.principal_id.clone(),
            updated_at: now()?,
            reason: Some(request.reason),
            ..old
        };
        self.persist_directory_team(&mut tx, &record, false, &mutation, &request_digest)
            .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(serde_json::json!({"team":record}))
    }

    pub async fn assign_organisation_team(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
        request: OrganisationTeamAssignmentRequest,
    ) -> V3StoreResult<serde_json::Value> {
        team_id(id)?;
        super::types::validate_key(&request.idempotency_key)?;
        if request.expected_version >= 1000
            || (request.role.is_some() && request.expected_version >= 999)
        {
            return Err(invalid("organisation_team_assignment_version_conflict"));
        }
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        super::super::transaction_store::lock_identity(&mut tx, principal, space).await?;
        let workspace = self
            .organisation_scope(&mut tx, principal, space, false)
            .await?;
        self.lock_workspace_mutations(&mut tx, &workspace).await?;
        let mutation = assignment_mutation_id(principal, space, &request.idempotency_key);
        let request_digest = digest(&(id, &request))?;
        if let Some(row)=sqlx::query("SELECT request_digest,canonical_document_jcs FROM layrs_collaboration.organisation_team_assignment_versions WHERE mutation_id=$1")
            .bind(&mutation).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("request_digest")!=request_digest{return Err(invalid("idempotency_digest_mismatch"));}
            return serde_json::from_str(&row.get::<String,_>("canonical_document_jcs")).map_err(bad);
        }
        let team = self
            .lock_directory_team(&mut tx, &workspace, id)
            .await?
            .ok_or(V3StoreError::NotFound)?;
        if team.state != "active" && request.role.is_some() {
            return Err(invalid("organisation_team_archived"));
        }
        let previous=sqlx::query("SELECT current_version,role FROM layrs_collaboration.organisation_team_assignments WHERE team_id=$1 AND space_id=$2 FOR UPDATE")
            .bind(id).bind(space.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        if previous
            .as_ref()
            .map_or(0, |row| row.get::<i32, _>("current_version") as u32)
            != request.expected_version
        {
            return Err(invalid("organisation_team_assignment_version_conflict"));
        }
        if request.role.is_none() && previous.is_none() {
            return Err(invalid("organisation_team_not_assigned"));
        }
        if request.role.is_some() {
            self.validate_directory_members(&mut tx, &workspace, &team.member_principal_ids)
                .await?;
        }
        let row=sqlx::query("SELECT workspace_id,current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 FOR UPDATE").bind(space.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if row.get::<String, _>("workspace_id") != workspace
            || row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let role = request.role.unwrap_or(TeamRole::Member);
        let (policy_set_id, _) = self
            .install_team_policy(
                &mut tx,
                &authority,
                id,
                role,
                if request.role.is_some() {
                    &team.member_principal_ids
                } else {
                    &[]
                },
            )
            .await?;
        self.notify_directory_change(
            &mut tx,
            principal,
            space,
            id,
            &policy_set_id,
            authority.authorization_epoch + 1,
        )
        .await?;
        let version = request.expected_version + 1;
        let role_text = request.role.map(team_role);
        sqlx::query("INSERT INTO layrs_collaboration.organisation_team_assignments(team_id,workspace_id,space_id,current_version,role) VALUES($1,$2,$3,$4,$5) ON CONFLICT(team_id,space_id) DO UPDATE SET current_version=EXCLUDED.current_version,role=EXCLUDED.role")
            .bind(id).bind(&workspace).bind(space.as_str()).bind(version as i32).bind(role_text).execute(&mut *tx).await.map_err(pg_error)?;
        let response = serde_json::json!({"teamId":id,"spaceId":space,"version":version,"role":role_text,"teamVersion":team.version,"policySetId":policy_set_id,"authorizationEpoch":authority.authorization_epoch+1});
        sqlx::query("INSERT INTO layrs_collaboration.organisation_team_assignment_versions(team_id,space_id,version,mutation_id,request_digest,actor_principal_id,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(id).bind(space.as_str()).bind(version as i32).bind(mutation).bind(request_digest).bind(principal.principal_id.as_str()).bind(jcs(&response)?).execute(&mut *tx).await.map_err(pg_error)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(response)
    }

    async fn lock_directory_team(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &str,
        id: &str,
    ) -> V3StoreResult<Option<DirectoryRecord>> {
        let row=sqlx::query("SELECT v.canonical_document_jcs FROM layrs_collaboration.organisation_teams t JOIN layrs_collaboration.organisation_team_versions v ON v.team_id=t.team_id AND v.version=t.current_version WHERE t.team_id=$1 AND t.workspace_id=$2 FOR UPDATE OF t")
            .bind(id).bind(workspace).fetch_optional(&mut **tx).await.map_err(pg_error)?;
        row.as_ref().map(parse_record).transpose()
    }

    async fn persist_directory_team(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record: &DirectoryRecord,
        new: bool,
        mutation: &str,
        request_digest: &str,
    ) -> V3StoreResult<()> {
        if new {
            sqlx::query("INSERT INTO layrs_collaboration.organisation_teams(team_id,workspace_id,current_version,name,state) VALUES($1,$2,$3,$4,$5)")
                .bind(&record.team_id).bind(&record.workspace_id).bind(record.version as i32).bind(&record.name).bind(&record.state).execute(&mut **tx).await.map_err(pg_error)?;
        } else {
            sqlx::query("UPDATE layrs_collaboration.organisation_teams SET current_version=$2,name=$3,state=$4 WHERE team_id=$1 AND workspace_id=$5")
                .bind(&record.team_id).bind(record.version as i32).bind(&record.name).bind(&record.state).bind(&record.workspace_id).execute(&mut **tx).await.map_err(pg_error)?;
        }
        sqlx::query("INSERT INTO layrs_collaboration.organisation_team_versions(team_id,version,mutation_id,request_digest,actor_principal_id,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(&record.team_id).bind(record.version as i32).bind(mutation).bind(request_digest).bind(record.changed_by.as_str()).bind(jcs(record)?).execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query("DELETE FROM layrs_collaboration.organisation_team_members WHERE team_id=$1")
            .bind(&record.team_id)
            .execute(&mut **tx)
            .await
            .map_err(pg_error)?;
        for member in &record.member_principal_ids {
            sqlx::query("INSERT INTO layrs_collaboration.organisation_team_members(team_id,workspace_id,principal_id) VALUES($1,$2,$3)")
                .bind(&record.team_id).bind(&record.workspace_id).bind(member.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        }
        Ok(())
    }

    async fn update_linked_policies(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        workspace: &str,
        id: &str,
        members: &[PrincipalId],
    ) -> V3StoreResult<()> {
        let rows=sqlx::query("SELECT space_id,role FROM layrs_collaboration.organisation_team_assignments WHERE team_id=$1 AND workspace_id=$2 AND role IS NOT NULL ORDER BY space_id")
            .bind(id).bind(workspace).fetch_all(&mut **tx).await.map_err(pg_error)?;
        for row in rows {
            let space: SpaceId = parse(row.get("space_id"))?;
            let role = parse_role(&row.get::<String, _>("role"))?;
            self.apply_directory_policy(tx, principal, workspace, &space, id, (role, members))
                .await?;
        }
        Ok(())
    }

    async fn archive_linked_assignments(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &str,
        id: &str,
        principal: &PrincipalContext,
        mutation: &str,
    ) -> V3StoreResult<()> {
        let rows=sqlx::query("SELECT space_id,role,current_version FROM layrs_collaboration.organisation_team_assignments WHERE team_id=$1 AND workspace_id=$2 AND role IS NOT NULL ORDER BY space_id")
            .bind(id).bind(workspace).fetch_all(&mut **tx).await.map_err(pg_error)?;
        for row in rows {
            let space: SpaceId = parse(row.get("space_id"))?;
            let role = parse_role(&row.get::<String, _>("role"))?;
            let version = row.get::<i32, _>("current_version") + 1;
            if version > 1000 {
                return Err(invalid("organisation_team_assignment_version_limit"));
            }
            let (authority, policy_set_id) = self
                .apply_directory_policy(tx, principal, workspace, &space, id, (role, &[]))
                .await?;
            sqlx::query("UPDATE layrs_collaboration.organisation_team_assignments SET role=NULL,current_version=$3 WHERE team_id=$1 AND space_id=$2")
                .bind(id).bind(space.as_str()).bind(version).execute(&mut **tx).await.map_err(pg_error)?;
            let event = serde_json::json!({"teamId":id,"spaceId":space,"version":version,"role":null,"teamVersion":null,"policySetId":policy_set_id,"authorizationEpoch":authority.authorization_epoch+1,"reason":"team_archived"});
            let assignment_mutation = format!(
                "org-team-assignment:{}",
                blake3::hash(format!("{mutation}\0{space}").as_bytes()).to_hex()
            );
            sqlx::query("INSERT INTO layrs_collaboration.organisation_team_assignment_versions(team_id,space_id,version,mutation_id,request_digest,actor_principal_id,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(id).bind(space.as_str()).bind(version).bind(assignment_mutation).bind(digest(&event)?).bind(principal.principal_id.as_str()).bind(jcs(&event)?).execute(&mut **tx).await.map_err(pg_error)?;
        }
        Ok(())
    }
}
