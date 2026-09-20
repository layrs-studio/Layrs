//! Creates Spaces in a caller-owned Workspace for an authenticated local account.
//! Existing workspace permissions are never modified by this operation.
use super::*;
use bytes::Bytes;
use futures_util::stream;
use layrs_core::scm::{
    Layer, LayerHeadSet, LayerId, LayerKind, LayerSettings, LayerStatus, Metadata, ObjectId,
    RepositoryView, SpaceId, Timestamp,
};
use layrs_sync::v3::*;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProvisionSpaceRequest {
    pub workspace_slug: String,
    pub space_slug: String,
    pub name: String,
    /// Optional existing native Layer; its Space and Layer IDs remain authoritative.
    #[serde(default)]
    pub initial_layer: Option<Layer>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProvisionSpaceResponse {
    pub workspace_id: String,
    pub space_id: SpaceId,
    pub layer_id: LayerId,
}

impl ProvisionSpaceRequest {
    pub fn validate(&self) -> V3StoreResult<()> {
        for slug in [&self.workspace_slug, &self.space_slug] {
            if slug.is_empty()
                || slug.len() > 63
                || !slug.as_bytes()[0].is_ascii_alphanumeric()
                || !slug
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err(V3StoreError::InvalidDescriptor(
                    "invalid workspace/Space slug".into(),
                ));
            }
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 255 {
            return Err(V3StoreError::InvalidDescriptor(
                "invalid display name".into(),
            ));
        }
        if self.initial_layer.as_ref().is_some_and(|layer| {
            !layer.parent_layer_ids().is_empty()
                || layer.kind() != LayerKind::Base
                || layer.status() != LayerStatus::Active
        }) {
            return Err(V3StoreError::InvalidDescriptor(
                "initial Layer must be an active base without parents".into(),
            ));
        }
        Ok(())
    }
}

impl PgV3Repository {
    pub async fn provision_space(
        &self,
        principal: &PrincipalContext,
        request: &ProvisionSpaceRequest,
    ) -> V3StoreResult<ProvisionSpaceResponse> {
        request.validate()?;
        let seed = blake3::hash(
            format!(
                "{}/{}/{}",
                principal.deployment_id, principal.principal_id, request.workspace_slug
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        let workspace_id = format!("workspace:{seed}");
        let space_seed = blake3::hash(format!("{workspace_id}/{}", request.space_slug).as_bytes())
            .to_hex()
            .to_string();
        let space_id = match &request.initial_layer {
            Some(layer) => layer.space_id().clone(),
            None => SpaceId::parse(format!("space:{space_seed}")).map_err(bad)?,
        };
        let layer_id = match &request.initial_layer {
            Some(layer) => layer.id().clone(),
            None => LayerId::parse(format!("layer:{space_seed}:main")).map_err(bad)?,
        };
        let response = ProvisionSpaceResponse {
            workspace_id: workspace_id.clone(),
            space_id: space_id.clone(),
            layer_id: layer_id.clone(),
        };
        let timestamp = Timestamp::parse(OffsetDateTime::now_utc().format(&Rfc3339).map_err(bad)?)
            .map_err(bad)?;
        let placeholder =
            ObjectId::parse(format!("blake3:{}", "0".repeat(64))).expect("valid placeholder");
        let layer = match &request.initial_layer {
            Some(layer) => layer.clone(),
            None => Layer::new(
                layer_id.clone(),
                space_id.clone(),
                "main",
                LayerKind::Base,
                LayerStatus::Active,
                vec![],
                LayerSettings::new(true, 1000).map_err(bad)?,
                timestamp.clone(),
                Metadata::empty(),
            )
            .map_err(bad)?,
        };
        let mut layer_product = ProductEnvelope::Layer {
            product_id: layer_id.clone(),
            product_object_id: placeholder.clone(),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: layer,
        };
        let layer_object_id = layer_product.computed_object_id().map_err(bad)?;
        if let ProductEnvelope::Layer {
            product_object_id, ..
        } = &mut layer_product
        {
            *product_object_id = layer_object_id.clone();
        }
        let rules = SpacePolicyRules {
            schema_version: SpacePolicyRulesVersion::V1,
            rules: vec![SpacePolicyRule {
                effect: RuleEffect::Allow,
                principal_ids: vec![principal.principal_id.clone()],
                actions: vec![
                    "space.bootstrap",
                    "object.plan_upload",
                    "object.complete_upload",
                    "object.plan_fetch",
                    "object.fetch",
                    "event.read",
                    "transaction.apply",
                    "policy.administer",
                    "audit.read_evidence",
                    "review.read",
                    "review.write",
                    "review.approve",
                    "proof.run",
                    "release.read",
                    "release.publish",
                    "release.revoke",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            }],
        };
        let rules_bytes = serde_jcs::to_vec(&rules).map_err(bad)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs-evidence-v1\0");
        hasher.update(&rules_bytes);
        let rules_id = ObjectId::parse(format!("blake3:{}", hasher.finalize())).map_err(bad)?;
        let policy_id = PolicyId::parse(format!("policy:{space_seed}:owner")).map_err(bad)?;
        let policy_version_id =
            PolicyVersionId::parse(format!("policy-version:{space_seed}:initial")).map_err(bad)?;
        let mut policy_product = ProductEnvelope::Policy {
            product_id: policy_id.clone(),
            product_object_id: placeholder.clone(),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: PolicyRecord {
                schema_version: PolicySchemaVersion::V1,
                policy_id: policy_id.clone(),
                version_id: policy_version_id.clone(),
                space_id: space_id.clone(),
                rules_object_id: rules_id.clone(),
                created_at: timestamp,
            },
        };
        let policy_product_id = policy_product.computed_object_id().map_err(bad)?;
        if let ProductEnvelope::Policy {
            product_object_id, ..
        } = &mut policy_product
        {
            *product_object_id = policy_product_id.clone();
        }
        let mut policy_set = PolicySet {
            schema_version: PolicySetSchemaVersion::V1,
            policy_set_id: placeholder,
            space_id: space_id.clone(),
            policy_versions: vec![PolicyVersionEntry {
                policy_id: policy_id.clone(),
                policy_version_id: policy_version_id.clone(),
                policy_product_object_id: policy_product_id.clone(),
                dependency_object_ids: vec![rules_id.clone()],
            }],
        };
        policy_set.policy_set_id = policy_set.computed_id().map_err(bad)?;
        let view = RepositoryView::new(
            space_id.clone(),
            vec![LayerHeadSet::new(layer_id.clone(), vec![]).map_err(bad)?],
            vec![],
            vec![],
            vec![],
        )
        .map_err(bad)?;
        let mut prepared = vec![];
        for (kind, id, bytes) in [
            (ObjectKind::Evidence, rules_id.clone(), rules_bytes),
            (
                ObjectKind::Product,
                layer_object_id.clone(),
                layer_product.canonical_bytes().map_err(bad)?,
            ),
            (
                ObjectKind::Product,
                policy_product_id.clone(),
                policy_product.canonical_bytes().map_err(bad)?,
            ),
            (
                ObjectKind::PolicySet,
                policy_set.policy_set_id.clone(),
                serde_jcs::to_vec(&policy_set).map_err(bad)?,
            ),
        ] {
            let descriptor = ObjectDescriptor {
                kind,
                object_id: id,
                canonical_byte_len: bytes.len() as u64,
                encoded_byte_len: bytes.len() as u64,
                compression: Compression::Identity,
            };
            self.cas()
                .put_if_absent(
                    &descriptor,
                    Box::pin(stream::once(async move { Ok(Bytes::from(bytes)) })),
                )
                .await?;
            prepared.push(self.cas().verify_existing(&descriptor).await?);
        }
        prepared.push(
            self.cas()
                .prepare_generated(
                    ObjectKind::RepositoryView,
                    Bytes::from(serde_jcs::to_vec(&view).map_err(bad)?),
                )
                .await?,
        );
        let _budget_writer = self.cas_budget_writer().await;
        let mut tx = self.pool().begin().await.map_err(pg_error)?;
        self.lock_cas_budget(&mut tx).await?;
        let expected_store:Option<String>=sqlx::query_scalar("SELECT d.store_id FROM principals p JOIN deployments d ON d.deployment_id=p.deployment_id WHERE p.principal_id=$1 AND p.actor_id=$2 AND p.subject_digest=$3 AND p.deployment_id=$4 AND p.status='active' AND p.principal_kind IN ('account','bootstrap_admin') AND d.status='ready' FOR SHARE OF p,d")
            .bind(principal.principal_id.as_str()).bind(principal.actor_id.as_str()).bind(&principal.subject_digest).bind(&principal.deployment_id).fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let expected_store = expected_store.ok_or(V3StoreError::InvalidCapability)?;
        if prepared
            .iter()
            .any(|object| object.store_id != expected_store)
        {
            return Err(V3StoreError::InvalidCapability);
        }
        // Serialize deterministic creation, then return the existing identity on an exact replay.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(&workspace_id)
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,5))")
            .bind(format!("product:{layer_id}"))
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        let used_elsewhere: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM products WHERE logical_id=$1 AND space_id<>$2)",
        )
        .bind(layer_id.as_str())
        .bind(space_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(pg_error)?;
        if used_elsewhere {
            return Err(V3StoreError::InvalidDescriptor(
                "Layer ID already belongs to another Space".into(),
            ));
        }
        let workspace_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=$1)")
                .bind(&workspace_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(pg_error)?;
        if workspace_exists {
            let owns: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspaces w JOIN workspace_memberships m ON m.workspace_id=w.workspace_id WHERE w.workspace_id=$1 AND w.deployment_id=$2 AND w.created_by_principal_id=$3 AND w.slug=$4 AND m.principal_id=$3 AND m.state='active' AND m.role='owner')")
                .bind(&workspace_id).bind(&principal.deployment_id).bind(principal.principal_id.as_str()).bind(&request.workspace_slug).fetch_one(&mut *tx).await.map_err(pg_error)?;
            if !owns {
                return Err(V3StoreError::InvalidCapability);
            }
        }
        let existing = sqlx::query(
            "SELECT s.display_name,s.created_by_principal_id,s.workspace_id,s.slug FROM spaces s WHERE s.space_id=$1",
        )
        .bind(space_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(pg_error)?;
        if let Some(existing) = existing {
            if existing.get::<String, _>("display_name") != request.name
                || existing.get::<String, _>("workspace_id") != workspace_id
                || existing.get::<String, _>("slug") != request.space_slug
                || existing.get::<String, _>("created_by_principal_id")
                    != principal.principal_id.as_str()
            {
                return Err(V3StoreError::InvalidDescriptor(
                    "Space already exists with different parameters".into(),
                ));
            }
            let actual: String = sqlx::query_scalar("SELECT current_product_object_id FROM current_layer_definitions WHERE space_id=$1 AND layer_id=$2")
                .bind(space_id.as_str()).bind(layer_id.as_str()).fetch_one(&mut *tx).await.map_err(pg_error)?;
            if request.initial_layer.is_some() && actual != layer_object_id.as_str() {
                return Err(V3StoreError::InvalidDescriptor(
                    "Space exists with different Layer definition".into(),
                ));
            }
            tx.commit().await.map_err(pg_error)?;
            return Ok(response);
        }
        if !workspace_exists {
            sqlx::query("INSERT INTO workspaces(workspace_id,deployment_id,slug,display_name,created_by_principal_id) VALUES($1,$2,$3,$4,$5)").bind(&workspace_id).bind(&principal.deployment_id).bind(&request.workspace_slug).bind(&request.name).bind(principal.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
            sqlx::query("INSERT INTO workspace_memberships(workspace_id,principal_id,role) VALUES($1,$2,'owner')").bind(&workspace_id).bind(principal.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        }
        sqlx::query("INSERT INTO spaces(space_id,workspace_id,slug,display_name,lifecycle_state,current_repository_view_id,current_policy_set_id,created_by_principal_id) VALUES($1,$2,$3,$4,'provisioning',$5,$6,$7)").bind(space_id.as_str()).bind(&workspace_id).bind(&request.space_slug).bind(&request.name).bind(view.id().as_str()).bind(policy_set.policy_set_id.as_str()).bind(principal.principal_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        let catalog = PgCatalogStore::new(self.clone());
        for object in &prepared {
            catalog.install_prepared(&mut tx, object).await?;
            catalog
                .make_available(&mut tx, &space_id, &object.descriptor.object_id)
                .await?;
        }
        for (kind, logical, id, product) in [
            ("layer", layer_id.as_str(), &layer_object_id, &layer_product),
            (
                "policy",
                policy_id.as_str(),
                &policy_product_id,
                &policy_product,
            ),
        ] {
            sqlx::query("INSERT INTO products(product_object_id,space_id,product_kind,logical_id,canonical_envelope_jcs) VALUES($1,$2,$3,$4,$5)").bind(id.as_str()).bind(space_id.as_str()).bind(kind).bind(logical).bind(serde_jcs::to_string(product).map_err(bad)?).execute(&mut *tx).await.map_err(pg_error)?;
        }
        sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,0,'policy_dependency',$2)").bind(policy_product_id.as_str()).bind(rules_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("SELECT layrs_cas_layer_definition($1,$2,NULL,0,$3,NULL)")
            .bind(space_id.as_str())
            .bind(layer_id.as_str())
            .bind(layer_object_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        sqlx::query("INSERT INTO repository_views(repository_view_id,space_id,canonical_record_jcs) VALUES($1,$2,$3)").bind(view.id().as_str()).bind(space_id.as_str()).bind(serde_jcs::to_string(&view).map_err(bad)?).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query(
            "INSERT INTO policy_sets(policy_set_id,space_id,canonical_record_jcs) VALUES($1,$2,$3)",
        )
        .bind(policy_set.policy_set_id.as_str())
        .bind(space_id.as_str())
        .bind(serde_jcs::to_string(&policy_set).map_err(bad)?)
        .execute(&mut *tx)
        .await
        .map_err(pg_error)?;
        sqlx::query("INSERT INTO policy_set_entries(space_id,policy_set_id,ordinal,policy_id,policy_version_id,policy_product_object_id,dependency_object_ids_jcs) VALUES($1,$2,0,$3,$4,$5,$6)").bind(space_id.as_str()).bind(policy_set.policy_set_id.as_str()).bind(policy_id.as_str()).bind(policy_version_id.as_str()).bind(policy_product_id.as_str()).bind(serde_jcs::to_string(&vec![rules_id]).map_err(bad)?).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO repository_view_history(space_id,generation,repository_view_id) VALUES($1,0,$2)").bind(space_id.as_str()).bind(view.id().as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("INSERT INTO policy_root_history(space_id,authorization_epoch,policy_set_id) VALUES($1,1,$2)").bind(space_id.as_str()).bind(policy_set.policy_set_id.as_str()).execute(&mut *tx).await.map_err(pg_error)?;
        sqlx::query("UPDATE spaces SET lifecycle_state='ready' WHERE space_id=$1")
            .bind(space_id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(pg_error)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(response)
    }
}
fn bad(error: impl std::fmt::Display) -> V3StoreError {
    V3StoreError::InvalidDescriptor(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provisioning_rejects_invalid_names_and_slugs() {
        for slug in ["", "../escape", "UPPER", "-leading", "with space"] {
            assert!(
                ProvisionSpaceRequest {
                    workspace_slug: slug.into(),
                    space_slug: "project".into(),
                    name: "Project".into(),
                    initial_layer: None
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            ProvisionSpaceRequest {
                workspace_slug: "team".into(),
                space_slug: "project".into(),
                name: " ".into(),
                initial_layer: None
            }
            .validate()
            .is_err()
        );
    }
}
