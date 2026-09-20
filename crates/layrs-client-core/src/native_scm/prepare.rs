use super::{NativeClientError, PreparedSnapshot, Result};
use layrs_core::scm::{
    CaptureReason, Layer, LayerHeadSet, Metadata, ObservationId, Operation, OperationId,
    OperationKind, RepositoryView, Space, Step, StepId, StepObservation, Timestamp, Worktree,
    WorktreeId, WorktreeStatus, WorktreeViewState,
};
use layrs_store::scm::{
    ApplyBatchOutcome, CanonicalObject, EntityRecord, OperationHeadsCas, RepositoryViewCas,
    ScmReader, ScmRepository, SpaceObjectAdmission, StoreMutation, TransactionId,
    WorktreeActiveOperationCas, WriteBatch,
};
use layrs_store_local::scm_v1::{open, NativeScmReader, NativeScmRepository};
use std::path::{Path, PathBuf};

/// Generated once by the caller and retained with a prepared batch for retries.
/// Time and observations never participate in Step identity.
#[derive(Clone, Debug)]
pub struct MutationStamp {
    pub transaction_id: TransactionId,
    pub operation_id: OperationId,
    pub observation_id: ObservationId,
    pub timestamp: Timestamp,
}

#[derive(Debug)]
pub struct PreparedMutation {
    pub(super) batch: WriteBatch,
    pub(super) step_id: StepId,
    pub(super) chunks: Option<std::sync::Arc<super::snapshot::SnapshotChunkSpool>>,
}
impl PreparedMutation {
    pub fn batch(&self) -> &WriteBatch {
        &self.batch
    }
    pub fn step_id(&self) -> &StepId {
        &self.step_id
    }
}

#[derive(Debug)]
pub enum CapturePlan {
    Unchanged { step_id: StepId },
    Change(PreparedMutation),
}

/// Read BEFORE scanning. Opaque expectations prevent accepting bytes captured
/// against an older Worktree state after a concurrent switch/capture/receive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureBase {
    store_id: layrs_core::scm::StoreId,
    worktree_id: WorktreeId,
    operation_id: OperationId,
    view_id: layrs_core::scm::RepositoryViewId,
    generation: u64,
    heads: Vec<OperationId>,
}

/// Initializes metadata in an already-created, fresh native Store. The Worktree
/// path/identity and snapshot must have been validated by the capture caller.
/// No checkout is written and no old store is converted by this function.
pub fn prepare_initialize(
    space: Space,
    layer: Layer,
    worktree: Worktree,
    snapshot: PreparedSnapshot,
    stamp: MutationStamp,
) -> Result<PreparedMutation> {
    if layer.space_id() != space.id()
        || worktree.space_id() != space.id()
        || worktree.active_operation_id() != &stamp.operation_id
        || !layer.parent_layer_ids().is_empty()
    {
        return Err(NativeClientError::InvalidState(
            "initial Space, Layer, Worktree and Operation disagree",
        ));
    }
    let step = Step::new(vec![], snapshot.root_tree_id().clone())?;
    let view = RepositoryView::new(
        space.id().clone(),
        vec![LayerHeadSet::new(
            layer.id().clone(),
            vec![step.id().clone()],
        )?],
        vec![],
        vec![],
        vec![WorktreeViewState::new(
            worktree.id().clone(),
            Some(layer.id().clone()),
            Some(step.id().clone()),
            Some(snapshot.root_tree_id().clone()),
            WorktreeStatus::Ready,
        )],
    )?;
    let operation = operation(
        &stamp,
        space.id().clone(),
        vec![],
        &view,
        OperationKind::Initialize,
        "Initialize native Space",
    )?;
    let observation = observation(&stamp, &step, &worktree);
    let chunks = snapshot.chunks.clone();
    let objects = objects(snapshot, &step, &view);
    let admissions = admissions_with_chunks(space.id(), &objects, chunks.as_deref());
    let batch = WriteBatch::new(
        stamp.transaction_id,
        objects,
        vec![
            EntityRecord::Space(space),
            EntityRecord::Layer(layer),
            EntityRecord::Worktree(worktree),
            EntityRecord::Operation(operation.clone()),
            EntityRecord::StepObservation(observation),
        ],
        admissions,
        vec![
            StoreMutation::RepositoryView(RepositoryViewCas::new(
                view.space_id().clone(),
                0,
                None,
                view.id().clone(),
            )?),
            StoreMutation::OperationHeads(OperationHeadsCas::new(
                view.space_id().clone(),
                vec![],
                operation.id().clone(),
                vec![operation.id().clone()],
            )?),
        ],
    )?;
    Ok(PreparedMutation {
        batch,
        step_id: step.id().clone(),
        chunks,
    })
}

