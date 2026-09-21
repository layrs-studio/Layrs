//! Immutable native policy admission. Policies are inert until an explicit root CAS.
use super::*;
use bytes::Bytes;
use layrs_core::scm::{ObjectId, SpaceId};
use layrs_sync::v3::*;
use sqlx::{Postgres, Transaction};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
#[path = "transaction_policy_tests.rs"]
mod tests;

#[derive(Default)]
pub(crate) struct PreparedPolicies {
    pub products: Vec<(ProductEnvelope, PreparedCasObject)>,
    pub proposed: Option<(PolicySet, PreparedCasObject)>,
    pub availability: BTreeSet<ObjectId>,
    pub predecessors: BTreeSet<ObjectId>,
}
pub(crate) fn required(request: &SyncTransactionRequest) -> bool {
    request.policy_root_mutation.is_some()
        || request
            .products
            .iter()
            .any(|p| matches!(p, ProductEnvelope::Policy { .. }))
}
fn invalid() -> V3StoreError {
    V3StoreError::InvalidDescriptor("unsupported_or_invalid_policy_v1".into())
}
fn bad(e: impl std::fmt::Display) -> V3StoreError {
    V3StoreError::CorruptObject(e.to_string())
}
async fn prepare_exact(
    repository: &PgV3Repository,
    kind: ObjectKind,
    id: &ObjectId,
    bytes: Vec<u8>,
) -> V3StoreResult<PreparedCasObject> {
    let descriptor = ObjectDescriptor {
        kind,
        object_id: id.clone(),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    };
    repository
        .cas()
        .put_if_absent(
            &descriptor,
            Box::pin(futures_util::stream::once(
                async move { Ok(Bytes::from(bytes)) },
            )),
        )
        .await?;
    repository.cas().verify_existing(&descriptor).await
}

async fn object(
    repository: &PgV3Repository,
    space: &SpaceId,
    id: &ObjectId,
    kind: ObjectKind,
) -> V3StoreResult<Vec<u8>> {
    let descriptor = load_space_descriptor(repository.pool(), space, id).await?;
    if descriptor.kind != kind {
        return Err(V3StoreError::IdentityMismatch);
    }
    read_canonical_object(repository.cas().as_ref(), &descriptor, 1024 * 1024).await
}

async fn validate_product(
    repository: &PgV3Repository,
    space: &SpaceId,
    product: &ProductEnvelope,
) -> V3StoreResult<Vec<ObjectId>> {
    product.validate_v3().map_err(bad)?;
    product.verify_identity().map_err(bad)?;
    let ProductEnvelope::Policy {
        record,
        depends_on_step_ids,
        depends_on_product_ids,
        previous_product_object_id,
        ..
    } = product
    else {
        return Err(invalid());
    };
    if record.space_id != *space || !depends_on_step_ids.is_empty() {
        return Err(invalid());
    }
    let mut dependencies = vec![record.rules_object_id.clone()];
    match previous_product_object_id.as_ref() {
        None if !depends_on_product_ids.is_empty() => return Err(invalid()),
        None => {}
        Some(previous) => {
            // A byte upload alone is insufficient: predecessor must be an already
            // admitted immutable Policy in this Space, never a same-batch claim.
            let admitted: Option<String> = sqlx::query_scalar("SELECT canonical_envelope_jcs FROM products WHERE space_id=$1 AND product_object_id=$2 AND product_kind='policy'")
                .bind(space.as_str()).bind(previous.as_str()).fetch_optional(repository.pool()).await.map_err(pg_error)?;
            let admitted = admitted.ok_or_else(invalid)?;
            let bytes = object(repository, space, previous, ObjectKind::Product).await?;
            if admitted.as_bytes() != bytes {
                return Err(V3StoreError::IdentityMismatch);
            }
            let parent: ProductEnvelope = serde_json::from_slice(&bytes).map_err(bad)?;
            validate_predecessor(product, &parent)?;
            dependencies.push(previous.clone());
        }
    }
    let bytes = object(
        repository,
        space,
        &record.rules_object_id,
        ObjectKind::Evidence,
    )
    .await?;
    let rules: NativePolicyRules = serde_json::from_slice(&bytes).map_err(bad)?;
    if serde_jcs::to_vec(&rules).map_err(bad)? != bytes {
        return Err(invalid());
    }
    // Evaluation validates every rule, including rules for unrelated principals/actions.
    rules
        .evaluate(
            &PrincipalId::parse("principal:policy-validation").expect("static principal"),
            "policy.administer",
        )
        .map_err(bad)?;
    dependencies.sort();
    dependencies.dedup();
    Ok(dependencies)
}

