use super::*;
use layrs_sync::v3::*;
use types::invalid;

pub(super) struct PreparedMemberGrant {
    authority: SpaceAuthority,
    request: GrantMemberRequest,
    policy_id: PolicyId,
    product_id: ObjectId,
    product: ProductEnvelope,
    policy: PolicySet,
    rules_object: PreparedCasObject,
    product_object: PreparedCasObject,
    set_object: PreparedCasObject,
}
impl PgCollaborationStore {
    pub async fn grant_member(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        request: GrantMemberRequest,
    ) -> V3StoreResult<serde_json::Value> {
        let prepared = self.prepare_member_grant(principal, space, request).await?;
        let mut tx = self.lock_member_grant(principal, space, &prepared).await?;
        let result = self
            .install_member_grant(&mut tx, principal, space, &prepared)
            .await?;
        tx.commit().await.map_err(pg_error)?;
        Ok(result)
    }
    pub(super) async fn prepare_member_grant(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        request: GrantMemberRequest,
    ) -> V3StoreResult<PreparedMemberGrant> {
        let authority = self
            .authority(principal, space, V3Action::AdministerPolicy)
            .await?;
        if request.principal_id == principal.principal_id
            && !matches!(request.role, MemberRole::ReleaseManager)
        {
            return Err(invalid("cannot_change_own_membership"));
        }
        let policy_id: PolicyId = parse(key_id(
            "policy",
            principal,
            space,
            request.principal_id.as_str(),
        ))?;
        let rules = SpacePolicyRules {
            schema_version: SpacePolicyRulesVersion::V1,
            rules: vec![SpacePolicyRule {
                effect: if matches!(request.role, MemberRole::Revoked) {
                    RuleEffect::Deny
                } else {
                    RuleEffect::Allow
                },
                principal_ids: vec![request.principal_id.clone()],
                actions: member_actions(request.role)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            }],
        };
        let rules_object = self.evidence(&rules).await?;
        let placeholder: ObjectId = parse(format!("blake3:{}", "0".repeat(64)))?;
        let version_id: PolicyVersionId = parse(format!(
            "policy-version:{}",
            digest(&(space, &request, authority.authorization_epoch))?
        ))?;
        let mut product = ProductEnvelope::Policy {
            product_id: policy_id.clone(),
            product_object_id: placeholder.clone(),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: PolicyRecord {
                schema_version: PolicySchemaVersion::V1,
                policy_id: policy_id.clone(),
                version_id: version_id.clone(),
                space_id: space.clone(),
                rules_object_id: rules_object.descriptor.object_id.clone(),
                created_at: now()?,
            },
        };
        let product_id = product.computed_object_id().map_err(bad)?;
        if let ProductEnvelope::Policy {
            product_object_id, ..
        } = &mut product
        {
            *product_object_id = product_id.clone()
        }
        let product_object = self
            .prepare(
                ObjectKind::Product,
                product_id.clone(),
                product.canonical_bytes().map_err(bad)?,
            )
            .await?;
        let original: String = sqlx::query_scalar(
            "SELECT canonical_record_jcs FROM policy_sets WHERE policy_set_id=$1",
        )
        .bind(authority.policy_set_id.as_str())
        .fetch_one(self.repository.pool())
        .await
        .map_err(pg_error)?;
        let mut policy: PolicySet = serde_json::from_str(&original).map_err(bad)?;
        policy
            .policy_versions
            .retain(|entry| entry.policy_id != policy_id);
        policy.policy_versions.push(PolicyVersionEntry {
            policy_id: policy_id.clone(),
            policy_version_id: version_id,
            policy_product_object_id: product_id.clone(),
            dependency_object_ids: vec![rules_object.descriptor.object_id.clone()],
        });
        policy
            .policy_versions
            .sort_by(|a, b| a.policy_id.as_str().cmp(b.policy_id.as_str()));
        if policy.policy_versions.len() > 32 {
            return Err(invalid("policy_member_limit"));
        }
        policy.policy_set_id = policy.computed_id().map_err(bad)?;
        let set_object = self
            .prepare(
                ObjectKind::PolicySet,
                policy.policy_set_id.clone(),
                serde_jcs::to_vec(&policy).map_err(bad)?,
            )
            .await?;
        Ok(PreparedMemberGrant {
            authority,
            request,
            policy_id,
            product_id,
            product,
            policy,
            rules_object,
            product_object,
            set_object,
        })
    }
    pub(super) async fn lock_member_grant<'a>(
        &'a self,
        principal: &PrincipalContext,
        space: &SpaceId,
        prepared: &PreparedMemberGrant,
    ) -> V3StoreResult<Transaction<'a, Postgres>> {
        let PreparedMemberGrant {
            authority, request, ..
        } = prepared;
        // Match transaction lock order: Principal/membership before Space.
        let _budget_writer = self.repository.cas_budget_writer().await;
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        self.repository.lock_cas_budget(&mut tx).await?;
        let kind:Option<String>=sqlx::query_scalar("SELECT principal_kind FROM principals WHERE principal_id=$1 AND deployment_id=$2 AND status='active' FOR SHARE").bind(request.principal_id.as_str()).bind(&principal.deployment_id).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let kind = kind.ok_or(V3StoreError::NotFound)?;
        if matches!(request.role, MemberRole::Automation) && kind != "service"
            || matches!(request.role, MemberRole::Member) && kind == "service"
        {
            return Err(invalid("membership_role_kind_mismatch"));
        }
        super::super::transaction_store::lock_identity(&mut tx, principal, space).await?;
        sqlx::query("SELECT m.principal_id FROM workspace_memberships m JOIN spaces s USING(workspace_id) WHERE s.space_id=$1 AND m.principal_id=$2 FOR UPDATE OF m").bind(space.as_str()).bind(request.principal_id.as_str()).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let row=sqlx::query("SELECT workspace_id,current_policy_set_id,authorization_epoch FROM spaces WHERE space_id=$1 FOR UPDATE").bind(space.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if row.get::<String, _>("current_policy_set_id") != authority.policy_set_id.as_str()
            || row.get::<i64, _>("authorization_epoch") != authority.authorization_epoch as i64
        {
            return Err(V3StoreError::AuthorizationEpochChanged);
        }
        let workspace: String = row.get("workspace_id");
        if !super::super::space_access::workspace_administrator(&mut *tx, principal, space).await? {
            return Err(V3StoreError::InvalidCapability);
        }
        super::super::policy_management::require_managed(
            &self.repository,
            &mut tx,
            space,
            &authority.policy_set_id,
        )
        .await?;
        let owner:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspace_memberships WHERE workspace_id=$1 AND principal_id=$2 AND role='owner')").bind(&workspace).bind(request.principal_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
        if owner
            && !(request.principal_id == principal.principal_id
                && matches!(request.role, MemberRole::ReleaseManager))
        {
            return Err(invalid("cannot_change_workspace_owner"));
        }
        // This API changes one Space policy. Never revoke or reactivate a
        // workspace-wide membership as a side effect of a per-Space grant.
        let membership: Option<String> = sqlx::query_scalar(
            "SELECT state FROM workspace_memberships WHERE workspace_id=$1 AND principal_id=$2",
        )
        .bind(&workspace)
        .bind(request.principal_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(pg_error)?;
        if membership.as_deref().is_some_and(|state| state != "active")
            && !matches!(request.role, MemberRole::Revoked)
        {
            return Err(invalid("workspace_membership_not_active"));
        }
        if membership.is_none() && !matches!(request.role, MemberRole::Revoked) {
            sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role,state) VALUES($1,$2,'member','active')").bind(&workspace).bind(request.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        }
        Ok(tx)
    }
    pub(super) async fn install_member_grant(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        principal: &PrincipalContext,
        space: &SpaceId,
        prepared: &PreparedMemberGrant,
    ) -> V3StoreResult<serde_json::Value> {
        let PreparedMemberGrant {
            authority,
            request,
            policy_id,
            product_id,
            product,
            policy,
            rules_object,
            product_object,
            set_object,
        } = prepared;
        super::super::policy_management::require_managed(
            &self.repository,
            tx,
            space,
            &authority.policy_set_id,
        )
        .await?;
        for object in [rules_object, product_object, set_object] {
            self.install(tx, space, object).await?;
        }
        sqlx::query("INSERT INTO products(product_object_id,space_id,product_kind,logical_id,canonical_envelope_jcs) VALUES($1,$2,'policy',$3,$4)").bind(product_id.as_str()).bind(space.as_str()).bind(policy_id.as_str()).bind(jcs(product)?).execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,0,'policy_dependency',$2)").bind(product_id.as_str()).bind(rules_object.descriptor.object_id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query(
            "INSERT INTO policy_sets(policy_set_id,space_id,canonical_record_jcs) VALUES($1,$2,$3)",
        )
        .bind(policy.policy_set_id.as_str())
        .bind(space.as_str())
        .bind(jcs(policy)?)
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;
        for (ordinal, entry) in policy.policy_versions.iter().enumerate() {
            sqlx::query("INSERT INTO policy_set_entries(space_id,policy_set_id,ordinal,policy_id,policy_version_id,policy_product_object_id,dependency_object_ids_jcs) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(space.as_str()).bind(policy.policy_set_id.as_str()).bind(ordinal as i32).bind(entry.policy_id.as_str()).bind(entry.policy_version_id.as_str()).bind(entry.policy_product_object_id.as_str()).bind(jcs(&entry.dependency_object_ids)?).execute(&mut **tx).await.map_err(pg_error)?;
        }
        let epoch = authority.authorization_epoch + 1;
        sqlx::query("INSERT INTO policy_root_history(space_id,authorization_epoch,policy_set_id) VALUES($1,$2,$3)").bind(space.as_str()).bind(epoch as i64).bind(policy.policy_set_id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query(
            "UPDATE spaces SET current_policy_set_id=$2,authorization_epoch=$3 WHERE space_id=$1",
        )
        .bind(space.as_str())
        .bind(policy.policy_set_id.as_str())
        .bind(epoch as i64)
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;
        self.notify(
            tx,
            principal,
            space,
            "membership.changed",
            request.principal_id.as_str(),
            &rules_object.descriptor.object_id,
        )
        .await?;

        Ok(
            serde_json::json!({"principalId":request.principal_id,"role":request.role,"policySetId":policy.policy_set_id,"authorizationEpoch":epoch}),
        )
    }
}

