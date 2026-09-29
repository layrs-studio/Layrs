//! Prepared local rewrites. Immutable Steps and named points remain reachable
//! through old Operations; acceptance only moves the selected active Layer.
use super::Result;
use super::{merge, references::base_step, restore, *};
use layrs_core::scm::*;
use layrs_store::scm::{CanonicalConflict, CanonicalObject};
#[path = "rewrite_conflict.rs"]
mod conflicted;
use layrs_store_local::scm_v1::{
    self,
    checkout::{CheckoutContent, CheckoutEntry},
    materialize::NoMaterializationFaults,
    NativeScmReader,
};
use layrs_weave::{
    graph_merge::{self, GraphMergeLimits, MergeObjects, PlannedConflict},
    reconciliation::{FailureAction, FileMergeLimits, FileMergePolicy},
};

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRewriteKind {
    Replay,
    Squash,
    Revert,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRewriteSummary {
    pub kind: NativeRewriteKind,
    pub expected_operation_id: OperationId,
    pub expected_view_id: RepositoryViewId,
    pub target_layer_id: LayerId,
    pub target_heads_before: Vec<StepId>,
    pub proposed_step: Step,
    pub rewrite_map: Vec<RewriteTarget>,
    pub conflict_count: usize,
    pub markers_unchanged: Vec<MarkerId>,
    pub releases_unchanged: Vec<ReleaseId>,
}

pub struct NativeRewritePlan {
    state: WorkspaceState,
    summary: NativeRewriteSummary,
    conflicts: Vec<PlannedConflict>,
    after: Vec<CheckoutEntry>,
    snapshot: Option<PreparedSnapshot>,
    description: String,
    canonical: Vec<CanonicalObject>,
}
impl NativeRewritePlan {
    pub fn summary(&self) -> &NativeRewriteSummary {
        &self.summary
    }
    pub fn conflicts(&self) -> &[PlannedConflict] {
        &self.conflicts
    }
}
fn reconciliation_error(error: impl std::fmt::Display) -> NativeClientError {
    NativeClientError::InvalidSnapshot(format!("rewrite: {error}"))
}
fn active_layer(state: &WorkspaceState) -> Result<LayerId> {
    let base = base_step(state)?;
    let worktree = state
        .view
        .worktrees()
        .iter()
        .find(|entry| entry.worktree_id() == state.worktree.id())
        .ok_or(NativeClientError::Missing("active Worktree"))?;
    let layer = worktree
        .active_layer_id()
        .ok_or(NativeClientError::InvalidState(
            "rewrite requires an active Layer",
        ))?;
    let heads = state
        .view
        .layer_heads()
        .iter()
        .find(|heads| heads.layer_id() == layer)
        .ok_or(NativeClientError::Missing("active Layer heads"))?;
    if heads.step_ids() != [base] {
        return Err(NativeClientError::NeedsIntegration);
    }
    Ok(layer.clone())
}

impl NativeWorkspace {
    /// Captures pending work first. The returned plan is pinned to that exact
    /// resulting Operation/View, and must be explicitly accepted afterwards.
    pub fn prepare_replay(
        &self,
        source: &StepId,
        mainline: Option<&StepId>,
    ) -> Result<NativeRewritePlan> {
        self.prepare_delta(source, mainline, false, true)
    }
    pub fn prepare_revert(
        &self,
        source: &StepId,
        mainline: Option<&StepId>,
    ) -> Result<NativeRewritePlan> {
        self.prepare_delta(source, mainline, true, true)
    }
    pub fn preview_replay(
        &self,
        source: &StepId,
        mainline: Option<&StepId>,
    ) -> Result<NativeRewritePlan> {
        self.prepare_delta(source, mainline, false, false)
    }
    pub fn preview_revert(
        &self,
        source: &StepId,
        mainline: Option<&StepId>,
    ) -> Result<NativeRewritePlan> {
        self.prepare_delta(source, mainline, true, false)
    }
    fn prepare_or_validate_checkout(&self, capture: bool) -> Result<()> {
        if capture {
            self.capture_with_reason(CaptureReason::BeforeOperation)?;
        } else if self.is_dirty()? {
            return Err(NativeClientError::InvalidState(
                "capture pending edits before requesting a read-only preview",
            ));
        }
        Ok(())
    }
    fn prepare_delta(
        &self,
        selected: &StepId,
        mainline: Option<&StepId>,
        inverse: bool,
        capture: bool,
    ) -> Result<NativeRewritePlan> {
        self.prepare_or_validate_checkout(capture)?;
        let state = self.state()?;
        let layer = active_layer(&state)?;
        let target = base_step(&state)?;
        let limits = GraphMergeLimits::default();
        let store = scm_v1::open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        let mut objects = merge::load_objects(
            &reader,
            state.space.id(),
            vec![selected.clone(), target.clone()],
            limits,
        )?;
        let bundle = graph_merge::plan_replay(
            &objects,
            selected,
            mainline,
            &target,
            inverse,
            FileMergePolicy {
                unsupported: FailureAction::StructuralFallback,
                quota: FailureAction::Reject,
                limits: FileMergeLimits::default(),
            },
            limits,
        )
        .map_err(reconciliation_error)?;
        let canonical = if bundle.conflicts.is_empty() {
            vec![]
        } else {
            let mut result = vec![CanonicalObject::Step(bundle.step.clone())];
            result.extend(bundle.trees.iter().cloned().map(CanonicalObject::Tree));
            result.extend(bundle.chunks.iter().cloned().map(CanonicalObject::ChunkRaw));
            result.extend(
                bundle
                    .file_manifests
                    .iter()
                    .cloned()
                    .map(CanonicalObject::FileManifest),
            );
            for conflict in &bundle.conflicts {
                result.push(CanonicalObject::Conflict(CanonicalConflict::new(
                    conflict.id(),
                    conflict.canonical_bytes().map_err(reconciliation_error)?,
                )?));
            }
            result
        };
        let step = bundle.step;
        let conflicts = bundle.conflicts;
        for tree in bundle.trees {
            objects.trees.insert(tree.id().clone(), tree);
        }
        let chunks = bundle
            .chunks
            .into_iter()
            .map(|chunk| (chunk.id().clone(), chunk))
            .collect::<std::collections::BTreeMap<_, _>>();
        for manifest in bundle.file_manifests {
            let bytes =
                manifest.reconstruct(|id| chunks.get(id).map(|chunk| chunk.bytes().to_vec()))?;
            objects.files.insert(manifest.file_id().clone(), bytes);
        }
        let (after, snapshot) = if conflicts.is_empty() {
            let entries = checkout_entries(&objects, step.root_tree_id(), limits)?;
            let snapshot = restore::snapshot_entries(&entries)?;
            if snapshot.root_tree_id() != step.root_tree_id() {
                return Err(NativeClientError::InvalidState(
                    "rewrite contains objects not representable by this checkout",
                ));
            }
            (entries, Some(snapshot))
        } else {
            (vec![], None)
        };
        let map = if inverse {
            vec![]
        } else {
            vec![RewriteTarget::new(
                selected.clone(),
                vec![step.id().clone()],
            )?]
        };
        let kind = if inverse {
            NativeRewriteKind::Revert
        } else {
            NativeRewriteKind::Replay
        };
        let description = format!(
            "{} Step {}",
            if inverse { "Revert" } else { "Replay" },
            selected
        );
        let mut plan = make_plan(
            state,
            layer,
            kind,
            step,
            map,
            conflicts,
            after,
            snapshot,
            description,
        );
        plan.canonical = canonical;
        Ok(plan)
    }

    /// Squashes the linear contiguous range first..active-head, inclusive.
    /// At least two Steps are required; merges and detached/multi-head Layers
    /// are rejected instead of choosing an implicit first parent.
    pub fn prepare_squash(&self, first: &StepId) -> Result<NativeRewritePlan> {
        self.squash(first, true)
    }
    pub fn preview_squash(&self, first: &StepId) -> Result<NativeRewritePlan> {
        self.squash(first, false)
    }
    fn squash(&self, first: &StepId, capture: bool) -> Result<NativeRewritePlan> {
        self.prepare_or_validate_checkout(capture)?;
        let state = self.state()?;
        let layer = active_layer(&state)?;
        let target = base_step(&state)?;
        let limits = GraphMergeLimits::default();
        let store = scm_v1::open(self.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        let objects = merge::load_objects(&reader, state.space.id(), vec![target.clone()], limits)?;
        let mut selected = vec![];
        let mut cursor = target.clone();
        let parents = loop {
            let step = objects
                .steps
                .get(&cursor)
                .ok_or(NativeClientError::Missing("squash Step"))?;
            if step.parent_step_ids().len() > 1 {
                return Err(NativeClientError::InvalidState(
                    "squash range must be linear; merge Steps unsupported",
                ));
            }
            selected.push(cursor.clone());
            if &cursor == first {
                break step.parent_step_ids().to_vec();
            }
            cursor = step
                .parent_step_ids()
                .first()
                .ok_or(NativeClientError::InvalidState(
                    "squash start is not on active linear ancestry",
                ))?
                .clone();
            if selected.len() >= limits.max_steps {
                return Err(NativeClientError::InvalidState("squash selection limit"));
            }
        };
        if selected.len() < 2 {
            return Err(NativeClientError::InvalidState(
                "squash requires at least two contiguous Steps",
            ));
        }
        let step = Step::new(parents, objects.steps[&target].root_tree_id().clone())?;
        let after = checkout_entries(&objects, step.root_tree_id(), limits)?;
        let snapshot = restore::snapshot_entries(&after)?;
        if snapshot.root_tree_id() != step.root_tree_id() {
            return Err(NativeClientError::InvalidState(
                "squash checkout cannot represent complete Tree",
            ));
        }
        let map = selected
            .into_iter()
            .map(|old| RewriteTarget::new(old, vec![step.id().clone()]))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(make_plan(
            state,
            layer,
            NativeRewriteKind::Squash,
            step,
            map,
            vec![],
            after,
            Some(snapshot),
            format!("Squash linear range from Step {first}"),
        ))
    }

    /// Does not recapture or reinterpret a stale plan. The materializer verifies
    /// exact disk preconditions and CAS state before publishing the Operation.
    pub fn apply_rewrite(&self, plan: &NativeRewritePlan) -> Result<StepId> {
        if plan.state.worktree.id() != self.worktree_id() {
            return Err(NativeClientError::InvalidState(
                "rewrite belongs to another Worktree",
            ));
        }
        let current = self.state()?;
        if current.operation.id() != plan.state.operation.id()
            || current.view.id() != plan.state.view.id()
        {
            return Err(NativeClientError::StaleCapture);
        }
        if !plan.conflicts.is_empty() {
            return self.apply_conflicted_rewrite(plan);
        }
        let snapshot = plan
            .snapshot
            .clone()
            .ok_or(NativeClientError::Missing("clean rewrite snapshot"))?;
        let kind = match plan.summary.kind {
            NativeRewriteKind::Revert => OperationKind::Revert,
            _ => OperationKind::RewriteSteps,
        };
        self.apply_rewrite_transition(
            &plan.state,
            plan.summary.proposed_step.id().clone(),
            Some(plan.summary.target_layer_id.clone()),
            plan.after.clone(),
            Some((plan.summary.proposed_step.clone(), snapshot)),
            None,
            kind,
            &plan.description,
            plan.summary.rewrite_map.clone(),
            &NoMaterializationFaults,
        )?;
        Ok(plan.summary.proposed_step.id().clone())
    }
}

#[allow(clippy::too_many_arguments)]
fn make_plan(
    state: WorkspaceState,
    layer: LayerId,
    kind: NativeRewriteKind,
    step: Step,
    map: Vec<RewriteTarget>,
    conflicts: Vec<PlannedConflict>,
    after: Vec<CheckoutEntry>,
    snapshot: Option<PreparedSnapshot>,
    description: String,
) -> NativeRewritePlan {
    let summary = NativeRewriteSummary {
        kind,
        expected_operation_id: state.operation.id().clone(),
        expected_view_id: state.view.id().clone(),
        target_heads_before: state
            .view
            .layer_heads()
            .iter()
            .find(|heads| heads.layer_id() == &layer)
            .map(|heads| heads.step_ids().to_vec())
            .unwrap_or_default(),
        target_layer_id: layer,
        proposed_step: step,
        rewrite_map: map,
        conflict_count: conflicts.len(),
        markers_unchanged: state.view.marker_ids().to_vec(),
        releases_unchanged: state.view.release_ids().to_vec(),
    };
    NativeRewritePlan {
        state,
        summary,
        conflicts,
        after,
        snapshot,
        description,
        canonical: vec![],
    }
}

pub(super) fn checkout_entries(
    objects: &MergeObjects,
    root: &TreeId,
    limits: GraphMergeLimits,
) -> Result<Vec<CheckoutEntry>> {
    let mut pending = vec![(String::new(), root.clone(), 0)];
    let mut entries = vec![];
    let mut paths = 0usize;
    let mut bytes = 0usize;
    while let Some((prefix, id, depth)) = pending.pop() {
        if depth > limits.max_depth {
            return Err(NativeClientError::InvalidState(
                "rewrite checkout depth limit",
            ));
        }
        let tree = objects
            .trees
            .get(&id)
            .ok_or(NativeClientError::Missing("rewrite Tree"))?;
        for entry in tree.entries() {
            paths += 1;
            if paths > limits.max_paths {
                return Err(NativeClientError::InvalidState(
                    "rewrite checkout path limit",
                ));
            }
            let path = if prefix.is_empty() {
                entry.name().as_str().to_owned()
            } else {
                format!("{prefix}/{}", entry.name())
            };
            let content = match entry.target() {
                TreeEntryTarget::Tree { tree_id } => {
                    pending.push((path, tree_id.clone(), depth + 1));
                    continue;
                }
                TreeEntryTarget::File { file_id } => {
                    let data = objects
                        .files
                        .get(file_id)
                        .ok_or(NativeClientError::Missing("rewrite File"))?;
                    bytes = bytes
                        .checked_add(data.len())
                        .ok_or(NativeClientError::InvalidState("rewrite checkout bytes"))?;
                    CheckoutContent::File {
                        bytes: data.clone(),
                        executable: entry.executable(),
                    }
                }
                TreeEntryTarget::Symlink { symlink_id } => CheckoutContent::Symlink {
                    target: objects
                        .symlinks
                        .get(symlink_id)
                        .ok_or(NativeClientError::Missing("rewrite Symlink"))?
                        .target()
                        .to_owned(),
                },
                TreeEntryTarget::Tombstone { .. } => continue,
                TreeEntryTarget::Conflict { .. } => {
                    return Err(NativeClientError::InvalidState(
                        "rewrite has unresolved structured conflicts",
                    ));
                }
            };
            if bytes > limits.max_output_bytes {
                return Err(NativeClientError::InvalidState(
                    "rewrite checkout byte limit",
                ));
            }
            entries.push(CheckoutEntry { path, content });
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}
