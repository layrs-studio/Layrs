use super::*;

pub(super) fn parse(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let timeout_seconds = take_string_option(&mut args, "--timeout-seconds")?
        .map(|v| parse_u32_option("--timeout-seconds", &v))
        .transpose()?
        .unwrap_or(60);
    if !(1..=86400).contains(&timeout_seconds) {
        return Err(CliParseError::new("Git timeout requires 1..86400 seconds"));
    }
    let dry_run = take_bool_flag(&mut args, "--dry-run")?;
    let resume = take_bool_flag(&mut args, "--resume")?;
    let snapshot = take_string_option(&mut args, "--snapshot")?;
    let output = take_string_option(&mut args, "--output")?;
    let digest = take_string_option(&mut args, "--digest")?;
    let mut references = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let value = if args[index] == "--ref" {
            if index + 1 == args.len() || args[index + 1].starts_with("--") {
                return Err(CliParseError::new("missing value for --ref"));
            }
            args.remove(index);
            Some(args.remove(index))
        } else if let Some(value) = args[index].strip_prefix("--ref=") {
            let value = value.to_owned();
            args.remove(index);
            Some(value)
        } else {
            None
        };
        if let Some(value) = value {
            if value.is_empty() || references.contains(&value) || references.len() >= 1024 {
                return Err(CliParseError::new(
                    "--ref requires distinct nonempty full references (maximum 1024)",
                ));
            }
            references.push(value);
        } else {
            index += 1;
        }
    }
    match args.as_slice() {
        [action, source]
            if action == "scan-cache"
                && output.is_some()
                && digest.is_none()
                && !dry_run
                && !resume
                && snapshot.is_none() =>
        {
            Ok(CliCommand::GitScanCache {
                source: PathBuf::from(source),
                output: PathBuf::from(output.unwrap()),
                timeout_seconds,
                references,
            })
        }
        [action, checkpoint]
            if action == "import-cache"
                && digest.is_some()
                && output.is_none()
                && references.is_empty()
                && !resume
                && snapshot.is_none() =>
        {
            Ok(CliCommand::GitImportCache {
                checkpoint: PathBuf::from(checkpoint),
                digest: digest.unwrap(),
                dry_run,
                timeout_seconds,
            })
        }
        [action, source]
            if action == "import"
                && !resume
                && snapshot.is_none()
                && output.is_none()
                && digest.is_none() =>
        {
            Ok(CliCommand::GitImport {
                source: PathBuf::from(source),
                dry_run,
                timeout_seconds,
                references,
            })
        }
        [action, destination]
            if action == "export"
                && !dry_run
                && (references.is_empty() || snapshot.is_none())
                && output.is_none()
                && digest.is_none() =>
        {
            Ok(CliCommand::GitExport {
                destination: PathBuf::from(destination),
                snapshot,
                resume,
                timeout_seconds,
                references,
            })
        }
        _ => Err(CliParseError::new(
            "usage: layrs git import SOURCE [--ref FULL_REF ...] [--dry-run] | export DESTINATION [--ref FULL_REF ... | --snapshot STEP_ID] [--resume] | scan-cache SOURCE --output NEW_FILE [--ref FULL_REF ...] | import-cache FILE --digest sha256:HEX [--dry-run] [--timeout-seconds N]",
        )),
    }
}
