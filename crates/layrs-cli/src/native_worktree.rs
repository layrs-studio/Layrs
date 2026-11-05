use crate::{args::WorktreeCommand, error::CliError, render::Rendered};
use layrs_client_core::native_scm::{NativeWorkspace, WorkspaceAction, execute_workspace};
use std::path::Path;

pub(super) fn run(root: &Path, command: &WorktreeCommand) -> Result<Rendered, CliError> {
    let path_text = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| CliError::usage("Worktree path must be UTF-8"))
    };
    let action = match command {
        WorktreeCommand::List => WorkspaceAction::ListWorktrees,
        WorktreeCommand::Create { path, layer, step } => WorkspaceAction::CreateWorktree {
            path: path_text(path)?,
            layer: layer.clone(),
            step: step.clone(),
        },
        WorktreeCommand::Open { path } => WorkspaceAction::OpenWorktree {
            path: path_text(path)?,
        },
        WorktreeCommand::Resume => WorkspaceAction::ResumeWorktreeCreation,
    };
    let source = if matches!(command, WorktreeCommand::Resume) {
        // An interrupted creation may not yet be registered, so discovery
        // cannot open it. The static resume API validates the durable intent.
        let absolute = std::path::absolute(root).map_err(error)?;
        let mut selected = absolute.clone();
        'ancestors: for directory in absolute.ancestors() {
            for marker in [".layrs", ".layrs.reservation"] {
                match std::fs::symlink_metadata(directory.join(marker)) {
                    Ok(_) => {
                        selected = directory.to_path_buf();
                        break 'ancestors;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(error(e)),
                }
            }
        }
        selected
    } else if matches!(command, WorktreeCommand::Open { .. }) {
        root.to_path_buf()
    } else {
        NativeWorkspace::discover(root)
            .and_then(|workspace| workspace.checkout_root())
            .map_err(error)?
    };
    let data = execute_workspace(source, action).map_err(error)?;
    let human = if matches!(command, WorktreeCommand::List) {
        data["worktrees"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|worktree| {
                format!(
                    "{}  {}",
                    worktree["worktreeId"].as_str().unwrap_or(""),
                    worktree["checkoutRoot"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        format!(
            "Worktree {} at {}",
            data["worktreeId"].as_str().unwrap_or(""),
            data["path"].as_str().unwrap_or("")
        )
    };
    Ok(Rendered { human, data })
}

fn error(error: impl std::fmt::Display) -> CliError {
    CliError::runtime(error.to_string())
}
