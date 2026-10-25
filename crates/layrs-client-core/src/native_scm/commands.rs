//! Serializable local application contract shared by interactive surfaces.
use super::{NativeClientError, NativeWorkspace, Result};
use layrs_core::scm::{MarkerId, OperationId, OperationKind, StepId};
use serde_json::{Value, json};
use std::path::Path;
#[path = "worktree_commands.rs"]
mod worktrees;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkspaceAction {
    LocalIdentity,
    ConfigureIdentity {
        identity: super::LocalIdentity,
    },
    Diagnose,
    ConflictBlockVariant {
        layer: String,
        conflict: layrs_core::scm::ConflictId,
        block: layrs_core::scm::ObjectId,
        term: layrs_core::scm::ObjectId,
    },
    PreviewBlockResolution {
        layer: String,
        conflict: layrs_core::scm::ConflictId,
        block: layrs_core::scm::ObjectId,
        choice: super::BlockResolutionChoice,
    },
    ApplyBlockResolution {
        layer: String,
        conflict: layrs_core::scm::ConflictId,
        block: layrs_core::scm::ObjectId,
        choice: super::BlockResolutionChoice,
        expected_operation: OperationId,
        expected_step: StepId,
    },
    ConflictVariant {
        layer: String,
        conflict: layrs_core::scm::ConflictId,
        term: layrs_core::scm::ObjectId,
    },
    CompareConflictVariants {
        layer: String,
        conflict: layrs_core::scm::ConflictId,
        before: layrs_core::scm::ObjectId,
        after: layrs_core::scm::ObjectId,
    },
    PreviewReplacement {
        layer: String,
        conflict: layrs_core::scm::ConflictId,
        text: String,
        executable: bool,
    },
    ApplyReplacement {
        layer: String,
        conflict: layrs_core::scm::ConflictId,
        text: String,
        executable: bool,
        expected_operation: OperationId,
        expected_step: StepId,
    },
    PreviewWeave {
        source: String,
        target: String,
    },
    ApplyWeave {
        source: String,
        target: String,
        expected_operation: OperationId,
        expected_step: StepId,
    },
    Conflicts {
        layer: String,
    },
    PreviewResolution {
        layer: String,
        choices: std::collections::BTreeMap<layrs_core::scm::ConflictId, layrs_core::scm::ObjectId>,
    },
    ApplyResolution {
        layer: String,
        choices: std::collections::BTreeMap<layrs_core::scm::ConflictId, layrs_core::scm::ObjectId>,
        expected_operation: OperationId,
        expected_step: StepId,
    },
    Initialize {
        name: String,
    },
    Status,
    ListWorktrees,
    CreateWorktree {
        path: String,
        layer: String,
        step: String,
    },
    OpenWorktree {
        path: String,
    },
    ResumeWorktreeCreation,
    Capture,
    CaptureAutomatic,
    StepHistory {
        selector: String,
        cursor: Option<super::StepHistoryCursor>,
        limit: usize,
        path: Option<String>,
        #[serde(default)]
        filter: super::StepHistoryFilter,
    },
    OperationHistory {
        cursor: Option<super::OperationHistoryCursor>,
        limit: usize,
        search: String,
        #[serde(default)]
        filter: super::OperationHistoryFilter,
    },
    CreateLayer {
        name: String,
    },
    SwitchLayer {
        layer: String,
    },
    Restore {
        step: StepId,
        path: Option<String>,
    },
    RestoreView {
        operation: OperationId,
    },
    ApplyRestore {
        step: StepId,
        path: Option<String>,
        expected: super::RestoreExpectation,
    },
    ApplyView {
        operation: OperationId,
        expected: super::RestoreExpectation,
    },
    Undo,
    Redo {
        operation: Option<OperationId>,
    },
    Recover,
    CreateMarker {
        name: String,
        notes: String,
    },
    HideMarker {
        id: MarkerId,
    },
    ShowMarker {
        id: MarkerId,
    },
    CreateRelease {
        version: String,
        notes: String,
    },
    PreviewRestore {
        step: StepId,
        path: Option<String>,
    },
    PreviewView {
        operation: OperationId,
    },
    PreviewRewrite {
        kind: String,
        source: String,
        mainline: Option<String>,
    },
    ApplyRewrite {
        kind: String,
        source: String,
        mainline: Option<String>,
        expected_operation: OperationId,
        expected_step: StepId,
    },
}

