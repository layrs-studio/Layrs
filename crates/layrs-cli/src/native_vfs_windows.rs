use super::{error, process};
use crate::{
    args::{Cli, VfsCommand},
    error::CliError,
    render::{self, Rendered},
};
use layrs_client_core::native_scm::{
    VfsControlRequest, VfsHostClient, VfsHostCommand, VfsHostResponse, VfsHostTicket,
    VfsLaunchAction, execute_vfs_control, prepare_vfs_host,
};
use layrs_core::scm::WorktreeId;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

fn source_root(cli: &Cli) -> Result<PathBuf, CliError> {
    cli.globals
        .space
        .clone()
        .map(Ok)
        .unwrap_or_else(std::env::current_dir)
        .map_err(error)
}
fn path_text(path: &Path) -> Result<String, CliError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| CliError::usage("VFS paths must be UTF-8"))
}
fn launch(command: &VfsCommand) -> Result<VfsLaunchAction, CliError> {
    match command {
        VfsCommand::Create {
            path, layer, step, ..
        } => Ok(VfsLaunchAction::Create {
            path: path_text(path)?,
            layer: layer.clone(),
            step: step.clone(),
        }),
        VfsCommand::Serve { path } => Ok(VfsLaunchAction::Serve {
            path: path_text(path)?,
        }),
        VfsCommand::Resume { worktree_id, .. } => Ok(VfsLaunchAction::Resume {
            worktree_id: WorktreeId::parse(worktree_id).map_err(error)?,
        }),
        _ => Err(CliError::usage(
            "foreground VFS host requires create, resume or serve",
        )),
    }
}

pub(super) fn foreground(
    cli: &Cli,
    command: &VfsCommand,
    ready: &mut bool,
) -> Result<(), CliError> {
    let action = launch(command)?;
    let host = prepare_vfs_host(source_root(cli)?, action).map_err(error)?;
    let path = host.checkout_root().to_path_buf();
    let data =
        json!({"status":"ready","ticket":host.ticket(),"pid":std::process::id(),"path":path});
    {
        let mut stdout = std::io::stdout().lock();
        if cli.globals.json {
            writeln!(stdout, "{}", render::ok_json(data)).map_err(error)?;
        } else {
            writeln!(
                stdout,
                "VFS host ready at {} (PID {}). Keep this process running.\nTicket: {}",
                path.display(),
                std::process::id(),
                serde_json::to_string(host.ticket()).map_err(error)?
            )
            .map_err(error)?;
        }
        stdout.flush().map_err(error)?;
    }
    *ready = true;
    host.serve().map_err(error)
}

pub(super) fn run(cli: &Cli, command: &VfsCommand) -> Result<Rendered, CliError> {
    match command {
        VfsCommand::Create {
            background: true,
            ready_timeout_seconds,
            ..
        }
        | VfsCommand::Resume {
            background: true,
            ready_timeout_seconds,
            ..
        }
        | VfsCommand::Start {
            ready_timeout_seconds,
            ..
        } => background(cli, command, *ready_timeout_seconds),
        VfsCommand::Status { ticket } => control(ticket, VfsHostCommand::Status {}),
        VfsCommand::Capture {
            ticket,
            writers_quiesced,
        } => control(
            ticket,
            VfsHostCommand::Capture {
                writers_quiesced: *writers_quiesced,
            },
        ),
        VfsCommand::Export {
            parent,
            ticket,
            writers_quiesced,
        } => control(
            ticket,
            VfsHostCommand::Export {
                parent: std::path::absolute(parent).map_err(error)?,
                writers_quiesced: *writers_quiesced,
            },
        ),
        VfsCommand::Stop { ticket } => control(ticket, VfsHostCommand::Stop {}),
        _ => Err(CliError::usage(
            "foreground VFS hosts must run through the layrs executable",
        )),
    }
}

