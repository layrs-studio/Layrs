use super::{CliParseError, take_required_positional, take_string_option};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeCommand {
    List,
    Create {
        path: PathBuf,
        layer: String,
        step: String,
    },
    Open {
        path: PathBuf,
    },
    Resume,
}

pub(super) fn parse(mut args: Vec<String>) -> Result<WorktreeCommand, CliParseError> {
    if args.is_empty() {
        return Err(usage());
    }
    let command = args.remove(0);
    match command.as_str() {
        "list" if args.is_empty() => Ok(WorktreeCommand::List),
        "resume" if args.is_empty() => Ok(WorktreeCommand::Resume),
        "open" => Ok(WorktreeCommand::Open {
            path: take_required_positional(args, "PATH", "worktree open")?.into(),
        }),
        "create" => {
            let layer = take_string_option(&mut args, "--layer")?
                .filter(|s| !s.is_empty())
                .ok_or_else(usage)?;
            let step = take_string_option(&mut args, "--step")?
                .filter(|s| !s.is_empty())
                .ok_or_else(usage)?;
            let path = take_required_positional(args, "EMPTY_DIR", "worktree create")?.into();
            Ok(WorktreeCommand::Create { path, layer, step })
        }
        _ => Err(usage()),
    }
}

fn usage() -> CliParseError {
    CliParseError::new(
        "usage: layrs worktree list | open PATH | resume | create EMPTY_DIR --layer NAME_OR_ID --step EXACT_SELECTOR",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{CliCommand, parse_args};

    #[test]
    fn worktree_flags_require_an_existing_layer_and_exact_selector() {
        let parsed = parse_args([
            "worktree",
            "create",
            "../task",
            "--layer=topic",
            "--step",
            "@layer:topic",
        ])
        .unwrap();
        assert_eq!(
            parsed.command,
            CliCommand::Worktree(WorktreeCommand::Create {
                path: PathBuf::from("../task"),
                layer: "topic".into(),
                step: "@layer:topic".into(),
            })
        );
        for args in [
            vec!["worktree"],
            vec!["worktree", "list", "extra"],
            vec!["worktree", "open"],
            vec!["worktree", "resume", "--step", "@layer:topic"],
            vec!["worktree", "create", "../task", "--step", "@layer:topic"],
            vec![
                "worktree",
                "create",
                "../task",
                "--layer",
                "topic",
                "--step",
                "@layer:topic",
                "--step=other",
            ],
            vec![
                "worktree",
                "create",
                "../task",
                "--layer",
                "topic",
                "--step",
                "@layer:topic",
                "--unknown",
            ],
        ] {
            assert!(parse_args(args.clone()).is_err(), "accepted {args:?}");
        }
    }
}
