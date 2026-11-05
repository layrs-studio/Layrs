//! Deployment-wide company, optional organisation and optional personal contexts.
use super::*;
use layrs_core::scm::SpaceId;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContext {
    pub id: String,
    pub workspace_id: String,
    pub kind: String,
    pub name: String,
    pub slug: String,
    pub role: String,
    pub can_create_project: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContexts {
    pub multi_organisation_enabled: bool,
    pub personal_projects_enabled: bool,
    pub default_context_id: String,
    pub contexts: Vec<ProjectContext>,
    pub can_create_organisation: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateOrganisationRequest {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedOrganisation {
    pub id: String,
    pub workspace_id: String,
    pub kind: &'static str,
    pub name: String,
    pub role: &'static str,
    pub can_create_project: bool,
}

pub(crate) fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 63
        && slug.as_bytes()[0].is_ascii_alphanumeric()
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Transactional gate for collaboration paths that do not use a PolicyAuthorizer.
pub(crate) async fn require_visible_space(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    space: &SpaceId,
) -> V3StoreResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended($1,18))")
        .bind(&principal.deployment_id)
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;
    let visible: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM spaces s JOIN workspaces w ON w.workspace_id=s.workspace_id LEFT JOIN layrs_collaboration.workspace_contexts c ON c.workspace_id=w.workspace_id JOIN layrs_collaboration.deployment_project_contexts dc ON dc.deployment_id=w.deployment_id WHERE s.space_id=$1 AND w.deployment_id=$2 AND ((c.kind='personal' AND dc.personal_projects_enabled AND c.owner_principal_id=$3) OR (COALESCE(c.kind,'organisation')<>'personal' AND (dc.multi_organisation_enabled OR dc.primary_workspace_id=w.workspace_id))))")
        .bind(space.as_str()).bind(&principal.deployment_id).bind(principal.principal_id.as_str())
        .fetch_one(&mut **tx).await.map_err(pg_error)?;
    if visible {
        Ok(())
    } else {
        Err(V3StoreError::InvalidCapability)
    }
}

async fn unused_internal_slug(
    tx: &mut Transaction<'_, Postgres>,
    deployment_id: &str,
    prefix: &str,
    digest: &str,
) -> V3StoreResult<String> {
    for index in 0..100 {
        let candidate = if index == 0 {
            format!("{prefix}-{}", &digest[..32])
        } else {
            format!("{prefix}-{}-{index}", &digest[..32])
        };
        let occupied: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE deployment_id=$1 AND slug=$2)",
        )
        .bind(deployment_id)
        .bind(&candidate)
        .fetch_one(&mut **tx)
        .await
        .map_err(pg_error)?;
        if !occupied {
            return Ok(candidate);
        }
    }
    Err(V3StoreError::InvalidDescriptor(
        "no available internal workspace slug".into(),
    ))
}

