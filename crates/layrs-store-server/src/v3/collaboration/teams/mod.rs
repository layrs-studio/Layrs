use super::types::invalid;
use super::*;
use layrs_sync::v3::*;
mod policy;
mod read;
mod types;
pub use types::{ArchiveTeamRequest, TeamHistoryQuery, TeamRecord, TeamRequest, TeamRole};
use types::{response, team_id};

impl PgCollaborationStore {
    pub async fn change_team(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        existing: Option<&str>,
        mut request: TeamRequest,
    ) -> V3StoreResult<serde_json::Value> {
        request.normalize()?;
        if existing.is_none() && request.expected_version != 0 {
            return Err(invalid("team_version_conflict"));
        }
        if existing.is_some() && request.expected_version == 0 {
            return Err(invalid("team_version_conflict"));
        }
        let id = existing
            .map(str::to_owned)
            .unwrap_or_else(|| key_id("team", principal, space, &request.idempotency_key));
        team_id(&id)?;
        self.mutate_team(
            principal,
            space,
            &id,
            Some(request.clone()),
            None,
            digest(&(existing, &request))?,
            &request.idempotency_key,
        )
        .await
    }
    pub async fn archive_team(
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
            return Err(invalid("invalid_team_archive"));
        }
        self.mutate_team(
            principal,
            space,
            id,
            None,
            Some(request.clone()),
            digest(&(id, &request))?,
            &request.idempotency_key,
        )
        .await
    }
    #[allow(clippy::too_many_arguments)]
    async fn mutate_team(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
        request: Option<TeamRequest>,
        archive: Option<ArchiveTeamRequest>,
        request_digest: String,
        key: &str,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        // Shared identity locks match Gate and transaction order; teams never add
        // or reactivate Workspace memberships. All requested members must exist.
        if let Some(request) = &request {
            let ids: Vec<&str> = request
                .member_principal_ids
                .iter()
                .map(PrincipalId::as_str)
                .collect();
            let rows=sqlx::query("SELECT p.principal_id,p.status,p.principal_kind,m.state FROM principals p JOIN workspace_memberships m USING(principal_id) JOIN spaces s ON s.workspace_id=m.workspace_id WHERE s.space_id=$1 AND p.deployment_id=$2 AND p.principal_id=ANY($3) ORDER BY p.principal_id FOR SHARE OF p,m")
                .bind(space.as_str()).bind(&principal.deployment_id).bind(ids).fetch_all(&mut *tx).await.map_err(pg_error)?;
            if rows.len() != request.member_principal_ids.len()
                || rows.iter().any(|r| {
                    r.get::<String, _>("status") != "active"
                        || r.get::<String, _>("state") != "active"
                        || !matches!(
                            r.get::<String, _>("principal_kind").as_str(),
                            "account" | "bootstrap_admin"
                        )
                })
            {
                return Err(invalid("team_member_not_active_human_workspace_member"));
            }
        }
        super::super::transaction_store::lock_identity(&mut tx, principal, space).await?;
        let current=sqlx::query("SELECT s.current_policy_set_id,s.authorization_epoch,m.role FROM spaces s JOIN workspace_memberships m ON m.workspace_id=s.workspace_id AND m.principal_id=$2 WHERE s.space_id=$1 FOR UPDATE OF s")
            .bind(space.as_str()).bind(principal.principal_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if !matches!(
            current.get::<String, _>("role").as_str(),
            "owner" | "administrator"
        ) {
            return Err(V3StoreError::InvalidCapability);
        }
        if current.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || current.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let mutation = key_id("team-mutation", principal, space, key);
        if let Some(row)=sqlx::query("SELECT request_digest,evidence_object_id,canonical_document_jcs FROM layrs_collaboration.team_versions WHERE mutation_id=$1 AND space_id=$2").bind(&mutation).bind(space.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("request_digest")!=request_digest {return Err(invalid("idempotency_digest_mismatch"));}
            return Ok(response(&serde_json::from_str(&row.get::<String,_>("canonical_document_jcs")).map_err(bad)?,&parse(row.get("evidence_object_id"))?));
        }
        let previous=sqlx::query("SELECT v.evidence_object_id,v.canonical_document_jcs FROM layrs_collaboration.space_teams t JOIN layrs_collaboration.team_versions v ON v.team_id=t.team_id AND v.version=t.current_version WHERE t.team_id=$1 AND t.space_id=$2 FOR UPDATE OF t")
            .bind(id).bind(space.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let old: Option<TeamRecord> = previous
            .as_ref()
            .map(|r| {
                serde_json::from_str(&r.get::<String, _>("canonical_document_jcs")).map_err(bad)
            })
            .transpose()?;
        let expected = request
            .as_ref()
            .map(|r| r.expected_version)
            .or_else(|| archive.as_ref().map(|a| a.expected_version))
            .expect("mutation has request");
        if old.as_ref().map_or(0, |old| old.version) != expected
            || old.as_ref().is_some_and(|old| old.state != "active")
        {
            return Err(invalid("team_version_conflict"));
        }
        if old.is_none() {
            if expected != 0 || archive.is_some() {
                return Err(V3StoreError::NotFound);
            }
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM layrs_collaboration.space_teams WHERE space_id=$1",
            )
            .bind(space.as_str())
            .fetch_one(&mut *tx)
            .await
            .map_err(pg_error)?;
            if count >= 32 {
                return Err(invalid("space_team_limit"));
            }
        }
        let timestamp = now()?;
        let (name, role, members) = match &request {
            Some(r) => (r.name.clone(), r.role, r.member_principal_ids.clone()),
            None => {
                let old = old.as_ref().ok_or(V3StoreError::NotFound)?;
                (old.name.clone(), old.role, vec![])
            }
        };
        let (policy, policy_product) = self
            .install_team_policy(&mut tx, &authority, id, role, &members)
            .await?;
        let record = TeamRecord {
            schema_version: "layrs.space-team.v1".into(),
            team_id: id.into(),
            space_id: space.clone(),
            version: expected + 1,
            name,
            role,
            member_principal_ids: members,
            state: if archive.is_some() {
                "archived"
            } else {
                "active"
            }
            .into(),
            previous_event_object_id: previous
                .as_ref()
                .map(|r| parse(r.get("evidence_object_id")))
                .transpose()?,
            policy_product_object_id: policy_product,
            previous_policy_set_id: authority.policy_set_id.clone(),
            policy_set_id: policy,
            authorization_epoch: authority.authorization_epoch + 1,
            changed_by: principal.principal_id.clone(),
            created_at: old
                .as_ref()
                .map_or(timestamp.clone(), |r| r.created_at.clone()),
            updated_at: timestamp,
            reason: archive.map(|a| a.reason),
        };
        if jcs(&record)?.len() > 32768 {
            return Err(invalid("team_event_too_large"));
        }
        let evidence = self.evidence(&record).await?;
        self.install(&mut tx, space, &evidence).await?;
        let mut refs = vec![
            record.previous_policy_set_id.clone(),
            record.policy_set_id.clone(),
        ];
        refs.extend(record.previous_event_object_id.clone());
        refs.extend(record.policy_product_object_id.clone());
        refs.sort();
        refs.dedup();
        for (ordinal, id) in refs.iter().enumerate() {
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,$2,'evidence',$3)").bind(evidence.descriptor.object_id.as_str()).bind(ordinal as i32).bind(id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        }
        if old.is_none() {
            sqlx::query("INSERT INTO layrs_collaboration.space_teams(team_id,space_id,current_version) VALUES($1,$2,$3)").bind(id).bind(space.as_str()).bind(record.version as i32).execute(&mut *tx).await.map_err(pg_error)?;
        } else {
            sqlx::query(
                "UPDATE layrs_collaboration.space_teams SET current_version=$2 WHERE team_id=$1",
            )
            .bind(id)
            .bind(record.version as i32)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        }
        sqlx::query("INSERT INTO layrs_collaboration.team_versions(team_id,space_id,version,mutation_id,actor_principal_id,request_digest,evidence_object_id,canonical_document_jcs) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(id).bind(space.as_str()).bind(record.version as i32).bind(mutation).bind(principal.principal_id.as_str()).bind(request_digest).bind(evidence.descriptor.object_id.as_str()).bind(jcs(&record)?).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("DELETE FROM layrs_collaboration.team_members WHERE team_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        for member in &record.member_principal_ids {
            sqlx::query("INSERT INTO layrs_collaboration.team_members(team_id,space_id,principal_id) VALUES($1,$2,$3)").bind(id).bind(space.as_str()).bind(member.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        }
        self.notify(
            &mut tx,
            principal,
            space,
            "team.changed",
            id,
            &evidence.descriptor.object_id,
        )
        .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(response(&record, &evidence.descriptor.object_id))
    }
}
