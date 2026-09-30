//! Derive managed/custom mode from immutable root history and verified Sync receipts.
use super::*;
use layrs_core::scm::{ObjectId, SpaceId};
use layrs_sync::v3::{
    PolicyRootOutcome, PolicySet, ReceiptStatus, SpaceEvent, SyncTransactionReceipt, ValidateV3,
};
use serde::Serialize;
use sqlx::{Postgres, Row, Transaction};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyManagementState {
    pub mode: &'static str,
    pub current_policy_set_id: ObjectId,
    pub managed_policy_set_id: ObjectId,
}
fn corrupt() -> V3StoreError {
    V3StoreError::CorruptObject("policy management provenance".into())
}

/// Caller holds the Space lock. Never infer a managed root from equivalent rules.
pub(crate) async fn state(
    repository: &PgV3Repository,
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    current: &ObjectId,
) -> V3StoreResult<PolicyManagementState> {
    let rows = sqlx::query("SELECT authorization_epoch,policy_set_id FROM policy_root_history WHERE space_id=$1 ORDER BY authorization_epoch DESC LIMIT 1001").bind(space.as_str()).fetch_all(&mut **tx).await.map_err(pg_error)?;
    for (index, row) in rows.iter().enumerate() {
        if index == 1000 {
            return Err(V3StoreError::LimitExceeded {
                resource: "policy management history",
                limit: 1000,
            });
        }
        let epoch: i64 = row.get("authorization_epoch");
        let id: String = row.get("policy_set_id");
        let changes = sqlx::query("SELECT transaction_id,receipt_canonical_jcs,receipt_digest FROM sync_transactions WHERE space_id=$1 AND resulting_authorization_epoch=$2 AND authorization_epoch<>resulting_authorization_epoch LIMIT 2").bind(space.as_str()).bind(epoch).fetch_all(&mut **tx).await.map_err(pg_error)?;
        if changes.len() > 1 {
            return Err(corrupt());
        }
        if let Some(change) = changes.first() {
            let bytes: String = change.get("receipt_canonical_jcs");
            let receipt: SyncTransactionReceipt =
                serde_json::from_str(&bytes).map_err(|_| corrupt())?;
            receipt.validate_v3().map_err(|_| corrupt())?;
            if receipt.canonical_storage_bytes().map_err(|_| corrupt())? != bytes.as_bytes()
                || receipt
                    .computed_storage_digest()
                    .map_err(|_| corrupt())?
                    .as_str()
                    != change.get::<String, _>("receipt_digest")
                || receipt.space_id != *space
                || receipt.status != ReceiptStatus::Applied
                || receipt.transaction_id.as_str() != change.get::<String, _>("transaction_id")
            {
                return Err(corrupt());
            }
            let result = receipt.policy_root_result.as_ref().ok_or_else(corrupt)?;
            if result.outcome != PolicyRootOutcome::Changed
                || result.new_policy_set_id.as_str() != id
                || result.new_authorization_epoch != epoch as u64
                || result.old_authorization_epoch.checked_add(1)
                    != Some(result.new_authorization_epoch)
                || receipt.authorization_epoch != result.old_authorization_epoch
                || receipt.resulting_authorization_epoch != result.new_authorization_epoch
                || receipt.current_policy_set_id != result.new_policy_set_id
            {
                return Err(corrupt());
            }
            let event_id = result.event_id.as_ref().ok_or_else(corrupt)?;
            if !receipt.event_ids.contains(event_id) {
                return Err(corrupt());
            }
            let event: Option<String> = sqlx::query_scalar("SELECT payload_canonical_jcs FROM space_events WHERE space_id=$1 AND event_id=$2 AND transaction_id=$3 AND event_kind='policy.root_changed'").bind(space.as_str()).bind(event_id.as_str()).bind(receipt.transaction_id.as_str()).fetch_optional(&mut **tx).await.map_err(pg_error)?;
            let bytes = event.ok_or_else(corrupt)?;
            let event: SpaceEvent = serde_json::from_str(&bytes).map_err(|_| corrupt())?;
            if serde_jcs::to_string(&event).map_err(|_| corrupt())? != bytes {
                return Err(corrupt());
            }
            let SpaceEvent::PolicyRootChanged { payload, .. } = event else {
                return Err(corrupt());
            };
            if payload.mutation_id != result.mutation_id
                || payload.old_policy_set_id != result.old_policy_set_id
                || payload.new_policy_set_id != result.new_policy_set_id
                || payload.old_authorization_epoch != result.old_authorization_epoch
                || payload.new_authorization_epoch != result.new_authorization_epoch
            {
                return Err(corrupt());
            }
            continue;
        }
        // A change event without its receipt is never reclassified as a managed transition.
        let orphan: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM space_events WHERE space_id=$1 AND event_kind='policy.root_changed' AND (payload_canonical_jcs::jsonb->'payload'->>'newAuthorizationEpoch')=$2)").bind(space.as_str()).bind(epoch.to_string()).fetch_one(&mut **tx).await.map_err(pg_error)?;
        if orphan {
            return Err(corrupt());
        }
        let managed = ObjectId::parse(id).map_err(|_| corrupt())?;
        let descriptor = load_space_descriptor(repository.pool(), space, &managed).await?;
        if descriptor.kind != layrs_sync::v3::ObjectKind::PolicySet {
            return Err(corrupt());
        }
        let bytes =
            read_canonical_object(repository.cas().as_ref(), &descriptor, 1024 * 1024).await?;
        let set: PolicySet = serde_json::from_slice(&bytes).map_err(|_| corrupt())?;
        set.validate_v3().map_err(|_| corrupt())?;
        if set.space_id != *space
            || set.policy_set_id != managed
            || set.computed_id().map_err(|_| corrupt())? != managed
        {
            return Err(corrupt());
        }
        return Ok(PolicyManagementState {
            mode: if current == &managed {
                "managed"
            } else {
                "custom"
            },
            current_policy_set_id: current.clone(),
            managed_policy_set_id: managed,
        });
    }
    Err(corrupt())
}

pub(crate) async fn require_managed(
    repository: &PgV3Repository,
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    current: &ObjectId,
) -> V3StoreResult<()> {
    if !managed(repository, tx, space, current).await? {
        return Err(V3StoreError::CustomPolicyRootActive);
    }
    Ok(())
}

/// Managed member, Team and invitation APIs are refused while a custom policy
/// root is active; this is the predicate behind `require_managed`.
pub(crate) async fn managed(
    repository: &PgV3Repository,
    tx: &mut Transaction<'_, Postgres>,
    space: &SpaceId,
    current: &ObjectId,
) -> V3StoreResult<bool> {
    Ok(state(repository, tx, space, current).await?.mode == "managed")
}
