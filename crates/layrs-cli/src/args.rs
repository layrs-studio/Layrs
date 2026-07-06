use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub globals: GlobalFlags,
    pub command: CliCommand,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalFlags {
    pub space: Option<PathBuf>,
    pub json: bool,
    pub no_color: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Help,
    Init {
        name: String,
        path: Option<PathBuf>,
    },
    Step,
    Diff {
        step_id: Option<String>,
        stat: bool,
        name_only: bool,
        window: Option<Window>,
        wrap: Option<bool>,
    },
    Timeline {
        limit: Option<u32>,
    },
    Publish {
        workspace: Option<String>,
    },
    Sync {
        workspace: Option<String>,
    },
    Receive,
    Compact,
    Status,
    Login {
        endpoint: Option<String>,
    },
    Whoami,
    Logout,
    Spaces,
    Layers,
    LayerUse {
        name_or_id: String,
    },
    LayerCreate {
        name: String,
    },
    LayerDelete {
        name_or_id: String,
        yes: bool,
    },
    LayerDisconnect {
        name_or_id: String,
        yes: bool,
    },
    LayerClearSteps {
        name_or_id: String,
        yes: bool,
    },
    Weave {
        source: String,
        target: String,
        preview: bool,
    },
    WeaveParent {
        preview: bool,
    },
    WeaveStatus,
    WeaveConflicts,
    WeaveResolve {
        path: String,
        resolution: String,
        file: Option<PathBuf>,
    },
    WeaveContinue,
    WeaveAbort,
    ConflictList,
    ConflictStatus,
    ConflictResolve {
        method: Option<ConflictResolveMethod>,
        path: Option<String>,
        block: Option<String>,
    },
    ConflictContinue,
    ConflictAbort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolveMethod {
    Existing,
    Incoming,
    Both,
    Manual,
}

impl ConflictResolveMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Incoming => "incoming",
            Self::Both => "both",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub start: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliParseError {
    message: String,
}

impl CliParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CliParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for CliParseError {}

pub fn parse_args<I, S>(args: I) -> Result<Cli, CliParseError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let globals = take_global_flags(&mut args)?;

    if args.is_empty() || take_help_flag(&mut args) {
        reject_remaining_help_context(&args)?;
        return Ok(Cli {
            globals,
            command: CliCommand::Help,
        });
    }

    let command_name = args.remove(0);
    let command = match command_name.as_str() {
        "init" => parse_init(args)?,
        "step" => parse_no_extra(args, CliCommand::Step)?,
        "diff" => parse_diff(args)?,
        "timeline" => parse_timeline(args)?,
        "publish" => parse_publish(args)?,
        "sync" => parse_sync(args)?,
        "receive" => parse_no_extra(args, CliCommand::Receive)?,
        "compact" => parse_no_extra(args, CliCommand::Compact)?,
        "status" => parse_no_extra(args, CliCommand::Status)?,
        "login" => parse_login(args)?,
        "whoami" => parse_no_extra(args, CliCommand::Whoami)?,
        "logout" => parse_no_extra(args, CliCommand::Logout)?,
        "spaces" => parse_no_extra(args, CliCommand::Spaces)?,
        "layers" => parse_no_extra(args, CliCommand::Layers)?,
        "layer" => parse_layer(args)?,
        "weave" => parse_weave(args)?,
        "conflict" => parse_conflict(args)?,
        _ => {
            return Err(CliParseError::new(format!(
                "unknown command `{command_name}`; run `layrs --help`"
            )));
        }
    };

    Ok(Cli { globals, command })
}

pub fn usage() -> &'static str {
    "Usage:
  layrs [--space PATH] [--json] [--no-color] COMMAND