fn read_ticket(path: &Path) -> Result<VfsHostTicket, CliError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(error)?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > 64 * 1024 {
        return Err(CliError::usage("VFS ticket exceeds 64 KiB"));
    }
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| CliError::usage("invalid VFS ticket JSON"))?;
    let ticket = if document.get("ok").is_some() {
        if document["ok"] != true {
            return Err(CliError::usage("VFS ticket file contains a failed launch"));
        }
        document
            .get("data")
            .and_then(|data| data.get("ticket"))
            .ok_or_else(|| CliError::usage("launch response has no VFS ticket"))?
    } else {
        &document
    };
    serde_json::from_value(ticket.clone()).map_err(|_| CliError::usage("invalid VFS ticket fields"))
}

fn control(path: &Path, command: VfsHostCommand) -> Result<Rendered, CliError> {
    let data = execute_vfs_control(VfsControlRequest {
        ticket: read_ticket(path)?,
        command,
    })
    .map_err(error)?;
    Ok(Rendered {
        human: serde_json::to_string_pretty(&data).map_err(error)?,
        data,
    })
}

fn background(cli: &Cli, command: &VfsCommand, timeout_seconds: u32) -> Result<Rendered, CliError> {
    if !(1..=300).contains(&timeout_seconds) {
        return Err(CliError::usage("readiness timeout must be 1..300 seconds"));
    }
    let mut child = Command::new(std::env::current_exe().map_err(error)?);
    child.arg("--json");
    if let Some(source) = &cli.globals.space {
        child.arg("--space").arg(source);
    }
    let (expected_root, expected_id) = match command {
        VfsCommand::Create {
            path, layer, step, ..
        } => {
            child
                .args(["vfs", "create"])
                .arg(path)
                .arg("--layer")
                .arg(layer)
                .arg("--step")
                .arg(step);
            (Some(std::path::absolute(path).map_err(error)?), None)
        }
        VfsCommand::Start { path, .. } => {
            child.args(["vfs", "serve"]).arg(path);
            (Some(std::path::absolute(path).map_err(error)?), None)
        }
        VfsCommand::Resume { worktree_id, .. } => {
            let id = WorktreeId::parse(worktree_id).map_err(error)?;
            child.args(["vfs", "resume"]).arg(worktree_id);
            (None, Some(id))
        }
        _ => {
            return Err(CliError::usage(
                "background VFS launch requires create/resume --background or start",
            ));
        }
    };
    use std::os::windows::process::CommandExt;
    child.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: explicit user-process host.
    child.stdin(Stdio::null());
    let data = process::start(
        &mut child,
        Duration::from_secs(u64::from(timeout_seconds)),
        move |data, remaining| {
            let ticket: VfsHostTicket = serde_json::from_value(data["ticket"].clone())
                .map_err(|_| "host returned an invalid VFS ticket".to_owned())?;
            let status = VfsHostClient::open(&ticket)
                .map_err(|e| e.to_string())?
                .request_with_timeout(
                    VfsHostCommand::Status {},
                    remaining.min(Duration::from_secs(3)),
                )
                .map_err(|e| e.to_string())?;
            match status {
                VfsHostResponse::Ok { value }
                    if value["connected"] == true
                        && value["worktreeId"] == json!(ticket.worktree_id)
                        && value["checkoutRoot"] == data["path"]
                        && expected_root
                            .as_ref()
                            .is_none_or(|root| value["checkoutRoot"] == json!(root))
                        && expected_id
                            .as_ref()
                            .is_none_or(|id| &ticket.worktree_id == id) =>
                {
                    Ok(())
                }
                VfsHostResponse::Error { code, message } => Err(format!("{code}: {message}")),
                _ => Err("VFS host did not confirm its connected Worktree identity".into()),
            }
        },
    )?;
    Ok(Rendered {
        human: format!(
            "VFS host ready at {} (PID {}).\nTicket: {}",
            expected_display(&data),
            data["pid"],
            serde_json::to_string(&data["ticket"]).map_err(error)?
        ),
        data,
    })
}

fn expected_display(data: &Value) -> &str {
    data["path"].as_str().unwrap_or("unknown")
}