/// Path-owned facade: reads are pinned under one shared Store capability;
/// preparation releases it before the caller performs acceptance.
pub struct NativeClient {
    pub(super) root: PathBuf,
}
impl NativeClient {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let store = open(root.as_ref())?;
        Ok(Self {
            root: store.layout().root().to_path_buf(),
        })
    }

    pub fn accept(&self, change: &PreparedMutation) -> Result<ApplyBatchOutcome> {
        let mut repository = NativeScmRepository::new(&self.root);
        Ok(match &change.chunks {
            Some(chunks) => {
                repository.apply_batch_with_chunks(change.batch(), &chunks.descriptors(), |id| {
                    chunks.read(id)
                })?
            }
            None => repository.apply_batch(change.batch())?,
        })
    }

    pub fn capture_base(&self, worktree_id: &WorktreeId) -> Result<CaptureBase> {
        let store = open(&self.root)?;
        if layrs_store_local::scm_v1::materialize::recovery_pending(&store, worktree_id)? {
            return Err(NativeClientError::NeedsRepair);
        }
        let reader = NativeScmReader::new(&store)?;
        let worktree = reader
            .get_worktree(worktree_id)?
            .ok_or(NativeClientError::Missing("Worktree"))?;
        let operation = reader
            .get_operation(worktree.active_operation_id())?
            .ok_or(NativeClientError::Missing("Operation"))?;
        let current = reader
            .current_repository_view(worktree.space_id())?
            .ok_or(NativeClientError::Missing("current View"))?;
        Ok(CaptureBase {
            store_id: store.store_id().clone(),
            worktree_id: worktree_id.clone(),
            operation_id: operation.id().clone(),
            view_id: operation.resulting_view_id().clone(),
            generation: current.generation(),
            heads: reader.operation_heads(worktree.space_id())?,
        })
    }

    pub fn prepare_capture(
        &self,
        expected: &CaptureBase,
        snapshot: PreparedSnapshot,
        stamp: MutationStamp,
    ) -> Result<CapturePlan> {
        self.prepare_capture_with_reason(expected, snapshot, stamp, CaptureReason::ManualSnapshot)
    }

    pub fn prepare_capture_with_reason(
        &self,
        expected: &CaptureBase,
        snapshot: PreparedSnapshot,
        stamp: MutationStamp,
        reason: CaptureReason,
    ) -> Result<CapturePlan> {
        let worktree_id = &expected.worktree_id;
        let store = open(&self.root)?;
        let reader = NativeScmReader::new(&store)?;
        let worktree = reader
            .get_worktree(worktree_id)?
            .ok_or(NativeClientError::Missing("Worktree"))?;
        let active = reader
            .get_operation(worktree.active_operation_id())?
            .ok_or(NativeClientError::Missing("Operation"))?;
        let view = reader
            .get_repository_view(active.resulting_view_id())?
            .ok_or(NativeClientError::Missing("RepositoryView"))?;
        let current = reader
            .current_repository_view(worktree.space_id())?
            .ok_or(NativeClientError::Missing("current View"))?;
        let heads = reader.operation_heads(worktree.space_id())?;
        if store.store_id() != &expected.store_id
            || active.id() != &expected.operation_id
            || view.id() != &expected.view_id
            || current.generation() != expected.generation
            || heads != expected.heads
        {
            return Err(NativeClientError::StaleCapture);
        }
        // This first slice refuses integration rather than silently choosing a
        // global head. The prepared bytes are never written into the checkout.
        if heads != [active.id().clone()] || current.view_id() != Some(view.id()) {
            return Err(NativeClientError::NeedsIntegration);
        }
        let state = view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == worktree_id)
            .ok_or(NativeClientError::Missing("Worktree View state"))?;
        if state.status() != WorktreeStatus::Ready {
            return Err(NativeClientError::InvalidState(
                "capture requires a ready Worktree",
            ));
        }
        let layer_id = state
            .active_layer_id()
            .ok_or(NativeClientError::Missing("active Layer"))?;
        let base_id = state
            .base_step_id()
            .ok_or(NativeClientError::Missing("base Step"))?;
        let layer_heads = view
            .layer_heads()
            .iter()
            .find(|h| h.layer_id() == layer_id)
            .ok_or(NativeClientError::Missing("Layer heads"))?;
        if !layer_heads.step_ids().contains(base_id) {
            return Err(NativeClientError::NeedsIntegration);
        }
        let base = reader
            .get_step(base_id)?
            .ok_or(NativeClientError::Missing("base Step"))?;
        if snapshot.root_tree_id() == base.root_tree_id() {
            return Ok(CapturePlan::Unchanged {
                step_id: base.id().clone(),
            });
        }
        let step = Step::new(vec![base.id().clone()], snapshot.root_tree_id().clone())?;
        let new_heads = view
            .layer_heads()
            .iter()
            .map(|h| {
                if h.layer_id() == layer_id {
                    // Capture descends only from the materialized base. Other
                    // concurrent heads remain visible until an explicit Weave.
                    let mut preserved = layer_heads
                        .step_ids()
                        .iter()
                        .filter(|id| *id != base_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    preserved.push(step.id().clone());
                    LayerHeadSet::new(layer_id.clone(), preserved)
                } else {
                    Ok(h.clone())
                }
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let states = view
            .worktrees()
            .iter()
            .map(|w| {
                if w.worktree_id() == worktree_id {
                    WorktreeViewState::new(
                        worktree_id.clone(),
                        Some(layer_id.clone()),
                        Some(step.id().clone()),
                        Some(snapshot.root_tree_id().clone()),
                        WorktreeStatus::Ready,
                    )
                } else {
                    w.clone()
                }
            })
            .collect();
        let next = RepositoryView::new(
            worktree.space_id().clone(),
            new_heads,
            view.marker_ids().to_vec(),
            view.release_ids().to_vec(),
            states,
        )?;
        let identity = layrs_store_local::scm_v1::local_identity(&store)?;
        let operation = operation_with_identity(
            &stamp,
            worktree.space_id().clone(),
            vec![active.id().clone()],
            &next,
            OperationKind::CaptureWorktree,
            "Capture Worktree",
            identity.as_ref(),
        )?;
        let observation =
            observation_with_identity(&stamp, &step, &worktree, reason, identity.as_ref());
        let chunks = snapshot.chunks.clone();
        let objects = objects(snapshot, &step, &next);
        let admissions = admissions_with_chunks(worktree.space_id(), &objects, chunks.as_deref());
        let batch = WriteBatch::new(
            stamp.transaction_id,
            objects,
            vec![
                EntityRecord::Operation(operation.clone()),
                EntityRecord::StepObservation(observation),
            ],
            admissions,
            vec![
                StoreMutation::RepositoryView(RepositoryViewCas::new(
                    worktree.space_id().clone(),
                    current.generation(),
                    Some(view.id().clone()),
                    next.id().clone(),
                )?),
                StoreMutation::OperationHeads(OperationHeadsCas::new(
                    worktree.space_id().clone(),
                    heads,
                    operation.id().clone(),
                    vec![operation.id().clone()],
                )?),
                StoreMutation::WorktreeActiveOperation(WorktreeActiveOperationCas::new(
                    worktree_id.clone(),
                    active.id().clone(),
                    operation.id().clone(),
                )?),
            ],
        )?;
        Ok(CapturePlan::Change(PreparedMutation {
            batch,
            step_id: step.id().clone(),
            chunks,
        }))
    }
}

pub(super) fn operation(
    stamp: &MutationStamp,
    space: layrs_core::scm::SpaceId,
    parents: Vec<OperationId>,
    view: &RepositoryView,
    kind: OperationKind,
    summary: &str,
) -> Result<Operation> {
    operation_with_identity(stamp, space, parents, view, kind, summary, None)
}
fn operation_with_identity(
    stamp: &MutationStamp,
    space: layrs_core::scm::SpaceId,
    parents: Vec<OperationId>,
    view: &RepositoryView,
    kind: OperationKind,
    summary: &str,
    identity: Option<&layrs_store_local::scm_v1::LocalIdentity>,
) -> Result<Operation> {
    Ok(Operation::new(
        stamp.operation_id.clone(),
        space,
        parents,
        view.id().clone(),
        kind,
        summary,
        vec![],
        identity.map(|i| i.actor_id.clone()),
        identity.and_then(|i| i.device_id.clone()),
        stamp.timestamp.clone(),
        Metadata::empty(),
    )?)
}
fn observation(stamp: &MutationStamp, step: &Step, worktree: &Worktree) -> StepObservation {
    observation_with_reason(stamp, step, worktree, CaptureReason::ManualSnapshot)
}
fn observation_with_reason(
    stamp: &MutationStamp,
    step: &Step,
    worktree: &Worktree,
    reason: CaptureReason,
) -> StepObservation {
    observation_with_identity(stamp, step, worktree, reason, None)
}
fn observation_with_identity(
    stamp: &MutationStamp,
    step: &Step,
    worktree: &Worktree,
    reason: CaptureReason,
    identity: Option<&layrs_store_local::scm_v1::LocalIdentity>,
) -> StepObservation {
    StepObservation::new(
        stamp.observation_id.clone(),
        step.id().clone(),
        Some(worktree.space_id().clone()),
        Some(stamp.operation_id.clone()),
        Some(worktree.id().clone()),
        identity.map(|i| i.actor_id.clone()),
        identity.and_then(|i| i.device_id.clone()),
        stamp.timestamp.clone(),
        reason,
        Metadata::empty(),
    )
}
fn admissions_with_chunks(
    space: &layrs_core::scm::SpaceId,
    objects: &[CanonicalObject],
    chunks: Option<&super::snapshot::SnapshotChunkSpool>,
) -> Vec<SpaceObjectAdmission> {
    let mut result = admissions(space, objects);
    if let Some(chunks) = chunks {
        result.extend(chunks.descriptors().into_iter().map(|(id, _)| {
            SpaceObjectAdmission::new(space.clone(), layrs_store::scm::StoreObjectId::ChunkRaw(id))
        }));
    }
    result
}

fn objects(snapshot: PreparedSnapshot, step: &Step, view: &RepositoryView) -> Vec<CanonicalObject> {
    let mut objects = snapshot.objects().to_vec();
    objects.push(CanonicalObject::Step(step.clone()));
    objects.push(CanonicalObject::RepositoryView(view.clone()));
    objects
}
pub(super) fn admissions(
    space: &layrs_core::scm::SpaceId,
    objects: &[CanonicalObject],
) -> Vec<SpaceObjectAdmission> {
    objects
        .iter()
        .map(|o| SpaceObjectAdmission::new(space.clone(), o.object_id()))
        .collect()
}
