use super::*;
impl PgPathPatchService {
    pub(super) async fn prepare_request(
        &self,
        patch: &PathPatchRequest,
    ) -> V3StoreResult<(SyncTransactionRequest, ScopedPatch, Vec<Tree>)> {
        let bytes: String = sqlx::query_scalar("SELECT s.canonical_record_jcs FROM steps s JOIN space_steps ss ON ss.step_id=s.step_id WHERE ss.space_id=$1 AND s.step_id=$2")
            .bind(patch.space_id.as_str()).bind(patch.base_step_id.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let step: Step = serde_json::from_str(&bytes).map_err(invalid)?;
        if step.id() != &patch.base_step_id {
            return Err(V3StoreError::IdentityMismatch);
        }
        let bytes: String = sqlx::query_scalar("SELECT canonical_record_jcs FROM repository_views WHERE space_id=$1 AND repository_view_id=$2")
            .bind(patch.space_id.as_str()).bind(patch.expected_repository_view_id.as_str()).fetch_optional(self.repository.pool()).await.map_err(pg_error)?.ok_or(V3StoreError::NotFound)?;
        let before: RepositoryView = serde_json::from_str(&bytes).map_err(invalid)?;
        if before.id() != &patch.expected_repository_view_id
            || before.space_id() != &patch.space_id
            || !before.worktrees().is_empty()
            || before
                .layer_heads()
                .iter()
                .find(|head| head.layer_id() == &patch.layer_id)
                .map(|head| head.step_ids())
                != Some(std::slice::from_ref(&patch.base_step_id))
        {
            return Err(invalid("patch requires the exact single base head"));
        }
        let bytes: String = sqlx::query_scalar(
            "SELECT canonical_record_jcs FROM policy_sets WHERE space_id=$1 AND policy_set_id=$2",
        )
        .bind(patch.space_id.as_str())
        .bind(patch.expected_policy_set_id.as_str())
        .fetch_optional(self.repository.pool())
        .await
        .map_err(pg_error)?
        .ok_or(V3StoreError::NotFound)?;
        let policy: PolicySet = serde_json::from_str(&bytes).map_err(invalid)?;
        policy.validate_v3().map_err(invalid)?;
        if policy.policy_set_id != patch.expected_policy_set_id || policy.space_id != patch.space_id
        {
            return Err(V3StoreError::IdentityMismatch);
        }
        let mut trees = BTreeMap::new();
        let mut total_bytes = 0u64;
        for change in &patch.changes {
            let mut id = step.root_tree_id().clone();
            for part in change.path.as_str().split('/') {
                if !trees.contains_key(&id) {
                    if trees.len() >= 16_384 {
                        return Err(invalid("patch metadata exceeds Tree limit"));
                    }
                    let descriptor = load_space_descriptor(
                        self.repository.pool(),
                        &patch.space_id,
                        &ObjectId::parse(id.as_str()).map_err(invalid)?,
                    )
                    .await?;
                    total_bytes = total_bytes
                        .checked_add(descriptor.canonical_byte_len)
                        .ok_or_else(|| invalid("patch metadata size overflow"))?;
                    if descriptor.kind != ObjectKind::Tree || total_bytes > 64 * 1024 * 1024 {
                        return Err(invalid("invalid or oversized patch metadata"));
                    }
                    let tree: Tree = serde_json::from_slice(
                        &read_canonical_object(
                            self.repository.cas().as_ref(),
                            &descriptor,
                            1024 * 1024,
                        )
                        .await?,
                    )
                    .map_err(invalid)?;
                    if tree.id() != &id {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                    trees.insert(id.clone(), tree);
                }
                let Some(entry) = trees[&id]
                    .entries()
                    .iter()
                    .find(|entry| entry.name().as_str() == part)
                else {
                    break;
                };
                if let TreeEntryTarget::Tree { tree_id } = entry.target() {
                    id = tree_id.clone();
                } else {
                    break;
                }
            }
        }
        let patched = rebuild_path_patch(step.root_tree_id(), &patch.changes, &mut |id| {
            trees.get(id).cloned().ok_or(V3StoreError::NotFound)
        })?;
        let next =
            Step::new(vec![patch.base_step_id.clone()], patched.root_tree_id).map_err(invalid)?;
        let heads = before
            .layer_heads()
            .iter()
            .map(|head| {
                if head.layer_id() == &patch.layer_id {
                    LayerHeadSet::new(patch.layer_id.clone(), vec![next.id().clone()])
                        .map_err(invalid)
                } else {
                    Ok(head.clone())
                }
            })
            .collect::<V3StoreResult<Vec<_>>>()?;
        let view = RepositoryView::new(
            patch.space_id.clone(),
            heads,
            before.marker_ids().to_vec(),
            before.release_ids().to_vec(),
            vec![],
        )
        .map_err(invalid)?;
        let mut request = SyncTransactionRequest {
            protocol: SyncV3Protocol::V3,
            message_type: TransactionRequestType::Request,
            space_id: patch.space_id.clone(),
            client_id: patch.client_id.clone(),
            idempotency_key: patch.idempotency_key.clone(),
            request_digest: patch.request_digest.clone(),
            known_event_cursor: None,
            steps: vec![next],
            observations: vec![],
            products: vec![],
            policy_root_mutation: None,
            view_mutation: Some(RepositoryViewMutation {
                mutation_id: MutationId::parse(format!(
                    "mutation:path-patch-{}",
                    &patch.request_digest.as_str()[7..]
                ))
                .map_err(invalid)?,
                expected_current_view_id: patch.expected_repository_view_id.clone(),
                expected_generation: patch.expected_generation,
                proposed_view: ProposedRepositoryView::Inline { view },
                layer_mutations: vec![LayerMutationIntent {
                    layer_id: patch.layer_id.clone(),
                    mode: LayerMutationMode::Advance,
                    decision_product_object_id: None,
                }],
                expected_policy_version_object_ids: captured_policy_version_object_ids(&policy)
                    .map_err(invalid)?,
            }),
        };
        request.request_digest = request.computed_request_digest().map_err(invalid)?;
        let scope = ScopedPatch {
            request: patch.clone(),
            root: step.root_tree_id().clone(),
            transaction_digest: request.request_digest.clone(),
        };
        Ok((request, scope, patched.regenerated_trees))
    }
}
