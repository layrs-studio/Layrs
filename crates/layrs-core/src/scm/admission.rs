//! Context-dependent admission checks.
//!
//! Constructors enforce local shape and identity invariants. Closure,
//! acyclicity, antichains and cross-Space relationships require repository
//! context and are therefore checked explicitly through this resolver API.

use super::content::FileManifest;
use super::error::{Result, ScmError};
use super::ids::*;
use super::repository::{
    Layer, Marker, Operation, OperationKind, Release, ReleasePublication, RepositoryView, Worktree,
};
use super::step::{Step, StepObservation};
use super::tree::Tree;

pub trait AdmissionResolver {
    fn chunk_bytes(&self, id: &ChunkId) -> Option<Vec<u8>>;
    fn tree_exists(&self, id: &TreeId) -> bool;
    fn tree_admitted_to_space(&self, id: &TreeId, space: &SpaceId) -> bool;
    fn conflict_exists(&self, id: &ConflictId) -> bool;
    fn symlink_exists(&self, id: &SymlinkId) -> bool;
    fn tombstone_exists(&self, id: &TombstoneId) -> bool;
    fn file_has_verified_manifest(&self, id: &FileId) -> bool;
    fn step_exists(&self, id: &StepId) -> bool;
    fn step_admitted_to_space(&self, id: &StepId, space: &SpaceId) -> bool;
    fn is_step_ancestor(&self, ancestor: &StepId, descendant: &StepId) -> bool;
    fn space_exists(&self, id: &SpaceId) -> bool;
    fn store_exists(&self, id: &StoreId) -> bool;
    fn layer_in_space(&self, id: &LayerId, space: &SpaceId) -> bool;
    fn marker_in_space(&self, id: &MarkerId, space: &SpaceId) -> bool;
    fn release_in_space(&self, id: &ReleaseId, space: &SpaceId) -> bool;
    fn worktree_in_space(&self, id: &WorktreeId, space: &SpaceId) -> bool;
    fn worktree_exists(&self, id: &WorktreeId) -> bool;
    fn operation_in_space(&self, id: &OperationId, space: &SpaceId) -> bool;
    fn operation_exists(&self, id: &OperationId) -> bool;
    fn operation_reaches_step(&self, operation: &OperationId, step: &StepId) -> bool;
    fn repository_view_in_space(&self, id: &RepositoryViewId, space: &SpaceId) -> bool;
    fn proof_in_space(&self, id: &ProofId, space: &SpaceId) -> bool;
    fn actor_exists(&self, id: &ActorId) -> bool;
    fn device_exists(&self, id: &DeviceId) -> bool;
}

pub fn validate_file_manifest_admission(
    manifest: &FileManifest,
    resolver: &impl AdmissionResolver,
) -> Result<()> {
    manifest.verify_chunks(|id| resolver.chunk_bytes(id))
}

pub fn validate_tree_admission(tree: &Tree, resolver: &impl AdmissionResolver) -> Result<()> {
    for entry in tree.entries() {
        let target = entry.target();
        let present = if let Some(id) = target.file_id() {
            resolver.file_has_verified_manifest(id)
        } else if let Some(id) = target.tree_id() {
            resolver.tree_exists(id)
        } else if let Some(id) = target.conflict_id() {
            resolver.conflict_exists(id)
        } else if let Some(id) = target.symlink_id() {
            resolver.symlink_exists(id)
        } else if let Some(id) = target.tombstone_id() {
            resolver.tombstone_exists(id)
        } else {
            false
        };
        if !present {
            return Err(missing("TreeEntry.target"));
        }
    }
    Ok(())
}

pub fn validate_step_admission(step: &Step, resolver: &impl AdmissionResolver) -> Result<()> {
    if !resolver.tree_exists(step.root_tree_id()) {
        return Err(missing("rootTreeId"));
    }
    for parent in step.parent_step_ids() {
        if !resolver.step_exists(parent) {
            return Err(missing("parentStepIds"));
        }
        if resolver.is_step_ancestor(step.id(), parent) {
            return Err(ScmError::InvalidField {
                field: "parentStepIds",
                reason: "admission would create a Step cycle".into(),
            });
        }
    }
    Ok(())
}