pub(super) fn member_actions(role: MemberRole) -> Vec<&'static str> {
    match role {
        MemberRole::Member => vec![
            "space.bootstrap",
            "object.plan_upload",
            "object.complete_upload",
            "object.plan_fetch",
            "object.fetch",
            "event.read",
            "transaction.apply",
            "review.read",
            "review.write",
            "review.approve",
            "release.read",
        ],
        MemberRole::Auditor => vec![
            "release.read",
            "space.bootstrap",
            "object.plan_fetch",
            "object.fetch",
            "event.read",
            "review.read",
        ],
        MemberRole::Automation => vec![
            "release.read",
            "space.bootstrap",
            "object.plan_fetch",
            "object.fetch",
            "event.read",
            "proof.run",
        ],
        MemberRole::ReleaseManager => vec![
            "space.bootstrap",
            "object.plan_fetch",
            "object.fetch",
            "event.read",
            "release.read",
            "release.publish",
            "release.revoke",
        ],
        MemberRole::Revoked => vec![
            "space.bootstrap",
            "object.plan_upload",
            "object.complete_upload",
            "object.plan_fetch",
            "object.fetch",
            "event.read",
            "transaction.apply",
            "review.read",
            "review.write",
            "review.approve",
            "proof.run",
            "policy.administer",
            "audit.read_evidence",
            "release.read",
            "release.publish",
            "release.revoke",
        ],
    }
}