fn validate_predecessor(product: &ProductEnvelope, parent: &ProductEnvelope) -> V3StoreResult<()> {
    parent.validate_v3().map_err(bad)?;
    parent.verify_identity().map_err(bad)?;
    let (
        ProductEnvelope::Policy {
            record,
            previous_product_object_id,
            depends_on_product_ids,
            ..
        },
        ProductEnvelope::Policy { record: old, .. },
    ) = (product, parent)
    else {
        return Err(invalid());
    };
    let expected_logical = ProductLogicalId::parse(old.policy_id.as_str()).map_err(bad)?;
    if previous_product_object_id.as_ref() != Some(parent.object_id())
        || parent.object_id() == product.object_id()
        || record.space_id != old.space_id
        || record.policy_id != old.policy_id
        || record.version_id == old.version_id
        || depends_on_product_ids != &[expected_logical]
    {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) async fn prepare(
    repository: &PgV3Repository,
    request: &SyncTransactionRequest,
) -> V3StoreResult<PreparedPolicies> {
    let mut result = PreparedPolicies::default();
    let mut overlay = BTreeMap::new();
    let mut total = 0usize;
    for product in request
        .products
        .iter()
        .filter(|p| matches!(p, ProductEnvelope::Policy { .. }))
    {
        if result.products.len() >= 32 {
            return Err(invalid());
        }
        let dependencies = validate_product(repository, &request.space_id, product).await?;
        result.availability.extend(dependencies);
        if let ProductEnvelope::Policy {
            previous_product_object_id,
            ..
        } = product
        {
            result
                .predecessors
                .extend(previous_product_object_id.as_ref().cloned());
        }
        let bytes = product.canonical_bytes().map_err(bad)?;
        total += bytes.len();
        if bytes.len() > 1024 * 1024 || total > 8 * 1024 * 1024 {
            return Err(invalid());
        }
        let prepared =
            prepare_exact(repository, ObjectKind::Product, product.object_id(), bytes).await?;
        if prepared.descriptor.object_id != *product.object_id() {
            return Err(V3StoreError::IdentityMismatch);
        }
        overlay.insert(product.object_id().clone(), product.clone());
        result.products.push((product.clone(), prepared));
    }
    if let Some(mutation) = &request.policy_root_mutation {
        let set: PolicySet = match &mutation.proposed_policy_set {
            ProposedPolicySet::Inline { policy_set } => policy_set.clone(),
            ProposedPolicySet::Stored { policy_set_id } => {
                let bytes = object(
                    repository,
                    &request.space_id,
                    policy_set_id,
                    ObjectKind::PolicySet,
                )
                .await?;
                let value: PolicySet = serde_json::from_slice(&bytes).map_err(bad)?;
                if value.policy_set_id != *policy_set_id {
                    return Err(V3StoreError::IdentityMismatch);
                }
                result.availability.insert(policy_set_id.clone());
                value
            }
        };
        set.validate_v3().map_err(bad)?;
        if set.space_id != request.space_id
            || set.computed_id().map_err(bad)? != set.policy_set_id
            || set.policy_versions.len() > 32
        {
            return Err(invalid());
        }
        for entry in &set.policy_versions {
            let product = match overlay.get(&entry.policy_product_object_id) {
                Some(product) => product.clone(),
                None => {
                    let bytes = object(
                        repository,
                        &request.space_id,
                        &entry.policy_product_object_id,
                        ObjectKind::Product,
                    )
                    .await?;
                    total += bytes.len();
                    if total > 8 * 1024 * 1024 {
                        return Err(invalid());
                    }
                    let product: ProductEnvelope = serde_json::from_slice(&bytes).map_err(bad)?;
                    if product.object_id() != &entry.policy_product_object_id {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                    result
                        .availability
                        .insert(entry.policy_product_object_id.clone());
                    // Uploaded Products must also become admitted immutable policy records.
                    let prepared = repository
                        .cas()
                        .verify_existing(
                            &load_space_descriptor(
                                repository.pool(),
                                &request.space_id,
                                &entry.policy_product_object_id,
                            )
                            .await?,
                        )
                        .await?;
                    result.products.push((product.clone(), prepared));
                    product
                }
            };
            let dependencies = validate_product(repository, &request.space_id, &product).await?;
            if let ProductEnvelope::Policy {
                previous_product_object_id,
                ..
            } = &product
            {
                result
                    .predecessors
                    .extend(previous_product_object_id.as_ref().cloned());
            }
            let ProductEnvelope::Policy { record, .. } = &product else {
                return Err(invalid());
            };
            if record.policy_id != entry.policy_id
                || record.version_id != entry.policy_version_id
                || entry.dependency_object_ids != dependencies
            {
                return Err(invalid());
            }
            result.availability.extend(dependencies);
        }
        let bytes = serde_jcs::to_vec(&set).map_err(bad)?;
        if bytes.len() > 1024 * 1024 {
            return Err(invalid());
        }
        let prepared =
            prepare_exact(repository, ObjectKind::PolicySet, &set.policy_set_id, bytes).await?;
        if prepared.descriptor.object_id != set.policy_set_id {
            return Err(V3StoreError::IdentityMismatch);
        }
        result.proposed = Some((set, prepared));
    }
    result
        .products
        .sort_by(|a, b| a.0.object_id().cmp(b.0.object_id()));
    result
        .products
        .dedup_by(|a, b| a.0.object_id() == b.0.object_id());
    Ok(result)
}

pub(crate) async fn recheck(
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    prepared: &PreparedPolicies,
) -> V3StoreResult<Option<Rejection>> {
    let logical: BTreeSet<_> = prepared
        .products
        .iter()
        .map(|(p, _)| p.logical_id())
        .collect();
    for id in logical {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,5))")
            .bind(format!("product:{id}"))
            .execute(&mut **tx)
            .await
            .map_err(pg_error)?;
        let wrong: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM products WHERE logical_id=$1 AND (space_id<>$2 OR product_kind<>'policy'))").bind(id).bind(space.as_str()).fetch_one(&mut **tx).await.map_err(pg_error)?;
        if wrong {
            return Ok(Some(super::transaction_plan::reject(
                RejectionCode::DuplicateIdentity,
                "Policy logical identity belongs to another Space or kind",
            )));
        }
    }
    for id in &prepared.availability {
        let found: Option<String> = sqlx::query_scalar("SELECT a.object_id FROM space_object_availability a JOIN canonical_objects o USING(object_id) WHERE a.space_id=$1 AND a.object_id=$2 AND a.availability_state='available' AND o.verification_state='verified' FOR SHARE OF a,o").bind(space.as_str()).bind(id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
        if found.is_none() {
            return Ok(Some(super::transaction_plan::reject(
                RejectionCode::MissingObject,
                "Policy dependency became unavailable",
            )));
        }
    }
    for id in &prepared.predecessors {
        // Products are immutable to the runtime role. The surrounding CAS budget
        // lock excludes GC, and the dependency FK protects insertion atomically.
        // FOR SHARE would require granting UPDATE on immutable Product rows.
        let found: Option<String> = sqlx::query_scalar("SELECT product_object_id FROM products WHERE space_id=$1 AND product_object_id=$2 AND product_kind='policy'")
            .bind(space.as_str()).bind(id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
        if found.is_none() {
            return Ok(Some(super::transaction_plan::reject(
                RejectionCode::MissingObject,
                "Policy predecessor is no longer admitted",
            )));
        }
    }
    let mut versions = BTreeMap::new();
    for (product, _) in &prepared.products {
        let ProductEnvelope::Policy { record, .. } = product else {
            return Err(invalid());
        };
        let key = (record.policy_id.clone(), record.version_id.clone());
        if versions
            .insert(key, product.object_id())
            .is_some_and(|old| old != product.object_id())
        {
            return Ok(Some(super::transaction_plan::reject(
                RejectionCode::DuplicateIdentity,
                "Policy version is bound to two immutable Products",
            )));
        }
        let conflict: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM products WHERE space_id=$1 AND product_kind='policy' AND logical_id=$2 AND product_object_id<>$3 AND canonical_envelope_jcs::jsonb->'record'->>'versionId'=$4)").bind(space.as_str()).bind(record.policy_id.as_str()).bind(product.object_id().as_str()).bind(record.version_id.as_str()).fetch_one(&mut **tx).await.map_err(pg_error)?;
        if conflict {
            return Ok(Some(super::transaction_plan::reject(
                RejectionCode::DuplicateIdentity,
                "Policy version already identifies another immutable Product",
            )));
        }
    }
    Ok(None)
}