Commands:
  layrs init \"Space Name\" [--path PATH]
  layrs step
  layrs diff [STEP_ID] [--stat] [--name-only] [--window START:LIMIT] [--wrap|--no-wrap]
  layrs timeline [--limit N]
  layrs publish [--workspace WORKSPACE_ID]
  layrs sync [--workspace WORKSPACE_ID]
  layrs receive
  layrs compact
  layrs status
  layrs login [--endpoint URL]
  layrs whoami
  layrs logout
  layrs spaces
  layrs layers
  layrs layer use NAME_OR_ID
  layrs layer create NAME
  layrs layer delete NAME_OR_ID [--yes]
  layrs layer disconnect NAME_OR_ID [--yes]
  layrs layer clear-steps NAME_OR_ID [--yes]
  layrs weave parent [--preview]
  layrs weave SOURCE --target TARGET [--preview]
  layrs weave status
  layrs weave conflicts
  layrs weave resolve PATH [--block N] --ours|--theirs|--base|--both-ours-first|--both-theirs-first|--manual-text FILE_OR_STDIN|--file FILE
  layrs weave continue
  layrs weave abort
  layrs conflict list
  layrs conflict status
  layrs conflict resolve
  layrs conflict resolve METHOD -f PATH --block BLOCK_ID
  layrs conflict continue
  layrs conflict abort

Global flags:
  --space PATH     Use a local Layrs space at PATH instead of discovering from cwd.
  --json           Emit stable JSON: {\"ok\":true,\"data\":...} or {\"ok\":false,\"error\":...}.
  --no-color       Disable ANSI color in human output."
}

fn parse_conflict(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let Some(subcommand) = args.first().cloned() else {
        return Err(CliParseError::new(
            "usage: layrs conflict list | status | resolve [METHOD -f PATH --block BLOCK_ID] | continue | abort",
        ));
    };
    args.remove(0);

    match subcommand.as_str() {
        "list" => parse_no_extra(args, CliCommand::ConflictList),
        "status" => parse_no_extra(args, CliCommand::ConflictStatus),
        "resolve" => parse_conflict_resolve(args),
        "continue" => parse_no_extra(args, CliCommand::ConflictContinue),
        "abort" => parse_no_extra(args, CliCommand::ConflictAbort),
        _ => Err(CliParseError::new(format!(
            "unknown conflict command `{subcommand}`; expected list, status, resolve, continue, or abort"
        ))),
    }
}

fn parse_conflict_resolve(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    if args.is_empty() {
        return Ok(CliCommand::ConflictResolve {
            method: None,
            path: None,
            block: None,
        });
    }

    let method = parse_conflict_resolve_method(&args.remove(0))?;
    let path = take_string_option_any(&mut args, &["-f", "--file"])?
        .ok_or_else(|| CliParseError::new("layrs conflict resolve METHOD requires -f PATH."))?;
    let block = take_string_option(&mut args, "--block")?;
    parse_no_extra(
        args,
        CliCommand::ConflictResolve {
            method: Some(method),
            path: Some(path),
            block,
        },
    )
}

fn parse_conflict_resolve_method(value: &str) -> Result<ConflictResolveMethod, CliParseError> {
    match value {
        "existing" => Ok(ConflictResolveMethod::Existing),
        "incoming" => Ok(ConflictResolveMethod::Incoming),
        "both" => Ok(ConflictResolveMethod::Both),
        "manual" => Ok(ConflictResolveMethod::Manual),
        _ => Err(CliParseError::new(format!(
            "unknown conflict resolve method `{value}`; expected existing, incoming, both, or manual"
        ))),
    }
}

fn parse_weave(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let Some(first) = args.first().cloned() else {
        return Err(CliParseError::new(
            "usage: layrs weave parent [--preview] | SOURCE --target TARGET [--preview] | status | conflicts | resolve PATH [--block N] --ours|--theirs|--base|--both-ours-first|--both-theirs-first|--manual-text FILE_OR_STDIN|--file FILE | continue | abort",
        ));
    };
    args.remove(0);

    match first.as_str() {
        "status" => parse_no_extra(args, CliCommand::WeaveStatus),
        "conflicts" => parse_no_extra(args, CliCommand::WeaveConflicts),
        "continue" => parse_no_extra(args, CliCommand::WeaveContinue),
        "abort" => parse_no_extra(args, CliCommand::WeaveAbort),
        "resolve" => parse_weave_resolve(args),
        "parent" => {
            let preview = take_bool_flag(&mut args, "--preview")?;
            parse_no_extra(args, CliCommand::WeaveParent { preview })
        }
        _ => {
            let preview = take_bool_flag(&mut args, "--preview")?;
            let target = take_string_option(&mut args, "--target")?
                .ok_or_else(|| CliParseError::new("layrs weave requires --target TARGET."))?;
            parse_no_extra(
                args,
                CliCommand::Weave {
                    source: first,
                    target,
                    preview,
                },
            )
        }
    }
}

