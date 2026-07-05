use layrs_cli::args::{Cli, CliCommand, parse_args, usage};
use layrs_cli::engine::{CliError, ClientCoreEngine, DiffRequest, EngineContext, ExitCode};
use layrs_cli::render::{self, Rendered};
use std::io::IsTerminal;

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

    let json = cli.globals.json;
    match run(cli) {
        Ok(rendered) => {
            if json {
                println!("{}", render::ok_json(rendered.data));
            } else {
                println!("{}", rendered.human);
            }
            std::process::exit(ExitCode::Success.as_i32());
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

    let color = color_enabled(cli.globals.no_color, cli.globals.json);
    let engine = ClientCoreEngine::new(EngineContext {
        space: cli.globals.space,
    });

    let rendered = match cli.command {
        CliCommand::Help => unreachable!("handled above"),
        CliCommand::Init { name, path } => {
            render::init(engine.init_local_space(&name, path.as_deref())?)
        }
        CliCommand::Step => render::step(engine.save_local_step()?),
        CliCommand::Diff {
            step_id,
            stat,
            name_only,
            window,
            wrap,
        } => render::diff(
            engine.diff(DiffRequest {
                step_id: step_id.as_deref(),
                stat,
                name_only,
                window: window.as_ref(),
                wrap,
            })?,
            color,
        ),
        CliCommand::Timeline { limit } => render::timeline(engine.timeline(limit)?),
        CliCommand::Publish { workspace } => render::publish(engine.publish(workspace.as_deref())?),
        CliCommand::Sync { workspace } => render::sync(engine.sync(workspace.as_deref())?),
        CliCommand::Receive => render::receive(engine.receive()?),
        CliCommand::Compact => render::compact(engine.compact()?),
        CliCommand::Status => render::status(engine.status()?),
        CliCommand::Login { endpoint } => render::login(engine.login(endpoint.as_deref())?),
        CliCommand::Whoami => render::whoami(engine.whoami()?),
        CliCommand::Logout => render::logout(engine.logout()?),
        CliCommand::Spaces => render::spaces(engine.spaces()?),
        CliCommand::Layers => render::layers(engine.layers()?),
        CliCommand::LayerUse { name_or_id } => {
            render::layer(engine.layer_use(&name_or_id)?, "Using")
        }
        CliCommand::LayerCreate { name } => render::layer(engine.layer_create(&name)?, "Created"),
        CliCommand::LayerDelete { name_or_id, yes } => {
            render::layer_deleted(engine.layer_delete(&name_or_id, yes)?)
        }
        CliCommand::LayerDisconnect { name_or_id, yes } => {
            render::layer_action(engine.layer_disconnect(&name_or_id, yes)?)
        }
        CliCommand::LayerClearSteps { name_or_id, yes } => {
            render::layer_action(engine.layer_clear_steps(&name_or_id, yes)?)
        }
        CliCommand::Weave {
            source,
            target,
            preview,
        } => render::weave(engine.weave(&source, &target, preview)?),
        CliCommand::WeaveParent { preview } => render::weave(engine.weave_parent(preview)?),
        CliCommand::WeaveStatus => render::weave_status(engine.weave_status()?),
        CliCommand::WeaveConflicts => render::weave_conflicts(engine.weave_conflicts()?),
        CliCommand::WeaveResolve {
            path,
            resolution,
            file,
        } => render::weave(engine.weave_resolve(&path, &resolution, file.as_deref())?),
        CliCommand::WeaveContinue => render::weave(engine.weave_continue()?),
        CliCommand::WeaveAbort => render::weave(engine.weave_abort()?),
    }
    .map_err(CliError::runtime)?;

    Ok(rendered)
}

fn color_enabled(no_color: bool, json: bool) -> bool {
    !no_color && !json && std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}
