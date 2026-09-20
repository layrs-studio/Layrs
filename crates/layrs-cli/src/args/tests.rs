use super::*;

#[test]
fn parses_init_with_name_and_path() {
    let cli =
        parse_args(["--json", "init", "Game Prototype", "--path", "D:/work/game"]).expect("parse");

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
    assert_eq!(cli.command, CliCommand::Compact { resume: false });
    assert_eq!(
        parse_args(["compact", "--resume"]).unwrap().command,
        CliCommand::Compact { resume: true }
    );
    assert!(parse_args(["compact", "--delete-history"]).is_err());
    assert!(parse_args(["compact", "extra"]).is_err());
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
    let cli =
        parse_args(["weave", "resolve", "story.txt", "--block", "2", "--theirs"]).expect("parse");

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
