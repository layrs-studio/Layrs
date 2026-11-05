//! `layrs room` (ADR 0025). `loom` is an alias; `worktree` keeps the historical
//! subcommands. Room-only subcommands: new, list, path.
use super::{CliCommand, CliParseError, take_bool_flag, take_required_positional, take_string_option, worktree};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomCommand {
    New {
        name: String,
        physical: bool,
        dir: Option<PathBuf>,
    },
    List,
    Path {
        name: String,
    },
    /// NAME may be omitted when run from inside the Room.
    Status {
        name: Option<String>,
    },
    Step {
        name: Option<String>,
    },
    Merge {
        name: Option<String>,
        into: Option<String>,
        preview: bool,
    },
    Drop {
        name: Option<String>,
        force: bool,
    },
}

fn optional_name(args: Vec<String>, command: &str) -> Result<Option<String>, CliParseError> {
    match args.as_slice() {
        [] => Ok(None),
        [name] if !name.starts_with("--") => Ok(Some(name.clone())),
        _ => Err(CliParseError::new(format!("usage: layrs room {command} [NAME]"))),
    }
}

pub(super) fn parse(invoked_as: &str, mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    if invoked_as == "worktree" {
        return Ok(CliCommand::Worktree(worktree::parse(args)?));
    }
    match args.first().map(String::as_str) {
        Some("new") => {
            args.remove(0);
            let physical = take_bool_flag(&mut args, "--physical")?;
            let dir = take_string_option(&mut args, "--dir")?
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from);
            let name = take_required_positional(args, "NAME", "room new")?;
            Ok(CliCommand::Room(RoomCommand::New {
                name,
                physical,
                dir,
            }))
        }
        Some("list") if args.len() == 1 => Ok(CliCommand::Room(RoomCommand::List)),
        Some("path") => {
            args.remove(0);
            let name = take_required_positional(args, "NAME", "room path")?;
            Ok(CliCommand::Room(RoomCommand::Path { name }))
        }
        Some("status") => {
            args.remove(0);
            Ok(CliCommand::Room(RoomCommand::Status {
                name: optional_name(args, "status")?,
            }))
        }
        Some("step") => {
            args.remove(0);
            Ok(CliCommand::Room(RoomCommand::Step {
                name: optional_name(args, "step")?,
            }))
        }
        Some("merge") => {
            args.remove(0);
            let preview = take_bool_flag(&mut args, "--preview")?;
            let into = take_string_option(&mut args, "--into")?.filter(|s| !s.is_empty());
            Ok(CliCommand::Room(RoomCommand::Merge {
                name: optional_name(args, "merge")?,
                into,
                preview,
            }))
        }
        Some("drop") => {
            args.remove(0);
            let force = take_bool_flag(&mut args, "--force")?;
            Ok(CliCommand::Room(RoomCommand::Drop {
                name: optional_name(args, "drop")?,
                force,
            }))
        }
        Some("create" | "open" | "resume") => Ok(CliCommand::Worktree(worktree::parse(args)?)),
        _ => Err(CliParseError::new(
            "usage: layrs room new NAME [--physical] [--dir PATH] | list | path NAME | status [NAME] | step [NAME] | merge [NAME] [--into LAYER] [--preview] | drop [NAME] [--force]  (alias: loom)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{WorktreeCommand, parse_args};

    #[test]
    fn room_and_loom_parse_room_commands_and_keep_worktree_subcommands() {
        for name in ["room", "loom"] {
            assert_eq!(
                parse_args([name, "new", "agent-a"]).unwrap().command,
                CliCommand::Room(RoomCommand::New {
                    name: "agent-a".into(),
                    physical: false,
                    dir: None
                })
            );
            assert_eq!(
                parse_args([name, "new", "agent-b", "--physical", "--dir", "../b"])
                    .unwrap()
                    .command,
                CliCommand::Room(RoomCommand::New {
                    name: "agent-b".into(),
                    physical: true,
                    dir: Some(PathBuf::from("../b"))
                })
            );
            assert_eq!(
                parse_args([name, "list"]).unwrap().command,
                CliCommand::Room(RoomCommand::List)
            );
            assert_eq!(
                parse_args([name, "path", "agent-a"]).unwrap().command,
                CliCommand::Room(RoomCommand::Path {
                    name: "agent-a".into()
                })
            );
            assert!(matches!(
                parse_args([name, "create", "../x", "--layer", "l", "--step", "@layer:l"])
                    .unwrap()
                    .command,
                CliCommand::Worktree(WorktreeCommand::Create { .. })
            ));
            assert_eq!(
                parse_args([name, "merge", "--into", "main"]).unwrap().command,
                CliCommand::Room(RoomCommand::Merge {
                    name: None,
                    into: Some("main".into()),
                    preview: false
                })
            );
            assert_eq!(
                parse_args([name, "drop", "agent-a", "--force"]).unwrap().command,
                CliCommand::Room(RoomCommand::Drop {
                    name: Some("agent-a".into()),
                    force: true
                })
            );
            assert_eq!(
                parse_args([name, "step"]).unwrap().command,
                CliCommand::Room(RoomCommand::Step { name: None })
            );
            for bad in [vec![name], vec![name, "new"], vec![name, "list", "x"], vec![name, "drop", "a", "b"]] {
                assert!(parse_args(bad.clone()).is_err(), "accepted {bad:?}");
            }
        }
        assert_eq!(
            parse_args(["worktree", "list"]).unwrap().command,
            CliCommand::Worktree(WorktreeCommand::List)
        );
    }
}