impl PgV3Repository {
    /// Lazily creates the company on first authenticated use. The instance
    /// administrator owns it; ordinary accounts become company members.
    pub async fn project_contexts(
        &self,
        principal: &PrincipalContext,
    ) -> V3StoreResult<ProjectContexts> {
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended($1,18))")
            .bind(&principal.deployment_id)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        let row = sqlx::query("SELECT c.multi_organisation_enabled,c.personal_projects_enabled,c.primary_workspace_id,d.display_name,p.principal_kind FROM deployments d JOIN layrs_collaboration.deployment_project_contexts c USING(deployment_id) JOIN principals p ON p.deployment_id=d.deployment_id WHERE d.deployment_id=$1 AND d.status='ready' AND p.principal_id=$2 AND p.actor_id=$3 AND p.subject_digest=$4 AND p.status='active' AND p.principal_kind IN ('account','bootstrap_admin') FOR SHARE OF d,p")
            .bind(&principal.deployment_id).bind(principal.principal_id.as_str()).bind(principal.actor_id.as_str()).bind(&principal.subject_digest)
            .fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        let multi: bool = row.get("multi_organisation_enabled");
        let personal: bool = row.get("personal_projects_enabled");
        let selected_primary: Option<String> = row.get("primary_workspace_id");
        let company_name: String = row.get("display_name");
        let instance_admin = row.get::<String, _>("principal_kind") == "bootstrap_admin";
        // Serialize the first company creation and account membership, including
        // concurrent requests from distinct users.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,17))")
            .bind(&principal.deployment_id)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        let administrator_id: String = sqlx::query_scalar("SELECT administrator_id FROM layrs_identity.bootstrap WHERE deployment_id=$1 AND administrator_id IS NOT NULL")
            .bind(&principal.deployment_id).fetch_optional(&mut *tx).await.map_err(pg_error)?.ok_or(V3StoreError::InvalidCapability)?;
        let company: Option<String> = sqlx::query_scalar("SELECT workspace_id FROM layrs_collaboration.workspace_contexts WHERE deployment_id=$1 AND kind='company'")
            .bind(&principal.deployment_id).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let company = match company {
            Some(id) => id,
            None => {
                let digest =
                    blake3::hash(format!("{}/company", principal.deployment_id).as_bytes())
                        .to_hex()
                        .to_string();
                let id = format!("workspace:company:{digest}");
                let slug =
                    unused_internal_slug(&mut tx, &principal.deployment_id, "company", &digest)
                        .await?;
                sqlx::query("INSERT INTO workspaces(workspace_id,deployment_id,slug,display_name,created_by_principal_id) VALUES($1,$2,$3,$4,$5)")
                    .bind(&id).bind(&principal.deployment_id).bind(&slug).bind(&company_name).bind(&administrator_id)
                    .execute(&mut *tx).await.map_err(pg_error)?;
                sqlx::query("INSERT INTO layrs_collaboration.workspace_contexts(workspace_id,deployment_id,kind) VALUES($1,$2,'company')")
                    .bind(&id).bind(&principal.deployment_id).execute(&mut *tx).await.map_err(pg_error)?;
                id
            }
        };
        sqlx::query("UPDATE layrs_collaboration.deployment_project_contexts SET primary_workspace_id=$2 WHERE deployment_id=$1 AND primary_workspace_id IS NULL")
            .bind(&principal.deployment_id).bind(&company).execute(&mut *tx).await.map_err(pg_error)?;
        let primary = selected_primary.unwrap_or_else(|| company.clone());
        if multi || primary == company {
            sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role) VALUES($1,$2,'owner') ON CONFLICT DO NOTHING")
                .bind(&company).bind(&administrator_id).execute(&mut *tx).await.map_err(pg_error)?;
            sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role) VALUES($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(&company).bind(principal.principal_id.as_str())
                .bind(if instance_admin { "owner" } else { "member" })
                .execute(&mut *tx).await.map_err(pg_error)?;
        }
        if personal {
            let digest = blake3::hash(
                format!(
                    "{}/personal/{}",
                    principal.deployment_id, principal.principal_id
                )
                .as_bytes(),
            )
            .to_hex()
            .to_string();
            let id = format!("workspace:personal:{digest}");
            let existing:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.workspace_contexts WHERE workspace_id=$1 AND deployment_id=$2 AND kind='personal' AND owner_principal_id=$3)")
                .bind(&id).bind(&principal.deployment_id).bind(principal.principal_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
            if !existing {
                let slug =
                    unused_internal_slug(&mut tx, &principal.deployment_id, "personal", &digest)
                        .await?;
                sqlx::query("INSERT INTO workspaces(workspace_id,deployment_id,slug,display_name,created_by_principal_id) VALUES($1,$2,$3,'Personnel',$4)")
                    .bind(&id).bind(&principal.deployment_id).bind(&slug).bind(principal.principal_id.as_str())
                    .execute(&mut *tx).await.map_err(pg_error)?;
                sqlx::query("INSERT INTO layrs_collaboration.workspace_contexts(workspace_id,deployment_id,kind,owner_principal_id) VALUES($1,$2,'personal',$3)")
                    .bind(&id).bind(&principal.deployment_id).bind(principal.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
            }
            sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role) VALUES($1,$2,'owner') ON CONFLICT DO NOTHING")
                .bind(&id).bind(principal.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        }
        let rows = sqlx::query("SELECT w.workspace_id,COALESCE(c.kind,'organisation') AS kind,w.display_name,w.slug,m.role FROM workspaces w LEFT JOIN layrs_collaboration.workspace_contexts c USING(workspace_id) JOIN workspace_memberships m ON m.workspace_id=w.workspace_id WHERE w.deployment_id=$1 AND m.principal_id=$2 AND m.state='active' AND ((COALESCE(c.kind,'organisation')<>'personal' AND ($3 OR w.workspace_id=$5)) OR (c.kind='personal' AND $4 AND c.owner_principal_id=$2)) ORDER BY CASE c.kind WHEN 'company' THEN 0 WHEN 'personal' THEN 1 ELSE 2 END,lower(w.display_name),w.workspace_id")
            .bind(&principal.deployment_id).bind(principal.principal_id.as_str()).bind(multi).bind(personal).bind(&primary)
            .fetch_all(&mut *tx).await.map_err(pg_error)?;
        tx.commit().await.map_err(pg_error)?;
        let contexts: Vec<ProjectContext> = rows
            .into_iter()
            .map(|row| {
                let id: String = row.get("workspace_id");
                let kind: String = row.get("kind");
                let role: String = row.get("role");
                let can_create_project = (kind == "company" && role != "auditor")
                    || role == "owner"
                    || role == "administrator";
                ProjectContext {
                    workspace_id: id.clone(),
                    id,
                    kind,
                    name: row.get("display_name"),
                    slug: row.get("slug"),
                    role,
                    can_create_project,
                }
            })
            .collect();
        let default_context_id = if contexts
            .iter()
            .any(|context: &ProjectContext| context.id == primary)
        {
            primary
        } else {
            contexts
                .first()
                .map(|context| context.id.clone())
                .unwrap_or(primary)
        };
        Ok(ProjectContexts {
            multi_organisation_enabled: multi,
            personal_projects_enabled: personal,
            default_context_id,
            contexts,
            can_create_organisation: multi && instance_admin,
        })
    }

    pub async fn create_organisation(
        &self,
        principal: &PrincipalContext,
        request: &CreateOrganisationRequest,
    ) -> V3StoreResult<CreatedOrganisation> {
        if !valid_slug(&request.slug)
            || request.name.trim().is_empty()
            || request.name.chars().count() > 255
        {
            return Err(V3StoreError::InvalidDescriptor(
                "invalid organisation name or slug".into(),
            ));
        }
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended($1,18))")
            .bind(&principal.deployment_id)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM principals p JOIN deployments d USING(deployment_id) JOIN layrs_collaboration.deployment_project_contexts c USING(deployment_id) WHERE p.principal_id=$1 AND p.actor_id=$2 AND p.subject_digest=$3 AND p.deployment_id=$4 AND p.status='active' AND p.principal_kind='bootstrap_admin' AND d.status='ready' AND c.multi_organisation_enabled FOR SHARE OF p,d)")
            .bind(principal.principal_id.as_str()).bind(principal.actor_id.as_str()).bind(&principal.subject_digest).bind(&principal.deployment_id)
            .fetch_one(&mut *tx).await.map_err(pg_error)?;
        if !allowed {
            return Err(V3StoreError::InvalidCapability);
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,17))")
            .bind(&principal.deployment_id)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        let digest = blake3::hash(
            format!("{}/organisation/{}", principal.deployment_id, request.slug).as_bytes(),
        )
        .to_hex()
        .to_string();
        let id = format!("workspace:organisation:{digest}");
        if let Some(row)=sqlx::query("SELECT w.workspace_id,w.display_name,COALESCE(c.kind,'organisation') AS kind,w.created_by_principal_id,COALESCE(m.state,'missing') AS membership_state,COALESCE(m.role,'missing') AS membership_role FROM workspaces w LEFT JOIN layrs_collaboration.workspace_contexts c USING(workspace_id) LEFT JOIN workspace_memberships m ON m.workspace_id=w.workspace_id AND m.principal_id=$3 WHERE w.deployment_id=$1 AND w.slug=$2")
            .bind(&principal.deployment_id).bind(&request.slug).bind(principal.principal_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)? {
            if row.get::<String,_>("workspace_id")==id && row.get::<String,_>("display_name")==request.name && row.get::<String,_>("kind")=="organisation" && row.get::<String,_>("created_by_principal_id")==principal.principal_id.as_str() && row.get::<String,_>("membership_state")=="active" && row.get::<String,_>("membership_role")=="owner" {
                tx.commit().await.map_err(pg_error)?;
                return Ok(CreatedOrganisation{id:id.clone(),workspace_id:id,kind:"organisation",name:request.name.clone(),role:"owner",can_create_project:true});
            }
            return Err(V3StoreError::InvalidDescriptor("organisation slug already exists".into()));
        }
        sqlx::query("INSERT INTO workspaces(workspace_id,deployment_id,slug,display_name,created_by_principal_id) VALUES($1,$2,$3,$4,$5)")
            .bind(&id).bind(&principal.deployment_id).bind(&request.slug).bind(&request.name).bind(principal.principal_id.as_str())
            .execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO layrs_collaboration.workspace_contexts(workspace_id,deployment_id,kind) VALUES($1,$2,'organisation')")
            .bind(&id).bind(&principal.deployment_id).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role) VALUES($1,$2,'owner')")
            .bind(&id).bind(principal.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(CreatedOrganisation {
            id: id.clone(),
            workspace_id: id,
            kind: "organisation",
            name: request.name.clone(),
            role: "owner",
            can_create_project: true,
        })
    }
}