pub(crate) async fn install(
    tx: &mut Transaction<'_, Postgres>,
    catalog: &PgCatalogStore,
    space: &SpaceId,
    prepared: &PreparedPolicies,
) -> V3StoreResult<Vec<ProductRoot>> {
    let mut accepted = Vec::new();
    for (product, object) in &prepared.products {
        catalog.install_prepared(tx, object).await?;
        catalog
            .make_available(tx, space, product.object_id())
            .await?;
        let inserted = sqlx::query("INSERT INTO products(product_object_id,space_id,product_kind,logical_id,canonical_envelope_jcs) VALUES($1,$2,'policy',$3,$4) ON CONFLICT DO NOTHING").bind(product.object_id().as_str()).bind(space.as_str()).bind(product.logical_id()).bind(serde_jcs::to_string(product).map_err(bad)?).execute(&mut **tx).await.map_err(pg_error)?.rows_affected() == 1;
        let ProductEnvelope::Policy {
            product_id,
            record,
            previous_product_object_id,
            ..
        } = product
        else {
            return Err(invalid());
        };
        sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,0,'policy_dependency',$2) ON CONFLICT DO NOTHING").bind(product.object_id().as_str()).bind(record.rules_object_id.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        if let Some(previous) = previous_product_object_id.as_ref() {
            sqlx::query("INSERT INTO canonical_object_edges(source_object_id,ordinal,edge_kind,target_object_id) VALUES($1,1,'policy_dependency',$2) ON CONFLICT DO NOTHING")
                .bind(product.object_id().as_str()).bind(previous.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
            sqlx::query("INSERT INTO product_dependencies(space_id,product_object_id,ordinal,dependency_product_object_id) VALUES($1,$2,0,$3) ON CONFLICT DO NOTHING")
                .bind(space.as_str()).bind(product.object_id().as_str()).bind(previous.as_str()).execute(&mut **tx).await.map_err(pg_error)?;
        }
        if inserted {
            accepted.push(ProductRoot::Policy {
                logical_id: product_id.clone(),
                object_id: product.object_id().clone(),
            });
        }
    }
    if let Some((set, object)) = &prepared.proposed {
        catalog.install_prepared(tx, object).await?;
        catalog
            .make_available(tx, space, &set.policy_set_id)
            .await?;
        sqlx::query("INSERT INTO policy_sets(policy_set_id,space_id,canonical_record_jcs) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(set.policy_set_id.as_str()).bind(space.as_str()).bind(serde_jcs::to_string(set).map_err(bad)?).execute(&mut **tx).await.map_err(pg_error)?;
        for (ordinal, entry) in set.policy_versions.iter().enumerate() {
            sqlx::query("INSERT INTO policy_set_entries(space_id,policy_set_id,ordinal,policy_id,policy_version_id,policy_product_object_id,dependency_object_ids_jcs) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING").bind(space.as_str()).bind(set.policy_set_id.as_str()).bind(ordinal as i32).bind(entry.policy_id.as_str()).bind(entry.policy_version_id.as_str()).bind(entry.policy_product_object_id.as_str()).bind(serde_jcs::to_string(&entry.dependency_object_ids).map_err(bad)?).execute(&mut **tx).await.map_err(pg_error)?;
        }
    }
    accepted.sort();
    Ok(accepted)
}
