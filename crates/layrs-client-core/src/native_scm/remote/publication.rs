//! Shared immutable preparation and exact durable receipt completion.
use super::*;
impl NativeRemote {
    pub(super) fn publish_controlled(
        &self,
        workspace: &NativeWorkspace,
        mode: Option<LayerMutationMode>,
    ) -> RemoteResult<SyncTransactionReceipt> {
        let lock = remote_lock(workspace)?;
        let mut binding = self.binding(workspace)?;
        if binding.clone_pending {
            return Err(NativeRemoteError::Invalid(
                "finish clone before publication",
            ));
        }
        if let Some(pending) = &binding.pending {
            // This request belongs to an earlier attempt whose commit point is
            // already durable. Never relabel that unknown outcome as Cancelled.
            if mode.is_none()
                && (pending.view_mutation.is_some() || pending.policy_root_mutation.is_some())
            {
                return Err(NativeRemoteError::Invalid(
                    "retry pending reference publication before candidates",
                ));
            }
            self.transfer_begin_commit().map_err(|error| match error {
                NativeRemoteError::Cancelled => NativeRemoteError::PublicationPending,
                other => other,
            })?;
            return self.finish_publication(workspace, &mut binding);
        }
        self.transfer_check()?;
        let source = workspace.state()?;
        if projection_dirty(workspace, &source)? {
            return Err(NativeRemoteError::Invalid(
                "capture or preserve edits before cancellable publication",
            ));
        }
        let before = publication_journal(workspace)?;
        drop(lock);
        // No Store writer or remote journal lock spans preparation/uploads.
        // The request freezes immutable history, never mutable filesystem bytes.
        let (remote, request) = self.prepare_publication(workspace, &source, &binding, mode)?;
        self.transfer_check()?;
        let _lock = remote_lock(workspace)?;
        let current = workspace.state()?;
        if publication_journal(workspace)? != before
            || current.operation.id() != source.operation.id()
            || current.view.id() != source.view.id()
        {
            return Err(NativeRemoteError::Local(NativeClientError::StaleCapture));
        }
        if projection_dirty(workspace, &current)? {
            return Err(NativeRemoteError::Invalid(
                "checkout changed during upload; capture or preserve edits before publication",
            ));
        }
        self.transfer_begin_commit()?;
        binding.observed = Some(remote);
        binding.pending = Some(request);
        save_binding(workspace, &binding)?;
        // Keep the per-checkout remote journal serialized until the matching
        // receipt is recorded. Native Store writes remain independently usable.
        self.finish_publication(workspace, &mut binding)
    }
    pub(super) fn prepare_publication(
        &self,
        workspace: &NativeWorkspace,
        state: &WorkspaceState,
        binding: &Binding,
        mode: Option<LayerMutationMode>,
    ) -> RemoteResult<(SpaceBootstrap, SyncTransactionRequest)> {
        let remote = match &binding.observed {
            _ if mode.is_none() => self.bootstrap(state.space.id())?,
            Some(observed) => observed.clone(),
            None => self.bootstrap(state.space.id())?,
        };
        let products = super::remote_products::prepare(workspace, state, &remote)?;
        let graph = GraphObjects::local(workspace, state)?;
        for object in graph.chunk_objects() {
            self.upload(binding, &object?)?;
        }
        let mut objects: Vec<_> = graph
            .objects
            .values()
            .filter(|o| !matches!(o, CanonicalObject::Step(_) | CanonicalObject::Product(_)))
            .collect();
        objects.sort_by_key(|o| match o {
            CanonicalObject::ChunkRaw(_) => 0,
            CanonicalObject::FileManifest(_) => 1,
            _ => 2,
        });
        for object in objects {
            self.upload(binding, object)?;
        }
        let dag = graph.dag()?;
        let steps = dag
            .topological()
            .map_err(graph_error)?
            .into_iter()
            .map(|id| graph.steps[&id].clone())
            .collect();
        let view = RepositoryView::new(
            state.space.id().clone(),
            state.view.layer_heads().to_vec(),
            state.view.marker_ids().to_vec(),
            state.view.release_ids().to_vec(),
            vec![],
        )?;
        let layer_mutations = view
            .layer_heads()
            .iter()
            .filter(|heads| {
                remote
                    .current_repository_view
                    .record
                    .layer_heads()
                    .iter()
                    .find(|h| h.layer_id() == heads.layer_id())
                    .is_none_or(|h| h.step_ids() != heads.step_ids())
            })
            .map(|heads| LayerMutationIntent {
                layer_id: heads.layer_id().clone(),
                mode: mode.unwrap_or(LayerMutationMode::Advance),
                decision_product_object_id: None,
            })
            .collect();
        let mut request = SyncTransactionRequest {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionRequestType::Request,
            space_id: binding.space_id.clone(),
            client_id: binding.client_id.clone(),
            idempotency_key: IdempotencyKey::parse(uuid::Uuid::new_v4().to_string()).expect("UUID"),
            request_digest: ObjectId::parse(format!("blake3:{}", "0".repeat(64)))?,
            known_event_cursor: Some(remote.watermark_cursor.clone()),
            steps,
            observations: vec![],
            products,
            view_mutation: Some(RepositoryViewMutation {
                mutation_id: MutationId::parse(format!("mutation:{}", uuid::Uuid::new_v4()))
                    .expect("UUID"),
                expected_current_view_id: remote.current_repository_view.repository_view_id.clone(),
                expected_generation: remote.current_repository_view_generation,
                proposed_view: ProposedRepositoryView::Inline { view },
                layer_mutations,
                expected_policy_version_object_ids: captured_policy_version_object_ids(
                    &remote.current_policy_set.record,
                )
                .map_err(|_| NativeRemoteError::Invalid("invalid policies"))?,
            }),
            policy_root_mutation: None,
        };
        if mode.is_none() {
            request.view_mutation = None;
        }
        if let Some(RepositoryViewMutation {
            proposed_view: ProposedRepositoryView::Inline { view },
            ..
        }) = &request.view_mutation
        {
            if view.id() == remote.current_repository_view.record.id() {
                request.view_mutation = None;
            }
        }
        request.request_digest = request
            .computed_request_digest()
            .map_err(|_| NativeRemoteError::Invalid("invalid transaction"))?;
        request
            .validate_v3()
            .map_err(|_| NativeRemoteError::Invalid("invalid transaction"))?;
        Ok((remote, request))
    }
    pub(super) fn finish_publication(
        &self,
        workspace: &NativeWorkspace,
        binding: &mut Binding,
    ) -> RemoteResult<SyncTransactionReceipt> {
        let request = binding.pending.as_ref().expect("pending persisted");
        if request.policy_root_mutation.is_some() {
            return Err(NativeRemoteError::Invalid(
                "retry the exact policy preview before ordinary publication",
            ));
        }
        if !request.observations.is_empty()
            && request.steps.is_empty()
            && request.products.is_empty()
            && request.view_mutation.is_none()
        {
            return Err(NativeRemoteError::Invalid(
                "retry the exact observation request before ordinary publication",
            ));
        }
        let receipt: SyncTransactionReceipt = self.post(
            &format!("v3/spaces/{}/transactions", binding.space_id),
            request,
        )?;
        receipt
            .validate_v3()
            .map_err(|_| NativeRemoteError::Invalid("invalid transaction receipt"))?;
        if receipt.space_id != request.space_id
            || receipt.client_id != request.client_id
            || receipt.request_digest != request.request_digest
            || receipt.idempotency_key != request.idempotency_key
        {
            return Err(NativeRemoteError::Invalid(
                "transaction receipt binding mismatch",
            ));
        }
        if receipt.status == ReceiptStatus::Applied {
            if let (Some(observed), Some(result)) =
                (&mut binding.observed, receipt.view_result.as_ref())
            {
                super::remote_products::apply_projections(observed, &request.products);
                observed
                    .product_roots
                    .extend(receipt.accepted_products.iter().cloned());
                observed.product_roots.sort();
                observed.product_roots.dedup();
                let mut heads = observed
                    .current_repository_view
                    .record
                    .layer_heads()
                    .to_vec();
                for change in &result.layer_results {
                    let replacement =
                        LayerHeadSet::new(change.layer_id.clone(), change.resulting_heads.clone())?;
                    if let Some(old) = heads.iter_mut().find(|h| h.layer_id() == &change.layer_id) {
                        *old = replacement;
                    } else {
                        heads.push(replacement);
                    }
                    if let Some(layer) = observed
                        .layers
                        .iter_mut()
                        .find(|l| l.record.id() == &change.layer_id)
                    {
                        layer.head_step_ids = change.resulting_heads.clone();
                    }
                }
                let proposed = match &request
                    .view_mutation
                    .as_ref()
                    .expect("publication mutation")
                    .proposed_view
                {
                    ProposedRepositoryView::Inline { view } => view,
                    _ => {
                        return Err(NativeRemoteError::Invalid(
                            "publication journal requires inline View",
                        ));
                    }
                };
                let markers: BTreeSet<_> = observed
                    .current_repository_view
                    .record
                    .marker_ids()
                    .iter()
                    .chain(proposed.marker_ids())
                    .cloned()
                    .collect();
                let releases: BTreeSet<_> = observed
                    .current_repository_view
                    .record
                    .release_ids()
                    .iter()
                    .chain(proposed.release_ids())
                    .cloned()
                    .collect();
                let view = RepositoryView::new(
                    observed.space_id.clone(),
                    heads,
                    markers.into_iter().collect(),
                    releases.into_iter().collect(),
                    vec![],
                )?;
                if view.id() != &result.new_repository_view_id {
                    return Err(NativeRemoteError::Invalid(
                        "receipt resulting View identity mismatch",
                    ));
                }
                observed.current_repository_view = RepositoryViewProjection {
                    repository_view_id: view.id().clone(),
                    record: view,
                };
                observed.current_repository_view_generation = result.new_generation;
                if let Some(cursor) = receipt.event_cursor.as_ref() {
                    observed.watermark_cursor = cursor.clone();
                }
                if let Some(sequence) = receipt.last_event_sequence.as_ref() {
                    observed.watermark_sequence = *sequence;
                }
            }
        }
        binding.pending = None;
        binding.last_receipt = Some(receipt.clone());
        save_binding(workspace, binding)?;
        Ok(receipt)
    }
}

