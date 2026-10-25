//! Read-only restore previews and optimistic guards. A verified capture is
//! prepared once; checkout writes still pass the materializer's lock and CAS.
use super::Result;
use super::*;
use layrs_core::scm::*;
use layrs_store::scm::{EntityRecord, ScmReader};
use layrs_store_local::scm_v1::{
    self,
    checkout::{CheckoutLimits, CheckoutReader},
    NativeScmReader,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreExpectation {
    pub operation_id: OperationId,
    pub worktree_id: WorktreeId,
    pub projection: TreeId,
    pub plan_digest: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreCollision {
    pub path: String,
    pub reason: &'static str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRestorePreview {
    pub step_id: StepId,
    pub path: Option<String>,
    pub changes: Vec<NativeFileChange>,
    pub collisions: Vec<RestoreCollision>,
    pub expected: RestoreExpectation,
}

enum Target<'a> {
    Step(&'a StepId, Option<&'a str>),
    View(&'a OperationId),
}
struct Checked {
    preview: NativeRestorePreview,
    state: WorkspaceState,
    base: CaptureBase,
    snapshot: PreparedSnapshot,
}

impl NativeWorkspace {
    pub fn preview_restore_step(
        &self,
        step: &StepId,
        path: Option<&str>,
    ) -> Result<NativeRestorePreview> {
        Ok(self.check_restore(Target::Step(step, path))?.preview)
    }
    pub fn preview_restore_operation(
        &self,
        operation: &OperationId,
    ) -> Result<NativeRestorePreview> {
        Ok(self.check_restore(Target::View(operation))?.preview)
    }
    pub fn restore_step_guarded(
        &self,
        step: &StepId,
        path: Option<&str>,
        expected: &RestoreExpectation,
    ) -> Result<StepId> {
        let checked = self.check_restore(Target::Step(step, path))?;
        let source = self.accept_restore_capture(checked, expected, false)?;
        self.restore_step_from_state(
            &source,
            step,
            path,
            &scm_v1::materialize::NoMaterializationFaults,
        )
    }
    pub fn restore_operation_guarded(
        &self,
        operation: &OperationId,
        expected: &RestoreExpectation,
    ) -> Result<OperationId> {
        let checked = self.check_restore(Target::View(operation))?;
        let source = self.accept_restore_capture(checked, expected, true)?;
        self.restore_operation_from_state(&source, operation, OperationKind::Restore)
    }

    fn check_restore(&self, target: Target<'_>) -> Result<Checked> {
        let base = self.client().capture_base(self.worktree_id())?;
        let state = self.state()?;
        let (step, path, scope) = match target {
            Target::Step(step, path) => {
                let path = path
                    .map(RepoPath::parse)
                    .transpose()?
                    .map(|p| p.as_str().to_owned());
                (
                    step.clone(),
                    path.clone(),
                    serde_json::json!({"step":step,"path":path}),
                )
            }
            Target::View(id) => {
                let store = self.open_store()?;
                let reader = NativeScmReader::new(&store)?;
                let operation = reader
                    .get_operation(id)?
                    .ok_or(NativeClientError::Missing("target Operation"))?;
                if operation.space_id() != state.space.id() {
                    return Err(NativeClientError::InvalidState(
                        "target Operation belongs to another Space",
                    ));
                }
                let view = reader
                    .get_repository_view(operation.resulting_view_id())?
                    .ok_or(NativeClientError::Missing("target View"))?;
                if view.worktrees().len() != state.view.worktrees().len()
                    || state
                        .view
                        .worktrees()
                        .iter()
                        .filter(|w| w.worktree_id() != self.worktree_id())
                        .any(|w| !view.worktrees().contains(w))
                {
                    return Err(NativeClientError::InvalidState(
                        "restore would affect another Worktree; acquire all leases first",
                    ));
                }
                let step = view
                    .worktrees()
                    .iter()
                    .find(|w| w.worktree_id() == self.worktree_id())
                    .and_then(|w| w.base_step_id())
                    .ok_or(NativeClientError::Missing("historical base Step"))?
                    .clone();
                (
                    step,
                    None,
                    serde_json::json!({"operation":id,"view":view.id()}),
                )
            }
        };
        let checkout = CheckoutReader::open(
            state.worktree.checkout_root(),
            Some(state.worktree.filesystem_identity()),
        )?;
        let limits = restore_projection::limits();
        let limits = CheckoutLimits {
            max_entries: limits.max_entries,
            max_depth: limits.max_depth,
            max_file_bytes: limits.max_file_bytes,
            max_total_bytes: limits.max_total_bytes,
        };
        let (_, snapshot) =
            self.capture_projection_streaming(&state, restore_projection::limits())?;
        let before = restore_projection::captured(&snapshot)?;
        let mut after = self.read_step_projection(&step)?;
        if let Some(path) = &path {
            let prefix = format!("{path}/");
            let matches = |p: &str| p == path || p.starts_with(&prefix);
            after.retain(|e| matches(&e.path));
            after.extend(before.iter().filter(|e| !matches(&e.path)).cloned());
            after.sort_by(|a, b| a.path.cmp(&b.path));
        }
        scm_v1::checkout::validate_streaming_projection(&after)?;
        let after_tree = scm_v1::checkout::projection_tree_id(&after)?;
        let union: BTreeSet<_> = before
            .iter()
            .chain(&after)
            .map(|e| e.path.clone())
            .collect();
        let (physical, _) =
            checkout.capture_streaming(limits, &union, &mut |_| Ok(()), &mut || Ok(()))?;
        let before_paths: BTreeSet<_> = before.iter().map(|e| e.path.as_str()).collect();
        let before_by_path: BTreeMap<_, _> = before
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        let mut collisions = physical
            .iter()
            .filter(|e| !before_paths.contains(e.path.as_str()))
            .map(|e| RestoreCollision {
                path: e.path.clone(),
                reason: "ignored_untracked_target",
            })
            .collect::<Vec<_>>();
        let mut collision_paths: BTreeSet<_> = collisions
            .iter()
            .map(|collision| collision.path.clone())
            .collect();
        for path in checkout.untracked_obstructions_projection(&before, &after, limits)? {
            if collision_paths.insert(path.clone()) {
                collisions.push(RestoreCollision {
                    path,
                    reason: "ignored_untracked_directory_content",
                });
            }
        }
        for entry in &after {
            if before_by_path
                .get(entry.path.as_str())
                .is_some_and(|old| **old == *entry)
            {
                continue;
            }
            if !cfg!(target_os = "linux")
                && matches!(
                    entry.content,
                    scm_v1::checkout::CheckoutProjectionContent::Symlink { .. }
                )
            {
                collisions.push(RestoreCollision {
                    path: entry.path.clone(),
                    reason: "symlink_materialization_unsupported",
                });
            }
            if cfg!(windows)
                && matches!(
                    entry.content,
                    scm_v1::checkout::CheckoutProjectionContent::File {
                        executable: true,
                        ..
                    }
                )
            {
                collisions.push(RestoreCollision {
                    path: entry.path.clone(),
                    reason: "executable_bit_materialization_unsupported",
                });
            }
        }
        // Ignore extra target collisions for this equality, but reject changes
        // to any file participating in the original projection during preview.
        if physical
            .iter()
            .filter(|e| before_paths.contains(e.path.as_str()))
            .ne(before.iter())
            || self.client().capture_base(self.worktree_id())? != base
        {
            return Err(NativeClientError::StaleCapture);
        }
        let digest = canonical_json(&serde_json::json!({"scope":scope,"after":after_tree,
            "view":state.view.id(),"filesystemIdentity":checkout.identity()}))?;
        let expected = RestoreExpectation {
            operation_id: state.operation.id().clone(),
            worktree_id: self.worktree_id().clone(),
            projection: snapshot.root_tree_id().clone(),
            plan_digest: blake3::hash(&digest).to_hex().to_string(),
        };
        Ok(Checked {
            preview: NativeRestorePreview {
                step_id: step,
                path,
                changes: restore_projection::preview_changes(self, &snapshot, &before, &after)?,
                collisions,
                expected,
            },
            state,
            base,
            snapshot,
        })
    }

    fn accept_restore_capture(
        &self,
        checked: Checked,
        expected: &RestoreExpectation,
        allow_conflicted: bool,
    ) -> Result<WorkspaceState> {
        if &checked.preview.expected != expected {
            return Err(NativeClientError::StaleCapture);
        }
        if !checked.preview.collisions.is_empty() {
            return Err(NativeClientError::InvalidState(
                "restore has checkout collisions; preserve these paths before retrying",
            ));
        }
        let worktree = checked
            .state
            .view
            .worktrees()
            .iter()
            .find(|w| w.worktree_id() == self.worktree_id())
            .ok_or(NativeClientError::Missing("current Worktree state"))?;
        let operation = if allow_conflicted && worktree.status() == WorktreeStatus::Conflicted {
            if worktree.materialized_tree_id() != Some(checked.snapshot.root_tree_id()) {
                return Err(NativeClientError::StaleCapture);
            }
            checked.state.operation.id().clone()
        } else {
            // Crucially, never rescan here: only the bytes bound by the preview
            // may be captured. The Store checks this exact CaptureBase by CAS.
            let capture = self.client().prepare_capture_with_reason(
                &checked.base,
                checked.snapshot,
                MutationStamp::fresh()?,
                CaptureReason::BeforeOperation,
            )?;
            match capture {
                CapturePlan::Unchanged { .. } => checked.state.operation.id().clone(),
                CapturePlan::Change(change) => {
                    let operation = change
                        .batch()
                        .entity_records()
                        .iter()
                        .find_map(|e| match e {
                            EntityRecord::Operation(operation) => Some(operation.id().clone()),
                            _ => None,
                        })
                        .ok_or(NativeClientError::Missing("prepared capture Operation"))?;
                    self.client().accept(&change)?;
                    operation
                }
            }
        };
        let source = self.state()?;
        if source.operation.id() != &operation {
            return Err(NativeClientError::StaleCapture);
        }
        Ok(source)
    }
}
