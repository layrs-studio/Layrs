//! A conflicted rewrite is a real descendant; the old physical projection stays
//! explicit until canonical term resolution can install a complete result.
use super::Result;
use super::*;
use layrs_store::scm::*;
use layrs_store_local::scm_v1;
impl NativeWorkspace {
    pub(super) fn apply_conflicted_rewrite(&self, plan: &NativeRewritePlan) -> Result<StepId> {
        let source = &plan.state;
        let (_, snapshot) = self.capture_projection(source, SnapshotLimits::default())?;
        let active = source
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .ok_or(NativeClientError::Missing("rewrite Worktree"))?;
        if active.status() != WorktreeStatus::Ready
            || active.materialized_tree_id() != Some(snapshot.root_tree_id())
        {
            return Err(NativeClientError::StaleCapture);
        }
        let store = self.open_store()?;
        let reader = NativeScmReader::new(&store)?;
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
        let result = &plan.summary.proposed_step;
        let layers = source
            .view
            .layer_heads()
            .iter()
            .map(|h| {
                if h.layer_id() == &plan.summary.target_layer_id {
                    LayerHeadSet::new(h.layer_id().clone(), vec![result.id().clone()])
                } else {
                    Ok(h.clone())
                }
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let states = source
            .view
            .worktrees()
            .iter()
            .map(|w| {
                if w.worktree_id() == self.worktree_id() {
                    WorktreeViewState::new(
                        w.worktree_id().clone(),
                        w.active_layer_id().cloned(),
                        w.base_step_id().cloned(),
                        w.materialized_tree_id().cloned(),
                        WorktreeStatus::Conflicted,
                    )
                } else {
                    w.clone()
                }
            })
            .collect();
        let view = RepositoryView::new(
            source.space.id().clone(),
            layers,
            source.view.marker_ids().to_vec(),
            source.view.release_ids().to_vec(),
            states,
        )?;
        let stamp = MutationStamp::fresh()?;
        let kind = match plan.summary.kind {
            NativeRewriteKind::Revert => OperationKind::Revert,
            _ => OperationKind::RewriteSteps,
        };
        let operation = Operation::new(
            stamp.operation_id.clone(),
            source.space.id().clone(),
            vec![source.operation.id().clone()],
            view.id().clone(),
            kind,
            &plan.description,
            plan.summary.rewrite_map.clone(),
            None,
            None,
            stamp.timestamp,
            Metadata::empty(),
        )?;
        let mut canonical = plan.canonical.clone();
        canonical.push(CanonicalObject::RepositoryView(view.clone()));
        let admissions = prepare::admissions(source.space.id(), &canonical);
        let batch = WriteBatch::new(
            stamp.transaction_id,
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
                    stamp.operation_id.clone(),
                    super::super::worktree_operations::advance_heads(
                        &heads,
                        source.operation.id(),
                        &stamp.operation_id,
                    ),
                )?),
                StoreMutation::WorktreeActiveOperation(WorktreeActiveOperationCas::new(
                    self.worktree_id().clone(),
                    source.operation.id().clone(),
                    stamp.operation_id,
                )?),
            ],
        )?;
        drop(reader);
        drop(store);
        let before = self.read_step_files(&base_step(source)?)?;
        scm_v1::materialize::apply_checkout_bound_metadata(
            self.store_root(),
            self.store_identity(),
            self.metadata_identity(),
            self.worktree_id(),
            &before,
            &before,
            &batch,
            &NoMaterializationFaults,
        )?;
        Ok(result.id().clone())
    }
}
