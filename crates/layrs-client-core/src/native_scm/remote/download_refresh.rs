//! Exact-authority capability renewal; never refresh a different epoch or closure.
use super::*;

pub(super) const CLAIM_BATCH: usize = 32;

enum BatchPlan {
    Ready(FetchPlanResponse),
    Split,
}

impl NativeRemote {
    /// Visits verified individual objects without retaining their byte buffers.
    /// The caller must not commit references until the complete closure succeeds.
    pub(super) fn fetch_visit(
        &self,
        binding: &Binding,
        claim: FetchClaim,
        visit: impl FnMut(CanonicalObject) -> RemoteResult<()>,
    ) -> RemoteResult<()> {
        self.fetch_visit_from_store(binding, &[claim], None, visit)
    }

    /// A complete batch is validated before visiting any of its objects. Pages
    /// are retried as smaller complete batches, never admitted as a partial plan.
    pub(super) fn fetch_visit_from_store(
        &self,
        binding: &Binding,
        claims: &[FetchClaim],
        local_workspace: Option<&NativeWorkspace>,
        mut visit: impl FnMut(CanonicalObject) -> RemoteResult<()>,
    ) -> RemoteResult<()> {
        let mut pending: Vec<_> = claims.chunks(CLAIM_BATCH).rev().collect();
        while let Some(claims) = pending.pop() {
            let plan = match self.fetch_batch_plan(binding, claims, self.transfer_timeout()) {
                // The server also bounds the complete persisted snapshot, so a
                // batch may exceed that limit before it can return a page.
                Err(NativeRemoteError::Service { status: 413, code })
                    if code == "payload_too_large" && claims.len() > 1 =>
                {
                    BatchPlan::Split
                }
                result => result?,
            };
            let BatchPlan::Ready(plan) = plan else {
                let (left, right) = claims.split_at(claims.len() / 2);
                pending.extend([right, left]);
                continue;
            };
            let mut active = capability_index(&plan)?;
            for cap in &plan.download_capabilities {
                self.transfer_check()?;
                let key = capability_key(cap)?;
                let current = active
                    .get(&key)
                    .ok_or(NativeRemoteError::Invalid("renewed download object absent"))?
                    .clone();
                let object =
                    self.download_object_from_store(&current, local_workspace, |remaining| {
                        let BatchPlan::Ready(renewed) =
                            self.fetch_batch_plan(binding, claims, remaining)?
                        else {
                            return Err(NativeRemoteError::Invalid(
                                "download renewal changed authority or object closure",
                            ));
                        };
                        same_authority_and_objects(&plan, &renewed)?;
                        active = capability_index(&renewed)?;
                        let next = active
                            .get(&key)
                            .ok_or(NativeRemoteError::Invalid("renewed download object absent"))?
                            .clone();
                        Ok(next)
                    })?;
                self.transfer_check()?;
                visit(object)?;
            }
        }
        Ok(())
    }

    pub(super) fn fetch_plan(
        &self,
        binding: &Binding,
        claim: &FetchClaim,
        timeout: Duration,
    ) -> RemoteResult<FetchPlanResponse> {
        let BatchPlan::Ready(plan) =
            self.fetch_batch_plan(binding, std::slice::from_ref(claim), timeout)?
        else {
            return Err(NativeRemoteError::Invalid("incomplete single fetch claim"));
        };
        Ok(plan)
    }

