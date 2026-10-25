mod blocks;
mod git;
pub(crate) mod offline;
mod vfs;
mod worktree;
pub use blocks::BlockChoice;
pub use offline::OfflineCommand;
use std::fmt;
use std::path::PathBuf;
pub use vfs::VfsCommand;
pub use worktree::WorktreeCommand;

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
    Offline(OfflineCommand),
    Vfs(VfsCommand),
    Worktree(WorktreeCommand),
    ResolveBlock {
        layer: String,
        conflict: String,
        block: String,
        choice: BlockChoice,
        preview: bool,
        expected_operation: Option<String>,
        expected_step: Option<String>,
    },
    LocalIdentity {
        actor: Option<String>,
        device: Option<String>,
        name: Option<String>,
    },
    Doctor,
    Compact {
        resume: bool,
    },
    ConflictVariant {
        layer: String,
        conflict: String,
        term: String,
        against: Option<String>,
    },
    ResolveFile {
        layer: String,
        conflict: String,
        file: PathBuf,
        executable: bool,
        preview: bool,
    },
    ResolveTerm {
        layer: String,
        conflict: String,
        term: String,
        preview: bool,
    },
    History {
        selector: String,
        path: Option<String>,
        actor: Option<String>,
        since: Option<String>,
        until: Option<String>,
        cursor: Option<String>,
        limit: u32,
    },
    OperationHistory {
        search: String,
        actor: Option<String>,
        since: Option<String>,
        until: Option<String>,
        cursor: Option<String>,
        limit: u32,
    },
    Rewrite {
        kind: String,
        source: String,
        mainline: Option<String>,
        preview: bool,
    },
    Remote {
        args: Vec<String>,
    },
    Partial {
        args: Vec<String>,
    },
    Help,
    Init {
        name: String,
        path: Option<PathBuf>,
    },
    Step,
    GitImport {
        source: PathBuf,
        dry_run: bool,
        timeout_seconds: u32,
        references: Vec<String>,
    },
    GitScanCache {
        source: PathBuf,
        output: PathBuf,
        timeout_seconds: u32,
        references: Vec<String>,
    },
    GitImportCache {
        checkpoint: PathBuf,
        digest: String,
        dry_run: bool,
        timeout_seconds: u32,
    },
    GitExport {
        destination: PathBuf,
        snapshot: Option<String>,
        resume: bool,
        timeout_seconds: u32,
        references: Vec<String>,
    },
    Restore {
        step: String,
        path: Option<String>,
        preview: bool,
        expected: Option<String>,
    },
    RestoreView {
        operation: String,
        preview: bool,
        expected: Option<String>,
    },
    Undo,
    Redo {
        operation: Option<String>,
    },
    Recover,
    Markers,
    MarkerCreate {
        name: String,
    },
    MarkerHide {
        id: String,
    },
    MarkerShow {
        id: String,
    },
    Releases,
    ReleaseCreate {
        version: String,
    },
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
    Status,
    Layers,
    LayerUse {
        name_or_id: String,
    },
    LayerCreate {
        name: String,
    },
    Weave {
        source: String,
        target: String,
        preview: bool,
    },
    WeaveStatus,
    WeaveConflicts,
    WeaveContinue,
    WeaveAbort,
    ConflictList,
    ConflictStatus,
    ConflictContinue,
    ConflictAbort,
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
        "resolve-block" => blocks::parse(args)?,
        "conflict-variant" => {
            let layer = take_string_option(&mut args, "--layer")?
                .ok_or_else(|| CliParseError::new("conflict-variant requires --layer LAYER"))?;
            let against = take_string_option(&mut args, "--against")?;
            match args.as_slice() {
                [conflict, term] => CliCommand::ConflictVariant {
                    layer,
                    conflict: conflict.clone(),
                    term: term.clone(),
                    against,
                },
                _ => {
                    return Err(CliParseError::new(
                        "usage: conflict-variant CONFLICT_ID TERM_ID --layer LAYER [--against TERM_ID]",
                    ));
                }
            }
        }
        "resolve-file" => {
            let mut args = args;
            let layer = take_string_option(&mut args, "--layer")?
                .ok_or_else(|| CliParseError::new("resolve-file requires --layer LAYER"))?;
            let executable = take_bool_flag(&mut args, "--executable")?;
            let preview = take_bool_flag(&mut args, "--preview")?;
            match args.as_slice() {
                [conflict, file] => CliCommand::ResolveFile {
                    layer,
                    conflict: conflict.clone(),
                    file: PathBuf::from(file),
                    executable,
                    preview,
                },
                _ => {
                    return Err(CliParseError::new(
                        "usage: resolve-file CONFLICT_ID FILE --layer LAYER [--executable] [--preview]",
                    ));
                }
            }
        }
        "resolve-term" => {
            let mut args = args;
            let preview = take_bool_flag(&mut args, "--preview")?;
            let layer = take_string_option(&mut args, "--layer")?
                .ok_or_else(|| CliParseError::new("resolve-term requires --layer LAYER"))?;
            match args.as_slice() {
                [conflict, term] => CliCommand::ResolveTerm {
                    layer,
                    conflict: conflict.clone(),
                    term: term.clone(),
                    preview,
                },
                _ => {
                    return Err(CliParseError::new(
                        "usage: resolve-term CONFLICT_ID TERM_ID --layer LAYER [--preview]",
                    ));
                }
            }
        }
        "operation-history" => {
            let mut args = args;
            let search = take_string_option(&mut args, "--search")?.unwrap_or_default();
            let actor = take_string_option(&mut args, "--actor")?;
            let since = take_string_option(&mut args, "--since")?;
            let until = take_string_option(&mut args, "--until")?;
            let cursor = take_string_option(&mut args, "--cursor")?;
            let limit = take_string_option(&mut args, "--limit")?
                .map(|v| parse_u32_option("--limit", &v))
                .transpose()?
                .unwrap_or(50);
            parse_no_extra(
                args,
                CliCommand::OperationHistory {
                    search,
                    actor,
                    since,
                    until,
                    cursor,
                    limit,
                },
            )?
        }
        "history" => {
            let mut args = args;
            let path = take_string_option(&mut args, "--path")?;
            let actor = take_string_option(&mut args, "--actor")?;
            let since = take_string_option(&mut args, "--since")?;
            let until = take_string_option(&mut args, "--until")?;
            let cursor = take_string_option(&mut args, "--cursor")?;
            let limit = take_string_option(&mut args, "--limit")?
                .map(|v| parse_u32_option("--limit", &v))
                .transpose()?
                .unwrap_or(50);
            let selector = take_required_positional(args, "STEP_OR_REFERENCE", "history")?;
            CliCommand::History {
                selector,
                path,
                actor,
                since,
                until,
                cursor,
                limit,
            }
        }
        "replay" | "revert" | "squash" => {
            let mut args = args;
            let preview = take_bool_flag(&mut args, "--preview")?;
            let mainline = take_string_option(&mut args, "--mainline")?;
            if command_name == "squash" && mainline.is_some() {
                return Err(CliParseError::new("squash does not accept --mainline"));
            }
            let source = take_required_positional(args, "STEP_OR_REFERENCE", &command_name)?;
            CliCommand::Rewrite {
                kind: command_name,
                source,
                mainline,
                preview,
            }
        }
        "identity" => {
            let mut args = args;
            let actor = take_string_option(&mut args, "--actor")?;
            let device = take_string_option(&mut args, "--device")?;
            let name = take_string_option(&mut args, "--name")?;
            if actor.is_none() && (device.is_some() || name.is_some()) {
                return Err(CliParseError::new(
                    "identity configuration requires --actor ACTOR_ID",
                ));
            }
            parse_no_extra(
                args,
                CliCommand::LocalIdentity {
                    actor,
                    device,
                    name,
                },
            )?
        }
        "remote" => CliCommand::Remote { args },
        "worktree" => CliCommand::Worktree(worktree::parse(args)?),
        "vfs" => CliCommand::Vfs(vfs::parse(args)?),
        "partial" if args.first().is_some_and(|arg| arg == "offline") => {
            CliCommand::Offline(offline::parse(args.into_iter().skip(1).collect())?)
        }
        "partial" => CliCommand::Partial { args },
        "init" => parse_init(args)?,
        "step" => parse_no_extra(args, CliCommand::Step)?,
        "doctor" => parse_no_extra(args, CliCommand::Doctor)?,
        "compact" => {
            let mut args = args;
            let resume = take_bool_flag(&mut args, "--resume")?;
            parse_no_extra(args, CliCommand::Compact { resume })?
        }
        "git" => git::parse(args)?,
        "restore" | "restore-view" => {
            let mut args = args;
            let preview = take_bool_flag(&mut args, "--preview")?;
            let expected = take_string_option(&mut args, "--expected")?;
            if preview && expected.is_some() {
                return Err(CliParseError::new(
                    "--preview and --expected are mutually exclusive",
                ));
            }
            if command_name == "restore" {
                let path = take_string_option(&mut args, "--path")?;
                let step = take_required_positional(args, "STEP_OR_REFERENCE", "restore")?;
                CliCommand::Restore {
                    step,
                    path,
                    preview,
                    expected,
                }
            } else {
                let operation = take_required_positional(args, "OPERATION_ID", "restore-view")?;
                CliCommand::RestoreView {
                    operation,
                    preview,
                    expected,
                }
            }
        }
        "undo" => parse_no_extra(args, CliCommand::Undo)?,
        "redo" => match args.as_slice() {
            [] => CliCommand::Redo { operation: None },
            [operation] => CliCommand::Redo {
                operation: Some(operation.clone()),
            },
            _ => return Err(CliParseError::new("usage: layrs redo [OPERATION_ID]")),
        },
        "recover" => parse_no_extra(args, CliCommand::Recover)?,
        "markers" => parse_no_extra(args, CliCommand::Markers)?,
        "marker" => match args.as_slice() {
            [action, name] if action == "create" => CliCommand::MarkerCreate { name: name.clone() },
            [action, id] if action == "hide" => CliCommand::MarkerHide { id: id.clone() },
            [action, id] if action == "show" => CliCommand::MarkerShow { id: id.clone() },
            _ => {
                return Err(CliParseError::new(
                    "usage: layrs marker create NAME | hide MARKER_ID",
                ));
            }
        },
        "releases" => parse_no_extra(args, CliCommand::Releases)?,
        "release" => match args.as_slice() {
            [action, version] if action == "create" => CliCommand::ReleaseCreate {
                version: version.clone(),
            },
            _ => return Err(CliParseError::new("usage: layrs release create VERSION")),
        },
        "diff" => parse_diff(args)?,
        "timeline" => parse_timeline(args)?,
        "status" => parse_no_extra(args, CliCommand::Status)?,
        // Account commands are aliases of their `layrs remote` form.
        "login" | "whoami" | "logout" => CliCommand::Remote {
            args: std::iter::once(command_name.clone()).chain(args).collect(),
        },
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

