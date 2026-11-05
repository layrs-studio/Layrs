//! Explicit authenticated assertions, independent of local capture identity.
use super::*;
use layrs_core::scm::{ActorId, CaptureReason, Timestamp};

impl NativeRemote {
    /// Assert a client-observed date/reason for an admitted Step. This never
    /// captures files, moves references or exports local Operation/Worktree IDs.
    /// Keep the idempotency key and arguments unchanged for an uncertain retry.
    pub fn publish_observation(
        &self,
        workspace: &NativeWorkspace,
        step: &StepId,
        observed_at: &Timestamp,
        reason: CaptureReason,
        idempotency_key: &IdempotencyKey,
    ) -> RemoteResult<SyncTransactionReceipt> {
        let _lock = remote_lock(workspace)?;
        let mut binding = self.binding(workspace)?;
        if binding.clone_pending {
            return Err(NativeRemoteError::Invalid(
                "finish clone before publishing an observation",
            ));
        }
        let identity = self.whoami()?;
        let principal: PrincipalId = serde_json::from_value(identity["principal_id"].clone())?;
        let actor: ActorId = serde_json::from_value(identity["actor_id"].clone())?;
        let observation = client_observation(
            &binding.space_id,
            &principal,
            &actor,
            step,
            observed_at,
            reason,
        )?;
        let mut request = SyncTransactionRequest {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionRequestType::Request,
            space_id: binding.space_id.clone(),
            client_id: binding.client_id.clone(),
            idempotency_key: idempotency_key.clone(),
            request_digest: ObjectId::parse(step.as_str())?,
            known_event_cursor: None,
            steps: vec![],
            observations: vec![observation.clone()],
            products: vec![],
            view_mutation: None,
            policy_root_mutation: None,
        };
        request.request_digest = request.computed_request_digest()?;
        request
            .validate_v3()
            .map_err(|_| NativeRemoteError::Invalid("invalid observation request"))?;
        if let Some(pending) = &binding.pending {
            if pending != &request {
                return Err(NativeRemoteError::PublicationPending);
            }
        } else {
            binding.pending = Some(request.clone());
            save_binding(workspace, &binding)?;
        }
        let receipt: SyncTransactionReceipt = self.post(
            &format!("v3/spaces/{}/transactions", binding.space_id),
            &request,
        )?;
        receipt
            .validate_v3()
            .map_err(|_| NativeRemoteError::Invalid("invalid observation receipt"))?;
        if receipt.principal_id != principal
            || receipt.space_id != request.space_id
            || receipt.client_id != request.client_id
            || receipt.idempotency_key != request.idempotency_key
            || receipt.request_digest != request.request_digest
            || (receipt.status == ReceiptStatus::Applied
                && (receipt.accepted_observation_ids != [observation.id().clone()]
                    || !receipt.accepted_step_ids.is_empty()
                    || !receipt.accepted_products.is_empty()
                    || receipt.view_result.is_some()
                    || receipt.policy_root_result.is_some()))
        {
            return Err(NativeRemoteError::Invalid(
                "observation receipt binding mismatch",
            ));
        }
        binding.pending = None;
        binding.last_receipt = Some(receipt.clone());
        save_binding(workspace, &binding)?;
        Ok(receipt)
    }
}
