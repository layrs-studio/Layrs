use super::*;
use crate::native_scm::{prepare, references::base_step, rewrite, SnapshotLimits};
use layrs_store_local::scm_v1::{
    materialize::{self, MaterializationFaults, NoMaterializationFaults},
    NativeScmRepository,
};
use layrs_sync::v3::{ProductEnvelope, WeaveLogicalId};
use layrs_weave::graph_merge::ReceiptContext;
use std::collections::BTreeSet;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeAcceptedMerge {
    pub step_id: StepId,
    pub operation_id: OperationId,
    pub product_object_id: ObjectId,
    pub conflict_ids: Vec<ConflictId>,
    pub materialized: bool,
    pub already_accepted: bool,
}

impl NativeWorkspace {
    /// Installs a zero-Lens Weave and its canonical receipt in one logical
    /// transaction. Semantic executions reject; explicitly request structural
    /// preview when no actually constrained semantic host is available.
    pub fn accept_merge(&self, preview: &NativeMergePreview) -> Result<NativeAcceptedMerge> {
        self.accept_merge_with_faults(preview, &NoMaterializationFaults)
    }
    pub fn accept_merge_with_faults(
        &self,
        preview: &NativeMergePreview,
        faults: &dyn MaterializationFaults,
    ) -> Result<NativeAcceptedMerge> {
        let source = &preview.state;
        if self.worktree_id() != source.worktree.id() {
            return Err(NativeClientError::InvalidState(
                "merge preview belongs to another Worktree",
            ));
        }
        let receipt = preview
            .plan
            .accepted_receipt(ReceiptContext {
                space_id: source.space.id().clone(),
                operation_id: preview.stamp.operation_id.clone(),
                created_at: preview.stamp.timestamp.clone(),
                gate_snapshot_id: None,
            })
            .map_err(merge_error)?;
        let mut deps = receipt
            .source_heads
            .iter()
            .chain(&receipt.target_heads_before)
            .cloned()
            .collect::<BTreeSet<_>>();
        deps.extend(preview.plan.merge_bases().iter().cloned());
        deps.insert(receipt.result_step_id.clone());
        let mut envelope = ProductEnvelope::Weave {
            product_id: WeaveLogicalId::parse(format!("weave:{}", receipt.weave_id))
                .map_err(merge_error)?,
            product_object_id: receipt.weave_id.clone(),
            depends_on_step_ids: deps.into_iter().collect(),
            depends_on_product_ids: vec![],
            record: Box::new(receipt.clone()),
        };
        let product_id = envelope.computed_object_id().map_err(merge_error)?;
        if let ProductEnvelope::Weave {
            product_object_id, ..
        } = &mut envelope
        {
            *product_object_id = product_id.clone();
        }
        let product = CanonicalProduct::from_envelope(envelope)?;
        let worktree = source
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .ok_or(NativeClientError::Missing("merge Worktree"))?;
        let active = worktree.active_layer_id() == Some(&preview.target_layer);
        let clean = receipt.conflict_ids.is_empty();
        let mut outcome = NativeAcceptedMerge {
            step_id: receipt.result_step_id.clone(),
            operation_id: receipt.operation_id.clone(),
            product_object_id: product_id.clone(),
            conflict_ids: receipt.conflict_ids.clone(),
            materialized: active && clean,
            already_accepted: false,
        };
        let store = open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        // The preview owns a stable mutation stamp. A lost commit response is
        // recognized before stale-state checks and cannot install a second Weave.
        if let Some(existing) = reader.get_operation(&receipt.operation_id)? {
            if existing.kind() != OperationKind::Weave
                || existing.space_id() != source.space.id()
                || reader.get_product(&product_id)?.as_ref() != Some(&product)
            {
                return Err(NativeClientError::InvalidState(
                    "merge replay identity mismatch",
                ));
            }
            outcome.already_accepted = true;
            return Ok(outcome);
        }
        let current = reader
            .current_repository_view(source.space.id())?
            .ok_or(NativeClientError::Missing("current View"))?;
        let heads = reader.operation_heads(source.space.id())?;
        if current.view_id() != Some(source.view.id())
            || reader
                .worktree_active_operation(self.worktree_id())?
                .as_ref()
                != Some(source.operation.id())
        {
            return Err(NativeClientError::StaleCapture);
        }
        if heads != [source.operation.id().clone()] {
            return Err(NativeClientError::NeedsIntegration);
        }
        if active && worktree.status() != WorktreeStatus::Ready {
            return Err(NativeClientError::InvalidState(
                "merge requires a ready active Worktree",
            ));
        }
        // Also covers a graph-only join whose checkout delta is empty.
        if active {
            let (_, captured) = self.capture_projection(source, SnapshotLimits::default())?;
            if worktree.materialized_tree_id() != Some(captured.root_tree_id()) {
                return Err(NativeClientError::StaleCapture);
            }
        }
        let mut objects = load_objects(
            &reader,
            source.space.id(),
            preview
                .plan
                .source_heads()
                .iter()
                .chain(preview.plan.target_heads())
                .cloned()
                .collect(),
            preview.plan.limits(),
        )?;
        let bundle = preview.plan.installation_bundle().map_err(merge_error)?;
        for tree in &bundle.trees {
            objects.trees.insert(tree.id().clone(), tree.clone());
        }
        for (id, bytes) in preview.plan.files() {
            objects.files.insert(id.clone(), bytes.clone());
        }
        let after = if active && clean {
            rewrite::checkout_entries(
                &objects,
                preview.plan.step().root_tree_id(),
                preview.plan.limits(),
            )?
        } else {
            vec![]
        };
        if active
            && clean
            && super::super::restore::snapshot_entries(&after)?.root_tree_id()
                != preview.plan.step().root_tree_id()
        {
            return Err(NativeClientError::InvalidState(
                "clean merge Tree cannot be represented by checkout",
            ));
        }
        let states = source
            .view
            .worktrees()
            .iter()
            .map(|state| {
                if state.worktree_id() != self.worktree_id() || !active {
                    return state.clone();
                }
                if clean {
                    WorktreeViewState::new(
                        state.worktree_id().clone(),
                        state.active_layer_id().cloned(),
                        Some(receipt.result_step_id.clone()),
                        Some(preview.plan.step().root_tree_id().clone()),
                        WorktreeStatus::Ready,
                    )
                } else {
                    WorktreeViewState::new(
                        state.worktree_id().clone(),
                        state.active_layer_id().cloned(),
                        state.base_step_id().cloned(),
                        state.materialized_tree_id().cloned(),
                        WorktreeStatus::Conflicted,
                    )
                }
            })
            .collect();
        let layers = source
            .view
            .layer_heads()
            .iter()
            .map(|heads| {
                if heads.layer_id() == &preview.target_layer {
                    LayerHeadSet::new(
                        heads.layer_id().clone(),
                        vec![receipt.result_step_id.clone()],
                    )
                } else {
                    Ok(heads.clone())
                }
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let view = RepositoryView::new(
            source.space.id().clone(),
            layers,
            source.view.marker_ids().to_vec(),
            source.view.release_ids().to_vec(),
            states,
        )?;
        let operation = prepare::operation(
            &preview.stamp,
            source.space.id().clone(),
            heads.clone(),
            &view,
            OperationKind::Weave,
            "Accept structural native Weave",
        )?;
        let mut canonical = bundle_objects(bundle)?;
        canonical.push(CanonicalObject::Product(product));
        canonical.push(CanonicalObject::RepositoryView(view.clone()));
        let admissions = prepare::admissions(source.space.id(), &canonical);
        let batch = WriteBatch::new(
            preview.stamp.transaction_id.clone(),
            canonical,
            vec![EntityRecord::Operation(operation)],
            admissions,
            vec![
                StoreMutation::RepositoryView(RepositoryViewCas::new(
                    source.space.id().clone(),
                    current.generation(),
                    current.view_id().cloned(),
                    view.id().clone(),
                )?),
                StoreMutation::OperationHeads(OperationHeadsCas::new(
                    source.space.id().clone(),
                    heads,
                    receipt.operation_id.clone(),
                    vec![receipt.operation_id.clone()],
                )?),
                StoreMutation::WorktreeActiveOperation(WorktreeActiveOperationCas::new(
                    self.worktree_id().clone(),
                    source.operation.id().clone(),
                    receipt.operation_id.clone(),
                )?),
            ],
        )?;
        drop(reader);
        drop(store);
        if active {
            let before = self.read_step_files(&base_step(source)?)?;
            if clean {
                materialize::apply_checkout(
                    self.store_root(),
                    self.worktree_id(),
                    &before,
                    &after,
                    &batch,
                    faults,
                )?;
            } else {
                // No files change. Verify current bytes still match the claimed
                // old projection, and retain that old projection in the View.
                NativeScmRepository::new(self.store_root()).apply_batch(&batch)?;
            }
        } else {
            NativeScmRepository::new(self.store_root()).apply_batch(&batch)?;
        }
        Ok(outcome)
    }
}
