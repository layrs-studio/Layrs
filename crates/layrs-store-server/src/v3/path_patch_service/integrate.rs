use super::*;
use layrs_core::scm::SpaceId;
use sqlx::Row;

fn integration_key(
    principal: &PrincipalContext,
    review_id: &str,
    user_key: &IdempotencyKey,
) -> V3StoreResult<IdempotencyKey> {
    if review_id.len() > 100 || !review_id.starts_with("scoped-review:") {
        return Err(V3StoreError::InvalidCapability);
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-scoped-review-integration-v1\0");
    hash.update(principal.principal_id.as_str().as_bytes());
    hash.update(b"\0");
    hash.update(review_id.as_bytes());
    hash.update(b"\0");
    hash.update(user_key.as_str().as_bytes());
    IdempotencyKey::parse(format!("scoped-int-{}", hash.finalize().to_hex())).map_err(invalid)
}

impl PgPathPatchService {
    async fn candidate_source(
        &self,
        candidate: &ScopedPatchCandidate,
    ) -> V3StoreResult<ScopedPathPatchRequest> {
        let id = format!(
            "scoped-patch:{}",
            &candidate.source_request_digest.as_str()[7..]
        );
        let row = sqlx::query("SELECT principal_id,request_jcs FROM layrs_collaboration.scoped_patch_sessions WHERE session_id=$1 AND space_id=$2 AND request_digest=$3")
            .bind(&id).bind(candidate.space_id.as_str()).bind(candidate.source_request_digest.as_str())
            .fetch_optional(self.repository.pool()).await.map_err(pg_error)?.ok_or(V3StoreError::IdentityMismatch)?;
        let canonical: String = row.get("request_jcs");
        if canonical.len() > 4 * 1024 * 1024 {
            return Err(V3StoreError::IdentityMismatch);
        }
        let source: ScopedPathPatchRequest = serde_json::from_str(&canonical).map_err(invalid)?;
        source.validate().map_err(invalid)?;
        if source.intent != ScopedPathPatchIntent::Candidate
            || source.request_digest != candidate.source_request_digest
            || source.capture_digest != candidate.capture_digest
            || source.space_id != candidate.space_id
            || source.layer_id != candidate.layer_id
            || source.base_step_id != candidate.base_step_id
            || source.base_root_tree_id != candidate.base_root_tree_id
            || source.expected_repository_view_id != candidate.expected_view_id
            || source.expected_policy_set_id != candidate.expected_policy_set_id
            || source.expected_authorization_epoch != candidate.expected_authorization_epoch
            || row.get::<String, _>("principal_id") != candidate.author_principal_id.as_str()
            || source
                .changes
                .iter()
                .map(|change| &change.path)
                .ne(candidate.changed_paths.iter())
            || serde_jcs::to_string(&source).map_err(invalid)? != canonical
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        Ok(source)
    }

    /// The caller nominates a review and an idempotency key. Candidate, Step,
    /// paths and View mutation are resolved from server-owned durable records.
    pub async fn integrate_scoped_review(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review_id: &str,
        user_key: &IdempotencyKey,
    ) -> Result<SyncTransactionReceipt, TransactionServiceError> {
        let key = integration_key(principal, review_id, user_key)?;
        let candidate_id: String = sqlx::query_scalar("SELECT candidate_id FROM layrs_collaboration.scoped_reviews WHERE space_id=$1 AND review_id=$2")
            .bind(space.as_str()).bind(review_id).fetch_optional(self.repository.pool()).await.map_err(pg_error)?
            .ok_or(V3StoreError::NotFound)?;
        let candidate = load_candidate(&self.repository, space, &candidate_id)
            .await?
            .ok_or(V3StoreError::NotFound)?;
        if let Some(receipt) = self
            .replay_scoped_integration(principal, space, review_id, &candidate, &key)
            .await?
        {
            let source = self.candidate_source(&candidate).await?;
            self.scoped_replay_authority(principal, &source).await?;
            return Ok(receipt);
        }
        let target = PgCollaborationStore::new(self.repository.clone())
            .load_scoped_integration_target(principal, space, review_id)
            .await?;
        if target.candidate_id != candidate.candidate_id
            || target.candidate_step_id != candidate.candidate_step_id
            || target.layer_id != candidate.layer_id
            || target.expected_view_id != candidate.expected_view_id
            || target.expected_policy_set_id != candidate.expected_policy_set_id
            || target.expected_epoch != candidate.expected_authorization_epoch
        {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let source = self.candidate_source(&candidate).await?;
        if source.expected_generation != target.expected_generation {
            return Err(V3StoreError::AuthorizationEpochChanged.into());
        }
        let mut patch = scoped::sync_patch(&source)?;
        patch.client_id = ClientId::parse("client:scoped-integration").map_err(invalid)?;
        patch.idempotency_key = key.clone();
        let (mut transaction, mut scope, _) = self.prepare_request(&patch).await?;
        let step = transaction
            .steps
            .first()
            .ok_or(V3StoreError::IdentityMismatch)?;
        if transaction.steps.len() != 1
            || step.id() != &candidate.candidate_step_id
            || step.root_tree_id() != &candidate.candidate_root_tree_id
        {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let mutation = transaction
            .view_mutation
            .as_mut()
            .ok_or(V3StoreError::IdentityMismatch)?;
        mutation.mutation_id =
            MutationId::parse(format!("mutation:{}", &key.as_str()[11..])).map_err(invalid)?;
        transaction.request_digest = transaction.computed_request_digest().map_err(invalid)?;
        scope.request = patch;
        scope.transaction_digest = transaction.request_digest.clone();
        scope.integration = Some(path_patch_scope::ScopedIntegrationBinding {
            review_id: review_id.into(),
            candidate_id,
            step_id: candidate.candidate_step_id,
            layer_id: candidate.layer_id,
        });
        Ok(
            PgTransactionStore::new(self.repository.clone(), self.cursors.clone())
                .transact_scoped(principal, &transaction, &scope)
                .await?
                .receipt,
        )
    }

    async fn replay_scoped_integration(
        &self,
        principal: &PrincipalContext,
        space: &SpaceId,
        review_id: &str,
        candidate: &ScopedPatchCandidate,
        key: &IdempotencyKey,
    ) -> Result<Option<SyncTransactionReceipt>, TransactionServiceError> {
        let row = sqlx::query("SELECT t.principal_id,t.receipt_canonical_jcs,t.receipt_digest,t.request_digest,a.canonical_record_jcs AS snapshot_jcs FROM sync_transactions t JOIN authorization_snapshots a ON a.space_id=t.space_id AND a.authorization_snapshot_id=t.authorization_snapshot_id WHERE t.space_id=$1 AND t.client_id='client:scoped-integration' AND t.idempotency_key=$2")
            .bind(space.as_str()).bind(key.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?;
        let Some(row) = row else { return Ok(None) };
        if row.get::<String, _>("principal_id") != principal.principal_id.as_str() {
            return Err(V3StoreError::InvalidCapability.into());
        }
        let bytes: String = row.get("receipt_canonical_jcs");
        let snapshot_jcs: String = row.get("snapshot_jcs");
        if bytes.len() > 16 * 1024 * 1024 || snapshot_jcs.len() > 16 * 1024 * 1024 {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        let receipt: SyncTransactionReceipt = serde_json::from_str(&bytes).map_err(invalid)?;
        let snapshot: AuthorizationSnapshot =
            serde_json::from_str(&snapshot_jcs).map_err(invalid)?;
        receipt.validate_v3().map_err(invalid)?;
        snapshot.validate_v3().map_err(invalid)?;
        if receipt.request_digest.as_str() != row.get::<String, _>("request_digest")
            || receipt.idempotency_key != *key
            || receipt.principal_id != principal.principal_id
            || receipt.computed_storage_digest().map_err(invalid)?.as_str()
                != row.get::<String, _>("receipt_digest")
            || receipt.canonical_storage_bytes().map_err(invalid)? != bytes.as_bytes()
            || receipt.authorization_snapshot_id != snapshot.authorization_snapshot_id
            || snapshot.request_binding.path_patch_request_digest.as_ref()
                != Some(&candidate.source_request_digest)
            || snapshot.request_binding.idempotency_key != *key
            || snapshot.request_binding.request_digest != receipt.request_digest
            || serde_jcs::to_string(&snapshot).map_err(invalid)? != snapshot_jcs
        {
            return Err(V3StoreError::IdentityMismatch.into());
        }
        if receipt.status == ReceiptStatus::Applied {
            let linked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM layrs_collaboration.scoped_review_integrations WHERE space_id=$1 AND review_id=$2 AND candidate_id=$3 AND transaction_id=$4)")
                .bind(space.as_str()).bind(review_id).bind(&candidate.candidate_id).bind(receipt.transaction_id.as_str())
                .fetch_one(self.repository.pool()).await.map_err(pg_error)?;
            if !linked {
                return Err(V3StoreError::IdentityMismatch.into());
            }
        }
        Ok(Some(receipt))
    }
}
