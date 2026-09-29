//! Closed v1/v2 documents become one additive Policy, never an arbitrary transaction.
use super::*;
pub(super) const SCHEMA: &str = "layrs.policy-root-preview.v2";
pub(super) const REPLACE_SCHEMA: &str = "layrs.policy-root-preview.v3";
#[path = "policy_read.rs"]
mod read;
#[path = "policy_replace.rs"]
mod replacement;
pub use read::{NativePolicyRulesEntry, NativePolicyRulesSnapshot};
const MAX_RULES: usize = 64 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeInlinePolicyRules {
    pub object_id: ObjectId,
    pub rules: NativePolicyRules,
    pub base_policy_set: PolicySet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<NativePolicyReplacement>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativePolicyReplacement {
    pub policy_id: PolicyId,
    pub previous_product_object_id: ObjectId,
}
fn checked_rules(bytes: &[u8]) -> RemoteResult<NativePolicyRules> {
    if bytes.is_empty() || bytes.len() > MAX_RULES {
        return Err(invalid());
    }
    let rules: NativePolicyRules = serde_json::from_slice(bytes)?;
    rules
        .evaluate(
            &PrincipalId::parse("principal:policy-preview-validation").expect("static principal"),
            "policy.administer",
        )
        .map_err(|_| invalid())?;
    if serde_jcs::to_vec(&rules)?.len() > MAX_RULES {
        return Err(invalid());
    }
    Ok(rules)
}
fn evidence_id(bytes: &[u8]) -> RemoteResult<ObjectId> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-evidence-v1\0");
    hash.update(bytes);
    Ok(ObjectId::parse(format!("blake3:{}", hash.finalize()))?)
}
pub(super) fn validate_inline(
    preview: &NativePolicyPreview,
    proposed: &PolicySet,
) -> RemoteResult<()> {
    let inline = preview.inline_rules.as_ref().ok_or_else(invalid)?;
    let bytes = serde_jcs::to_vec(&inline.rules)?;
    checked_rules(&bytes)?;
    if evidence_id(&bytes)? != inline.object_id
        || inline.base_policy_set.space_id != preview.space_id
        || inline.base_policy_set.policy_set_id != preview.expected_policy_set_id
        || inline.base_policy_set.computed_id()? != preview.expected_policy_set_id
        || inline.base_policy_set.policy_versions.len() > 32
        || (inline.replacement.is_none() && inline.base_policy_set.policy_versions.len() == 32)
    {
        return Err(invalid());
    }
    inline
        .base_policy_set
        .validate_v3()
        .map_err(|_| invalid())?;
    let [product] = preview.request.products.as_slice() else {
        return Err(invalid());
    };
    product.validate_v3().map_err(|_| invalid())?;
    product.verify_identity().map_err(|_| invalid())?;
    let ProductEnvelope::Policy {
        product_id,
        record,
        depends_on_step_ids,
        depends_on_product_ids,
        previous_product_object_id,
        ..
    } = product
    else {
        return Err(invalid());
    };
    if record.space_id != preview.space_id
        || record.rules_object_id != inline.object_id
        || !depends_on_step_ids.is_empty()
    {
        return Err(invalid());
    }
    let mut expected = inline.base_policy_set.clone();
    let mut dependencies = vec![inline.object_id.clone()];
    if let Some(replacement) = &inline.replacement {
        let previous = expected
            .policy_versions
            .iter()
            .find(|p| p.policy_id == replacement.policy_id)
            .ok_or_else(invalid)?;
        if product_id != &replacement.policy_id
            || previous.policy_product_object_id != replacement.previous_product_object_id
            || previous.policy_version_id == record.version_id
            || previous_product_object_id.as_ref() != Some(&replacement.previous_product_object_id)
            || depends_on_product_ids
                != &vec![ProductLogicalId::parse(replacement.policy_id.as_str())
                    .map_err(|_| invalid())?]
        {
            return Err(invalid());
        }
        dependencies.push(replacement.previous_product_object_id.clone());
        dependencies.sort();
        dependencies.dedup();
        expected
            .policy_versions
            .retain(|p| p.policy_id != replacement.policy_id);
    } else if !depends_on_product_ids.is_empty()
        || previous_product_object_id.is_some()
        || expected
            .policy_versions
            .iter()
            .any(|p| &p.policy_id == product_id)
    {
        return Err(invalid());
    }
    expected.policy_versions.push(PolicyVersionEntry {
        policy_id: product_id.clone(),
        policy_version_id: record.version_id.clone(),
        policy_product_object_id: product.object_id().clone(),
        dependency_object_ids: dependencies,
    });
    expected
        .policy_versions
        .sort_by(|a, b| a.policy_id.cmp(&b.policy_id));
    expected.policy_set_id = expected.computed_id()?;
    if &expected != proposed {
        return Err(invalid());
    }
    Ok(())
}
impl NativeRemote {
    /// Read-only additive policy preview. Input is a closed rules document, not a request.
    pub fn preview_policy_rules(
        &self,
        workspace: &NativeWorkspace,
        rules_json: &[u8],
    ) -> RemoteResult<NativePolicyPreview> {
        let rules = checked_rules(rules_json)?;
        let lock = remote_lock(workspace)?;
        let binding = self.binding(workspace)?;
        let principal = self.policy_identity()?;
        if binding.pending.is_some() {
            let preview = load_intent(workspace)?;
            self.check_policy_scope(workspace, &preview, &principal)?;
            let inline = preview
                .inline_rules
                .as_ref()
                .ok_or(NativeRemoteError::PublicationPending)?;
            if inline.replacement.is_some()
                || binding.pending.as_ref() != Some(&preview.request)
                || serde_jcs::to_vec(&inline.rules)? != serde_jcs::to_vec(&rules)?
            {
                return Err(NativeRemoteError::PublicationPending);
            }
            return Ok(preview);
        }
        drop(lock);
        let base = self.bootstrap(&binding.space_id)?;
        if base.current_policy_set.record.policy_versions.len() >= 32 {
            return Err(invalid());
        }
        let mut preview = self.preview_stored_policy(workspace, &base.current_policy_set_id)?;
        if preview.expected_policy_set_id != base.current_policy_set_id
            || preview.expected_authorization_epoch != base.authorization_epoch
        {
            return Err(invalid());
        }
        let evidence = evidence_id(&serde_jcs::to_vec(&rules)?)?;
        let nonce = uuid::Uuid::new_v4();
        let id = PolicyId::parse(format!("policy:{nonce}")).map_err(|_| invalid())?;
        let version =
            PolicyVersionId::parse(format!("policy-version:{nonce}")).map_err(|_| invalid())?;
        let timestamp = MutationStamp::fresh()?.timestamp;
        let mut product = ProductEnvelope::Policy {
            product_id: id.clone(),
            product_object_id: evidence.clone(),
            depends_on_step_ids: vec![],
            depends_on_product_ids: vec![],
            previous_product_object_id: Nullable::null(),
            record: PolicyRecord {
                schema_version: PolicySchemaVersion::V1,
                policy_id: id.clone(),
                version_id: version.clone(),
                space_id: binding.space_id,
                rules_object_id: evidence.clone(),
                created_at: timestamp,
            },
        };
        let object = product.computed_object_id().map_err(|_| invalid())?;
        if let ProductEnvelope::Policy {
            product_object_id, ..
        } = &mut product
        {
            *product_object_id = object.clone();
        }
        let mut set = base.current_policy_set.record.clone();
        set.policy_versions.push(PolicyVersionEntry {
            policy_id: id,
            policy_version_id: version,
            policy_product_object_id: object,
            dependency_object_ids: vec![evidence.clone()],
        });
        set.policy_versions
            .sort_by(|a, b| a.policy_id.cmp(&b.policy_id));
        set.policy_set_id = set.computed_id()?;
        preview.schema_version = SCHEMA.into();
        preview.proposed_policy_set_id = set.policy_set_id.clone();
        preview.request.products = vec![product];
        preview
            .request
            .policy_root_mutation
            .as_mut()
            .ok_or_else(invalid)?
            .proposed_policy_set = ProposedPolicySet::Inline { policy_set: set };
        preview.request.request_digest = preview.request.computed_request_digest()?;
        preview.inline_rules = Some(NativeInlinePolicyRules {
            object_id: evidence,
            rules,
            base_policy_set: base.current_policy_set.record,
            replacement: None,
        });
        validate(&preview)?;
        Ok(preview)
    }
}
pub(super) fn upload(
    remote: &NativeRemote,
    binding: &Binding,
    preview: &NativePolicyPreview,
) -> RemoteResult<()> {
    let inline = preview.inline_rules.as_ref().ok_or_else(invalid)?;
    let bytes = serde_jcs::to_vec(&inline.rules)?;
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Evidence,
        object_id: inline.object_id.clone(),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    };
    let request = ObjectPlanRequest {
        protocol: SyncV3Protocol::V3,
        message_type: ObjectPlanRequestType::Request,
        space_id: binding.space_id.clone(),
        client_id: binding.client_id.clone(),
        request_id: new_request_id(),
        objects: vec![descriptor.clone()],
    };
    let plan: ObjectPlanResponse = remote.post(
        &format!("v3/spaces/{}/object-plans", binding.space_id),
        &request,
    )?;
    plan.validate_v3().map_err(|_| invalid())?;
    if plan.space_id != binding.space_id
        || plan.client_id != binding.client_id
        || plan.request_id != request.request_id
        || plan.principal_id != preview.principal_id
    {
        return Err(invalid());
    }
    if plan.present_objects == [descriptor.clone()] && plan.missing_objects.is_empty() {
        return Ok(());
    }
    if plan.missing_objects != [descriptor.clone()]
        || !plan.present_objects.is_empty()
        || plan.upload_capabilities.len() != 1
    {
        return Err(invalid());
    }
    let cap = &plan.upload_capabilities[0];
    if cap.descriptor != descriptor {
        return Err(invalid());
    }
    checked(
        remote
            .http
            .put(remote.transfer_url(&cap.upload_url)?)
            .timeout(remote.transfer_timeout())
            .bearer_auth(&cap.token)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()?,
    )?;
    Ok(())
}
