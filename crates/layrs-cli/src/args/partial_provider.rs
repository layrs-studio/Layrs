use super::{take_string_option, CliParseError};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartialProviderCommand {
    Status { ticket: PathBuf },
}

pub(super) fn parse(mut args: Vec<String>) -> Result<PartialProviderCommand, CliParseError> {
    if args.first().map(String::as_str) != Some("status") {
        return Err(usage());
    }
    args.remove(0);
    let ticket = take_string_option(&mut args, "--ticket")?
        .filter(|value| !value.is_empty())
        .ok_or_else(usage)?;
    if !args.is_empty() {
        return Err(usage());
    }
    Ok(PartialProviderCommand::Status {
        ticket: PathBuf::from(ticket),
    })
}

fn usage() -> CliParseError {
    CliParseError::new("usage: layrs partial provider status --ticket FILE")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{parse_args, CliCommand};

    #[test]
    fn exact_provider_status_command_has_separate_route() {
        let args = vec!["partial", "provider", "status", "--ticket", "bound.json"];
        let cli = parse_args(args.into_iter().map(str::to_owned)).unwrap();
        assert!(
            matches!(cli.command, CliCommand::PartialProvider(PartialProviderCommand::Status { ticket }) if ticket == PathBuf::from("bound.json"))
        );
        for tail in [
            vec!["status"],
            vec!["status", "--ticket", "a", "extra"],
            vec!["status", "--ticket", "a", "--ticket", "b"],
            vec!["capture", "--ticket", "a"],
        ] {
            let mut words = vec!["partial", "provider"];
            words.extend(tail);
            assert!(parse_args(words.into_iter().map(str::to_owned)).is_err());
        }
    }
}
