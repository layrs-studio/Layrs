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
