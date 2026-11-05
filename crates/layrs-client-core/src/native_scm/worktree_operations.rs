//! A Worktree advances its own Operation without consuming another history.
use super::{NativeClientError, Result};
use layrs_core::scm::*;
use layrs_store::scm::ScmReader;
use std::collections::BTreeSet;

/// The parent may already be an ancestor of a different Worktree's head.
/// Only an actual parent head is replaced; all independent heads survive.
pub(super) fn advance_heads(
    heads: &[OperationId],
    parent: &OperationId,
    next: &OperationId,
) -> Vec<OperationId> {
    let mut result: BTreeSet<_> = heads.iter().filter(|id| *id != parent).cloned().collect();
    result.insert(next.clone());
    result.into_iter().collect()
}

/// Preserve every observed frontier of the captured Layer. The new Step has
/// exactly `base` as its parent: remove only ancestors subsumed by that Step.
pub(super) fn capture_layer_heads(
    reader: &impl ScmReader,
    view: &RepositoryView,
    heads: &[OperationId],
    layer: &LayerId,
    base: &StepId,
    next: &StepId,
) -> Result<LayerHeadSet> {
    let mut candidates = BTreeSet::new();
    for operation_id in heads {
        let operation = reader
            .get_operation(operation_id)?
            .ok_or(NativeClientError::Missing("Operation head"))?;
        if operation.space_id() != view.space_id() {
            return Err(NativeClientError::InvalidState(
                "cross-Space Operation head",
            ));
        }
        let other = reader
            .get_repository_view(operation.resulting_view_id())?
            .ok_or(NativeClientError::Missing("head View"))?;
        if let Some(h) = other.layer_heads().iter().find(|h| h.layer_id() == layer) {
            candidates.extend(h.step_ids().iter().cloned());
        }
    }
    if let Some(h) = view.layer_heads().iter().find(|h| h.layer_id() == layer) {
        candidates.extend(h.step_ids().iter().cloned());
    }
    let mut preserved = Vec::new();
    let mut next_subsumed = false;
    let next_exists = reader.get_step(next)?.is_some();
    for candidate in &candidates {
        if candidate == next || candidate == base || reader.is_step_ancestor(candidate, base)? {
            continue;
        }
        if next_exists && reader.is_step_ancestor(next, candidate)? {
            next_subsumed = true;
        }
        let mut subsumed = false;
        for other in &candidates {
            if other != candidate && reader.is_step_ancestor(candidate, other)? {
                subsumed = true;
                break;
            }
        }
        if !subsumed {
            preserved.push(candidate.clone());
        }
    }
    if !next_subsumed {
        preserved.push(next.clone());
    }
    Ok(LayerHeadSet::new(layer.clone(), preserved)?)
}