pub fn execute_workspace(root: impl AsRef<Path>, action: WorkspaceAction) -> Result<Value> {
    if let WorkspaceAction::OpenWorktree { path } = &action {
        return worktrees::describe(&NativeWorkspace::open(path)?);
    }
    if matches!(action, WorkspaceAction::ResumeWorktreeCreation) {
        return worktrees::describe(&NativeWorkspace::resume_worktree(&root)?);
    }
    if matches!(action, WorkspaceAction::Diagnose) {
        return super::diagnose_checkout(root);
    }
    let workspace = match &action {
        WorkspaceAction::Initialize { name } => NativeWorkspace::initialize(&root, name)?,
        _ => NativeWorkspace::open(&root)?,
    };
    match action {
        WorkspaceAction::ListWorktrees => {
            return Ok(
                json!({"worktrees":workspace.list_worktrees()?,"currentWorktreeId":workspace.worktree_id()}),
            );
        }
        WorkspaceAction::CreateWorktree { path, layer, step } => {
            let layer = worktrees::resolve_layer(&workspace, &layer)?;
            let step = workspace.resolve_step_name(&step)?.step_id;
            return worktrees::describe(&workspace.create_worktree(path, &layer, &step)?);
        }
        WorkspaceAction::ResumeWorktreeCreation => {
            unreachable!("handled before opening the source")
        }
        WorkspaceAction::OpenWorktree { .. } => unreachable!("handled before opening the source"),
        WorkspaceAction::LocalIdentity => {
            return Ok(json!({"identity": workspace.local_identity()?}));
        }
        WorkspaceAction::ConfigureIdentity { identity } => {
            return Ok(json!({"identity": workspace.configure_identity(identity)?}));
        }
        WorkspaceAction::ConflictVariant {
            layer,
            conflict,
            term,
        } => {
            return workspace.conflict_variant(&workspace.layer_named(&layer)?, &conflict, &term);
        }
        WorkspaceAction::ConflictBlockVariant {
            layer,
            conflict,
            block,
            term,
        } => {
            return workspace.conflict_block_variant(
                &workspace.layer_named(&layer)?,
                &conflict,
                &block,
                &term,
            );
        }
        WorkspaceAction::PreviewBlockResolution {
            layer,
            conflict,
            block,
            choice,
        } => {
            let plan = workspace.preview_block_resolution(&layer, conflict, block, choice)?;
            return Ok(
                json!({"expectedOperation":plan.expected_state().operation.id(),"expectedStep":plan.bundle().step.id(),"conflicts":plan.bundle().conflicts}),
            );
        }
        WorkspaceAction::ApplyBlockResolution {
            layer,
            conflict,
            block,
            choice,
            expected_operation,
            expected_step,
        } => {
            if workspace.state()?.operation.id() != &expected_operation {
                return Err(NativeClientError::StaleCapture);
            }
            let plan = workspace.preview_block_resolution(&layer, conflict, block, choice)?;
            if plan.expected_state().operation.id() != &expected_operation
                || plan.bundle().step.id() != &expected_step
            {
                return Err(NativeClientError::StaleCapture);
            }
            workspace.accept_conflict_resolution(&plan)?;
        }
        WorkspaceAction::CompareConflictVariants {
            layer,
            conflict,
            before,
            after,
        } => {
            return workspace.compare_conflict_variants(
                &workspace.layer_named(&layer)?,
                &conflict,
                &before,
                &after,
            );
        }
        WorkspaceAction::PreviewReplacement {
            layer,
            conflict,
            text,
            executable,
        } => {
            let plan = replacement(&workspace, &layer, conflict, text, executable)?;
            return Ok(
                json!({"expectedOperation":plan.expected_state().operation.id(),"expectedStep":plan.bundle().step.id(),"conflicts":plan.bundle().conflicts}),
            );
        }
        WorkspaceAction::ApplyReplacement {
            layer,
            conflict,
            text,
            executable,
            expected_operation,
            expected_step,
        } => {
            if workspace.state()?.operation.id() != &expected_operation {
                return Err(NativeClientError::StaleCapture);
            }
            let plan = replacement(&workspace, &layer, conflict, text, executable)?;
            if plan.expected_state().operation.id() != &expected_operation
                || plan.bundle().step.id() != &expected_step
            {
                return Err(NativeClientError::StaleCapture);
            }
            workspace.accept_conflict_resolution(&plan)?;
        }
        WorkspaceAction::PreviewWeave { source, target } => {
            let plan = workspace.preview_named_weave(&source, &target)?;
            return Ok(
                json!({"expectedOperation":plan.expected_state().operation.id(),"expectedStep":plan.plan().step().id(),"conflicts":plan.plan().conflicts().values().collect::<Vec<_>>(),"sourceHeads":plan.plan().source_heads(),"targetHeads":plan.plan().target_heads()}),
            );
        }
        WorkspaceAction::ApplyWeave {
            source,
            target,
            expected_operation,
            expected_step,
        } => {
            if workspace.state()?.operation.id() != &expected_operation {
                return Err(NativeClientError::StaleCapture);
            }
            let plan = workspace.preview_named_weave(&source, &target)?;
            if plan.expected_state().operation.id() != &expected_operation
                || plan.plan().step().id() != &expected_step
            {
                return Err(NativeClientError::StaleCapture);
            }
            workspace.accept_merge(&plan)?;
        }
        WorkspaceAction::Conflicts { layer } => {
            return Ok(
                json!({"conflicts":workspace.layer_conflicts(&workspace.layer_named(&layer)?)?}),
            );
        }
        WorkspaceAction::PreviewResolution { layer, choices } => {
            let plan = workspace.preview_conflict_resolution(
                &workspace.layer_named(&layer)?,
                &choices,
                Default::default(),
            )?;
            return Ok(
                json!({"expectedOperation":plan.expected_state().operation.id(),"expectedStep":plan.bundle().step.id(),"conflicts":plan.bundle().conflicts}),
            );
        }
        WorkspaceAction::ApplyResolution {
            layer,
            choices,
            expected_operation,
            expected_step,
        } => {
            if workspace.state()?.operation.id() != &expected_operation {
                return Err(NativeClientError::StaleCapture);
            }
            let plan = workspace.preview_conflict_resolution(
                &workspace.layer_named(&layer)?,
                &choices,
                Default::default(),
            )?;
            if plan.expected_state().operation.id() != &expected_operation
                || plan.bundle().step.id() != &expected_step
            {
                return Err(NativeClientError::StaleCapture);
            }
            workspace.accept_conflict_resolution(&plan)?;
        }
        WorkspaceAction::StepHistory {
            selector,
            cursor,
            limit,
            path,
            filter,
        } => {
            return serde_json::to_value(workspace.step_history_filtered(
                &selector,
                cursor.as_ref(),
                limit,
                path.as_deref(),
                &filter,
            )?)
            .map_err(|_| NativeClientError::InvalidState("history serialization failed"));
        }
        WorkspaceAction::OperationHistory {
            cursor,
            limit,
            search,
            filter,
        } => {
            return serde_json::to_value(workspace.operation_history_filtered(
                cursor.as_ref(),
                limit,
                &search,
                &filter,
            )?)
            .map_err(|_| NativeClientError::InvalidState("history serialization failed"));
        }
        WorkspaceAction::PreviewRewrite {
            kind,
            source,
            mainline,
        } => {
            let plan = prepare_rewrite(&workspace, &kind, &source, mainline.as_deref())?;
            return Ok(json!({"rewrite":plan.summary(),"conflicts":plan.conflicts()}));
        }
        WorkspaceAction::ApplyRewrite {
            kind,
            source,
            mainline,
            expected_operation,
            expected_step,
        } => {
            if workspace.state()?.operation.id() != &expected_operation {
                return Err(NativeClientError::StaleCapture);
            }
            let plan = prepare_rewrite(&workspace, &kind, &source, mainline.as_deref())?;
            if plan.summary().expected_operation_id != expected_operation
                || plan.summary().proposed_step.id() != &expected_step
            {
                return Err(NativeClientError::StaleCapture);
            }
            workspace.apply_rewrite(&plan)?;
        }
        WorkspaceAction::Initialize { .. } | WorkspaceAction::Status => {}
        WorkspaceAction::Diagnose => unreachable!("diagnosis runs before normal Store admission"),
        WorkspaceAction::Capture => {
            workspace.capture()?;
        }
        WorkspaceAction::CaptureAutomatic => {
            workspace.capture_with_reason(layrs_core::scm::CaptureReason::AutomaticIdle)?;
        }
        WorkspaceAction::CreateLayer { name } => {
            workspace.create_layer(&name)?;
        }
        WorkspaceAction::SwitchLayer { layer } => {
            workspace.switch_layer(&layer)?;
        }
        WorkspaceAction::Restore { step, path } => {
            workspace.restore_step(&step, path.as_deref())?;
        }
        WorkspaceAction::RestoreView { operation } => {
            workspace.restore_operation(&operation, OperationKind::Restore)?;
        }
        WorkspaceAction::ApplyRestore {
            step,
            path,
            expected,
        } => {
            workspace.restore_step_guarded(&step, path.as_deref(), &expected)?;
        }
        WorkspaceAction::ApplyView {
            operation,
            expected,
        } => {
            workspace.restore_operation_guarded(&operation, &expected)?;
        }
        WorkspaceAction::Undo => {
            workspace.undo()?;
        }
        WorkspaceAction::Redo { operation } => {
            workspace.redo(operation.as_ref())?;
        }
        WorkspaceAction::Recover => {
            workspace.recover()?;
        }
        WorkspaceAction::CreateMarker { name, notes } => {
            workspace.create_marker(&name, &notes)?;
        }
        WorkspaceAction::HideMarker { id } => {
            workspace.hide_marker(&id)?;
        }
        WorkspaceAction::ShowMarker { id } => {
            workspace.show_marker(&id)?;
        }
        WorkspaceAction::CreateRelease { version, notes } => {
            workspace.create_release(&version, &notes)?;
        }
        WorkspaceAction::PreviewRestore { step, path } => {
            return preview_restore(&workspace, &step, path.as_deref());
        }
        WorkspaceAction::PreviewView { operation } => {
            use layrs_store::scm::ScmReader;
            let state = workspace.state()?;
            let store = workspace.open_store()?;
            let reader = layrs_store_local::scm_v1::NativeScmReader::new(&store)?;
            let operation = reader
                .get_operation(&operation)?
                .ok_or(NativeClientError::Missing("Operation"))?;
            if operation.space_id() != state.space.id() {
                return Err(NativeClientError::InvalidState(
                    "Operation belongs to another Space",
                ));
            }
            let view = reader
                .get_repository_view(operation.resulting_view_id())?
                .ok_or(NativeClientError::Missing("View"))?;
            let mut preview =
                serde_json::to_value(workspace.preview_restore_operation(operation.id())?)
                    .map_err(|_| {
                        NativeClientError::InvalidState("restore preview serialization failed")
                    })?;
            preview["operationId"] = json!(operation.id());
            preview["layerHeads"] = json!(view.layer_heads());
            preview["markerIds"] = json!(view.marker_ids());
            preview["releaseIds"] = json!(view.release_ids());
            return Ok(preview);
        }
    }
    let state = workspace.state()?;
    let needs_repair = workspace.needs_repair()?;
    let changes = if needs_repair {
        None
    } else {
        Some(workspace.working_diff()?)
    };
    let operations = workspace.operations(200)?;
    let markers = workspace.markers()?;
    let releases = workspace.releases()?;
    if workspace.state()?.operation.id() != state.operation.id() {
        return Err(NativeClientError::StaleCapture);
    }
    Ok(
        json!({"state":state,"needsRepair":needs_repair,"changes":changes,"operations":operations,"markers":markers,"releases":releases}),
    )
}