fn parse_weave_resolve(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let block = take_string_option(&mut args, "--block")?;
    let file = take_path_option(&mut args, "--file")?;
    let manual_text = take_path_option(&mut args, "--manual-text")?;
    let ours = take_bool_flag(&mut args, "--ours")?;
    let theirs = take_bool_flag(&mut args, "--theirs")?;
    let base = take_bool_flag(&mut args, "--base")?;
    let both_ours_first = take_bool_flag(&mut args, "--both-ours-first")?;
    let both_theirs_first = take_bool_flag(&mut args, "--both-theirs-first")?;
    let selected = [
        ours,
        theirs,
        base,
        both_ours_first,
        both_theirs_first,
        file.is_some(),
        manual_text.is_some(),
    ]
    .into_iter()
    .filter(|selected| *selected)
    .count();
    if selected != 1 {
        return Err(CliParseError::new(
            "layrs weave resolve needs exactly one of --ours, --theirs, --base, --both-ours-first, --both-theirs-first, --manual-text FILE, or --file FILE.",
        ));
    }
    if block.is_some() && file.is_some() {
        return Err(CliParseError::new(
            "layrs weave resolve --block cannot be combined with --file.",
        ));
    }
    if block.is_none() && manual_text.is_some() {
        return Err(CliParseError::new(
            "layrs weave resolve --manual-text requires --block.",
        ));
    }
    let path = take_required_positional(args, "PATH", "weave resolve")?;
    let choice = if ours {
        "ours"
    } else if theirs {
        "theirs"
    } else if base {
        "base"
    } else if both_ours_first {
        "both_ours_then_theirs"
    } else if both_theirs_first {
        "both_theirs_then_ours"
    } else if manual_text.is_some() {
        "manual"
    } else {
        "file"
    }
    .to_string();
    let replacement = file.or(manual_text);
    let resolution = block
        .map(|block| format!("block:{block}:{choice}"))
        .unwrap_or(choice);
    Ok(CliCommand::WeaveResolve {
        path,
        resolution,
        file: replacement,
    })
}

fn parse_init(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let path = take_path_option(&mut args, "--path")?;
    let name = take_required_positional(args, "Space Name", "init")?;
    Ok(CliCommand::Init { name, path })
}

fn parse_diff(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let stat = take_bool_flag(&mut args, "--stat")?;
    let name_only = take_bool_flag(&mut args, "--name-only")?;
    let window = take_string_option(&mut args, "--window")?
        .map(|value| parse_window(&value))
        .transpose()?;
    let wrap = take_wrap_option(&mut args)?;
    let step_id = take_optional_positional(args, "STEP_ID", "diff")?;

    Ok(CliCommand::Diff {
        step_id,
        stat,
        name_only,
        window,
        wrap,
    })
}

fn parse_timeline(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let limit = take_string_option(&mut args, "--limit")?
        .map(|value| parse_u32_option("--limit", &value))
        .transpose()?;
    parse_no_extra(args, CliCommand::Timeline { limit })
}

fn parse_publish(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let workspace = take_string_option(&mut args, "--workspace")?;
    parse_no_extra(args, CliCommand::Publish { workspace })
}

fn parse_sync(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let workspace = take_string_option(&mut args, "--workspace")?;
    parse_no_extra(args, CliCommand::Sync { workspace })
}

fn parse_login(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let endpoint = take_string_option(&mut args, "--endpoint")?;
    parse_no_extra(args, CliCommand::Login { endpoint })
}

