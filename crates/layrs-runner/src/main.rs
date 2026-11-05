use layrs_core::scm::{LayerId, SpaceId, StepId};
use layrs_runner::{Completion, Job, Recipe, Result};
fn main() {
    if let Err(error) = run() {
        eprintln!("layrs-runner: {error}");
        std::process::exit(1)
    }
}
fn read(path: impl AsRef<std::path::Path>, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = vec![];
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("input file exceeds limit".into());
    }
    Ok(bytes)
}
fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 6 && !(args.len() == 2 && args[0] == "retry") {
        return Err("usage: layrs-runner ORIGIN SPACE STEP LAYER RECIPE_JSON NEW_ABSOLUTE_RUN_DIRECTORY | layrs-runner retry COMPLETION_JSON; credentials: LAYRS_RUNNER_TOKEN_FILE, optional LAYRS_CA_FILE".into());
    }
    let token = String::from_utf8(read(
        std::env::var("LAYRS_RUNNER_TOKEN_FILE")
            .map_err(|_| "LAYRS_RUNNER_TOKEN_FILE is required")?,
        8192,
    )?)?;
    let token = token.trim();
    if token.is_empty() {
        return Err("empty runner token".into());
    }
    let ca = std::env::var_os("LAYRS_CA_FILE")
        .map(|p| read(p, 1024 * 1024))
        .transpose()?;
    if args[0] == "retry" {
        let completion: Completion = serde_json::from_slice(&read(&args[1], 1024 * 1024)?)?;
        layrs_runner::submit(&completion, token, ca.as_deref())?;
        println!("callback accepted");
        return Ok(());
    }
    let recipe: Recipe = serde_json::from_slice(&read(&args[4], 65536)?)?;
    let result = layrs_runner::run(
        Job {
            origin: args[0].clone(),
            space: SpaceId::parse(&args[1])?,
            step: StepId::parse(&args[2])?,
            layer: LayerId::parse(&args[3])?,
            recipe,
            directory: args[5].clone().into(),
            ca_pem: ca,
        },
        token,
    )?;
    println!(
        "{}",
        serde_json::json!({"runId":result.run_id,"stepId":result.step_id,"outcome":result.outcome})
    );
    if result.outcome != "passed" {
        std::process::exit(2)
    }
    Ok(())
}
