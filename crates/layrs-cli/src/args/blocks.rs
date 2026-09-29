use super::*;

pub(super) fn parse(mut args: Vec<String>) -> Result<CliCommand, CliParseError> {
    let layer = take_string_option(&mut args, "--layer")?.unwrap_or_else(|| "main".into());
    let term = take_string_option(&mut args, "--term")?;
    let file = take_string_option(&mut args, "--file")?;
    let sources = take_string_option(&mut args, "--sources")?;
    let expected_operation = take_string_option(&mut args, "--expected-operation")?;
    let expected_step = take_string_option(&mut args, "--expected-step")?;
    let preview = take_bool_flag(&mut args, "--preview")?;
    if expected_operation.is_some() != expected_step.is_some()
        || (preview && expected_step.is_some())
    {
        return Err(CliParseError::new(
            "use --expected-operation and --expected-step together on apply",
        ));
    }
    let choice = match (term, file, sources) {
        (Some(term), None, None) => BlockChoice::Term(term),
        (None, Some(file), Some(sources)) if !sources.is_empty() => BlockChoice::File {
            file: PathBuf::from(file),
            sources: sources.split(',').map(str::to_owned).collect(),
        },
        _ => {
            return Err(CliParseError::new(
                "resolve-block requires either --term TERM or --file FILE --sources TERM,...",
            ));
        }
    };
    match args.as_slice() {
        [conflict, block] if !conflict.starts_with("--") && !block.starts_with("--") => {
            Ok(CliCommand::ResolveBlock {
                layer,
                conflict: conflict.clone(),
                block: block.clone(),
                choice,
                preview,
                expected_operation,
                expected_step,
            })
        }
        _ => Err(CliParseError::new(
            "expected resolve-block CONFLICT BLOCK [options]",
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockChoice {
    Term(String),
    File { file: PathBuf, sources: Vec<String> },
}
