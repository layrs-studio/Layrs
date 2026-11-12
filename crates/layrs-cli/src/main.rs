use layrs_cli::args::{Cli, CliCommand, parse_args, usage};
use layrs_cli::error::{CliError, ExitCode};
use layrs_cli::render::{self, Rendered};

fn main() {
    let raw_args = std::env::args().skip(1).collect::<Vec<_>>();
    // Detached deletion of a dropped Room's folder (spawned by `room drop`).
    if raw_args.first().map(String::as_str) == Some(layrs_cli::native_room::PURGE_COMMAND) {
        std::process::exit(layrs_cli::native_room::purge_dropped(&raw_args[1..]));
    }
    // Detached maintenance pass (started after a batch found one due).
    if raw_args.first().map(String::as_str) == Some(layrs_cli::native::MAINTENANCE_COMMAND) {
        std::process::exit(layrs_cli::native::run_detached_maintenance(&raw_args[1..]));
    }
    #[cfg(windows)]
    if raw_args.first().map(String::as_str) == Some(layrs_cli::native_room::PURGE_VIRTUAL_COMMAND) {
        std::process::exit(layrs_cli::native_room::purge_dropped_virtual(
            &raw_args[1..],
        ));
    }
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
    let run_span = layrs_store_local::perf::span("cli.run");
    let result = run(cli);
    drop(run_span);
    // Opt-in diagnostics (LAYRS_PERF=1) go to stderr, never into the JSON envelope.
    layrs_store_local::perf::emit(&std::env::args().skip(1).collect::<Vec<_>>().join(" "));
    match result {
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
            layrs_cli::native::start_due();
            std::process::exit(exit_code.as_i32());
        }
        Err(error) => {
            if json {
                println!("{}", render::error_json(&error.message));
            } else {
                eprintln!("{error}");
            }
            // A command may fail after committing a batch.
            layrs_cli::native::start_due();
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
