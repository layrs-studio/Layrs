use super::*;
use crate::native_scm::{prepare, references::base_step, rewrite, SnapshotLimits};
use layrs_store_local::scm_v1::materialize::{
    self, MaterializationFaults, NoMaterializationFaults,
};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeAcceptedResolution {
    pub step_id: StepId,
    pub operation_id: OperationId,
    pub conflict_ids: Vec<ConflictId>,
    pub materialized: bool,
    pub already_accepted: bool,
}

impl NativeWorkspace {
    /// Resolves selected canonical terms in a descendant Step. This is a
    /// ResolveConflict Operation, not a new reconciliation/Weave receipt.
    pub fn accept_conflict_resolution(
        &self,
        preview: &NativeResolutionPreview,
    ) -> Result<NativeAcceptedResolution> {
        self.accept_conflict_resolution_with_faults(preview, &NoMaterializationFaults)
    }
    pub fn accept_conflict_resolution_with_faults(
        &self,
        preview: &NativeResolutionPreview,
        faults: &dyn MaterializationFaults,
    ) -> Result<NativeAcceptedResolution> {
        let source = &preview.state;
        if self.worktree_id() != source.worktree.id() {
            return Err(NativeClientError::InvalidState(
                "resolution preview belongs to another Worktree",
            ));
        }
        let worktree = source
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .ok_or(NativeClientError::Missing("resolution Worktree"))?;
        let active = worktree.active_layer_id() == Some(&preview.target_layer);
        let conflict_ids = preview
            .bundle
            .conflicts
            .iter()
            .map(PlannedConflict::id)
            .collect::<Vec<_>>();
        let clean = conflict_ids.is_empty();
        let result = &preview.bundle.step;
        let mut outcome = NativeAcceptedResolution {
            step_id: result.id().clone(),
            operation_id: preview.stamp.operation_id.clone(),
            conflict_ids,
            materialized: active && clean,
            already_accepted: false,
        };
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
        if let Some(operation) = reader.get_operation(&preview.stamp.operation_id)? {
            let view = reader
                .get_repository_view(operation.resulting_view_id())?
                .ok_or(NativeClientError::Missing("resolution View"))?;
            if operation.kind() != OperationKind::ResolveConflict
                || operation.space_id() != source.space.id()
                || !view.layer_heads().iter().any(|h| {
                    h.layer_id() == &preview.target_layer && h.step_ids() == [result.id().clone()]
                })
            {
                return Err(NativeClientError::InvalidState(
                    "resolution replay identity mismatch",
                ));
            }
            outcome.already_accepted = true;
            return Ok(outcome);
        }
        let current = reader
            .current_repository_view(source.space.id())?
            .ok_or(NativeClientError::Missing("current View"))?;
        let heads = reader.operation_heads(source.space.id())?;
        if reader
            .worktree_active_operation(self.worktree_id())?
            .as_ref()
            != Some(source.operation.id())
        {
            return Err(NativeClientError::StaleCapture);
        }
        if active
            && !matches!(
                worktree.status(),
                WorktreeStatus::Ready | WorktreeStatus::Conflicted
            )
        {
            return Err(NativeClientError::InvalidState(
                "resolution requires Ready or Conflicted Worktree",
            ));
        }
        let mut objects = load_objects(
            &reader,
            source.space.id(),
            result.parent_step_ids().to_vec(),
            preview.limits,
        )?;
        for tree in &preview.bundle.trees {
            objects.trees.insert(tree.id().clone(), tree.clone());
        }
        // Replacement bundles contain single-chunk files or empty manifests.
        // Populate the preview closure before deriving exact checkout entries.
        for chunk in &preview.bundle.chunks {
            objects
                .files
                .insert(FileId::from_bytes(chunk.bytes()), chunk.bytes().to_vec());
        }
        for manifest in &preview.bundle.file_manifests {
            if manifest.byte_length() == 0 {
                objects.files.insert(manifest.file_id().clone(), vec![]);
            }
        }
        let after = if active && clean {
            rewrite::checkout_entries(&objects, result.root_tree_id(), preview.limits)?
        } else {
            vec![]
        };
        if active
            && clean
            && super::super::restore::snapshot_entries(&after)?.root_tree_id()
                != result.root_tree_id()
        {
            return Err(NativeClientError::InvalidState(
                "resolved Tree cannot be represented by checkout",
            ));
        }
        if active {
            let (_, captured) = self.capture_projection(source, SnapshotLimits::default())?;
            if worktree.materialized_tree_id() != Some(captured.root_tree_id()) {
                return Err(NativeClientError::StaleCapture);
            }
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
                        Some(result.id().clone()),
                        Some(result.root_tree_id().clone()),
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
            .map(|h| {
                if h.layer_id() == &preview.target_layer {
                    LayerHeadSet::new(h.layer_id().clone(), vec![result.id().clone()])
                } else {
                    Ok(h.clone())
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
            vec![source.operation.id().clone()],
            &view,
            OperationKind::ResolveConflict,
            "Resolve canonical conflicts",
        )?;
        let mut canonical = preview.canonical_objects()?;
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
                    heads.clone(),
                    preview.stamp.operation_id.clone(),
                    super::super::worktree_operations::advance_heads(
                        &heads,
                        source.operation.id(),
                        &preview.stamp.operation_id,
                    ),
                )?),
                StoreMutation::WorktreeActiveOperation(WorktreeActiveOperationCas::new(
                    self.worktree_id().clone(),
                    source.operation.id().clone(),
                    preview.stamp.operation_id.clone(),
                )?),
            ],
        )?;
        drop(reader);
        drop(store);
        if active && clean {
            let before = self.read_step_files(&base_step(source)?)?;
            materialize::apply_checkout_bound_metadata(
                self.store_root(),
                self.store_identity(),
                self.metadata_identity(),
                self.worktree_id(),
                &before,
                &after,
                &batch,
                faults,
            )?;
        } else {
            self.client.repository().apply_batch(&batch)?;
        }
        Ok(outcome)
    }
}
