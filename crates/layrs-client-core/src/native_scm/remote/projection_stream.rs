//! Resumable metadata-only Store3 publications; each upload carries at most 1MiB.
use super::projection_publish::completed_projection_publication;
use super::*;
use layrs_store_local::projection::{ProjectionIdentity, ProjectionStore, VersionedProjection};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedProjectionPublicationOutcome {
    pub receipt: ScopedPathPatchReceipt,
    pub projection: VersionedProjection,
    pub pending_local_acceptance: bool,
    pub historical: bool,
}
fn local(error: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(error.to_string()))
}
fn binding(
    identity: &ProjectionIdentity,
    digest: &str,
    layer: &LayerId,
    intent: ScopedPathPatchIntent,
) -> RemoteResult<(ClientId, IdempotencyKey)> {
    let intent = match intent {
        ScopedPathPatchIntent::Advance => "advance",
        ScopedPathPatchIntent::Candidate => "candidate",
    };
    let key = blake3::hash(format!("{digest}\0{layer}\0{intent}").as_bytes());
    Ok((
        ClientId::parse(format!(
            "client:{}",
            identity
                .worktree_id
                .as_str()
                .trim_start_matches("worktree:")
        ))
        .map_err(local)?,
        IdempotencyKey::parse(format!("projection-v2-{key}")).map_err(local)?,
    ))
}
impl NativeRemote {
    pub fn publish_projection(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
    ) -> RemoteResult<ProjectionPublicationOutcome> {
        // Old durable requests/receipts keep their exact protocol and digest.
        let legacy = if let Some((_, done)) = completed_projection_publication(store, sequence)? {
            serde_json::from_slice::<serde_json::Value>(&done.receipt_bytes)?
                .get("intent")
                .is_none()
        } else if let Some(pending) = store.pending_publication().map_err(local)? {
            serde_json::from_slice::<serde_json::Value>(&pending.request_bytes)?
                .get("request")
                .is_some()
        } else {
            false
        };
        if legacy {
            return self.publish_projection_v1(store, sequence, layer);
        }
        let result =
            self.publish_projection_scoped(store, sequence, layer, ScopedPathPatchIntent::Advance)?;
        Ok(ProjectionPublicationOutcome {
            receipt: PathPatchReceipt {
                request_digest: result.receipt.request_digest,
                receipt: result.receipt.receipt,
            },
            projection: result.projection,
            pending_local_acceptance: result.pending_local_acceptance,
            historical: result.historical,
        })
    }
    pub fn publish_projection_candidate(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
    ) -> RemoteResult<ScopedProjectionPublicationOutcome> {
        self.publish_projection_scoped(store, sequence, layer, ScopedPathPatchIntent::Candidate)
    }
    fn publish_projection_scoped(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
        intent: ScopedPathPatchIntent,
    ) -> RemoteResult<ScopedProjectionPublicationOutcome> {
        if self.origin().trim_end_matches('/') != store.identity().origin {
            return Err(NativeRemoteError::Invalid("projection origin mismatch"));
        }
        let completed = completed_projection_publication(store, sequence)?;
        let capture = store.captured_patch(sequence).map_err(local)?;
        let identity = completed
            .as_ref()
            .map_or(store.identity(), |(identity, _)| identity);
        if capture.identity != *identity {
            return Err(NativeRemoteError::Invalid(
                "publication capture identity mismatch",
            ));
        }
        let (client, key) = binding(identity, &capture.digest, layer, intent)?;
        if let Some((identity, done)) = completed {
            let receipt: ScopedPathPatchReceipt = serde_json::from_slice(&done.receipt_bytes)?;
            validate_receipt(
                &identity,
                &receipt,
                &client,
                &key,
                intent,
                &capture.digest,
                layer,
            )?;
            return Ok(ScopedProjectionPublicationOutcome {
                receipt,
                projection: done.projection,
                pending_local_acceptance: false,
                historical: identity != *store.identity(),
            });
        }
        let state = store.state().map_err(local)?;
        let pending = match store.pending_publication().map_err(local)? {
            Some(pending) => pending,
            None => {
                let request = self.prepare_scoped_projection(store, sequence, layer, intent)?;
                store
                    .begin_publication(sequence, state.generation, &serde_jcs::to_vec(&request)?)
                    .map_err(local)?
            }
        };
        let request: ScopedPathPatchRequest = serde_json::from_slice(&pending.request_bytes)?;
        request.validate().map_err(local)?;
        if pending.sequence != sequence
            || pending.projection_generation != state.generation
            || pending.projection_digest != state.digest
            || request.intent != intent
            || &request.layer_id != layer
            || request.client_id != client
            || request.idempotency_key != key
            || request.space_id != store.identity().space_id
            || request.capture_digest.as_str() != capture.digest
            || request.base_step_id != state.snapshot.base_step_id
            || request.base_root_tree_id != state.snapshot.base_root_tree_id
        {
            return Err(NativeRemoteError::Invalid(
                "another exact partial publication is pending",
            ));
        }
        let receipt: ScopedPathPatchReceipt = match &pending.receipt_bytes {
            Some(bytes) => serde_json::from_slice(bytes)?,
            None => {
                let principal: PrincipalId =
                    serde_json::from_value(self.whoami()?["principal_id"].clone())?;
                if principal.as_str() != store.identity().principal_id {
                    return Err(NativeRemoteError::Invalid(
                        "projection publication principal changed",
                    ));
                }
                let receipt = self.submit_scoped_projection(store, sequence, &request)?;
                validate_receipt(
                    store.identity(),
                    &receipt,
                    &client,
                    &key,
                    intent,
                    &capture.digest,
                    layer,
                )?;
                if receipt.request_digest != request.request_digest {
                    return Err(NativeRemoteError::Invalid("scoped receipt digest mismatch"));
                }
                store
                    .record_publication_receipt(sequence, &serde_jcs::to_vec(&receipt)?)
                    .map_err(local)?;
                receipt
            }
        };
        validate_receipt(
            store.identity(),
            &receipt,
            &client,
            &key,
            intent,
            &capture.digest,
            layer,
        )?;
        if receipt.request_digest != request.request_digest {
            return Err(NativeRemoteError::Invalid("saved scoped receipt mismatch"));
        }
        let next = if receipt.receipt.status == ReceiptStatus::Rejected
            || intent == ScopedPathPatchIntent::Candidate
        {
            None
        } else {
            match self.accepted_projection(
                store,
                &state.snapshot,
                layer,
                &request.base_step_id,
                &request.changes,
                &receipt.receipt,
            ) {
                Ok(next) => Some(next),
                Err(_) => {
                    return Ok(ScopedProjectionPublicationOutcome {
                        receipt,
                        projection: state,
                        pending_local_acceptance: true,
                        historical: false,
                    });
                }
            }
        };
        let projection = store
            .finish_publication(sequence, &serde_jcs::to_vec(&receipt)?, next.as_ref())
            .map_err(local)?;
        Ok(ScopedProjectionPublicationOutcome {
            receipt,
            projection,
            pending_local_acceptance: false,
            historical: false,
        })
    }
    fn prepare_scoped_projection(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
        intent: ScopedPathPatchIntent,
    ) -> RemoteResult<ScopedPathPatchRequest> {
        store.require_publishable_capture(sequence).map_err(local)?;
        let state = store.state().map_err(local)?;
        let capture = store.captured_patch(sequence).map_err(local)?;
        if capture.projection_generation != state.generation
            || capture.projection_digest != state.digest
            || capture.identity != *store.identity()
            || capture.changes.is_empty()
        {
            return Err(NativeRemoteError::Invalid(
                "partial capture is empty or stale",
            ));
        }
        let base = self.bootstrap(&store.identity().space_id)?;
        if base.principal_id.as_str() != store.identity().principal_id
            || base.current_policy_set_id != state.snapshot.observed_policy_set_id
            || base.authorization_epoch != state.snapshot.observed_authorization_epoch
            || !base.layers.iter().any(|l| {
                l.record.id() == layer && l.head_step_ids == [state.snapshot.base_step_id.clone()]
            })
        {
            return Err(NativeRemoteError::Invalid(
                "receive current head and permissions before publishing",
            ));
        }
        let mut files = Vec::new();
        let mut changes = Vec::new();
        for change in &capture.changes {
            let replacement = if let Some(file) = &change.replacement {
                let manifest = store
                    .captured_file_manifest(sequence, &change.path)
                    .map_err(local)?;
                if manifest.file_id() != &file.file_id || manifest.byte_length() != file.bytes {
                    return Err(NativeRemoteError::Invalid("captured manifest mismatch"));
                }
                files.push(ScopedPatchFile {
                    path: change.path.clone(),
                    manifest,
                });
                Some(PathPatchEntry {
                    target: TreeEntryTarget::file(file.file_id.clone()),
                    executable: file.executable,
                })
            } else {
                None
            };
            changes.push(PathPatchChange {
                path: change.path.clone(),
                expected: Nullable(change.expected.as_ref().map(|old| PathPatchEntry {
                    target: TreeEntryTarget::file(old.file_id.clone()),
                    executable: old.executable,
                })),
                replacement: Nullable(replacement),
            });
        }
        let (client_id, idempotency_key) =
            binding(store.identity(), &capture.digest, layer, intent)?;
        let mut request = ScopedPathPatchRequest {
            schema_version: ScopedPathPatchSchema::V2,
            intent,
            space_id: base.space_id,
            client_id,
            idempotency_key,
            request_digest: base.current_policy_set_id.clone(),
            capture_digest: ObjectId::parse(capture.digest).map_err(local)?,
            layer_id: layer.clone(),
            base_step_id: state.snapshot.base_step_id,
            base_root_tree_id: state.snapshot.base_root_tree_id,
            expected_repository_view_id: base.current_repository_view.repository_view_id,
            expected_generation: base.current_repository_view_generation,
            expected_policy_set_id: base.current_policy_set_id,
            expected_authorization_epoch: base.authorization_epoch,
            changes,
            files,
        };
        request.request_digest = request.computed_request_digest()?;
        request.validate().map_err(local)?;
        Ok(request)
    }
    fn submit_scoped_projection(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        request: &ScopedPathPatchRequest,
    ) -> RemoteResult<ScopedPathPatchReceipt> {
        let endpoint = format!("v3/spaces/{}/path-patches/scoped", request.space_id);
        let progress: ScopedPatchProgress = self.post(&endpoint, request)?;
        validate_progress(request, &progress)?;
        let mut verified: Option<(u32, layrs_store_local::projection::VerifiedCapturedFile)> = None;
        for missing in &progress.missing {
            self.transfer_check()?;
            let file = &request.files[missing.file_index as usize];
            if verified
                .as_ref()
                .is_none_or(|(index, _)| *index != missing.file_index)
            {
                let witness = store
                    .verify_captured_file(sequence, &file.path)
                    .map_err(local)?;
                if witness.manifest() != &file.manifest {
                    return Err(NativeRemoteError::Invalid("saved upload manifest changed"));
                }
                verified = Some((missing.file_index, witness));
            }
            let expected = &file.manifest.chunks()[missing.ordinal as usize];
            let mut bytes = Vec::with_capacity(expected.byte_length() as usize);
            store
                .read_verified_captured_chunk(
                    &verified.as_ref().expect("verified file").1,
                    missing.ordinal as usize,
                    &mut bytes,
                )
                .map_err(local)?;
            let chunk = Chunk::new(bytes).map_err(local)?;
            if chunk.id() != expected.chunk_id()
                || chunk.bytes().len() as u64 != expected.byte_length()
            {
                return Err(NativeRemoteError::Invalid(
                    "local upload chunk differs from saved manifest",
                ));
            }
            let response = self
                .http
                .put(self.url(&format!(
                    "{endpoint}/{}/files/{}/chunks/{}",
                    progress.session_id, missing.file_index, missing.ordinal
                ))?)
                .bearer_auth(&self.token)
                .timeout(self.transfer_timeout())
                .body(chunk.bytes().to_vec())
                .send()?;
            checked(response)?;
        }
        self.transfer_begin_commit()?;
        self.post(
            &format!("{endpoint}/{}/finalize", progress.session_id),
            &serde_json::json!({}),
        )
    }
}
fn validate_receipt(
    identity: &ProjectionIdentity,
    receipt: &ScopedPathPatchReceipt,
    client: &ClientId,
    key: &IdempotencyKey,
    intent: ScopedPathPatchIntent,
    capture_digest: &str,
    layer: &LayerId,
) -> RemoteResult<()> {
    receipt.receipt.validate_v3().map_err(local)?;
    if receipt.intent != intent
        || receipt.receipt.principal_id.as_str() != identity.principal_id
        || receipt.receipt.space_id != identity.space_id
        || &receipt.receipt.client_id != client
        || &receipt.client_id != client
        || &receipt.idempotency_key != key
        || receipt.capture_digest.as_str() != capture_digest
        || &receipt.layer_id != layer
        || receipt.receipt.idempotency_key.as_str()
            != format!("scoped-v2-{}", &receipt.request_digest.as_str()[7..])
    {
        return Err(NativeRemoteError::Invalid(
            "scoped receipt correlation mismatch",
        ));
    }
    if intent == ScopedPathPatchIntent::Candidate
        && receipt.receipt.status == ReceiptStatus::Applied
        && (receipt.candidate_id.as_deref()
            != Some(format!("candidate:{}", &receipt.request_digest.as_str()[7..]).as_str())
            || receipt.receipt.view_result.0.is_some()
            || receipt.receipt.accepted_step_ids.len() > 1)
    {
        return Err(NativeRemoteError::Invalid(
            "candidate receipt unexpectedly moved the View or omitted provenance",
        ));
    }
    Ok(())
}
fn validate_progress(
    request: &ScopedPathPatchRequest,
    progress: &ScopedPatchProgress,
) -> RemoteResult<()> {
    if progress.request_digest != request.request_digest
        || progress.session_id.is_empty()
        || progress.session_id.len() > 128
        || !progress
            .session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_'))
        || progress.missing.len() > MAX_SCOPED_CHUNKS
    {
        return Err(NativeRemoteError::Invalid(
            "scoped progress identity or bounds mismatch",
        ));
    }
    let mut previous = None;
    for missing in &progress.missing {
        let key = (missing.file_index, missing.ordinal);
        let file = request
            .files
            .get(missing.file_index as usize)
            .ok_or(NativeRemoteError::Invalid("unknown upload file"))?;
        if file.path != missing.path
            || missing.ordinal as usize >= file.manifest.chunks().len()
            || previous.is_some_and(|old| old >= key)
        {
            return Err(NativeRemoteError::Invalid(
                "scoped missing chunks differ from exact request",
            ));
        }
        previous = Some(key);
    }
    Ok(())
}