pub fn validate_layer_admission(layer: &Layer, resolver: &impl AdmissionResolver) -> Result<()> {
    if !resolver.space_exists(layer.space_id()) {
        return Err(cross_space("spaceId"));
    }
    if layer
        .parent_layer_ids()
        .iter()
        .any(|id| !resolver.layer_in_space(id, layer.space_id()))
    {
        return Err(cross_space("parentLayerIds"));
    }
    Ok(())
}

pub fn validate_step_observation_admission(
    observation: &StepObservation,
    resolver: &impl AdmissionResolver,
) -> Result<()> {
    if !resolver.step_exists(observation.step_id()) {
        return Err(missing("stepId"));
    }
    if let Some(space) = observation.space_id() {
        if !resolver.space_exists(space)
            || !resolver.step_admitted_to_space(observation.step_id(), space)
        {
            return Err(cross_space("spaceId/stepId"));
        }
    }
    if let Some(operation) = observation.operation_id() {
        if !resolver.operation_exists(operation)
            || !resolver.operation_reaches_step(operation, observation.step_id())
        {
            return Err(missing("operationId/stepId"));
        }
        if observation
            .space_id()
            .is_some_and(|space| !resolver.operation_in_space(operation, space))
        {
            return Err(cross_space("operationId"));
        }
    }
    if let Some(worktree) = observation.worktree_id() {
        if !resolver.worktree_exists(worktree) {
            return Err(missing("worktreeId"));
        }
        if observation
            .space_id()
            .is_some_and(|space| !resolver.worktree_in_space(worktree, space))
        {
            return Err(cross_space("worktreeId"));
        }
    }
    validate_actor_device(observation.actor_id(), observation.device_id(), resolver)
}

pub fn validate_marker_admission(marker: &Marker, resolver: &impl AdmissionResolver) -> Result<()> {
    if !resolver.space_exists(marker.space_id())
        || !resolver.step_admitted_to_space(marker.target_step_id(), marker.space_id())
    {
        return Err(cross_space("targetStepId"));
    }
    if marker
        .proof_ids()
        .iter()
        .any(|id| !resolver.proof_in_space(id, marker.space_id()))
    {
        return Err(cross_space("proofIds"));
    }
    validate_actor_device(marker.created_by(), None, resolver)
}

pub fn validate_release_admission(
    release: &Release,
    resolver: &impl AdmissionResolver,
) -> Result<()> {
    if !resolver.space_exists(release.space_id())
        || !resolver.step_admitted_to_space(release.target_step_id(), release.space_id())
    {
        return Err(cross_space("targetStepId"));
    }
    if release
        .proof_ids()
        .iter()
        .any(|id| !resolver.proof_in_space(id, release.space_id()))
    {
        return Err(cross_space("proofIds"));
    }
    validate_actor_device(release.created_by(), None, resolver)
}

pub fn validate_release_publication_admission(
    publication: &ReleasePublication,
    resolver: &impl AdmissionResolver,
) -> Result<()> {
    if !resolver.space_exists(publication.space_id())
        || !resolver.release_in_space(publication.release_id(), publication.space_id())
    {
        return Err(cross_space("releaseId"));
    }
    if publication
        .operation_id()
        .is_some_and(|id| !resolver.operation_in_space(id, publication.space_id()))
    {
        return Err(cross_space("operationId"));
    }
    validate_actor_device(
        publication.published_by(),
        publication.device_id(),
        resolver,
    )
}

