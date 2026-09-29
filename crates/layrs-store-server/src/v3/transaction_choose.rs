//! Explicit administrative head selection. A discarded head is retained in the
//! immutable DAG and Decision; this only changes the published View.
use super::*;
use layrs_core::scm::SpaceId;
use layrs_sync::v3::*;
use sqlx::{Postgres, Transaction};

pub(crate) fn required(request: &SyncTransactionRequest) -> bool {
    request
        .products
        .iter()
        .any(|p| matches!(p, ProductEnvelope::Decision { .. }))
        || request.view_mutation.as_ref().is_some_and(|v| {
            v.layer_mutations
                .iter()
                .any(|m| m.mode == LayerMutationMode::Choose)
        })
}

pub(crate) async fn administrator(
    tx: &mut Transaction<'_, Postgres>,
    principal: &PrincipalContext,
    space: &SpaceId,
) -> V3StoreResult<bool> {
    super::space_access::workspace_administrator(&mut **tx, principal, space).await
}

pub(crate) async fn authorize(
    repository: &PgV3Repository,
    principal: &PrincipalContext,
    authority: &SpaceAuthority,
) -> V3StoreResult<super::AuthorizationDecision> {
    let mut decision = repository
        .authorizer()
        .authorize(&AuthorizationCheck {
            principal: principal.clone(),
            action: V3Action::AdministerPolicy,
            target: AuthorizationTarget::Space(authority.space_id.clone()),
            policy_set_id: authority.policy_set_id.clone(),
            authorization_epoch: authority.authorization_epoch,
        })
        .await?;
    let mut tx = repository.pool().begin().await.map_err(pg_error)?;
    if !administrator(&mut tx, principal, &authority.space_id).await? {
        decision = super::AuthorizationDecision::deny("choose_requires_workspace_administrator");
    }
    Ok(decision)
}

/// Existing decisions are read through the verified, Space-scoped CAS, then
/// included in normal Product dependency/availability revalidation at commit.
pub(crate) async fn products(
    repository: &PgV3Repository,
    request: &SyncTransactionRequest,
) -> V3StoreResult<Vec<ProductEnvelope>> {
    let mut products = request.products.clone();
    let mut stored_bytes = 0usize;
    if let Some(view) = &request.view_mutation {
        for intent in &view.layer_mutations {
            if intent.mode != LayerMutationMode::Choose {
                continue;
            }
            let Some(id) = &intent.decision_product_object_id else {
                continue;
            };
            if products.iter().any(|p| p.object_id() == id) {
                continue;
            }
            let descriptor =
                load_space_descriptor(repository.pool(), &request.space_id, id).await?;
            if descriptor.kind != ObjectKind::Product {
                return Err(V3StoreError::IdentityMismatch);
            }
            let bytes =
                read_canonical_object(repository.cas().as_ref(), &descriptor, 1024 * 1024).await?;
            stored_bytes =
                stored_bytes
                    .checked_add(bytes.len())
                    .ok_or(V3StoreError::LimitExceeded {
                        resource: "stored Decision bytes",
                        limit: 8 * 1024 * 1024,
                    })?;
            if stored_bytes > 8 * 1024 * 1024 {
                return Err(V3StoreError::LimitExceeded {
                    resource: "stored Decision bytes",
                    limit: 8 * 1024 * 1024,
                });
            }
            let product: ProductEnvelope = serde_json::from_slice(&bytes)
                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
            product
                .verify_descriptor(&descriptor)
                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
            product
                .verify_canonical_wire(&bytes)
                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
            products.push(product);
        }
    }
    Ok(products)
}