    fn fetch_batch_plan(
        &self,
        binding: &Binding,
        claims: &[FetchClaim],
        timeout: Duration,
    ) -> RemoteResult<BatchPlan> {
        if claims.is_empty() || claims.len() > CLAIM_BATCH {
            return Err(NativeRemoteError::Invalid(
                "fetch claim batch exceeds limits",
            ));
        }
        let request = FetchPlanRequest {
            protocol: SyncV3Protocol::V3,
            message_type: FetchPlanRequestType::Request,
            space_id: binding.space_id.clone(),
            client_id: binding.client_id.clone(),
            request_id: new_request_id(),
            claims: claims.to_vec(),
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
        {
            return Err(NativeRemoteError::Invalid(
                "fetch denied, incomplete or binding mismatch",
            ));
        }
        bounded_batch_plan(plan, claims)
    }
}

fn bounded_batch_plan(plan: FetchPlanResponse, claims: &[FetchClaim]) -> RemoteResult<BatchPlan> {
    let mut total = 0u64;
    for cap in &plan.download_capabilities {
        if !claims.contains(&cap.claim)
            || cap.descriptor.canonical_byte_len > MAX_OBJECT
            || cap.descriptor.encoded_byte_len > MAX_OBJECT
        {
            return Err(NativeRemoteError::Invalid(
                "download bounds or claim mismatch",
            ));
        }
        total = total
            .checked_add(cap.descriptor.canonical_byte_len)
            .ok_or(NativeRemoteError::Invalid("fetch plan exceeds limits"))?;
    }
    if !plan.has_more {
        validate_batch_descriptors(&plan, claims)?;
    }
    if plan.has_more || total > MAX_STREAM_BYTES {
        // The byte budget applies to one transfer plan, including manifests.
        // Several individually valid File claims can exceed it only together.
        if claims.len() > 1 {
            return Ok(BatchPlan::Split);
        }
        return Err(NativeRemoteError::Invalid(
            "fetch denied, incomplete or exceeds limits",
        ));
    }
    Ok(BatchPlan::Ready(plan))
}

type CapabilityKey = (Vec<u8>, ObjectDescriptor);
fn capability_key(cap: &DownloadCapability) -> RemoteResult<CapabilityKey> {
    Ok((serde_json::to_vec(&cap.claim)?, cap.descriptor.clone()))
}
fn capability_index(
    plan: &FetchPlanResponse,
) -> RemoteResult<BTreeMap<CapabilityKey, DownloadCapability>> {
    plan.download_capabilities
        .iter()
        .map(|cap| Ok((capability_key(cap)?, cap.clone())))
        .collect()
}

fn validate_batch_descriptors(plan: &FetchPlanResponse, claims: &[FetchClaim]) -> RemoteResult<()> {
    let mut selections = plan.file_manifest_selections.clone();
    let mut seen = BTreeSet::new();
    for claim in claims {
        if !seen.insert(serde_json::to_vec(claim)?) {
            return Err(NativeRemoteError::Invalid("duplicate fetch claim"));
        }
        let caps: Vec<_> = plan
            .download_capabilities
            .iter()
            .filter(|cap| &cap.claim == claim)
            .cloned()
            .collect();
        let selection = if let FetchClaim::File { file_id, .. } = claim {
            let index = selections
                .iter()
                .position(|selection| {
                    &selection.file_id == file_id
                        && caps
                            .iter()
                            .any(|cap| cap.descriptor == selection.manifest_descriptor)
                })
                .ok_or(NativeRemoteError::Invalid("missing file fetch selection"))?;
            vec![selections.remove(index)]
        } else {
            vec![]
        };
        // Reuse the strict single-claim closure check without cloning the other
        // claims' buffers. Selection multiplicity matters for equal files at
        // different paths; each must have its own exact authorized capability.
        let single = FetchPlanResponse {
            protocol: plan.protocol,
            message_type: plan.message_type,
            principal_id: plan.principal_id.clone(),
            authorization_epoch: plan.authorization_epoch,
            space_id: plan.space_id.clone(),
            client_id: plan.client_id.clone(),
            request_id: plan.request_id.clone(),
            plan_id: plan.plan_id.clone(),
            file_manifest_selections: selection,
            download_capabilities: caps,
            denied_claims: vec![],
            next_page_cursor: Nullable::null(),
            has_more: false,
            expires_at: plan.expires_at.clone(),
        };
        validate_fetch_descriptors(&single, claim)?;
    }
    if !selections.is_empty() {
        return Err(NativeRemoteError::Invalid(
            "unrequested file fetch selection",
        ));
    }
    Ok(())
}

fn same_authority_and_objects(
    before: &FetchPlanResponse,
    after: &FetchPlanResponse,
) -> RemoteResult<()> {
    let descriptors = |plan: &FetchPlanResponse| -> RemoteResult<Vec<Vec<u8>>> {
        let mut values = plan
            .download_capabilities
            .iter()
            .map(|cap| {
                serde_json::to_vec(&(
                    &cap.claim,
                    &cap.claim_digest,
                    &cap.descriptor,
                    cap.accept_ranges,
                ))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        values.sort();
        Ok(values)
    };
    if before.space_id != after.space_id
        || before.principal_id != after.principal_id
        || before.authorization_epoch != after.authorization_epoch
        || before.client_id != after.client_id
        || before.file_manifest_selections != after.file_manifest_selections
        || after.has_more
        || !after.denied_claims.is_empty()
        || descriptors(before)? != descriptors(after)?
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

    #[test]
    fn renewal_keeps_distinct_authority_for_equal_object_descriptors() {
        let mut before = plan();
        let mut second = before.download_capabilities[0].clone();
        if let FetchClaim::Object { repo_path, .. } = &mut second.claim {
            *repo_path = Nullable::some(RepoPath::parse("other.txt").unwrap());
        }
        before.download_capabilities.push(second);
        let mut after = before.clone();
        after.download_capabilities.reverse();
        same_authority_and_objects(&before, &after).unwrap();
        after.download_capabilities.pop();
        assert!(same_authority_and_objects(&before, &after).is_err());
        let mut after = before.clone();
        after.download_capabilities[0].accept_ranges = false;
        assert!(same_authority_and_objects(&before, &after).is_err());
    }

    /// Descriptor-only large files: no corresponding payload is allocated.
    fn large_file_plan(
        files: usize,
        chunks_per_file: usize,
    ) -> (FetchPlanResponse, Vec<FetchClaim>) {
        let id = |n: usize| ObjectId::parse(format!("blake3:{n:064x}")).unwrap();
        let mut result = plan();
        let template = result.download_capabilities[0].clone();
        result.download_capabilities.clear();
        result.file_manifest_selections.clear();
        let mut claims = Vec::new();
        for file in 0..files {
            let file_id = FileId::parse(id(file + 10).as_str()).unwrap();
            let claim = FetchClaim::File {
                file_id: file_id.clone(),
                preferred_manifest_ids: vec![],
                root: AuthorizationRoot::Step {
                    step_id: StepId::parse(id(1).as_str()).unwrap(),
                },
                repo_path: RepoPath::parse(format!("{file}.bin")).unwrap(),
            };
            let manifest = ObjectDescriptor {
                kind: ObjectKind::FileManifest,
                object_id: id(file + 20),
                canonical_byte_len: 4096,
                encoded_byte_len: 4096,
                compression: Compression::Identity,
            };
            let chunks: Vec<_> = (0..chunks_per_file)
                .map(|chunk| ObjectDescriptor {
                    kind: ObjectKind::Chunk,
                    object_id: id(1000 + file * 100 + chunk),
                    canonical_byte_len: MAX_OBJECT,
                    encoded_byte_len: MAX_OBJECT,
                    compression: Compression::Identity,
                })
                .collect();
            result.file_manifest_selections.push(FileManifestSelection {
                file_id,
                file_manifest_id: FileManifestId::parse(manifest.object_id.as_str()).unwrap(),
                manifest_descriptor: manifest.clone(),
                chunk_descriptors: chunks.clone(),
            });
            for descriptor in std::iter::once(manifest).chain(chunks) {
                result.download_capabilities.push(DownloadCapability {
                    claim: claim.clone(),
                    descriptor,
                    ..template.clone()
                });
            }
            claims.push(claim);
        }
        (result, claims)
    }

    #[test]
    fn aggregate_manifest_overhead_splits_valid_four_gib_file_budget() {
        let (plan, claims) = large_file_plan(4, 16);
        assert_eq!(4 * 16 * MAX_OBJECT, MAX_STREAM_BYTES);
        assert_eq!(16 * MAX_OBJECT, MAX_FILE_STREAM);
        assert!(matches!(
            bounded_batch_plan(plan.clone(), &claims).unwrap(),
            BatchPlan::Split
        ));
        // Both halves produced by the transfer loop retain full exact claims.
        for subset in claims.chunks(2) {
            let mut half = plan.clone();
            half.download_capabilities
                .retain(|cap| subset.contains(&cap.claim));
            half.file_manifest_selections.retain(|selection| subset.iter().any(|claim| matches!(claim, FetchClaim::File { file_id, .. } if file_id == &selection.file_id)));
            assert!(matches!(
                bounded_batch_plan(half, subset).unwrap(),
                BatchPlan::Ready(_)
            ));
        }
        let mut exact = plan;
        for selection in &mut exact.file_manifest_selections {
            let chunk = selection.chunk_descriptors.last_mut().unwrap();
            chunk.canonical_byte_len -= 4096;
            chunk.encoded_byte_len -= 4096;
            let cap = exact
                .download_capabilities
                .iter_mut()
                .find(|cap| cap.descriptor.object_id == chunk.object_id)
                .unwrap();
            cap.descriptor = chunk.clone();
        }
        assert!(matches!(
            bounded_batch_plan(exact, &claims).unwrap(),
            BatchPlan::Ready(_)
        ));
        let (oversized, single) = large_file_plan(1, 65);
        assert!(matches!(
            bounded_batch_plan(oversized, &single),
            Err(NativeRemoteError::Invalid(
                "fetch denied, incomplete or exceeds limits"
            ))
        ));
        let (mut oversized_object, single) = large_file_plan(1, 16);
        oversized_object.download_capabilities[0]
            .descriptor
            .encoded_byte_len = MAX_OBJECT + 1;
        assert!(matches!(
            bounded_batch_plan(oversized_object, &single),
            Err(NativeRemoteError::Invalid(
                "download bounds or claim mismatch"
            ))
        ));
    }
}
