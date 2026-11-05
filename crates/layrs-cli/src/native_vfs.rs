//! User-process VFS lifecycle. Ordinary workspace commands never fall back to a scan.
use crate::{
    args::{Cli, VfsCommand},
    error::{CliError, ExitCode},
    render::{self, Rendered},
};
use std::path::Path;

#[cfg(any(windows, test))]
#[path = "native_vfs_process.rs"]
mod process;
#[cfg(windows)]
#[path = "native_vfs_windows.rs"]
mod windows;

/// Makes this process's standard handles non-inheritable before spawning a
/// long-lived child (see `process::keep_standard_handles_private`).
pub(crate) fn keep_standard_handles_private() {
    #[cfg(any(windows, test))]
    process::keep_standard_handles_private();
}

pub fn run_foreground(cli: &Cli) -> Option<ExitCode> {
    let crate::args::CliCommand::Vfs(command) = &cli.command else {
        return None;
    };
    if !matches!(
        command,
        VfsCommand::Create {
            background: false,
            ..
        } | VfsCommand::Resume {
            background: false,
            ..
        } | VfsCommand::Serve { .. }
    ) {
        return None;
    }
    #[cfg(windows)]
    let mut ready = false;
    #[cfg(not(windows))]
    let ready = false;
    #[cfg(windows)]
    let result = windows::foreground(cli, command, &mut ready);
    #[cfg(not(windows))]
    let result: Result<(), CliError> = Err(unsupported());
    Some(match result {
        Ok(()) => ExitCode::Success,
        Err(error) => {
            if ready {
                eprintln!(
                    "VFS host exited with an error: {error}. The root and recoverable work are retained."
                );
            } else if cli.globals.json {
                println!("{}", render::error_json(&error.message));
            } else {
                eprintln!("{error}");
            }
            error.exit_code
        }
    })
}

pub fn run(cli: &Cli, command: &VfsCommand) -> Result<Rendered, CliError> {
    #[cfg(windows)]
    {
        windows::run(cli, command)
    }
    #[cfg(not(windows))]
    {
        let _ = (cli, command);
        Err(unsupported())
    }
}

/// Starts a background VFS host that creates `path` on `layer` at `step` and
/// returns its launch data (path, pid, ticket). Used by `layrs room new`.
pub(crate) fn create_room_host(
    cli: &Cli,
    path: &Path,
    layer: &str,
    step: &str,
) -> Result<serde_json::Value, CliError> {
    #[cfg(windows)]
    {
        windows::run(
            cli,
            &VfsCommand::Create {
                path: path.to_path_buf(),
                layer: layer.to_owned(),
                step: step.to_owned(),
                background: true,
                ready_timeout_seconds: 300,
            },
        )
        .map(|rendered| rendered.data)
    }
    #[cfg(not(windows))]
    {
        let _ = (cli, path, layer, step);
        Err(unsupported())
    }
}

/// Starts (in the background) the VFS host of an existing virtual Room root
/// and returns its launch data. Used when a Room's host is not running.
pub(crate) fn start_room_host(cli: &Cli, path: &Path) -> Result<serde_json::Value, CliError> {
    #[cfg(windows)]
    {
        windows::run(
            cli,
            &VfsCommand::Start {
                path: path.to_path_buf(),
                ready_timeout_seconds: 300,
            },
        )
        .map(|rendered| rendered.data)
    }
    #[cfg(not(windows))]
    {
        let _ = (cli, path);
        Err(unsupported())
    }
}

/// Inspect only the nearest fixed control entry. The shared classifier proves
/// VFS identity without connecting a provider or reading ordinary file contents.
pub(crate) fn refuse_ordinary_workspace(start: &Path) -> Result<(), CliError> {
    #[cfg(windows)]
    {
        let start = std::path::absolute(start).map_err(error)?;
        for root in start.ancestors() {
            match std::fs::symlink_metadata(root.join(".layrs")) {
                Ok(_) => {
                    if layrs_client_core::native_scm::inspect_vfs_checkout(root)
                        .map_err(error)?
                        .is_some()
                    {
                        return Err(CliError::usage(
                            "This is a VFS Worktree. Use `layrs vfs status/capture/export/stop --ticket FILE`; capture/export require --writers-quiesced. Ordinary workspace commands, including Layer switching and restore, are unsupported here.",
                        ));
                    }
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(error(e)),
            }
        }
    }
    #[cfg(not(windows))]
    let _ = start;
    Ok(())
}

#[cfg(windows)]
fn error(error: impl std::fmt::Display) -> CliError {
    CliError::runtime(error.to_string())
}
#[cfg(not(windows))]
fn unsupported() -> CliError {
    CliError::runtime(
        "VFS Worktrees require Windows and a supported local NTFS volume; no provider was started.",
    )
}
