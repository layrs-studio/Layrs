use crate::{error::CliError, render::Rendered};
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_store_local::scm_v1::{MaintenanceOutcome, maintain as maintain_store};
use std::path::Path;

/// Hidden first argument of the detached maintenance pass (council s17,
/// B2c).
pub const MAINTENANCE_COMMAND: &str = "__layrs-maintenance";

/// The most passes one maintenance run makes: each consolidates one group.
const MAX_PASSES: usize = 8;

/// What maintenance passes did: the last outcome, the pack files
/// consolidated in all, and the failure that ended them after some did.
struct Passes {
    last: MaintenanceOutcome,
    consolidated: usize,
    failure: Option<String>,
}

/// Passes over the Store at `store_root` while they consolidate (or a
/// candidate moved meanwhile), at most `MAX_PASSES`.
fn passes(store_root: &Path) -> Result<Passes, String> {
    let mut consolidated = 0;
    let mut last = MaintenanceOutcome::NothingToDo;
    for _ in 0..MAX_PASSES {
        last = match maintain_store(store_root) {
            Ok(outcome) => outcome,
            // What earlier passes consolidated stays done; the failure is
            // recorded by the Store and reported.
            Err(error) if consolidated > 0 => {
                return Ok(Passes {
                    last,
                    consolidated,
                    failure: Some(error),
                });
            }
            Err(error) => return Err(error),
        };
        match last {
            MaintenanceOutcome::Consolidated { packs } => consolidated += packs,
            MaintenanceOutcome::Abandoned => {}
            MaintenanceOutcome::Busy | MaintenanceOutcome::NothingToDo => break,
        }
    }
    Ok(Passes {
        last,
        consolidated,
        failure: None,
    })
}

/// Starts, detached, a maintenance run on each Store a batch of this
/// command found one due on (council s17, B2c), as git starts its
/// automatic maintenance: unless `LAYRS_AUTO_MAINTENANCE=0` or a pass is
/// running there. Best effort: a failure to start changes nothing (the
/// next batch notes it due again).
pub fn start_due() {
    let roots = layrs_store_local::scm_v1::take_maintenance_due();
    if roots.is_empty()
        || std::env::var_os("LAYRS_AUTO_MAINTENANCE").is_some_and(|value| value == "0")
    {
        return;
    }
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    for root in roots {
        if layrs_store_local::scm_v1::maintenance_running(&root)
            || layrs_store_local::scm_v1::maintenance_paused(&root)
        {
            continue;
        }
        let mut command = std::process::Command::new(&executable);
        // Its own working directory: it holds no checkout folder open.
        command
            .arg(MAINTENANCE_COMMAND)
            .arg(&root)
            .current_dir(std::env::temp_dir())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: outlives this console.
        #[cfg(windows)]
        const FLAGS: u32 = 0x0800_0000 | 0x0000_0200;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_BREAKAWAY_FROM_JOB too: a kill-on-close job (a CI runner,
            // an IDE task) would end the pass with its command. Refused
            // unless the job allows it, then retried without (council s19 r2).
            command.creation_flags(FLAGS | 0x0100_0000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        crate::native_vfs::keep_standard_handles_private();
        let started = command.spawn().is_ok();
        #[cfg(windows)]
        let started = started || {
            use std::os::windows::process::CommandExt;
            command.creation_flags(FLAGS);
            command.spawn().is_ok()
        };
        if started {
            layrs_store_local::perf::count("cli.maintenance_started", 1);
        }
    }
}

/// The detached maintenance run on the Store at `args[0]`: 0, else 1 when
/// it failed (2 for bad arguments).
pub fn run_detached(args: &[String]) -> i32 {
    let [root] = args else {
        return 2;
    };
    match passes(Path::new(root)) {
        Ok(Passes { failure: None, .. }) => 0,
        Ok(_) | Err(_) => 1,
    }
}

/// Maintenance passes over the Store at `store_root` (council s17, B2b):
/// consolidate the pack files batches leave out, building outside the
/// Store lock.
pub(super) fn maintain(store_root: &Path) -> Result<Rendered, CliError> {
    let Passes {
        last: outcome,
        consolidated,
        failure,
    } = passes(store_root).map_err(CliError::runtime)?;
    if consolidated > 0 {
        let mut human = format!("Consolidated {consolidated} pack files.");
        if let Some(failure) = &failure {
            human.push_str(&format!("\nA later pass failed: {failure}"));
        }
        return Ok(Rendered {
            human,
            data: serde_json::json!({
                "status": "consolidated",
                "packs": consolidated,
                "warning": failure,
            }),
        });
    }
    let (human, status, packs) = match outcome {
        MaintenanceOutcome::Busy => (
            "Busy: another maintenance pass or a long command holds the Store. Run it again."
                .to_owned(),
            "busy",
            0,
        ),
        MaintenanceOutcome::NothingToDo => ("Nothing to consolidate.".to_owned(), "nothingToDo", 0),
        MaintenanceOutcome::Abandoned => (
            "Pack files changed during maintenance; nothing published. Run it again.".to_owned(),
            "abandoned",
            0,
        ),
        MaintenanceOutcome::Consolidated { packs } => (
            format!("Consolidated {packs} pack files into one."),
            "consolidated",
            packs,
        ),
    };
    Ok(Rendered {
        human,
        data: serde_json::json!({ "status": status, "packs": packs }),
    })
}

/// Discovery uses audit validation: a durable maintenance journal fences
/// normal Store admission until this explicit resume command finishes.
pub(super) fn compact(start: &Path, resume: bool) -> Result<Rendered, CliError> {
    let start = std::path::absolute(start).map_err(super::error)?;
    let mut checkout = None;
    for ancestor in start.ancestors() {
        match std::fs::symlink_metadata(ancestor.join(".layrs")) {
            Ok(_) => {
                checkout = Some(ancestor);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(super::error(error)),
        }
    }
    let checkout = checkout.ok_or_else(|| CliError::runtime("No native .layrs checkout found."))?;
    let store = NativeWorkspace::audit_worktree_store(checkout).map_err(super::error)?;
    let report = if resume {
        store.resume_metadata_compaction()
    } else {
        store.compact_metadata()
    }
    .map_err(super::error)?;
    Ok(Rendered {
        human: format!(
            "Metadata compacted: {} -> {} bytes. {} rows retained.\nHistory preserved. Backup: {}\nRestart any active history traversal.",
            report.before_bytes,
            report.after_bytes,
            report.retained_rows,
            report.backup_directory.display()
        ),
        data: serde_json::to_value(report).map_err(super::error)?,
    })
}
