use super::*;
impl PgCollaborationStore {
    pub async fn decide_invitation(
        &self,
        principal: &PrincipalContext,
        id: &str,
        action: InvitationAction,
    ) -> V3StoreResult<Value> {
        valid_id(id)?;
        // Only the named parties may even observe a receipt. No Space membership
        // is required to reject an invitation to a project not yet accessible.
        let row=sqlx::query("SELECT i.space_id,i.inviter_principal_id,i.recipient_principal_id,i.member_role,i.authorization_epoch,i.state FROM layrs_collaboration.space_invitations i JOIN spaces s USING(space_id) JOIN workspaces w USING(workspace_id) WHERE i.invitation_id=$1 AND w.deployment_id=$2 AND (i.recipient_principal_id=$3 OR i.inviter_principal_id=$3)").bind(id).bind(&principal.deployment_id).bind(principal.principal_id.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let recipient: String = row.get("recipient_principal_id");
        let inviter: String = row.get("inviter_principal_id");
        if match action {
            InvitationAction::Revoke => inviter != principal.principal_id.as_str(),
            _ => recipient != principal.principal_id.as_str(),
        } {
            return Err(V3StoreError::InvalidCapability);
        }
        let space: SpaceId = parse(row.get("space_id"))?;
        let desired = match action {
            InvitationAction::Accept => "accepted",
            InvitationAction::Reject => "rejected",
            InvitationAction::Revoke => "revoked",
        };
        // Replays never regrant access after a subsequent explicit revocation.
        if row.get::<String, _>("state") == desired {
            let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
            self.lock_invitation_principal(&mut tx, principal).await?;
            return Ok(document(&self.invitation_row(&mut tx, id, false).await?));
        }
        let evidence=self.evidence(&json!({"schemaVersion":"layrs.space-invitation-decision.v1","invitationId":id,"spaceId":space,"actorPrincipalId":principal.principal_id,"decision":desired,"decidedAt":now()?})).await?;
        let inviter_context = if action == InvitationAction::Accept {
            Some(load_principal_context(self.repository.pool(), &parse(inviter)?).await?)
        } else {
            None
        };
        let prepared = if let Some(inviter) = &inviter_context {
            let role: InvitationRole =
                serde_json::from_value(Value::String(row.get("member_role"))).map_err(bad)?;
            Some(
                self.prepare_member_grant(
                    inviter,
                    &space,
                    GrantMemberRequest {
                        principal_id: parse(recipient)?,
                        role: role.member(),
                    },
                )
                .await?,
            )
        } else {
            None
        };
        let mut tx = if let (Some(inviter), Some(grant)) = (&inviter_context, &prepared) {
            self.lock_member_grant(inviter, &space, grant).await?
        } else if action == InvitationAction::Revoke {
            let authority = self
                .authority(principal, &space, V3Action::AdministerPolicy)
                .await?;
            self.locked(principal, &authority).await?
        } else {
            let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
            self.repository.lock_cas_budget(&mut tx).await?;
            self.lock_invitation_principal(&mut tx, principal).await?;
            sqlx::query("SELECT space_id FROM spaces WHERE space_id=$1 FOR UPDATE")
                .bind(space.as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(pg_error)?;
            tx
        };
        self.lock_invitation_principal(&mut tx, principal).await?;
        let current = self.invitation_row(&mut tx, id, true).await?;
        if current.get::<String, _>("state") == desired {
            return Ok(document(&current));
        }
        if current.get::<String, _>("state") != "pending" {
            return Err(invalid("invitation_already_decided"));
        }
        if action == InvitationAction::Accept
            && (current.get::<bool, _>("expired") || current.get::<bool, _>("stale"))
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let grant = if let (Some(inviter), Some(prepared)) = (&inviter_context, &prepared) {
            Some(jcs(&self
                .install_member_grant(&mut tx, inviter, &space, prepared)
                .await?)?)
        } else {
            None
        };
        self.install(&mut tx, &space, &evidence).await?;
        sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,0,'evidence',$2)")
            .bind(evidence.descriptor.object_id.as_str()).bind(current.get::<String,_>("proposal_object_id")).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("UPDATE layrs_collaboration.space_invitations SET state=$2,decided_at=transaction_timestamp(),decision_object_id=$3,grant_result_jcs=$4 WHERE invitation_id=$1").bind(id).bind(desired).bind(evidence.descriptor.object_id.as_str()).bind(grant).execute(&mut *tx).await.map_err(pg_error)?;
        self.notify(
            &mut tx,
            principal,
            &space,
            "invitation.decided",
            id,
            &evidence.descriptor.object_id,
        )
        .await?;
        let result = document(&self.invitation_row(&mut tx, id, false).await?);
        tx.commit().await.map_err(pg_error)?;
        Ok(result)
    }
}
