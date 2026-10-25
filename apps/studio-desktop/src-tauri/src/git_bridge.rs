//! Thin Desktop adapter over the same bounded bridge as CLI. Reviews are native
//! session objects; a renderer cannot assert that a different target was reviewed.
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_git_bridge::{
    export_workspace_reviewed, prepare_git_scan_checkpoint, resume_export_workspace_reviewed,
    resume_git_scan_checkpoint, review_export_destination, ExpectedBridgeState, ExportDestination,
    ExportMode, GitBridgeLimits, ImportPlan, ImportSelection,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GitBridgeAction {
    PrepareImport {
        source: String,
        checkpoint: String,
        references: Vec<String>,
    },
    ReviewImport {
        checkpoint: String,
        digest: String,
    },
    ApplyImport {
        review_id: String,
        checkpoint: String,
        digest: String,
    },
    ReviewExport {
        destination: String,
        mode: ExportMode,
    },
    Export {
        review_id: String,
        destination: String,
        mode: ExportMode,
    },
    ResumeExport {
        review_id: String,
        destination: String,
        mode: ExportMode,
    },
}

#[derive(Clone)]
enum Intent {
    Import {
        checkpoint: PathBuf,
        digest: String,
        import_id: String,
    },
    Export {
        destination: ExportDestination,
        mode: ExportMode,
    },
}
#[derive(Clone)]
struct Review {
    expected: ExpectedBridgeState,
    intent: Intent,
}
#[derive(Default)]
struct Reviews {
    serial: u64,
    entries: BTreeMap<String, Review>,
}
#[derive(Clone, Default)]
pub struct GitBridgeState(Arc<Mutex<Reviews>>);

impl GitBridgeState {
    fn remember(&self, review: Review) -> Result<String, String> {
        let mut records = self
            .0
            .lock()
            .map_err(|_| "git_bridge_error: review registry unavailable")?;
        records.serial = records
            .serial
            .checked_add(1)
            .ok_or("git_bridge_error: review registry exhausted")?;
        let id = format!("git-review:{:016x}", records.serial);
        if records.entries.len() >= 16 {
            if let Some(oldest) = records.entries.keys().next().cloned() {
                records.entries.remove(&oldest);
            }
        }
        records.entries.insert(id.clone(), review);
        Ok(id)
    }
    fn take(&self, id: &str) -> Result<Review, String> {
        self.0
            .lock()
            .map_err(|_| "git_bridge_error: review registry unavailable")?
            .entries
            .remove(id)
            .ok_or_else(|| {
                "git_review_expired: relisez le checkpoint ou la cible avant de confirmer".into()
            })
    }
}

fn error(error: impl std::fmt::Display) -> String {
    format!("git_bridge_error: {error}")
}
fn absolute(path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("git_bridge_error: explicit path required".into());
    }
    std::path::absolute(path).map_err(error)
}
fn open(root: &Path) -> Result<NativeWorkspace, String> {
    let workspace = NativeWorkspace::open(root).map_err(error)?;
    if workspace.needs_repair().map_err(error)? {
        return Err("git_bridge_error: recover the native Worktree before Git operations".into());
    }
    Ok(workspace)
}
fn target(expected: &ExpectedBridgeState) -> Value {
    let state = expected.state();
    json!({
        "path": state.worktree.checkout_root(), "storeId": state.worktree.store_id(),
        "spaceId": state.space.id(), "worktreeId": state.worktree.id(),
        "operationId": state.operation.id(), "viewId": state.view.id(),
    })
}
fn report(plan: &ImportPlan) -> Value {
    let report = plan.report();
    // Do not send raw commit/tag provenance across IPC merely to render a review.
    json!({"importId":report.import_id, "objectFormat":report.object_format,
        "commits":report.commits.len(), "sourceBytes":report.source_bytes,
        "warnings":report.warnings, "references":report.references.iter().map(|reference|
            json!({"gitRef":reference.git_ref,"gitId":reference.git_id,"stepId":reference.step_id})
        ).collect::<Vec<_>>()})
}

