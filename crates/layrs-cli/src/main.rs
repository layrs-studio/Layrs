use layrs_cli::args::{Cli, CliCommand, parse_args, usage};
use layrs_cli::error::{CliError, ExitCode};
use layrs_cli::render::{self, Rendered};

fn main() {
    let raw_args = std::env::args().skip(1).collect::<Vec<_>>();
    let wants_json = raw_args.iter().any(|arg| arg == "--json");

    let cli = match parse_args(raw_args) {
        Ok(cli) => cli,
        Err(error) => {
            if wants_json {
                println!("{}", render::error_json(error.message()));
            } else {
                eprintln!("{error}");
                eprintln!("{}", usage());
            }
            std::process::exit(ExitCode::Usage.as_i32());
        }
    };

    if let Some(exit_code) = layrs_cli::native_vfs::run_foreground(&cli) {
        std::process::exit(exit_code.as_i32());
    }
    let json = cli.globals.json;
    let diagnosis = matches!(cli.command, CliCommand::Doctor);
    match run(cli) {
        Ok(rendered) => {
            let exit_code = if diagnosis
                && (rendered.data["clean"] != true || rendered.data["complete"] != true)
            {
                ExitCode::Runtime
            } else {
                ExitCode::Success
            };
            if json {
                println!("{}", render::ok_json(rendered.data));
            } else {
                println!("{}", rendered.human);
            }
            std::process::exit(exit_code.as_i32());
        }
        Err(error) => {
            if json {
                println!("{}", render::error_json(&error.message));
            } else {
                eprintln!("{error}");
            }
            std::process::exit(error.exit_code.as_i32());
        }
    }
}

fn run(cli: Cli) -> Result<Rendered, CliError> {
    if cli.command == CliCommand::Help {
        return Ok(Rendered {
            human: usage().to_string(),
            data: serde_json::json!({ "usage": usage() }),
        });
    }
    #[cfg(windows)]
    if matches!(
        &cli.command,
        CliCommand::PartialProvider(layrs_cli::args::PartialProviderCommand::Status { .. })
    ) {
        // This executable owns the audited startup/native closure. Keep this
        // assertion before the route's first SQLite access and out of libraries:
        // embedding the client does not qualify another host process.
        layrs_store_local::projection::declare_provider_sqlite_process_profile(
            layrs_store_local::projection::ProviderSqliteProcessDeclaration::AuditedLayrsHostV1,
        )
        .map_err(|error| CliError::runtime(format!("process_profile_unavailable: {error}")))?;
    }
    layrs_cli::native::try_run(&cli).unwrap_or_else(|| {
        Err(CliError::runtime(
            "No Layrs project found here. Run `layrs init NAME` to create one.",
        ))
    })
}
