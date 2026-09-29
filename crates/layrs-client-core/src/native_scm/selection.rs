use super::{NativeClient, NativeClientError, NativeWorkspace, Result};
use layrs_core::scm::{OperationId, RepositoryViewId, StepId, WorktreeId};
use layrs_graph::scm::{
    ReferenceKind, ReferenceSelector, SelectorError, SelectorSnapshot, StepDag, StepSelector,
};
use layrs_store::scm::{ScmReader, StoreObjectId};
use layrs_store_local::scm_v1::{open, NativeScmReader};
use std::collections::BTreeMap;

/// A read result, not permission to mutate. Applications must bind their later
/// write to these exact expectations and revalidate admission/authorization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedStep {
    pub step_id: StepId,
    pub source_operation_id: OperationId,
    pub repository_view_id: RepositoryViewId,
}

impl NativeWorkspace {
    /// Exact Step ID, or an explicitly typed visible name. No prefix matching
    /// and no precedence that might silently pick one kind of reference.
    pub fn resolve_step_name(&self, input: &str) -> Result<ResolvedStep> {
        let selector = if let Some(name) = input.strip_prefix("@layer:") {
            StepSelector::Layer {
                reference: ReferenceSelector::Name(name.into()),
                head: None,
            }
        } else if let Some(name) = input.strip_prefix("@marker:") {
            StepSelector::Marker(ReferenceSelector::Name(name.into()))
        } else if let Some(name) = input.strip_prefix("@release:") {
            StepSelector::Release(ReferenceSelector::Name(name.into()))
        } else {
            StepSelector::Step(StepId::parse(input)?)
        };
        self.client()
            .resolve_step(self.worktree_id(), &selector, 100_000)
    }
}

impl NativeClient {
    /// Resolves against the Worktree's current Operation under a shared Store
    /// capability. The authorized closure comes from native Space admissions,
    /// never from the mere existence of a content-addressed Step.
    pub fn resolve_step(
        &self,
        worktree_id: &WorktreeId,
        selector: &StepSelector,
        max_steps: usize,
    ) -> Result<ResolvedStep> {
        let store = open(&self.root)?;
        let reader = NativeScmReader::new(&store)?;
        let worktree = reader
            .get_worktree(worktree_id)?
            .ok_or(NativeClientError::Missing("Worktree"))?;
        let operation = reader
            .get_operation(worktree.active_operation_id())?
            .ok_or(NativeClientError::Missing("Operation"))?;
        let view = reader
            .get_repository_view(operation.resulting_view_id())?
            .ok_or(NativeClientError::Missing("View"))?;
        let mut layers = Vec::new();
        let mut markers = Vec::new();
        let mut releases = Vec::new();
        let mut pending = Vec::new();
        for head in view.layer_heads() {
            layers.push(
                reader
                    .get_layer(head.layer_id())?
                    .ok_or(NativeClientError::Missing("Layer"))?,
            );
            pending.extend_from_slice(head.step_ids());
        }
        for id in view.marker_ids() {
            let marker = reader
                .get_marker(id)?
                .ok_or(NativeClientError::Missing("Marker"))?;
            pending.push(marker.target_step_id().clone());
            markers.push(marker);
        }
        for id in view.release_ids() {
            let release = reader
                .get_release(id)?
                .ok_or(NativeClientError::Missing("Release"))?;
            pending.push(release.target_step_id().clone());
            releases.push(release);
        }
        for state in view.worktrees() {
            if let Some(id) = state.base_step_id() {
                pending.push(id.clone());
            }
        }
        if let StepSelector::Step(id) = selector {
            pending.push(id.clone());
        }
        let mut steps = BTreeMap::new();
        while let Some(id) = pending.pop() {
            if steps.contains_key(&id) {
                continue;
            }
            if steps.len() >= max_steps.min(100_000) {
                return Err(NativeClientError::InvalidState(
                    "selector Step traversal limit exceeded",
                ));
            }
            if !reader
                .object_admitted_to_space(&StoreObjectId::Step(id.clone()), worktree.space_id())?
            {
                return Err(SelectorError::OutsideSpace {
                    kind: ReferenceKind::Step,
                    id: id.to_string(),
                }
                .into());
            }
            let step = reader
                .get_step(&id)?
                .ok_or(NativeClientError::Missing("Step"))?;
            pending.extend_from_slice(step.parent_step_ids());
            steps.insert(id, step);
        }
        let graph = StepDag::from_steps(steps.values()).map_err(SelectorError::from)?;
        let snapshot = SelectorSnapshot::new(
            worktree.space_id(),
            &graph,
            &view,
            steps.keys().cloned(),
            &layers,
            &markers,
            &releases,
        )?;
        Ok(ResolvedStep {
            step_id: snapshot.resolve(selector)?,
            source_operation_id: operation.id().clone(),
            repository_view_id: view.id().clone(),
        })
    }
}
