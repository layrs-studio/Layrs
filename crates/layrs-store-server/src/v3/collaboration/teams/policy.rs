use super::*;
impl PgCollaborationStore {
    pub(super) async fn install_team_policy(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        authority: &SpaceAuthority,
        team: &str,
        role: TeamRole,
        members: &[PrincipalId],
    ) -> V3StoreResult<(ObjectId, Option<ObjectId>)> {
        let space = &authority.space_id;
        super::super::super::policy_management::require_managed(
            &self.repository,
            tx,
            space,
            &authority.policy_set_id,
        )
        .await?;
        let logical: PolicyId = parse(format!("policy:{}", digest(&("space-team", space, team))?))?;
        let raw: String = sqlx::query_scalar(
            "SELECT canonical_record_jcs FROM policy_sets WHERE policy_set_id=$1 AND space_id=$2",
        )
        .bind(authority.policy_set_id.as_str())
        .bind(space.as_str())
        .fetch_one(&mut **tx)
        .await
        .map_err(pg_error)?;
        let mut set: PolicySet = serde_json::from_str(&raw).map_err(bad)?;
        if set.policy_set_id != authority.policy_set_id
            || set.computed_id().map_err(bad)? != authority.policy_set_id
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        set.policy_versions.retain(|p| p.policy_id != logical);
        let mut policy_product = None;
        if !members.is_empty() {
            let rules = SpacePolicyRules {
                schema_version: SpacePolicyRulesVersion::V1,
                rules: vec![SpacePolicyRule {
                    effect: RuleEffect::Allow,
                    principal_ids: members.to_vec(),
                    actions: super::super::members::member_actions(role.member_role())
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                }],
            };
            let rules_object = self.evidence(&rules).await?;
            let version: PolicyVersionId = parse(format!(
                "policy-version:{}",
                digest(&(team, authority.authorization_epoch, &rules))?
            ))?;
            let mut product = ProductEnvelope::Policy {
                product_id: logical.clone(),
                product_object_id: parse(format!("blake3:{}", "0".repeat(64)))?,
                depends_on_step_ids: vec![],
                depends_on_product_ids: vec![],
                previous_product_object_id: Nullable::null(),
                record: PolicyRecord {
                    schema_version: PolicySchemaVersion::V1,
                    policy_id: logical.clone(),
                    version_id: version.clone(),
                    space_id: space.clone(),
                    rules_object_id: rules_object.descriptor.object_id.clone(),
                    created_at: now()?,
                },
            };
            let id = product.computed_object_id().map_err(bad)?;
            if let ProductEnvelope::Policy {
                product_object_id, ..
            } = &mut product
            {
                *product_object_id = id.clone();
            }
            product.validate_v3().map_err(bad)?;
            let object = self
                .prepare(
                    ObjectKind::Product,
                    id.clone(),
                    product.canonical_bytes().map_err(bad)?,
                )
                .await?;
            self.install(tx, space, &rules_object).await?;
            self.install(tx, space, &object).await?;
            sqlx::query("INSERT INTO products(product_object_id,space_id,product_kind,logical_id,canonical_envelope_jcs) VALUES($1,$2,'policy',$3,$4)").bind(id.as_str()).bind(space.as_str()).bind(logical.as_str()).bind(jcs(&product)?).execute(&mut **tx).await.map_err(pg_error)?;
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,0,'policy_dependency',$2)").bind(id.as_str()).bind(rules_object.descriptor.object_id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
            set.policy_versions.push(PolicyVersionEntry {
                policy_id: logical,
                policy_version_id: version,
                policy_product_object_id: id.clone(),
                dependency_object_ids: vec![rules_object.descriptor.object_id],
            });
            policy_product = Some(id);
        }
        set.policy_versions
            .sort_by(|a, b| a.policy_id.as_str().cmp(b.policy_id.as_str()));
        if set.policy_versions.len() > 32 {
            return Err(invalid("policy_member_limit"));
        }
        set.policy_set_id = set.computed_id().map_err(bad)?;
        let object = self
            .prepare(
                ObjectKind::PolicySet,
                set.policy_set_id.clone(),
                jcs(&set)?.into_bytes(),
            )
            .await?;
        self.install(tx, space, &object).await?;
        sqlx::query("INSERT INTO policy_sets(policy_set_id,space_id,canonical_record_jcs) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(set.policy_set_id.as_str()).bind(space.as_str()).bind(jcs(&set)?).execute(&mut **tx).await.map_err(pg_error)?;
        for (ordinal, entry) in set.policy_versions.iter().enumerate() {
            sqlx::query("INSERT INTO policy_set_entries(space_id,policy_set_id,ordinal,policy_id,policy_version_id,policy_product_object_id,dependency_object_ids_jcs) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING").bind(space.as_str()).bind(set.policy_set_id.as_str()).bind(ordinal as i32).bind(entry.policy_id.as_str()).bind(entry.policy_version_id.as_str()).bind(entry.policy_product_object_id.as_str()).bind(jcs(&entry.dependency_object_ids)?).execute(&mut **tx).await.map_err(pg_error)?;
        }
        let epoch = authority.authorization_epoch + 1;
        sqlx::query("INSERT INTO policy_root_history(space_id,authorization_epoch,policy_set_id) VALUES($1,$2,$3)").bind(space.as_str()).bind(epoch as i64).bind(set.policy_set_id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        sqlx::query(
            "UPDATE spaces SET current_policy_set_id=$2,authorization_epoch=$3 WHERE space_id=$1",
        )
        .bind(space.as_str())
        .bind(set.policy_set_id.as_str())
        .bind(epoch as i64)
        .execute(&mut **tx)
        .await
        .map_err(pg_error)?;
        Ok((set.policy_set_id, policy_product))
    }
}
