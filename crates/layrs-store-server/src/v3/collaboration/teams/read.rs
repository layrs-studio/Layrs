use super::*;
impl PgCollaborationStore {
    pub async fn eligible_team_members(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let admin:bool=sqlx::query_scalar("SELECT m.role IN ('owner','administrator') FROM workspace_memberships m JOIN spaces s USING(workspace_id) WHERE s.space_id=$1 AND m.principal_id=$2 AND m.state='active'").bind(space.as_str()).bind(principal.principal_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if !admin {
            return Err(V3StoreError::InvalidCapability);
        }
        let rows=sqlx::query("SELECT p.principal_id,p.display_name FROM principals p JOIN workspace_memberships m USING(principal_id) JOIN spaces s USING(workspace_id) WHERE s.space_id=$1 AND p.deployment_id=$2 AND p.status='active' AND m.state='active' AND p.principal_kind IN ('account','bootstrap_admin') ORDER BY p.principal_id LIMIT 257").bind(space.as_str()).bind(&principal.deployment_id).fetch_all(&mut *tx).await.map_err(pg_error)?;
        if rows.len() > 256 {
            return Err(invalid("team_eligible_member_limit"));
        }
        Ok(
            serde_json::json!({"members":rows.iter().map(|r|serde_json::json!({"principalId":r.get::<String,_>("principal_id"),"displayName":r.get::<String,_>("display_name")})).collect::<Vec<_>>()}),
        )
    }
    pub async fn list_teams(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
    ) -> V3StoreResult<serde_json::Value> {
        let authority = self
            .authority(principal, space, V3Action::Bootstrap)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let rows=sqlx::query("SELECT v.evidence_object_id,v.canonical_document_jcs FROM layrs_collaboration.space_teams t JOIN layrs_collaboration.team_versions v ON v.team_id=t.team_id AND v.version=t.current_version WHERE t.space_id=$1 ORDER BY t.team_id LIMIT 33").bind(space.as_str()).fetch_all(&mut *tx).await.map_err(pg_error)?;
        if rows.len() > 32 {
            return Err(invalid("space_team_limit"));
        }
        let teams = rows
            .iter()
            .map(|row| {
                Ok(response(
                    &serde_json::from_str(&row.get::<String, _>("canonical_document_jcs"))
                        .map_err(bad)?,
                    &parse(row.get("evidence_object_id"))?,
                )["team"]
                    .clone())
            })
            .collect::<V3StoreResult<Vec<_>>>()?;
        let policy_management = super::super::super::policy_management::state(&self.repository, &mut tx, space, &authority.policy_set_id).await?;
        Ok(
            serde_json::json!({"teams":teams,"policySetId":authority.policy_set_id,"authorizationEpoch":authority.authorization_epoch,"policyManagement":policy_management}),
        )
    }
    pub async fn team_history(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        id: &str,
        query: TeamHistoryQuery,
    ) -> V3StoreResult<serde_json::Value> {
        team_id(id)?;
        let limit = query.limit.unwrap_or(50);
        let after = query.after.unwrap_or(0);
        if !(1..=100).contains(&limit) || after > 1000 {
            return Err(invalid("invalid_team_history_page"));
        }
        let authority = self
            .authority(principal, space, V3Action::Bootstrap)
            .await?;
        let mut tx = self.locked(principal, &authority).await?;
        let current:Option<i32>=sqlx::query_scalar("SELECT current_version FROM layrs_collaboration.space_teams WHERE team_id=$1 AND space_id=$2").bind(id).bind(space.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let current = current.ok_or(V3StoreError::NotFound)? as u32;
        let anchor = query.anchor.unwrap_or(current);
        if anchor == 0 || anchor > current || after > anchor {
            return Err(invalid("invalid_team_history_anchor"));
        }
        let rows=sqlx::query("SELECT evidence_object_id,canonical_document_jcs,version FROM layrs_collaboration.team_versions WHERE team_id=$1 AND space_id=$2 AND version>$3 AND version<=$4 ORDER BY version LIMIT $5")
            .bind(id).bind(space.as_str()).bind(after as i32).bind(anchor as i32).bind(i64::from(limit)+1).fetch_all(&mut *tx).await.map_err(pg_error)?;
        let more = rows.len() > usize::from(limit);
        let rows = &rows[..rows.len().min(usize::from(limit))];
        if rows
            .iter()
            .map(|r| r.get::<String, _>("canonical_document_jcs").len())
            .sum::<usize>()
            > 1024 * 1024
        {
            return Err(invalid("team_history_page_too_large"));
        }
        let records=rows.iter().map(|r|Ok(serde_json::json!({"eventObjectId":r.get::<String,_>("evidence_object_id"),"record":serde_json::from_str::<serde_json::Value>(&r.get::<String,_>("canonical_document_jcs")).map_err(bad)?}))).collect::<V3StoreResult<Vec<_>>>()?;
        Ok(
            serde_json::json!({"events":records,"anchorVersion":anchor,"nextAfter":rows.last().map(|r|r.get::<i32,_>("version") as u32).unwrap_or(after),"hasMore":more}),
        )
    }
}
