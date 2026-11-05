use super::{CliParseError, take_bool_flag, take_required_positional, take_string_option};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsCommand {
    Create {
        path: PathBuf,
        layer: String,
        step: String,
        background: bool,
        ready_timeout_seconds: u32,
    },
    Serve {
        path: PathBuf,
    },
    Resume {
        worktree_id: String,
        background: bool,
        ready_timeout_seconds: u32,
    },
    Start {
        path: PathBuf,
        ready_timeout_seconds: u32,
    },
    Status {
        ticket: PathBuf,
    },
    Capture {
        ticket: PathBuf,
        writers_quiesced: bool,
    },
    Export {
        parent: PathBuf,
        ticket: PathBuf,
        writers_quiesced: bool,
    },
    Stop {
        ticket: PathBuf,
    },
}

pub(super) fn parse(mut args: Vec<String>) -> Result<VfsCommand, CliParseError> {
    if args.is_empty() {
        return Err(usage());
    }
    let action = args.remove(0);
    match action.as_str() {
        "create" => {
            let layer = required(&mut args, "--layer")?;
            let step = required(&mut args, "--step")?;
            let background = take_bool_flag(&mut args, "--background")?;
            let timeout = take_string_option(&mut args, "--ready-timeout-seconds")?;
            if !background && timeout.is_some() {
                return Err(CliParseError::new(
                    "--ready-timeout-seconds requires --background or vfs start",
                ));
            }
            let ready_timeout_seconds = readiness(timeout)?;
            let path = take_required_positional(args, "EMPTY_DIR", "vfs create")?.into();
            Ok(VfsCommand::Create {
                path,
                layer,
                step,
                background,
                ready_timeout_seconds,
            })
        }
        "serve" => Ok(VfsCommand::Serve {
            path: take_required_positional(args, "ROOT", "vfs serve")?.into(),
        }),
        "resume" => {
            let background = take_bool_flag(&mut args, "--background")?;
            let timeout = take_string_option(&mut args, "--ready-timeout-seconds")?;
            if !background && timeout.is_some() {
                return Err(CliParseError::new(
                    "--ready-timeout-seconds requires --background or vfs start",
                ));
            }
            let ready_timeout_seconds = readiness(timeout)?;
            let worktree_id = take_required_positional(args, "WORKTREE_ID", "vfs resume")?;
            layrs_core::scm::WorktreeId::parse(&worktree_id)
                .map_err(|_| CliParseError::new("vfs resume requires an exact Worktree ID"))?;
            Ok(VfsCommand::Resume {
                worktree_id,
                background,
                ready_timeout_seconds,
            })
        }
        "start" => {
            let ready_timeout_seconds =
                readiness(take_string_option(&mut args, "--ready-timeout-seconds")?)?;
            Ok(VfsCommand::Start {
                path: take_required_positional(args, "ROOT", "vfs start")?.into(),
                ready_timeout_seconds,
            })
        }
        "status" | "capture" | "export" | "stop" => {
            let ticket = PathBuf::from(required(&mut args, "--ticket")?);
            let writers_quiesced = if action == "capture" || action == "export" {
                let value = take_bool_flag(&mut args, "--writers-quiesced")?;
                if !value {
                    return Err(CliParseError::new(
                        "stop all Worktree writers, then pass --writers-quiesced; keep them stopped until the command finishes",
                    ));
                }
                value
            } else {
                false
            };
            if action == "export" {
                return Ok(VfsCommand::Export {
                    parent: take_required_positional(args, "PARENT", "vfs export")?.into(),
                    ticket,
                    writers_quiesced,
                });
            }
            if !args.is_empty() {
                return Err(usage());
            }
            Ok(match action.as_str() {
                "status" => VfsCommand::Status { ticket },
                "capture" => VfsCommand::Capture {
                    ticket,
                    writers_quiesced,
                },
                _ => VfsCommand::Stop { ticket },
            })
        }
        _ => Err(usage()),
    }
}

fn required(args: &mut Vec<String>, flag: &str) -> Result<String, CliParseError> {
    take_string_option(args, flag)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CliParseError::new(format!("vfs command requires {flag}")))
}

fn readiness(value: Option<String>) -> Result<u32, CliParseError> {
    match value {
        None => Ok(30),
        Some(value) => value
            .parse::<u32>()
            .ok()
            .filter(|seconds| (1..=300).contains(seconds))
            .ok_or_else(|| CliParseError::new("--ready-timeout-seconds must be between 1 and 300")),
    }
}