mod help;
pub use help::usage;

fn parse_conflict(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let Some(subcommand) = args.first().cloned() else {
        return Err(CliParseError::new(
            "usage: layrs conflict list | status | continue | abort",
        ));
    };
    args.remove(0);

    match subcommand.as_str() {
        "list" => parse_no_extra(args, CliCommand::ConflictList),
        "status" => parse_no_extra(args, CliCommand::ConflictStatus),
        "continue" => parse_no_extra(args, CliCommand::ConflictContinue),
        "abort" => parse_no_extra(args, CliCommand::ConflictAbort),
        _ => Err(CliParseError::new(format!(
            "unknown conflict command `{subcommand}`; expected list, status, continue, or abort"
        ))),
    }
}

fn parse_weave(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let Some(first) = args.first().cloned() else {
        return Err(CliParseError::new(
            "usage: layrs weave SOURCE --target TARGET [--preview] | status | conflicts | continue | abort",
        ));
    };
    args.remove(0);

    match first.as_str() {
        "status" => parse_no_extra(args, CliCommand::WeaveStatus),
        "conflicts" => parse_no_extra(args, CliCommand::WeaveConflicts),
        "continue" => parse_no_extra(args, CliCommand::WeaveContinue),
        "abort" => parse_no_extra(args, CliCommand::WeaveAbort),
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

fn parse_layer(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let Some(subcommand) = args.first().cloned() else {
        return Err(CliParseError::new(
            "usage: layrs layer use NAME_OR_ID | create NAME",
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
        _ => Err(CliParseError::new(format!(
            "unknown layer command `{subcommand}`; expected use or create"
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
#[path = "args/tests.rs"]
mod tests;
