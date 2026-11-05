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
fn account_commands_alias_remote() {
    let cli = parse_args(["login", "--endpoint", "https://layrs.test"]).expect("parse");

    assert_eq!(
        cli.command,
        CliCommand::Remote {
            args: vec![
                "login".to_string(),
                "--endpoint".to_string(),
                "https://layrs.test".to_string()
            ]
        }
    );
    for command in ["whoami", "logout"] {
        assert_eq!(
            parse_args([command]).expect("parse").command,
            CliCommand::Remote {
                args: vec![command.to_string()]
            }
        );
    }
}

#[test]
fn parses_conflict_continue_and_abort() {
    assert_eq!(
        parse_args(["conflict", "continue"]).expect("parse").command,
        CliCommand::ConflictContinue
    );
    assert_eq!(
        parse_args(["conflict", "abort"]).expect("parse").command,
        CliCommand::ConflictAbort
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
fn rejects_conflicting_wrap_flags() {
    let error = parse_args(["diff", "--wrap", "--no-wrap"]).expect_err("parse should fail");

    assert!(error.message().contains("either --wrap or --no-wrap"));
}
