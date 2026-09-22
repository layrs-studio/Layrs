use super::*;
use sqlx::Row;

impl PgPathPatchService {
    /// Durable source-request binding is part of the canonical authorization
    /// snapshot, committed atomically with the receipt. No old CAS dependency,
    /// present content grant, or freshly generated View is needed to replay.
    pub(super) async fn replay_patch(
        &self,
        principal: &PrincipalContext,
        patch: &PathPatchRequest,
    ) -> Result<Option<PathPatchReceipt>, TransactionServiceError> {
        let mut tx = self.repository.pool().begin().await.map_err(pg_error)?;
        transaction_store::lock_identity(&mut tx, principal, &patch.space_id).await?;
        let row = sqlx::query("SELECT t.request_digest,t.principal_id,t.authorization_snapshot_id,t.receipt_canonical_jcs,t.receipt_digest,a.canonical_record_jcs AS snapshot_jcs FROM sync_transactions t JOIN authorization_snapshots a ON a.space_id=t.space_id AND a.authorization_snapshot_id=t.authorization_snapshot_id WHERE t.space_id=$1 AND t.client_id=$2 AND t.idempotency_key=$3")
            .bind(patch.space_id.as_str()).bind(patch.client_id.as_str()).bind(patch.idempotency_key.as_str())
            .fetch_optional(&mut *tx).await.map_err(pg_error)?;
        let Some(row) = row else { return Ok(None) };
        if row.get::<String, _>("principal_id") != principal.principal_id.as_str() {
            return Err(V3StoreError::InvalidCapability.into());
        }
        let snapshot_bytes: String = row.get("snapshot_jcs");
        if snapshot_bytes.len() > 16 * 1024 * 1024 {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let snapshot: AuthorizationSnapshot =
            serde_json::from_str(&snapshot_bytes).map_err(invalid)?;
        snapshot.validate_v3().map_err(invalid)?;
        let stored_digest: String = row.get("request_digest");
        if snapshot.space_id != patch.space_id
            || snapshot.authorization_snapshot_id.as_str()
                != row.get::<String, _>("authorization_snapshot_id")
            || snapshot.subject
                != (AuthorizationSubject::Principal {
                    principal_id: principal.principal_id.clone(),
                })
            || snapshot.request_binding.client_id != patch.client_id
            || snapshot.request_binding.idempotency_key != patch.idempotency_key
            || snapshot.request_binding.request_digest.as_str() != stored_digest
            || serde_jcs::to_string(&snapshot).map_err(invalid)? != snapshot_bytes
        {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        if snapshot.request_binding.path_patch_request_digest.as_ref()
            != Some(&patch.request_digest)
        {
            return Err(TransactionServiceError::IdempotencyDigestMismatch);
        }
        let expected_paths = patch
            .changes
            .iter()
            .map(|change| &change.path)
            .collect::<std::collections::BTreeSet<_>>();
        let mut audited_paths = std::collections::BTreeSet::new();
        for decision in &snapshot.decisions {
            match &decision.scope {
                AuthorizationScope::Path {
                    root: AuthorizationRoot::Step { step_id },
                    repo_path,
                } if step_id == &patch.base_step_id => {
                    audited_paths.insert(repo_path);
                }
                _ => return Err(V3StoreError::IdentityMismatch.into()),
            }
        }
        if expected_paths != audited_paths {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let bytes: String = row.get("receipt_canonical_jcs");
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let receipt: SyncTransactionReceipt = serde_json::from_str(&bytes).map_err(invalid)?;
        receipt.validate_v3().map_err(invalid)?;
        if receipt.request_digest.as_str() != stored_digest
            || receipt.idempotency_key != patch.idempotency_key
            || receipt.authorization_snapshot_id != snapshot.authorization_snapshot_id
            || receipt.computed_storage_digest().map_err(invalid)?.as_str()
                != row.get::<String, _>("receipt_digest")
            || receipt.canonical_storage_bytes().map_err(invalid)? != bytes.as_bytes()
            || (receipt.status == ReceiptStatus::Applied
                && snapshot.decisions.iter().any(|decision| !decision.allowed))
        {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        receipt
            .authorize_replay(
                Some(&principal.principal_id),
                &patch.client_id,
                &patch.space_id,
                false,
            )
            .map_err(|_| V3StoreError::InvalidCapability)?;
        tx.commit().await.map_err(pg_error)?;
        Ok(Some(PathPatchReceipt {
            request_digest: patch.request_digest.clone(),
            receipt,
        }))
    }
}