fn usage() -> CliParseError {
    CliParseError::new(
        "usage: layrs vfs create EMPTY_DIR --layer NAME_OR_ID --step EXACT_SELECTOR [--background] | resume WORKTREE_ID [--background] | serve ROOT | start ROOT | status/capture/stop --ticket FILE | export PARENT --ticket FILE; capture/export require --writers-quiesced",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{CliCommand, parse_args};

    #[test]
    fn resume_uses_exact_id_and_source_without_a_new_root_or_base() {
        let parsed = parse_args([
            "--space",
            "ordinary source",
            "vfs",
            "resume",
            "worktree:retained",
            "--background",
            "--ready-timeout-seconds",
            "60",
        ])
        .unwrap();
        assert_eq!(parsed.globals.space, Some(PathBuf::from("ordinary source")));
        assert!(
            matches!(parsed.command, CliCommand::Vfs(VfsCommand::Resume {
            worktree_id, background: true, ready_timeout_seconds: 60,
        }) if worktree_id == "worktree:retained")
        );
        assert!(matches!(
            parse_args(["vfs", "resume", "worktree:retained"])
                .unwrap()
                .command,
            CliCommand::Vfs(VfsCommand::Resume {
                background: false,
                ready_timeout_seconds: 30,
                ..
            })
        ));
        for args in [
            vec!["vfs", "resume"],
            vec!["vfs", "resume", "root"],
            vec!["vfs", "resume", "step:wrong"],
            vec!["vfs", "resume", "worktree:retained", "--step", "base"],
            vec!["vfs", "resume", "worktree:retained", "--layer", "main"],
            vec!["vfs", "resume", "worktree:retained", "another-root"],
            vec![
                "vfs",
                "resume",
                "worktree:retained",
                "--ready-timeout-seconds",
                "30",
            ],
            vec![
                "vfs",
                "resume",
                "worktree:retained",
                "--background",
                "--background",
            ],
            vec![
                "vfs",
                "resume",
                "worktree:retained",
                "--background",
                "--ready-timeout-seconds",
                "301",
            ],
        ] {
            assert!(parse_args(args.clone()).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn vfs_launch_requires_explicit_selection_and_bounds_background_readiness() {
        let parsed = parse_args([
            "vfs",
            "create",
            "target with spaces",
            "--layer",
            "topic",
            "--step",
            "@layer:topic",
            "--background",
            "--ready-timeout-seconds",
            "45",
        ])
        .unwrap();
        assert!(matches!(
            parsed.command,
            CliCommand::Vfs(VfsCommand::Create {
                background: true,
                ready_timeout_seconds: 45,
                ..
            })
        ));
        assert!(matches!(
            parse_args(["vfs", "start", "root"]).unwrap().command,
            CliCommand::Vfs(VfsCommand::Start {
                ready_timeout_seconds: 30,
                ..
            })
        ));
        for args in [
            vec!["vfs", "create", "root"],
            vec![
                "vfs",
                "create",
                "root",
                "--layer",
                "topic",
                "--step",
                "base",
                "--ready-timeout-seconds",
                "30",
            ],
            vec!["vfs", "start", "root", "--ready-timeout-seconds", "0"],
            vec!["vfs", "start", "root", "--ready-timeout-seconds", "301"],
            vec!["vfs", "start", "root", "--ready-timeout-seconds", "NaN"],
            vec![
                "vfs",
                "start",
                "root",
                "--ready-timeout-seconds",
                "1",
                "--ready-timeout-seconds",
                "2",
            ],
            vec!["vfs", "serve", "root", "--background"],
            vec!["vfs", "serve", "root", "--unknown"],
        ] {
            assert!(parse_args(args.clone()).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn vfs_controls_require_ticket_and_explicit_quiescence_only_for_capture_export() {
        assert!(matches!(
            parse_args([
                "vfs",
                "capture",
                "--ticket",
                "host.json",
                "--writers-quiesced"
            ])
            .unwrap()
            .command,
            CliCommand::Vfs(VfsCommand::Capture {
                writers_quiesced: true,
                ..
            })
        ));
        assert!(matches!(
            parse_args([
                "vfs",
                "export",
                "parent",
                "--ticket",
                "host.json",
                "--writers-quiesced"
            ])
            .unwrap()
            .command,
            CliCommand::Vfs(VfsCommand::Export {
                writers_quiesced: true,
                ..
            })
        ));
        for args in [
            vec!["vfs", "status"],
            vec!["vfs", "stop", "--ticket"],
            vec!["vfs", "capture", "--ticket", "host.json"],
            vec!["vfs", "export", "parent", "--ticket", "host.json"],
            vec![
                "vfs",
                "capture",
                "--ticket",
                "host.json",
                "--writers-quiesced=false",
            ],
            vec![
                "vfs",
                "capture",
                "--ticket",
                "host.json",
                "--writers-quiesced",
                "--writers-quiesced",
            ],
            vec![
                "vfs",
                "status",
                "--ticket",
                "host.json",
                "--ticket",
                "other.json",
            ],
            vec![
                "vfs",
                "status",
                "--ticket",
                "host.json",
                "--writers-quiesced",
            ],
            vec!["vfs", "restore", "--ticket", "host.json"],
        ] {
            assert!(parse_args(args.clone()).is_err(), "accepted {args:?}");
        }
    }
}
