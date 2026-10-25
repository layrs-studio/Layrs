//! Exact, durable publications from a format3 capture. No complete local Step.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use layrs_core::projection::{EntryState, ProjectionSnapshot};
use layrs_store_local::projection::{
    CompletedPublication, ProjectionIdentity, ProjectionStore, VersionedProjection,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionPublicationOutcome {
    pub receipt: PathPatchReceipt,
    pub projection: VersionedProjection,
    pub pending_local_acceptance: bool,
    pub historical: bool,
}
fn local(e: impl std::fmt::Display) -> NativeRemoteError {
    NativeRemoteError::Io(std::io::Error::other(e.to_string()))
}
/// Imported receipts retain the original Worktree correlation. An imported
/// capture without a completed receipt is never submitted from its destination.
pub(super) fn completed_projection_publication(
    store: &ProjectionStore,
    sequence: u64,
) -> RemoteResult<Option<(ProjectionIdentity, CompletedPublication)>> {
    if let Some(completed) = store.imported_publication(sequence).map_err(local)? {
        return Ok(Some(completed));
    }
    store.require_publishable_capture(sequence).map_err(local)?;
    Ok(store
        .completed_publication(sequence)
        .map_err(local)?
        .map(|completed| (store.identity().clone(), completed)))
}
fn publication_binding(
    worktree: &WorktreeId,
    capture_digest: &str,
    layer: &LayerId,
) -> RemoteResult<(ClientId, IdempotencyKey)> {
    let key = blake3::hash(format!("{capture_digest}\0{layer}").as_bytes());
    Ok((
        ClientId::parse(format!(
            "client:{}",
            worktree.as_str().trim_start_matches("worktree:")
        ))
        .map_err(local)?,
        IdempotencyKey::parse(format!("projection-{key}")).map_err(local)?,
    ))
}
impl NativeRemote {
    pub(super) fn publish_projection_v1(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
    ) -> RemoteResult<ProjectionPublicationOutcome> {
        if self.origin().trim_end_matches('/') != store.identity().origin {
            return Err(NativeRemoteError::Invalid("projection origin mismatch"));
        }
        if let Some((identity, completed)) = completed_projection_publication(store, sequence)? {
            let receipt: PathPatchReceipt = serde_json::from_slice(&completed.receipt_bytes)?;
            receipt.receipt.validate_v3().map_err(local)?;
            let capture = store.captured_patch(sequence).map_err(local)?;
            let (client, key) = publication_binding(&identity.worktree_id, &capture.digest, layer)?;
            if capture.identity != identity
                || receipt.receipt.principal_id.as_str() != identity.principal_id
                || receipt.receipt.space_id != identity.space_id
                || receipt.receipt.client_id != client
                || receipt.receipt.idempotency_key != key
                || receipt
                    .receipt
                    .view_result
                    .0
                    .as_ref()
                    .is_some_and(|result| result.layer_results.iter().any(|r| &r.layer_id != layer))
            {
                return Err(NativeRemoteError::Invalid(
                    "completed publication identity or Layer mismatch",
                ));
            }
            return Ok(ProjectionPublicationOutcome {
                receipt,
                projection: completed.projection,
                pending_local_acceptance: false,
                historical: identity != *store.identity(),
            });
        }
        let state = store.state().map_err(local)?;
        let pending = match store.pending_publication().map_err(local)? {
            Some(pending) => pending,
            None => {
                let submission = self.prepare_projection_publication(store, sequence, layer)?;
                store
                    .begin_publication(sequence, state.generation, &serde_jcs::to_vec(&submission)?)
                    .map_err(local)?
            }
        };
        let submission: PathPatchSubmission = serde_json::from_slice(&pending.request_bytes)?;
        if pending.sequence != sequence
            || &submission.request.layer_id != layer
            || pending.projection_generation != state.generation
            || pending.projection_digest != state.digest
            || submission.request.space_id != store.identity().space_id
            || submission.request.base_step_id != state.snapshot.base_step_id
        {
            return Err(NativeRemoteError::Invalid(
                "another exact partial publication is pending",
            ));
        }
        let receipt: PathPatchReceipt = match &pending.receipt_bytes {
            Some(bytes) => serde_json::from_slice(bytes)?,
            None => {
                // A saved request may not have reached the server yet. Do not
                // submit it under a different account after a session switch.
                let principal: PrincipalId =
                    serde_json::from_value(self.whoami()?["principal_id"].clone())?;
                if principal.as_str() != store.identity().principal_id {
                    return Err(NativeRemoteError::Invalid(
                        "projection publication principal changed",
                    ));
                }
                let receipt = self.submit_path_patch(&submission)?;
                if receipt.receipt.principal_id.as_str() != store.identity().principal_id {
                    return Err(NativeRemoteError::Invalid(
                        "projection receipt principal mismatch",
                    ));
                }
                store
                    .record_publication_receipt(sequence, &serde_jcs::to_vec(&receipt)?)
                    .map_err(local)?;
                receipt
            }
        };
        receipt.receipt.validate_v3().map_err(local)?;
        if receipt.request_digest != submission.request.request_digest
            || receipt.receipt.principal_id.as_str() != store.identity().principal_id
            || receipt.receipt.space_id != store.identity().space_id
            || receipt.receipt.client_id != submission.request.client_id
            || receipt.receipt.idempotency_key != submission.request.idempotency_key
        {
            return Err(NativeRemoteError::Invalid(
                "saved projection receipt correlation mismatch",
            ));
        }
        if receipt.receipt.status == ReceiptStatus::Rejected {
            let projection = store
                .finish_publication(sequence, &serde_jcs::to_vec(&receipt)?, None)
                .map_err(local)?;
            return Ok(ProjectionPublicationOutcome {
                receipt,
                projection,
                pending_local_acceptance: false,
                historical: false,
            });
        }
        // A committed receipt survives loss of bootstrap/read permission. Leave
        // acceptance pending rather than inventing a root or forgetting the commit.
        let next = match self.accepted_projection(
            store,
            &state.snapshot,
            &submission.request.layer_id,
            &submission.request.base_step_id,
            &submission.request.changes,
            &receipt.receipt,
        ) {
            Ok(next) => next,
            Err(_) => {
                return Ok(ProjectionPublicationOutcome {
                    receipt,
                    projection: state,
                    pending_local_acceptance: true,
                    historical: false,
                });
            }
        };
        let projection = store
            .finish_publication(sequence, &serde_jcs::to_vec(&receipt)?, Some(&next))
            .map_err(local)?;
        Ok(ProjectionPublicationOutcome {
            receipt,
            projection,
            pending_local_acceptance: false,
            historical: false,
        })
    }

    fn prepare_projection_publication(
        &self,
        store: &ProjectionStore,
        sequence: u64,
        layer: &LayerId,
    ) -> RemoteResult<PathPatchSubmission> {
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
                "partial checkout must receive current head and permissions before publishing",
            ));
        }
        let mut files = Vec::new();
        let mut changes = Vec::new();
        for change in &capture.changes {
            let replacement = if let Some(file) = &change.replacement {
                let mut bytes = Vec::new();
                store
                    .write_captured_file(sequence, &change.path, &mut bytes)
                    .map_err(local)?;
                if FileId::from_bytes(&bytes) != file.file_id || bytes.len() as u64 != file.bytes {
                    return Err(NativeRemoteError::Invalid("captured file mismatch"));
                }
                files.push(PathPatchFile {
                    path: change.path.clone(),
                    bytes_base64: STANDARD.encode(bytes),
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
            publication_binding(&store.identity().worktree_id, &capture.digest, layer)?;
        let mut request = PathPatchRequest {
            schema_version: PathPatchSchema::V1,
            space_id: base.space_id,
            client_id,
            idempotency_key,
            request_digest: base.current_policy_set_id.clone(),
            layer_id: layer.clone(),
            base_step_id: state.snapshot.base_step_id,
            expected_repository_view_id: base.current_repository_view.repository_view_id,
            expected_generation: base.current_repository_view_generation,
            expected_policy_set_id: base.current_policy_set_id,
            expected_authorization_epoch: base.authorization_epoch,
            changes,
        };
        request.request_digest = request.computed_request_digest()?;
        request.validate().map_err(local)?;
        Ok(PathPatchSubmission { request, files })
    }

    pub(super) fn accepted_projection(
        &self,
        store: &ProjectionStore,
        old: &ProjectionSnapshot,
        layer_id: &LayerId,
        base_step_id: &StepId,
        changes: &[PathPatchChange],
        receipt: &SyncTransactionReceipt,
    ) -> RemoteResult<ProjectionSnapshot> {
        let result = receipt
            .view_result
            .0
            .as_ref()
            .ok_or(NativeRemoteError::Invalid(
                "partial publication has no View result",
            ))?;
        if receipt.accepted_step_ids.len() > 1 || result.layer_results.len() != 1 {
            return Err(NativeRemoteError::Invalid(
                "unexpected partial publication result",
            ));
        }
        let layer = &result.layer_results[0];
        if layer.resulting_heads.len() != 1 {
            return Err(NativeRemoteError::Invalid(
                "partial publication requires one resulting head",
            ));
        }
        let step = &layer.resulting_heads[0];
        if &layer.layer_id != layer_id
            || layer.previous_heads != [base_step_id.clone()]
            || receipt
                .accepted_step_ids
                .first()
                .is_some_and(|accepted| accepted != step)
        {
            return Err(NativeRemoteError::Invalid(
                "partial publication Layer mismatch",
            ));
        }
        let current = self.bootstrap(&store.identity().space_id)?;
        if current.principal_id.as_str() != store.identity().principal_id {
            return Err(NativeRemoteError::Invalid("projection principal changed"));
        }
        let (root, entries) = self.collect_projection(&current, step)?;
        let mut next = old.clone();
        next.base_step_id = step.clone();
        next.base_root_tree_id = root;
        for change in changes {
            let entry = next
                .entries
                .iter_mut()
                .find(|e| e.path == change.path)
                .ok_or(NativeRemoteError::Invalid(
                    "patch path missing from local registry",
                ))?;
            entry.target = change.replacement.0.as_ref().map(|e| e.target.clone());
            entry.executable = change.replacement.0.as_ref().is_some_and(|e| e.executable);
            entry.state = if entry.target.is_some() {
                EntryState::Materialized
            } else {
                EntryState::Absent
            };
        }
        // Verify every exact leaf, including the protected ones, without fetching
        // any protected content. Permissions remain the explicitly observed base.
        let expected: BTreeMap<_, _> = next
            .entries
            .iter()
            .filter_map(|e| {
                e.target
                    .as_ref()
                    .map(|t| (e.path.clone(), (t.clone(), e.executable)))
            })
            .collect();
        let actual: BTreeMap<_, _> = entries
            .into_iter()
            .filter(|e| !matches!(e.target, TreeEntryTarget::Tree { .. }))
            .map(|e| (e.path, (e.target, e.executable)))
            .collect();
        if expected != actual {
            return Err(NativeRemoteError::Invalid(
                "published projection differs from exact patch",
            ));
        }
        next.validate().map_err(local)?;
        Ok(next)
    }

    pub(super) fn collect_projection(
        &self,
        base: &SpaceBootstrap,
        step: &StepId,
    ) -> RemoteResult<(TreeId, Vec<ProjectionEntry>)> {
        let mut request = ProjectionPageRequest {
            step_id: step.clone(),
            expected_policy_set_id: base.current_policy_set_id.clone(),
            expected_authorization_epoch: base.authorization_epoch,
            cursor: None,
            limit: 200,
        };
        let mut root = None;
        let mut entries = BTreeMap::new();
        let mut offset = 0u32;
        loop {
            let page = self.projection_page(&base.space_id, &request)?;
            if page.principal_id != base.principal_id
                || page.offset != offset
                || root.as_ref().is_some_and(|r| r != &page.root_tree_id)
            {
                return Err(NativeRemoteError::Invalid(
                    "projection pages changed identity",
                ));
            }
            root = Some(page.root_tree_id);
            offset = offset
                .checked_add(page.entries.len() as u32)
                .filter(|n| *n <= MAX_OBJECTS as u32)
                .ok_or(NativeRemoteError::Invalid("projection exceeds path budget"))?;
            for entry in page.entries {
                if entries.insert(entry.path.clone(), entry).is_some() {
                    return Err(NativeRemoteError::Invalid("projection repeats a path"));
                }
            }
            match page.next_cursor {
                None => break,
                Some(cursor) if request.cursor.as_ref() != Some(&cursor) => {
                    request.cursor = Some(cursor)
                }
                _ => {
                    return Err(NativeRemoteError::Invalid(
                        "projection cursor did not advance",
                    ));
                }
            }
        }
        Ok((
            root.ok_or(NativeRemoteError::Invalid("missing projection root"))?,
            entries.into_values().collect(),
        ))
    }
}