fn parse_layer(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let Some(subcommand) = args.first().cloned() else {
        return Err(CliParseError::new(
            "usage: layrs layer use NAME_OR_ID | create NAME | delete NAME_OR_ID [--yes] | disconnect NAME_OR_ID [--yes] | clear-steps NAME_OR_ID [--yes]",
        ));
    };
    args.remove(0);

    match subcommand.as_str() {
        "use" => {
            let name_or_id = take_required_positional(args, "NAME_OR_ID", "layer use")?;
            Ok(CliCommand::LayerUse { name_or_id })
        }
        "create" => {
            let name = take_required_positional(args, "NAME", "layer create")?;
            Ok(CliCommand::LayerCreate { name })
        }
        "delete" => {
            let yes = take_bool_flag(&mut args, "--yes")?;
            let name_or_id = take_required_positional(args, "NAME_OR_ID", "layer delete")?;
            Ok(CliCommand::LayerDelete { name_or_id, yes })
        }
        "disconnect" => {
            let yes = take_bool_flag(&mut args, "--yes")?;
            let name_or_id = take_required_positional(args, "NAME_OR_ID", "layer disconnect")?;
            Ok(CliCommand::LayerDisconnect { name_or_id, yes })
        }
        "clear-steps" => {
            let yes = take_bool_flag(&mut args, "--yes")?;
            let name_or_id = take_required_positional(args, "NAME_OR_ID", "layer clear-steps")?;
            Ok(CliCommand::LayerClearSteps { name_or_id, yes })
        }
        _ => Err(CliParseError::new(format!(
            "unknown layer command `{subcommand}`; expected use, create, delete, disconnect, or clear-steps"
        ))),
    }
}

fn parse_no_extra(args: Vec<String>, command: CliCommand) -> Result<CliCommand, CliParseError> {
    if args.is_empty() {
        Ok(command)
    } else {
        Err(CliParseError::new(format!(
            "unexpected argument `{}`",
            args[0]
        )))
    }
}

fn take_global_flags(args: &mut Vec<String>) -> Result<GlobalFlags, CliParseError> {
    Ok(GlobalFlags {
        space: take_path_option(args, "--space")?,
        json: take_bool_flag(args, "--json")?,
        no_color: take_bool_flag(args, "--no-color")?,
    })
}

fn take_help_flag(args: &mut Vec<String>) -> bool {
    take_flag_anywhere(args, "--help") || take_flag_anywhere(args, "-h")
}

fn reject_remaining_help_context(args: &[String]) -> Result<(), CliParseError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(CliParseError::new(format!(
            "unexpected argument `{}` before help flag",
            args[0]
        )))
    }
}

fn take_optional_positional(
    args: Vec<String>,
    name: &str,
    command: &str,
) -> Result<Option<String>, CliParseError> {
    match args.as_slice() {
        [] => Ok(None),
        [value] if !value.starts_with("--") => Ok(Some(value.clone())),
        [unexpected] => Err(CliParseError::new(format!(
            "unexpected argument `{unexpected}` for `layrs {command}`"
        ))),
        _ => Err(CliParseError::new(format!(
            "too many arguments for `layrs {command}`; expected optional {name}"
        ))),
    }
}

fn take_required_positional(
    args: Vec<String>,
    name: &str,
    command: &str,
) -> Result<String, CliParseError> {
    match args.as_slice() {
        [value] if !value.starts_with("--") => Ok(value.clone()),
        [] => Err(CliParseError::new(format!(
            "missing {name} for `layrs {command}`"
        ))),
        [unexpected] => Err(CliParseError::new(format!(
            "unexpected argument `{unexpected}` for `layrs {command}`"
        ))),
        _ => Err(CliParseError::new(format!(
            "too many arguments for `layrs {command}`; expected {name}"
        ))),
    }
}

fn take_wrap_option(args: &mut Vec<String>) -> Result<Option<bool>, CliParseError> {
    let wrap = take_bool_flag(args, "--wrap")?;
    let no_wrap = take_bool_flag(args, "--no-wrap")?;
    match (wrap, no_wrap) {
        (true, true) => Err(CliParseError::new(
            "`layrs diff` accepts either --wrap or --no-wrap, not both",
        )),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (false, false) => Ok(None),
    }
}