fn publication_journal(workspace: &NativeWorkspace) -> RemoteResult<Option<Vec<u8>>> {
    let path = workspace.store_root().join("native-remote.json");
    if path.try_exists()? {
        Ok(Some(bounded_file(&path, MAX_OBJECT)?))
    } else {
        Ok(None)
    }
}

/// Reqwest reads on its own body worker. Bound each read and observe the same
/// atomic token; cancellation of a blocked response is reported on return too.
pub(super) struct UploadReader {
    bytes: std::io::Cursor<Vec<u8>>,
    control: NativeTransferControl,
}
impl UploadReader {
    pub(super) fn new(bytes: Vec<u8>, control: NativeTransferControl) -> Self {
        Self {
            bytes: std::io::Cursor::new(bytes),
            control,
        }
    }
}
impl Read for UploadReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.control
            .check()
            .map_err(|_| std::io::Error::other("native upload cancelled"))?;
        let count = output.len().min(64 * 1024);
        self.bytes.read(&mut output[..count])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upload_reads_are_bounded_and_stop_without_consuming_more_bytes() {
        let control = NativeTransferControl::new();
        let mut reader = UploadReader::new(vec![42; 128 * 1024], control.clone());
        let mut output = vec![0; 256 * 1024];
        assert_eq!(reader.read(&mut output).unwrap(), 64 * 1024);
        assert_eq!(&output[..64 * 1024], vec![42; 64 * 1024]);
        control.request_cancel();
        assert!(reader.read(&mut output).is_err());
        assert_eq!(reader.bytes.position(), 64 * 1024);
    }
    #[test]
    fn an_unknown_durable_publication_has_a_distinct_error_code() {
        assert_eq!(
            NativeRemoteError::PublicationPending.code(),
            "publication_pending"
        );
        assert_ne!(
            NativeRemoteError::PublicationPending.code(),
            NativeRemoteError::Cancelled.code()
        );
    }
}