fn replacement(
    workspace: &NativeWorkspace,
    layer: &str,
    conflict: layrs_core::scm::ConflictId,
    text: String,
    executable: bool,
) -> Result<super::NativeResolutionPreview> {
    if text.len() > 1024 * 1024 {
        return Err(NativeClientError::InvalidState(
            "manual text exceeds 1 MiB; use CLI exact file replacement",
        ));
    }
    let choices = std::collections::BTreeMap::from([(
        conflict,
        super::ConflictReplacement {
            bytes: text.into_bytes(),
            executable,
        },
    )]);
    workspace.preview_conflict_replacements(
        &workspace.layer_named(layer)?,
        &choices,
        Default::default(),
    )
}

fn prepare_rewrite(
    workspace: &NativeWorkspace,
    kind: &str,
    source: &str,
    mainline: Option<&str>,
) -> Result<super::NativeRewritePlan> {
    let source = workspace.resolve_step_name(source)?.step_id;
    let mainline = mainline
        .map(|id| workspace.resolve_step_name(id).map(|r| r.step_id))
        .transpose()?;
    match kind {
        "replay" => workspace.preview_replay(&source, mainline.as_ref()),
        "revert" => workspace.preview_revert(&source, mainline.as_ref()),
        "squash" if mainline.is_none() => workspace.preview_squash(&source),
        _ => Err(NativeClientError::InvalidState(
            "invalid rewrite action or parameters",
        )),
    }
}

fn preview_restore(
    workspace: &NativeWorkspace,
    step: &StepId,
    path: Option<&str>,
) -> Result<Value> {
    serde_json::to_value(workspace.preview_restore_step(step, path)?)
        .map_err(|_| NativeClientError::InvalidState("restore preview serialization failed"))
}
