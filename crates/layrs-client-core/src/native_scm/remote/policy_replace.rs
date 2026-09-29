//! Explicit replacement; old additive previews can never become replacements.
use super::*;
#[cfg(test)]
#[path = "policy_replace_tests.rs"]
mod tests;

fn replace(
    mut preview: NativePolicyPreview,
    base: PolicySet,
    policy_id: &PolicyId,
    rules: NativePolicyRules,
) -> RemoteResult<NativePolicyPreview> {
    let previous = base
        .policy_versions
        .iter()
        .find(|p| &p.policy_id == policy_id)
        .ok_or_else(invalid)?;
    if base.policy_set_id != preview.expected_policy_set_id
        || base.computed_id()? != base.policy_set_id
    {
        return Err(invalid());
    }
    let evidence = evidence_id(&serde_jcs::to_vec(&rules)?)?;
    let previous_object = previous.policy_product_object_id.clone();
    let version = PolicyVersionId::parse(format!("policy-version:{}", uuid::Uuid::new_v4()))
        .map_err(|_| invalid())?;
    let mut product = ProductEnvelope::Policy {
        product_id: policy_id.clone(),
        product_object_id: evidence.clone(),
        depends_on_step_ids: vec![],
        depends_on_product_ids: vec![
            ProductLogicalId::parse(policy_id.as_str()).map_err(|_| invalid())?
        ],
        previous_product_object_id: Nullable::some(previous_object.clone()),
        record: PolicyRecord {
            schema_version: PolicySchemaVersion::V1,
            policy_id: policy_id.clone(),
            version_id: version.clone(),
            space_id: preview.space_id.clone(),
            rules_object_id: evidence.clone(),
            created_at: MutationStamp::fresh()?.timestamp,
        },
    };
    let object = product.computed_object_id().map_err(|_| invalid())?;
    if let ProductEnvelope::Policy {
        product_object_id, ..
    } = &mut product
    {
        *product_object_id = object.clone();
    }
    let mut proposed = base.clone();
    let entry = proposed
        .policy_versions
        .iter_mut()
        .find(|p| &p.policy_id == policy_id)
        .ok_or_else(invalid)?;
    let mut dependencies = vec![evidence.clone(), previous_object.clone()];
    dependencies.sort();
    dependencies.dedup();
    *entry = PolicyVersionEntry {
        policy_id: policy_id.clone(),
        policy_version_id: version,
        policy_product_object_id: object,
        dependency_object_ids: dependencies,
    };
    proposed.policy_set_id = proposed.computed_id()?;
    preview.schema_version = REPLACE_SCHEMA.into();
    preview.proposed_policy_set_id = proposed.policy_set_id.clone();
    preview.request.products = vec![product];
    preview
        .request
        .policy_root_mutation
        .as_mut()
        .ok_or_else(invalid)?
        .proposed_policy_set = ProposedPolicySet::Inline {
        policy_set: proposed,
    };
    preview.request.request_digest = preview.request.computed_request_digest()?;
    preview.inline_rules = Some(NativeInlinePolicyRules {
        object_id: evidence,
        rules,
        base_policy_set: base,
        replacement: Some(NativePolicyReplacement {
            policy_id: policy_id.clone(),
            previous_product_object_id: previous_object,
        }),
    });
    validate(&preview)?;
    Ok(preview)
}

impl NativeRemote {
    /// Read-only preview replacing exactly one current Policy, retaining every other entry.
    pub fn preview_replace_policy_rules(
        &self,
        workspace: &NativeWorkspace,
        policy_id: &PolicyId,
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
            if inline
                .replacement
                .as_ref()
                .is_none_or(|r| &r.policy_id != policy_id)
                || binding.pending.as_ref() != Some(&preview.request)
                || serde_jcs::to_vec(&inline.rules)? != serde_jcs::to_vec(&rules)?
            {
                return Err(NativeRemoteError::PublicationPending);
            }
            return Ok(preview);
        }
        drop(lock);
        let base = self.bootstrap(&binding.space_id)?;
        let preview = self.preview_stored_policy(workspace, &base.current_policy_set_id)?;
        if preview.expected_policy_set_id != base.current_policy_set_id
            || preview.expected_authorization_epoch != base.authorization_epoch
        {
            return Err(invalid());
        }
        replace(preview, base.current_policy_set.record, policy_id, rules)
    }
}