fn take_bool_flag(args: &mut Vec<String>, flag: &str) -> Result<bool, CliParseError> {
    let mut found = false;
    let mut index = 0;

    while index < args.len() {
        if args[index] == flag {
            if found {
                return Err(CliParseError::new(format!("duplicate `{flag}` option")));
            }
            found = true;
            args.remove(index);
        } else if args[index].starts_with(&format!("{flag}=")) {
            return Err(CliParseError::new(format!(
                "`{flag}` does not accept a value"
            )));
        } else {
            index += 1;
        }
    }

    Ok(found)
}

fn take_flag_anywhere(args: &mut Vec<String>, flag: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == flag) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_path_option(args: &mut Vec<String>, flag: &str) -> Result<Option<PathBuf>, CliParseError> {
    take_string_option(args, flag).map(|value| value.map(PathBuf::from))
}

fn take_string_option(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, CliParseError> {
    let mut found = None;
    let mut index = 0;

    while index < args.len() {
        if args[index] == flag {
            if found.is_some() {
                return Err(CliParseError::new(format!("duplicate `{flag}` option")));
            }
            if index + 1 >= args.len() {
                return Err(CliParseError::new(format!("missing value for `{flag}`")));
            }
            let value = args.remove(index + 1);
            if value.starts_with("--") {
                return Err(CliParseError::new(format!("missing value for `{flag}`")));
            }
            args.remove(index);
            found = Some(value);
        } else if let Some(value) = args[index].strip_prefix(&format!("{flag}=")) {
            if found.is_some() {
                return Err(CliParseError::new(format!("duplicate `{flag}` option")));
            }
            if value.is_empty() {
                return Err(CliParseError::new(format!("missing value for `{flag}`")));
            }
            found = Some(value.to_string());
            args.remove(index);
        } else {
            index += 1;
        }
    }

    Ok(found)
}

fn take_string_option_any(
    args: &mut Vec<String>,
    flags: &[&str],
) -> Result<Option<String>, CliParseError> {
    let mut found = None;
    let mut index = 0;

    while index < args.len() {
        let matched = flags.iter().copied().find(|flag| args[index] == *flag);
        if let Some(flag) = matched {
            if found.is_some() {
                return Err(CliParseError::new(format!(
                    "duplicate `{}` option",
                    flags.join("|")
                )));
            }
            if index + 1 >= args.len() {
                return Err(CliParseError::new(format!("missing value for `{flag}`")));
            }
            let value = args.remove(index + 1);
            if value.starts_with("--") {
                return Err(CliParseError::new(format!("missing value for `{flag}`")));
            }
            args.remove(index);
            found = Some(value);
        } else if let Some((flag, value)) = flags.iter().find_map(|flag| {
            args[index]
                .strip_prefix(&format!("{flag}="))
                .map(|value| (*flag, value))
        }) {
            if found.is_some() {
                return Err(CliParseError::new(format!(
                    "duplicate `{}` option",
                    flags.join("|")
                )));
            }
            if value.is_empty() {
                return Err(CliParseError::new(format!("missing value for `{flag}`")));
            }
            found = Some(value.to_string());
            args.remove(index);
        } else {
            index += 1;
        }
    }

    Ok(found)
}

fn parse_window(value: &str) -> Result<Window, CliParseError> {
    let Some((start, limit)) = value.split_once(':') else {
        return Err(CliParseError::new(
            "`--window` must use START:LIMIT, for example --window 0:200",
        ));
    };
    Ok(Window {
        start: parse_u32_option("--window START", start)?,
        limit: parse_u32_option("--window LIMIT", limit)?,
    })
}

fn parse_u32_option(flag: &str, value: &str) -> Result<u32, CliParseError> {
    value
        .parse::<u32>()
        .map_err(|_| CliParseError::new(format!("`{flag}` expects a non-negative integer")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_with_name_and_path() {
        let cli = parse_args(["--json", "init", "Game Prototype", "--path", "D:/work/game"])
            .expect("parse");

        assert_eq!(
            cli,
            Cli {
                globals: GlobalFlags {
                    json: true,
                    no_color: false,
                    space: None,
                },
                command: CliCommand::Init {
                    name: "Game Prototype".to_string(),
                    path: Some(PathBuf::from("D:/work/game"))
                }
            }
        );
    }

    #[test]
    fn parses_diff_options() {
        let cli = parse_args([
            "diff",
            "step-123",
            "--stat",
            "--window",
            "20:40",
            "--no-wrap",
        ])
        .expect("parse");

        assert_eq!(
            cli.command,
            CliCommand::Diff {
                step_id: Some("step-123".to_string()),
                stat: true,
                name_only: false,
                window: Some(Window {
                    start: 20,
                    limit: 40
                }),
                wrap: Some(false),
            }
        );
    }

    #[test]
    fn parses_layer_delete_yes_with_global_space() {
        let cli = parse_args(["layer", "delete", "draft", "--yes", "--space", "."]).expect("parse");

        assert_eq!(cli.globals.space, Some(PathBuf::from(".")));
        assert_eq!(
            cli.command,
            CliCommand::LayerDelete {
                name_or_id: "draft".to_string(),
                yes: true
            }
        );
    }

    #[test]
    fn parses_compact() {
        let cli = parse_args(["compact"]).expect("parse");
        assert_eq!(cli.command, CliCommand::Compact);
    }

    #[test]
    fn parses_sync_workspace() {
        let cli = parse_args(["sync", "--workspace", "workspace_123"]).expect("parse");
        assert_eq!(
            cli.command,
            CliCommand::Sync {
                workspace: Some("workspace_123".to_string())
            }
        );
    }

    #[test]
    fn parses_weave_block_resolution() {
        let cli = parse_args(["weave", "resolve", "story.txt", "--block", "2", "--theirs"])
            .expect("parse");

        assert_eq!(
            cli.command,
            CliCommand::WeaveResolve {
                path: "story.txt".to_string(),
                resolution: "block:2:theirs".to_string(),
                file: None,
            }
        );
    }

    #[test]
    fn parses_weave_manual_text_block_resolution() {
        let cli = parse_args([
            "weave",
            "resolve",
            "story.txt",
            "--block",
            "2",
            "--manual-text",
            "resolved.txt",
        ])
        .expect("parse");

        assert_eq!(
            cli.command,
            CliCommand::WeaveResolve {
                path: "story.txt".to_string(),
                resolution: "block:2:manual".to_string(),
                file: Some(PathBuf::from("resolved.txt")),
            }
        );
    }

    #[test]
    fn rejects_weave_manual_text_without_block() {
        let error = parse_args([
            "weave",
            "resolve",
            "story.txt",
            "--manual-text",
            "resolved.txt",
        ])
        .expect_err("parse should fail");

        assert!(error.message().contains("--manual-text requires --block"));
    }

    #[test]
    fn rejects_weave_block_resolution_with_file() {
        let error = parse_args([
            "weave",
            "resolve",
            "story.txt",
            "--block",
            "1",
            "--file",
            "resolved.txt",
        ])
        .expect_err("parse should fail");

        assert!(error.message().contains("--block cannot be combined"));
    }

    #[test]
    fn parses_conflict_resolve_method_path_and_block() {
        let cli = parse_args([
            "conflict",
            "resolve",
            "incoming",
            "-f",
            "story.txt",
            "--block",
            "1",
        ])
        .expect("parse");

        assert_eq!(
            cli.command,
            CliCommand::ConflictResolve {
                method: Some(ConflictResolveMethod::Incoming),
                path: Some("story.txt".to_string()),
                block: Some("1".to_string()),
            }
        );
    }

    #[test]
    fn parses_conflict_resolve_without_args_as_interactive() {
        let cli = parse_args(["conflict", "resolve"]).expect("parse");

        assert_eq!(
            cli.command,
            CliCommand::ConflictResolve {
                method: None,
                path: None,
                block: None,
            }
        );
    }

    #[test]
    fn rejects_conflict_resolve_without_path() {
        let error = parse_args(["conflict", "resolve", "existing"]).expect_err("parse should fail");

        assert!(error.message().contains("requires -f PATH"));
    }

    #[test]
    fn rejects_conflicting_wrap_flags() {
        let error = parse_args(["diff", "--wrap", "--no-wrap"]).expect_err("parse should fail");

        assert!(error.message().contains("either --wrap or --no-wrap"));
    }
}
