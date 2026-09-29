//! Explicit administrative PolicyRoot intent; no capture or implicit receive.
use super::*;
#[path = "policy_rules.rs"]
mod rules;
pub use rules::{
    NativeInlinePolicyRules, NativePolicyReplacement, NativePolicyRulesEntry,
    NativePolicyRulesSnapshot,
};

const SCHEMA: &str = "layrs.policy-root-preview.v1";
const MAX_PREVIEW: u64 = 256 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativePolicyPreview {
    pub schema_version: String,
    pub origin: String,
    pub principal_id: PrincipalId,
    pub space_id: SpaceId,
    pub worktree_id: WorktreeId,
    pub store_root: String,
    pub expected_policy_set_id: ObjectId,
    pub expected_authorization_epoch: u64,
    pub proposed_policy_set_id: ObjectId,
    pub request: SyncTransactionRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_rules: Option<NativeInlinePolicyRules>,
}
fn invalid() -> NativeRemoteError {
    NativeRemoteError::Invalid(
        "policy preview or pending intent does not match this account, Store or exact request",
    )
}
fn store_root(workspace: &NativeWorkspace) -> RemoteResult<String> {
    workspace
        .store_root()
        .canonicalize()?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(invalid)
}
fn validate(preview: &NativePolicyPreview) -> RemoteResult<()> {
    if preview.schema_version
        != if preview
            .inline_rules
            .as_ref()
            .is_some_and(|r| r.replacement.is_some())
        {
            rules::REPLACE_SCHEMA
        } else if preview.inline_rules.is_some() {
            rules::SCHEMA
        } else {
            SCHEMA
        }
        || preview.expected_authorization_epoch == 0
        || preview.request.space_id != preview.space_id
        || !preview.request.steps.is_empty()
        || !preview.request.observations.is_empty()
        || preview.request.view_mutation.is_some()
        || preview.request.known_event_cursor.is_some()
        || preview.request.computed_request_digest()? != preview.request.request_digest
        || serde_json::to_vec(preview)?.len() as u64 > MAX_PREVIEW
    {
        return Err(invalid());
    }
    preview.request.validate_v3().map_err(|_| invalid())?;
    let mutation = preview
        .request
        .policy_root_mutation
        .as_ref()
        .ok_or_else(invalid)?;
    let policy_set_id = match &mutation.proposed_policy_set {
        ProposedPolicySet::Stored { policy_set_id }
            if preview.inline_rules.is_none() && preview.request.products.is_empty() =>
        {
            policy_set_id
        }
        ProposedPolicySet::Inline { policy_set } if preview.inline_rules.is_some() => {
            rules::validate_inline(preview, policy_set)?;
            &policy_set.policy_set_id
        }
        _ => return Err(invalid()),
    };
    if mutation.expected_policy_set_id != preview.expected_policy_set_id
        || mutation.expected_authorization_epoch != preview.expected_authorization_epoch
        || policy_set_id != &preview.proposed_policy_set_id
    {
        return Err(invalid());
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyIntent {
    preview: NativePolicyPreview,
    evidence_uploaded: bool,
}
fn load_intent_state(workspace: &NativeWorkspace) -> RemoteResult<PolicyIntent> {
    let bytes = policy_journal::read(
        &workspace.store_root().join("native-policy-intent.json"),
        MAX_PREVIEW,
    )?;
    // Existing stored-root pending journals predate the upload-stage wrapper.
    let intent = match serde_json::from_slice::<PolicyIntent>(&bytes) {
        Ok(intent) => intent,
        Err(_) => PolicyIntent {
            preview: serde_json::from_slice(&bytes)?,
            evidence_uploaded: false,
        },
    };
    validate(&intent.preview)?;
    Ok(intent)
}
fn load_intent(workspace: &NativeWorkspace) -> RemoteResult<NativePolicyPreview> {
    Ok(load_intent_state(workspace)?.preview)
}
fn save_intent(
    workspace: &NativeWorkspace,
    preview: &NativePolicyPreview,
    evidence_uploaded: bool,
) -> RemoteResult<()> {
    let path = workspace.store_root().join("native-policy-intent.json");
    policy_journal::write(
        &path,
        &serde_json::to_vec(&PolicyIntent {
            preview: preview.clone(),
            evidence_uploaded,
        })?,
        MAX_PREVIEW,
    )?;
    Ok(())
}
impl NativeRemote {
    fn policy_identity(&self) -> RemoteResult<PrincipalId> {
        serde_json::from_value(self.whoami()?["principal_id"].clone())
            .map_err(NativeRemoteError::from)
    }
    fn check_policy_scope(
        &self,
        workspace: &NativeWorkspace,
        preview: &NativePolicyPreview,
        principal: &PrincipalId,
    ) -> RemoteResult<()> {
        validate(preview)?;
        if self.origin() != preview.origin
            || principal != &preview.principal_id
            || preview.space_id != *workspace.state()?.space.id()
            || preview.worktree_id != *workspace.worktree_id()
            || preview.store_root != store_root(workspace)?
        {
            return Err(invalid());
        }
        Ok(())
    }
    /// Pure read: neither pending publication nor local Operations are changed.
    /// Existing pending policy intent is returned exactly, never rebuilt at a new epoch.
    pub fn preview_stored_policy(
        &self,
        workspace: &NativeWorkspace,
        proposed_policy_set_id: &ObjectId,
    ) -> RemoteResult<NativePolicyPreview> {
        let _lock = remote_lock(workspace)?;
        let binding = self.binding(workspace)?;
        if binding.clone_pending {
            return Err(invalid());
        }
        let principal = self.policy_identity()?;
        if let Some(pending) = &binding.pending {
            if pending.policy_root_mutation.is_none() {
                return Err(NativeRemoteError::PublicationPending);
            }
            let preview = load_intent(workspace)?;
            self.check_policy_scope(workspace, &preview, &principal)?;
            if pending != &preview.request
                || &preview.proposed_policy_set_id != proposed_policy_set_id
            {
                return Err(NativeRemoteError::PublicationPending);
            }
            return Ok(preview);
        }
        // An orphan intent written before the pending journal is harmless: no POST occurred.
        let bootstrap = self.bootstrap(&binding.space_id)?;
        let nonce = uuid::Uuid::new_v4();
        let mut request = SyncTransactionRequest {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionRequestType::Request,
            space_id: binding.space_id.clone(),
            client_id: binding.client_id.clone(),
            idempotency_key: IdempotencyKey::parse(format!("policy-{nonce}"))
                .map_err(|_| invalid())?,
            request_digest: bootstrap.current_policy_set_id.clone(),
            known_event_cursor: None,
            steps: vec![],
            observations: vec![],
            products: vec![],
            view_mutation: None,
            policy_root_mutation: Some(PolicyRootMutation {
                mutation_id: MutationId::parse(format!("mutation:policy-{nonce}"))
                    .map_err(|_| invalid())?,
                expected_policy_set_id: bootstrap.current_policy_set_id.clone(),
                expected_authorization_epoch: bootstrap.authorization_epoch,
                proposed_policy_set: ProposedPolicySet::Stored {
                    policy_set_id: proposed_policy_set_id.clone(),
                },
            }),
        };
        request.request_digest = request.computed_request_digest()?;
        let preview = NativePolicyPreview {
            schema_version: SCHEMA.into(),
            origin: self.origin().into(),
            principal_id: principal,
            space_id: binding.space_id,
            worktree_id: workspace.worktree_id().clone(),
            store_root: store_root(workspace)?,
            expected_policy_set_id: bootstrap.current_policy_set_id,
            expected_authorization_epoch: bootstrap.authorization_epoch,
            proposed_policy_set_id: proposed_policy_set_id.clone(),
            request,
            inline_rules: None,
        };
        validate(&preview)?;
        Ok(preview)
    }
    /// Persist before POST. On uncertain transport outcome only this exact intent
    /// may be retried. An Applied/Rejected receipt releases pending, never a timeout.
    pub fn apply_policy_preview(
        &self,
        workspace: &NativeWorkspace,
        preview: &NativePolicyPreview,
    ) -> RemoteResult<SyncTransactionReceipt> {
        validate(preview)?;
        let _lock = remote_lock(workspace)?;
        let mut binding = self.binding(workspace)?;
        if binding.clone_pending {
            return Err(invalid());
        }
        let principal = self.policy_identity()?;
        self.check_policy_scope(workspace, preview, &principal)?;
        let evidence_uploaded = if let Some(pending) = &binding.pending {
            if pending.policy_root_mutation.is_none() {
                return Err(NativeRemoteError::PublicationPending);
            }
            let intent = load_intent_state(workspace)?;
            self.check_policy_scope(workspace, &intent.preview, &principal)?;
            if pending != &preview.request
                || serde_jcs::to_vec(&intent.preview)? != serde_jcs::to_vec(preview)?
            {
                return Err(NativeRemoteError::PublicationPending);
            }
            intent.evidence_uploaded
        } else {
            if workspace
                .store_root()
                .join("native-remote.json")
                .try_exists()?
                && binding.client_id != preview.request.client_id
            {
                return Err(invalid());
            }
            binding.client_id = preview.request.client_id.clone();
            // This hint skips redundant upload only. The exact POST still asks
            // the server for its authoritative stored receipt on completed replay.
            let uploaded = binding.last_receipt.as_ref().is_some_and(|r| {
                r.principal_id == principal
                    && r.request_digest == preview.request.request_digest
                    && r.client_id == preview.request.client_id
                    && r.idempotency_key == preview.request.idempotency_key
            });
            save_intent(workspace, preview, uploaded)?;
            binding.pending = Some(preview.request.clone());
            save_binding(workspace, &binding)?;
            uploaded
        };
        if preview.inline_rules.is_some() && !evidence_uploaded {
            rules::upload(self, &binding, preview)?;
            // Durable BEFORE the transaction: a root which revokes upload rights
            // cannot prevent recovery of a committed-but-unacknowledged request.
            save_intent(workspace, preview, true)?;
        }
        let receipt: SyncTransactionReceipt = self.post(
            &format!("v3/spaces/{}/transactions", preview.space_id),
            &preview.request,
        )?;
        receipt.validate_v3().map_err(|_| invalid())?;
        if receipt.principal_id != preview.principal_id
            || receipt.space_id != preview.space_id
            || receipt.client_id != preview.request.client_id
            || receipt.idempotency_key != preview.request.idempotency_key
            || receipt.request_digest != preview.request.request_digest
        {
            return Err(invalid());
        }
        if receipt.status == ReceiptStatus::Applied {
            let result = receipt.policy_root_result.as_ref().ok_or_else(invalid)?;
            let changed = preview.expected_policy_set_id != preview.proposed_policy_set_id;
            let epoch = preview
                .expected_authorization_epoch
                .checked_add(u64::from(changed))
                .ok_or_else(invalid)?;
            if result.mutation_id
                != preview
                    .request
                    .policy_root_mutation
                    .as_ref()
                    .unwrap()
                    .mutation_id
                || result.old_policy_set_id != preview.expected_policy_set_id
                || result.new_policy_set_id != preview.proposed_policy_set_id
                || result.proposed_policy_set_id != preview.proposed_policy_set_id
                || result.old_authorization_epoch != preview.expected_authorization_epoch
                || result.new_authorization_epoch != epoch
                || result.outcome
                    != if changed {
                        PolicyRootOutcome::Changed
                    } else {
                        PolicyRootOutcome::NoOp
                    }
                || receipt.current_policy_set_id != preview.proposed_policy_set_id
                || receipt.resulting_authorization_epoch != epoch
                || receipt.authorization_epoch != preview.expected_authorization_epoch
                || receipt.view_result.is_some()
                || !receipt.accepted_step_ids.is_empty()
            {
                return Err(invalid());
            }
        }
        binding.pending = None;
        binding.last_receipt = Some(receipt.clone());
        // Keep the exact previously observed View. Receiving the new authority is explicit.
        save_binding(workspace, &binding)?;
        policy_journal::remove(&workspace.store_root().join("native-policy-intent.json"))?;
        Ok(receipt)
    }
}
