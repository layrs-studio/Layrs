//! Remote selection flags are distinct from history's --path filter.
use crate::error::CliError;
use layrs_core::{
    projection::{PathSemantics, ProjectionSelection},
    scm::RepoPath,
};

pub(super) fn take(args: &mut Vec<String>) -> Result<Option<ProjectionSelection>, CliError> {
    let mut paths = Vec::new();
    let mut subtrees = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let (flag, inline) = args[index]
            .split_once('=')
            .map_or((args[index].as_str(), None), |(flag, value)| {
                (flag, Some(value))
            });
        let target = match flag {
            "--include" => &mut paths,
            "--include-tree" => &mut subtrees,
            _ => {
                index += 1;
                continue;
            }
        };
        let flag = flag.to_owned();
        let value = if let Some(value) = inline {
            let value = value.to_owned();
            args.remove(index);
            value
        } else {
            if index + 1 >= args.len() || args[index + 1].starts_with('-') {
                return Err(CliError::usage(format!(
                    "{flag} needs a repository-relative path"
                )));
            }
            args.remove(index);
            args.remove(index)
        };
        if value.is_empty() {
            return Err(CliError::usage(format!(
                "{flag} needs a repository-relative path"
            )));
        }
        let path = RepoPath::parse(value).map_err(|e| CliError::usage(e.to_string()))?;
        if target.contains(&path) {
            return Err(CliError::usage(format!("duplicate {flag} path `{path}`")));
        }
        target.push(path);
    }
    if paths.is_empty() && subtrees.is_empty() {
        return Ok(None);
    }
    if args.first().is_none_or(|action| {
        !matches!(
            action.as_str(),
            "clone-partial" | "partial-select" | "partial-hydrate"
        )
    }) {
        return Err(CliError::usage(
            "--include/--include-tree require remote clone-partial, partial-select or partial-hydrate",
        ));
    }
    let semantics = if cfg!(windows) {
        PathSemantics::PortableWindows
    } else {
        PathSemantics::CaseSensitive
    };
    ProjectionSelection::selected(paths, subtrees, semantics)
        .map(Some)
        .map_err(|e| CliError::usage(e.to_string()))
}