pub fn execute(
    state: &GitBridgeState,
    root: &Path,
    request: GitBridgeAction,
) -> Result<Value, String> {
    let workspace = open(root)?;
    let limits = GitBridgeLimits::default();
    match request {
        GitBridgeAction::PrepareImport {
            source,
            checkpoint,
            references,
        } => {
            let selection = if references.is_empty() {
                ImportSelection::AllLocalReferences
            } else {
                ImportSelection::References(references)
            };
            let checkpoint = prepare_git_scan_checkpoint(
                absolute(&source)?,
                absolute(&checkpoint)?,
                &selection,
                limits,
                &mut |_| true,
            )
            .map_err(error)?;
            Ok(json!({"checkpoint":checkpoint}))
        }
        GitBridgeAction::ReviewImport { checkpoint, digest } => {
            let checkpoint = absolute(&checkpoint)?;
            let expected = ExpectedBridgeState::capture(&workspace).map_err(error)?;
            let plan = resume_git_scan_checkpoint(&checkpoint, &digest, limits, &mut |_| true)
                .map_err(error)?;
            expected.validate(&workspace).map_err(error)?;
            let result = json!({"target":target(&expected),"checkpoint":checkpoint,"digest":digest,"report":report(&plan)});
            let review_id = state.remember(Review {
                expected,
                intent: Intent::Import {
                    checkpoint,
                    digest,
                    import_id: plan.report().import_id.clone(),
                },
            })?;
            Ok(json!({"reviewId":review_id,"review":result}))
        }
        GitBridgeAction::ApplyImport {
            review_id,
            checkpoint,
            digest,
        } => {
            let review = state.take(&review_id)?;
            let Intent::Import {
                checkpoint: reviewed,
                digest: pin,
                import_id,
            } = &review.intent
            else {
                return Err("git_review_mismatch: review is not an import".into());
            };
            if absolute(&checkpoint)? != *reviewed || digest != *pin {
                return Err("git_review_mismatch: checkpoint path or digest changed".into());
            }
            review.expected.validate(&workspace).map_err(error)?;
            let plan =
                resume_git_scan_checkpoint(reviewed, pin, limits, &mut |_| true).map_err(error)?;
            if plan.report().import_id != *import_id {
                return Err("git_review_mismatch: import plan changed".into());
            }
            let outcome = plan
                .install_expected(&workspace, &review.expected)
                .map_err(error)?;
            Ok(json!({"import":outcome,"report":report(&plan),"target":target(&review.expected)}))
        }
        GitBridgeAction::ReviewExport { destination, mode } => {
            let mode = mode.validated(limits).map_err(error)?;
            let destination =
                review_export_destination(absolute(&destination)?, limits).map_err(error)?;
            let expected = ExpectedBridgeState::capture(&workspace).map_err(error)?;
            let result = json!({"target":target(&expected),"destination":destination.path(),"mode":mode,"contentPreview":"unavailable"});
            let review_id = state.remember(Review {
                expected,
                intent: Intent::Export { destination, mode },
            })?;
            Ok(json!({"reviewId":review_id,"review":result}))
        }
        GitBridgeAction::Export {
            review_id,
            destination,
            mode,
        } => export(state, &workspace, &review_id, &destination, mode, false),
        GitBridgeAction::ResumeExport {
            review_id,
            destination,
            mode,
        } => export(state, &workspace, &review_id, &destination, mode, true),
    }
}

fn export(
    state: &GitBridgeState,
    workspace: &NativeWorkspace,
    id: &str,
    destination: &str,
    mode: ExportMode,
    resume: bool,
) -> Result<Value, String> {
    let mode = mode.validated(GitBridgeLimits::default()).map_err(error)?;
    let review = state.take(id)?;
    let Intent::Export {
        destination: reviewed,
        mode: reviewed_mode,
    } = &review.intent
    else {
        return Err("git_review_mismatch: review is not an export".into());
    };
    if absolute(destination)? != reviewed.path() || mode != *reviewed_mode {
        return Err("git_review_mismatch: export destination or references changed".into());
    }
    let result = if resume {
        resume_export_workspace_reviewed(
            workspace,
            &review.expected,
            reviewed,
            mode,
            GitBridgeLimits::default(),
            &mut |_| true,
        )
    } else {
        export_workspace_reviewed(
            workspace,
            &review.expected,
            reviewed,
            mode,
            GitBridgeLimits::default(),
            &mut |_| true,
        )
    }
    .map_err(error)?;
    Ok(json!({"export":result,"target":target(&review.expected),"destination":reviewed.path()}))
}

#[tauri::command]
pub async fn native_git_bridge(
    root: String,
    request: GitBridgeAction,
    state: tauri::State<'_, GitBridgeState>,
) -> Result<Value, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || execute(&state, Path::new(&root), request))
        .await
        .map_err(error)?
}

#[cfg(test)]
mod tests;
