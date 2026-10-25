//! Instance administration of the visible organisation set.
use super::*;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganisationModeOrganisation {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub project_count: i64,
    pub member_count: i64,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganisationModeSettings {
    pub revision: i64,
    pub state_token: String,
    pub multi_organisation_enabled: bool,
    pub personal_projects_enabled: bool,
    pub primary_organisation_id: String,
    pub organisations: Vec<OrganisationModeOrganisation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateOrganisationModeRequest {
    pub expected_revision: i64,
    pub expected_state_token: String,
    pub multi_organisation_enabled: bool,
    pub primary_organisation_id: String,
    pub confirm_danger: bool,
}

async fn require_instance_admin(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
) -> V3StoreResult<()> {
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM principals p JOIN deployments d USING(deployment_id) WHERE p.principal_id=$1 AND p.actor_id=$2 AND p.subject_digest=$3 AND p.deployment_id=$4 AND p.status='active' AND p.principal_kind='bootstrap_admin' AND d.status='ready')")
        .bind(principal.principal_id.as_str()).bind(principal.actor_id.as_str())
        .bind(&principal.subject_digest).bind(&principal.deployment_id)
        .fetch_one(&mut **tx).await.map_err(pg_error)?;
    if allowed {
        Ok(())
    } else {
        Err(V3StoreError::InvalidCapability)
    }
}

async fn snapshot(
    tx: &mut Transaction<'_, Postgres>,
    deployment_id: &str,
) -> V3StoreResult<OrganisationModeSettings> {
    let config = sqlx::query("SELECT multi_organisation_enabled,personal_projects_enabled,primary_workspace_id,mode_revision FROM layrs_collaboration.deployment_project_contexts WHERE deployment_id=$1")
        .bind(deployment_id).fetch_one(&mut **tx).await.map_err(pg_error)?;
    let revision: i64 = config.get("mode_revision");
    let multi: bool = config.get("multi_organisation_enabled");
    let personal: bool = config.get("personal_projects_enabled");
    let primary: Option<String> = config.get("primary_workspace_id");
    let rows = sqlx::query("SELECT w.workspace_id,w.display_name,w.slug,COALESCE(c.kind,'organisation') AS kind,(SELECT count(*) FROM spaces s WHERE s.workspace_id=w.workspace_id) AS project_count,(SELECT count(*) FROM workspace_memberships m WHERE m.workspace_id=w.workspace_id AND m.state='active') AS member_count FROM workspaces w LEFT JOIN layrs_collaboration.workspace_contexts c ON c.workspace_id=w.workspace_id WHERE w.deployment_id=$1 AND COALESCE(c.kind,'organisation')<>'personal' ORDER BY lower(w.display_name),w.workspace_id")
        .bind(deployment_id).fetch_all(&mut **tx).await.map_err(pg_error)?;
    let primary = primary
        .or_else(|| {
            rows.iter()
                .find(|row| row.get::<String, _>("kind") == "company")
                .map(|row| row.get("workspace_id"))
        })
        .ok_or(V3StoreError::InvalidCapability)?;
    let organisations: Vec<_> = rows
        .into_iter()
        .map(|row| {
            let id: String = row.get("workspace_id");
            OrganisationModeOrganisation {
                is_primary: id == primary,
                id,
                name: row.get("display_name"),
                slug: row.get("slug"),
                project_count: row.get("project_count"),
                member_count: row.get("member_count"),
            }
        })
        .collect();
    // A digest of the complete displayed inventory detects stale warnings if
    // another administrator creates a project or changes memberships.
    let canonical = serde_jcs::to_vec(&serde_json::json!({
        "revision": revision,
        "multi": multi,
        "personal": personal,
        "primary": primary,
        "organisations": organisations.iter().map(|item| serde_json::json!({"id":item.id,"name":item.name,"slug":item.slug,"projectCount":item.project_count,"memberCount":item.member_count})).collect::<Vec<_>>()
    })).map_err(|error| V3StoreError::InvalidDescriptor(error.to_string()))?;
    let state_token = blake3::hash(&canonical).to_hex().to_string();
    Ok(OrganisationModeSettings {
        revision,
        state_token,
        multi_organisation_enabled: multi,
        personal_projects_enabled: personal,
        primary_organisation_id: primary,
        organisations,
    })
}

impl PgV3Repository {
    pub async fn organisation_mode_settings(
        &self,
        principal: &PrincipalContext,
    ) -> V3StoreResult<OrganisationModeSettings> {
        let mut auth = self.pool().begin().await.map_err(pg_error)?;
        require_instance_admin(&mut auth, principal).await?;
        auth.commit().await.map_err(pg_error)?;
        // Ensure the default company exists before presenting a primary choice.
        // This creates no membership in a selected legacy organisation.
        self.project_contexts(principal).await?;
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        require_instance_admin(&mut tx, principal).await?;
        let settings = snapshot(&mut tx, &principal.deployment_id).await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(settings)
    }

    pub async fn update_organisation_mode(
        &self,
        principal: &PrincipalContext,
        request: &UpdateOrganisationModeRequest,
    ) -> V3StoreResult<OrganisationModeSettings> {
        if request.expected_revision < 0
            || request.expected_state_token.len() != 64
            || !request
                .expected_state_token
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || !request.primary_organisation_id.starts_with("workspace:")
            || request.primary_organisation_id.len() > 128
        {
            return Err(V3StoreError::InvalidDescriptor(
                "invalid organisation mode request".into(),
            ));
        }
        if !request.multi_organisation_enabled && !request.confirm_danger {
            return Err(V3StoreError::InvalidDescriptor(
                "disabling organisations requires danger confirmation".into(),
            ));
        }
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,18))")
            .bind(&principal.deployment_id)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        require_instance_admin(&mut tx, principal).await?;
        let before = snapshot(&mut tx, &principal.deployment_id).await?;
        if before.revision != request.expected_revision
            || before.state_token != request.expected_state_token
        {
            return Err(V3StoreError::OrganisationModeConflict);
        }
        if !before
            .organisations
            .iter()
            .any(|organisation| organisation.id == request.primary_organisation_id)
        {
            return Err(V3StoreError::InvalidDescriptor(
                "select an existing organisation".into(),
            ));
        }
        sqlx::query("UPDATE layrs_collaboration.deployment_project_contexts SET multi_organisation_enabled=$2,primary_workspace_id=$3,mode_revision=mode_revision+1,multi_organisation_managed=TRUE WHERE deployment_id=$1")
            .bind(&principal.deployment_id).bind(request.multi_organisation_enabled).bind(&request.primary_organisation_id)
            .execute(&mut *tx).await.map_err(pg_error)?;
        let updated = snapshot(&mut tx, &principal.deployment_id).await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(updated)
    }
}
