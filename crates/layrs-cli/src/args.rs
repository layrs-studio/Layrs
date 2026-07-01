use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Help,
    Init {
        store: Option<PathBuf>,
    },
    WorkspaceCreate {
        store: Option<PathBuf>,
        name: String,
    },
    SpaceCreate {
        store: Option<PathBuf>,
        workspace: Option<String>,
        name: String,
    },
    LayerCreate {
        store: Option<PathBuf>,
        space: Option<String>,
        name: String,
    },
    Status {
        store: Option<PathBuf>,
    },
    StoreScrub {
        store: Option<PathBuf>,
    },
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

pub fn parse_args<I, S>(args: I) -> Result<CliCommand, CliParseError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();

    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(CliCommand::Help);
    }

    let store = take_path_option(&mut args, "--store")?;
    let command = args.remove(0);

    match command.as_str() {
        "init" => parse_init(args, store),
        "workspace" => parse_workspace(args, store),
        "space" => parse_space(args, store),
        "layer" => parse_layer(args, store),
        "status" => parse_no_extra(args, CliCommand::Status { store }),
        "store" => parse_store(args, store),
        _ => Err(CliParseError::new(format!(
            "unknown command `{command}`; run `layrs --help`"
        ))),
    }
}

pub fn usage() -> &'static str {
    "Usage:
  layrs init [PATH] [--store PATH]
  layrs workspace create NAME [--store PATH]
  layrs space create NAME [--workspace ID] [--store PATH]
  layrs layer create NAME [--space ID] [--store PATH]
  layrs status [--store PATH]
  layrs store scrub [--store PATH]"
}

fn parse_init(args: Vec<String>, store: Option<PathBuf>) -> Result<CliCommand, CliParseError> {
    match (args.as_slice(), store) {
        ([], store) => Ok(CliCommand::Init { store }),
        ([path], None) => Ok(CliCommand::Init {
            store: Some(PathBuf::from(path)),
        }),
        ([_path], Some(_)) => Err(CliParseError::new(
            "`layrs init` accepts either PATH or --store PATH, not both",
        )),
        _ => Err(CliParseError::new(
            "usage: layrs init [PATH] [--store PATH]",
        )),
    }
}

fn parse_workspace(
    mut args: Vec<String>,
    store: Option<PathBuf>,
) -> Result<CliCommand, CliParseError> {
    expect_subcommand(&mut args, "workspace", "create")?;
    let name = take_required_name(args, "workspace create")?;
    Ok(CliCommand::WorkspaceCreate { store, name })
}

fn parse_space(mut args: Vec<String>, store: Option<PathBuf>) -> Result<CliCommand, CliParseError> {
    expect_subcommand(&mut args, "space", "create")?;
    let workspace = take_string_option(&mut args, "--workspace")?;
    let name = take_required_name(args, "space create")?;
    Ok(CliCommand::SpaceCreate {
        store,
        workspace,
        name,
    })
}

fn parse_layer(mut args: Vec<String>, store: Option<PathBuf>) -> Result<CliCommand, CliParseError> {
    expect_subcommand(&mut args, "layer", "create")?;
    let space = take_string_option(&mut args, "--space")?;
    let name = take_required_name(args, "layer create")?;
    Ok(CliCommand::LayerCreate { store, space, name })
}

fn parse_store(mut args: Vec<String>, store: Option<PathBuf>) -> Result<CliCommand, CliParseError> {
    expect_subcommand(&mut args, "store", "scrub")?;
    parse_no_extra(args, CliCommand::StoreScrub { store })
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

fn expect_subcommand(
    args: &mut Vec<String>,
    command: &str,
    expected: &str,
) -> Result<(), CliParseError> {
    if args.first().map(String::as_str) == Some(expected) {
        args.remove(0);
        Ok(())
    } else {
        Err(CliParseError::new(format!(
            "usage: layrs {command} {expected} NAME"
        )))
    }
}

fn take_required_name(args: Vec<String>, command: &str) -> Result<String, CliParseError> {
    match args.as_slice() {
        [name] if !name.starts_with("--") => Ok(name.clone()),
        [] => Err(CliParseError::new(format!(
            "missing NAME for `layrs {command}`"
        ))),
        [unexpected] => Err(CliParseError::new(format!(
            "unexpected argument `{unexpected}`"
        ))),
        _ => Err(CliParseError::new(format!(
            "too many arguments for `layrs {command}`"
        ))),
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
            args.remove(index);
            found = Some(value);
        } else if let Some(value) = args[index].strip_prefix(&format!("{flag}=")) {
            if found.is_some() {
                return Err(CliParseError::new(format!("duplicate `{flag}` option")));
            }
            found = Some(value.to_string());
            args.remove(index);
        } else {
            index += 1;
        }
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_with_positional_store_path() {
        let command = parse_args(["init", ".layrs"]).expect("parse");

        assert_eq!(
            command,
            CliCommand::Init {
                store: Some(PathBuf::from(".layrs"))
            }
        );
    }

    #[test]
    fn parses_workspace_create() {
        let command = parse_args(["workspace", "create", "Acme"]).expect("parse");

        assert_eq!(
            command,
            CliCommand::WorkspaceCreate {
                store: None,
                name: "Acme".to_string()
            }
        );
    }

    #[test]
    fn parses_store_scrub_with_store_flag() {
        let command = parse_args(["store", "scrub", "--store", "D:/tmp/layrs"]).expect("parse");

        assert_eq!(
            command,
            CliCommand::StoreScrub {
                store: Some(PathBuf::from("D:/tmp/layrs"))
            }
        );
    }

    #[test]
    fn rejects_layer_create_without_name() {
        let error = parse_args(["layer", "create"]).expect_err("parse should fail");

        assert!(error.message().contains("missing NAME"));
    }
}
