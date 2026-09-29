//! Read-only PolicySet-rooted inspection, with current authority checked before and after.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativePolicyRulesEntry {
    pub policy_id: PolicyId,
    pub policy_version_id: PolicyVersionId,
    pub policy_product_object_id: ObjectId,
    pub rules: NativePolicyRules,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativePolicyRulesSnapshot {
    pub principal_id: PrincipalId,
    pub space_id: SpaceId,
    pub policy_set_id: ObjectId,
    pub authorization_epoch: u64,
    pub policies: Vec<NativePolicyRulesEntry>,
}

impl NativeRemote {
    fn policy_bytes(
        &self,
        binding: &Binding,
        base: &SpaceBootstrap,
        kind: ObjectKind,
        id: &ObjectId,
    ) -> RemoteResult<Vec<u8>> {
        let claim = FetchClaim::Object {
            object: ObjectReference {
                kind,
                object_id: id.clone(),
            },
            root: AuthorizationRoot::PolicySet {
                policy_set_id: base.current_policy_set_id.clone(),
            },
            repo_path: Nullable::null(),
        };
        let plan = self.fetch_plan(binding, &claim, Duration::from_secs(30))?;
        if plan.principal_id != base.principal_id
            || plan.authorization_epoch != base.authorization_epoch
        {
            return Err(invalid());
        }
        let [cap] = plan.download_capabilities.as_slice() else {
            return Err(invalid());
        };
        let d = &cap.descriptor;
        if cap.principal_id != base.principal_id
            || cap.authorization_epoch != base.authorization_epoch
            || d.kind != kind
            || &d.object_id != id
            || d.compression != Compression::Identity
            || d.canonical_byte_len != d.encoded_byte_len
            || d.canonical_byte_len > MAX_RULES as u64
        {
            return Err(invalid());
        }
        let response = checked(
            self.http
                .get(self.transfer_url(&cap.download_url)?)
                .bearer_auth(&cap.token)
                .timeout(Duration::from_secs(30))
                .send()?,
        )?;
        let one_header = |name| {
            let mut values = response.headers().get_all(name).iter();
            let first = values.next().and_then(|v| v.to_str().ok());
            if values.next().is_some() {
                None
            } else {
                first
            }
        };
        if response.status() != reqwest::StatusCode::OK
            || one_header(reqwest::header::CONTENT_LENGTH).and_then(|v| v.parse::<u64>().ok())
                != Some(d.encoded_byte_len)
            || response
                .headers()
                .contains_key(reqwest::header::CONTENT_RANGE)
            || (response
                .headers()
                .contains_key(reqwest::header::CONTENT_ENCODING)
                && one_header(reqwest::header::CONTENT_ENCODING) != Some("identity"))
        {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_RULES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != d.canonical_byte_len {
            return Err(invalid());
        }
        Ok(bytes)
    }
    /// Does not populate a native Store or grant rights from previously cached bytes.
    pub fn read_policy_rules(
        &self,
        workspace: &NativeWorkspace,
    ) -> RemoteResult<NativePolicyRulesSnapshot> {
        let binding = self.binding(workspace)?;
        let base = self.bootstrap(&binding.space_id)?;
        if base.current_policy_set.record.policy_versions.len() > 32
            || base.current_policy_set.record.computed_id()? != base.current_policy_set_id
        {
            return Err(invalid());
        }
        let mut policies = Vec::new();
        for entry in &base.current_policy_set.record.policy_versions {
            let bytes = self.policy_bytes(
                &binding,
                &base,
                ObjectKind::Product,
                &entry.policy_product_object_id,
            )?;
            let product: ProductEnvelope = serde_json::from_slice(&bytes)?;
            product.validate_v3().map_err(|_| invalid())?;
            if product.object_id() != &entry.policy_product_object_id
                || product.canonical_bytes().map_err(|_| invalid())? != bytes
            {
                return Err(invalid());
            }
            let ProductEnvelope::Policy {
                product_id, record, ..
            } = product
            else {
                return Err(invalid());
            };
            if product_id != entry.policy_id
                || record.policy_id != entry.policy_id
                || record.version_id != entry.policy_version_id
                || record.space_id != binding.space_id
                || !entry
                    .dependency_object_ids
                    .contains(&record.rules_object_id)
            {
                return Err(invalid());
            }
            let bytes = self.policy_bytes(
                &binding,
                &base,
                ObjectKind::Evidence,
                &record.rules_object_id,
            )?;
            let rules = checked_rules(&bytes)?;
            if evidence_id(&bytes)? != record.rules_object_id || serde_jcs::to_vec(&rules)? != bytes
            {
                return Err(invalid());
            }
            policies.push(NativePolicyRulesEntry {
                policy_id: entry.policy_id.clone(),
                policy_version_id: entry.policy_version_id.clone(),
                policy_product_object_id: entry.policy_product_object_id.clone(),
                rules,
            });
        }
        let after = self.bootstrap(&binding.space_id)?;
        if after.principal_id != base.principal_id
            || after.space_id != base.space_id
            || after.current_policy_set_id != base.current_policy_set_id
            || after.authorization_epoch != base.authorization_epoch
        {
            return Err(invalid());
        }
        Ok(NativePolicyRulesSnapshot {
            principal_id: base.principal_id,
            space_id: base.space_id,
            policy_set_id: base.current_policy_set_id,
            authorization_epoch: base.authorization_epoch,
            policies,
        })
    }
}