pub fn validate_repository_view_admission(
    view: &RepositoryView,
    resolver: &impl AdmissionResolver,
) -> Result<()> {
    let space = view.space_id();
    if !resolver.space_exists(space) {
        return Err(cross_space("spaceId"));
    }
    for head_set in view.layer_heads() {
        if !resolver.layer_in_space(head_set.layer_id(), space) {
            return Err(cross_space("layerHeads"));
        }
        for (index, head) in head_set.step_ids().iter().enumerate() {
            if !resolver.step_admitted_to_space(head, space) {
                return Err(cross_space("layerHeads.stepIds"));
            }
            for other in head_set.step_ids().iter().skip(index + 1) {
                if resolver.is_step_ancestor(head, other) || resolver.is_step_ancestor(other, head)
                {
                    return Err(ScmError::LayerHeadsNotAntichain);
                }
            }
        }
    }
    if view
        .marker_ids()
        .iter()
        .any(|id| !resolver.marker_in_space(id, space))
    {
        return Err(cross_space("markerIds"));
    }
    if view
        .release_ids()
        .iter()
        .any(|id| !resolver.release_in_space(id, space))
    {
        return Err(cross_space("releaseIds"));
    }
    for state in view.worktrees() {
        if !resolver.worktree_in_space(state.worktree_id(), space) {
            return Err(cross_space("worktrees"));
        }
        if state
            .active_layer_id()
            .is_some_and(|id| !resolver.layer_in_space(id, space))
        {
            return Err(cross_space("worktrees.activeLayerId"));
        }
        if state
            .base_step_id()
            .is_some_and(|id| !resolver.step_admitted_to_space(id, space))
        {
            return Err(cross_space("worktrees.baseStepId"));
        }
        if state
            .materialized_tree_id()
            .is_some_and(|id| !resolver.tree_admitted_to_space(id, space))
        {
            return Err(cross_space("worktrees.materializedTreeId"));
        }
    }
    Ok(())
}

pub fn validate_operation_admission(
    operation: &Operation,
    resolver: &impl AdmissionResolver,
) -> Result<()> {
    let parents = operation.parent_operation_ids();
    if matches!(operation.kind(), OperationKind::Initialize) != parents.is_empty() {
        return Err(ScmError::InvalidField {
            field: "parentOperationIds",
            reason: "only initialize has zero parents".into(),
        });
    }
    if !resolver.space_exists(operation.space_id())
        || parents
            .iter()
            .any(|id| !resolver.operation_in_space(id, operation.space_id()))
    {
        return Err(cross_space("spaceId/parentOperationIds"));
    }
    if !resolver.repository_view_in_space(operation.resulting_view_id(), operation.space_id()) {
        return Err(cross_space("resultingViewId"));
    }
    for rewrite in operation.rewrite_map() {
        if !resolver.step_admitted_to_space(rewrite.old_step_id(), operation.space_id())
            || rewrite
                .new_step_ids()
                .iter()
                .any(|id| !resolver.step_admitted_to_space(id, operation.space_id()))
        {
            return Err(cross_space("rewriteMap"));
        }
    }
    validate_actor_device(operation.actor_id(), operation.device_id(), resolver)
}

pub fn validate_worktree_admission(
    worktree: &Worktree,
    resolver: &impl AdmissionResolver,
) -> Result<()> {
    if !resolver.space_exists(worktree.space_id()) || !resolver.store_exists(worktree.store_id()) {
        return Err(cross_space("spaceId/storeId"));
    }
    if !resolver.operation_in_space(worktree.active_operation_id(), worktree.space_id()) {
        return Err(cross_space("activeOperationId"));
    }
    Ok(())
}

fn validate_actor_device(
    actor: Option<&ActorId>,
    device: Option<&DeviceId>,
    resolver: &impl AdmissionResolver,
) -> Result<()> {
    if actor.is_some_and(|id| !resolver.actor_exists(id)) {
        return Err(missing("actor"));
    }
    if device.is_some_and(|id| !resolver.device_exists(id)) {
        return Err(missing("device"));
    }
    Ok(())
}

fn cross_space(field: &'static str) -> ScmError {
    ScmError::InvalidField {
        field,
        reason: "reference is missing or belongs to another Space".into(),
    }
}

fn missing(field: &'static str) -> ScmError {
    ScmError::InvalidField {
        field,
        reason: "referenced object does not exist or does not reach its target".into(),
    }
}
