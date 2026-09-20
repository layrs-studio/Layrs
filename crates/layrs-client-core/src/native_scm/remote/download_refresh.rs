//! Exact-authority capability renewal; never refresh a different epoch or closure.
use super::*;

impl NativeRemote {
    pub(super) fn fetch(
        &self,
        binding: &Binding,
        claim: FetchClaim,
    ) -> RemoteResult<Vec<CanonicalObject>> {
        let mut objects = Vec::new();
        let mut bytes = 0usize;
        self.fetch_visit(binding, claim, |object| {
            let size = match &object {
                CanonicalObject::ChunkRaw(chunk) => chunk.bytes().len(),
                _ => encode(&object)?.1.len(),
            };
            bytes = bytes
                .checked_add(size)
                .filter(|n| *n <= MAX_TOTAL)
                .ok_or(NativeRemoteError::Invalid("in-memory fetch exceeds limits"))?;
            objects.push(object);
            Ok(())
        })?;
        Ok(objects)
    }

    /// Visits verified individual objects without retaining their byte buffers.
    /// The caller must not commit references until the complete closure succeeds.
    pub(super) fn fetch_visit(
        &self,
        binding: &Binding,
        claim: FetchClaim,
        mut visit: impl FnMut(CanonicalObject) -> RemoteResult<()>,
    ) -> RemoteResult<()> {
        let plan = self.fetch_plan(binding, &claim, self.transfer_timeout())?;
        let index = |plan: &FetchPlanResponse| {
            plan.download_capabilities
                .iter()
                .map(|cap| (cap.descriptor.object_id.clone(), cap.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let mut active = index(&plan);
        for cap in &plan.download_capabilities {
            let current = active
                .get(&cap.descriptor.object_id)
                .ok_or(NativeRemoteError::Invalid("renewed download object absent"))?
                .clone();
            visit(self.download_object(&current, |remaining| {
                let renewed = self.fetch_plan(binding, &claim, remaining)?;
                same_authority_and_objects(&plan, &renewed)?;
                let next = renewed
                    .download_capabilities
                    .iter()
                    .find(|next| next.descriptor == cap.descriptor)
                    .ok_or(NativeRemoteError::Invalid("renewed download object absent"))?
                    .clone();
                active = index(&renewed);
                Ok(next)
            })?)?;
        }
        Ok(())
    }

    fn fetch_plan(
        &self,
        binding: &Binding,
        claim: &FetchClaim,
        timeout: Duration,
    ) -> RemoteResult<FetchPlanResponse> {
        let request = FetchPlanRequest {
            protocol: SyncV3Protocol::V3,
            message_type: FetchPlanRequestType::Request,
            space_id: binding.space_id.clone(),
            client_id: binding.client_id.clone(),
            request_id: new_request_id(),
            claims: vec![claim.clone()],
            page_cursor: Nullable::null(),
            max_objects: 10_000,
        };
        self.transfer_check()?;
        let plan: FetchPlanResponse = self.transfer_result(
            self.http
                .post(self.url(&format!("v3/spaces/{}/fetch-plans", binding.space_id))?)
                .bearer_auth(&self.token)
                .json(&request)
                .timeout(timeout.min(self.transfer_timeout()))
                .send()
                .map_err(NativeRemoteError::from)
                .and_then(json),
        )?;
        plan.validate_v3()
            .map_err(|_| NativeRemoteError::Invalid("invalid fetch plan"))?;
        if plan.space_id != binding.space_id
            || plan.client_id != binding.client_id
            || plan.request_id != request.request_id
            || !plan.denied_claims.is_empty()
            || plan.has_more
            || plan.download_capabilities.is_empty()
        {
            return Err(NativeRemoteError::Invalid(
                "fetch denied, incomplete or binding mismatch",
            ));
        }
        validate_fetch_descriptors(&plan, claim)?;
        let mut total = 0u64;
        for cap in &plan.download_capabilities {
            total = total
                .checked_add(cap.descriptor.canonical_byte_len)
                .ok_or(NativeRemoteError::Invalid("fetch plan exceeds limits"))?;
            if total > MAX_STREAM_BYTES {
                return Err(NativeRemoteError::Invalid("fetch plan exceeds limits"));
            }
            if &cap.claim != claim
                || cap.descriptor.canonical_byte_len > MAX_OBJECT
                || cap.descriptor.encoded_byte_len > MAX_OBJECT
            {
                return Err(NativeRemoteError::Invalid(
                    "download bounds or claim mismatch",
                ));
            }
        }
        Ok(plan)
    }
}

fn same_authority_and_objects(
    before: &FetchPlanResponse,
    after: &FetchPlanResponse,
) -> RemoteResult<()> {
    let descriptors = |plan: &FetchPlanResponse| {
        plan.download_capabilities
            .iter()
            .map(|cap| (cap.descriptor.object_id.clone(), cap.descriptor.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    if before.space_id != after.space_id
        || before.principal_id != after.principal_id
        || before.authorization_epoch != after.authorization_epoch
        || before.client_id != after.client_id
        || before.file_manifest_selections != after.file_manifest_selections
        || descriptors(before) != descriptors(after)
    {
        return Err(NativeRemoteError::Invalid(
            "download renewal changed authority or object closure",
        ));
    }
    Ok(())
}

pub(super) fn expired_download(error: &NativeRemoteError, cap: &DownloadCapability) -> bool {
    matches!(error, NativeRemoteError::Service { status: 404, code } if code == "transfer_unavailable")
        && time::OffsetDateTime::parse(
            cap.expires_at.as_str(),
            &time::format_description::well_known::Rfc3339,
        )
        .is_ok_and(|expiry| expiry <= time::OffsetDateTime::now_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> FetchPlanResponse {
        let mut plan: FetchPlanResponse = serde_json::from_str(include_str!(
            "../../../../../docs/contracts/sync-v3/examples/fetch-plan-response.json"
        ))
        .unwrap();
        let id = format!("blake3:{}", "3".repeat(64));
        plan.download_capabilities.push(serde_json::from_value(serde_json::json!({
            "principalId":plan.principal_id,"authorizationEpoch":plan.authorization_epoch,
            "spaceId":plan.space_id,"clientId":plan.client_id,"requestId":plan.request_id,"planId":plan.plan_id,
            "claim":{"kind":"object","object":{"kind":"step","objectId":id},"root":{"kind":"step","stepId":id},"repoPath":null},
            "descriptor":{"kind":"step","objectId":id,"canonicalByteLen":100,"encodedByteLen":100,"compression":"identity"},
            "claimDigest":id,"method":"GET","downloadUrl":"https://example.invalid/v3/transfers/downloads",
            "token":"test-token","expiresAt":"2026-09-20T12:05:00Z","resumeToken":"test-resume","acceptRanges":true
        })).unwrap());
        plan
    }
    #[test]
    fn renewal_cannot_change_principal_epoch_space_selection_or_objects() {
        let before = plan();
        let mut after = before.clone();
        after.request_id = RequestId::parse("request:fresh").unwrap();
        after.plan_id = PlanId::parse("plan:fresh").unwrap();
        for cap in &mut after.download_capabilities {
            cap.token = "renewed-token".into();
        }
        same_authority_and_objects(&before, &after).unwrap();
        for change in 0..7 {
            let mut other = after.clone();
            match change {
                0 => other.authorization_epoch += 1,
                1 => other.principal_id = PrincipalId::parse("principal:other").unwrap(),
                2 => other.space_id = SpaceId::parse("space:other").unwrap(),
                3 => other.client_id = ClientId::parse("client:other").unwrap(),
                4 => {
                    other.download_capabilities.pop();
                }
                5 => other.download_capabilities[0].descriptor.canonical_byte_len += 1,
                _ => other.file_manifest_selections.clear(),
            }
            assert!(
                same_authority_and_objects(&before, &other).is_err(),
                "change {change}"
            );
        }
    }
    #[test]
    fn only_chronologically_expired_unavailable_capability_is_renewable() {
        let mut cap = plan().download_capabilities.remove(0);
        cap.expires_at = Timestamp::parse("2000-01-01T00:00:00Z").unwrap();
        let unavailable = NativeRemoteError::Service {
            status: 404,
            code: "transfer_unavailable".into(),
        };
        assert!(expired_download(&unavailable, &cap));
        for error in [
            NativeRemoteError::Http(404),
            NativeRemoteError::Http(401),
            NativeRemoteError::Invalid("download content identity mismatch"),
            NativeRemoteError::Service {
                status: 409,
                code: "authorization_epoch_changed".into(),
            },
            NativeRemoteError::Cancelled,
        ] {
            assert!(!expired_download(&error, &cap));
        }
        cap.expires_at = Timestamp::parse("2999-01-01T00:00:00Z").unwrap();
        assert!(!expired_download(&unavailable, &cap));
    }
}
