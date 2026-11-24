use crate::{error::CliError, render::Rendered};
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_store_local::scm_v1::{
    MaintenanceOutcome, MetadataRewrite, ProjectionTrim, maintain as maintain_store,
    maintain_explicit, rewrite_metadata, trim_projections,
};
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
/// candidate moved meanwhile), at most `MAX_PASSES`; `explicit` passes
/// consolidate every pack into one first in format 8 (council s36, D).
fn passes(store_root: &Path, explicit: bool) -> Result<Passes, String> {
    let mut consolidated = 0;
    let mut last = MaintenanceOutcome::NothingToDo;
    for _ in 0..MAX_PASSES {
        let pass = if explicit {
            maintain_explicit(store_root)
        } else {
            maintain_store(store_root)
        };
        last = match pass {
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
    let passes = passes(Path::new(root), false);
    let consolidated = passes.as_ref().map_or(0, |passes| passes.consolidated);
    let code = match passes {
        Ok(Passes { failure: None, .. }) => 0,
        Ok(_) | Err(_) => 1,
    };
    // Then (format 8) the projection cache trim and the metadata rewrite,
    // bounded for automatic runs; their failures do not change the code (the
    // rewrite's is recorded by the Store). The kept projections are read
    // again only once a pass retired packs they may name files in.
    if let Ok(ProjectionTrim::Trimmed { bases, .. }) = trim_projections(Path::new(root)) {
        if consolidated > 0 {
            layrs_client_core::native_scm::warm_step_projections(Path::new(root), &bases);
        }
    }
    let _ = rewrite_metadata(Path::new(root), true);
    // Opt-in diagnostics (`LAYRS_PERF_OUT` for a process without stderr).
    layrs_store_local::perf::emit(MAINTENANCE_COMMAND);
    code
}

/// Maintenance passes over the Store at `store_root` (council s17, B2b):
/// consolidate the pack files batches leave out, building outside the
/// Store lock.
pub(super) fn maintain(store_root: &Path) -> Result<Rendered, CliError> {
    let passes = passes(store_root, true);
    let consolidated = passes.as_ref().map_or(0, |passes| passes.consolidated);
    // Format 8: the cached projections no Worktree or Room stands on are
    // removed, and once a pass retired packs the others may name files in,
    // they are read again so the next command finds them (best effort,
    // reported in the data only).
    let projections = match trim_projections(store_root) {
        Ok(ProjectionTrim::Unsupported) => None,
        Ok(ProjectionTrim::Trimmed { removed, bases }) => {
            if consolidated > 0 {
                layrs_client_core::native_scm::warm_step_projections(store_root, &bases);
            }
            Some(serde_json::json!({ "status": "trimmed", "removed": removed }))
        }
        Ok(ProjectionTrim::Skipped) => Some(serde_json::json!({ "status": "skipped" })),
        Err(error) => Some(serde_json::json!({ "status": "failed", "error": error })),
    };
    // Format 8: the metadata database is rewritten once it has grown enough
    // (any size when asked explicitly), even after a failed pass; a failure
    // is reported, not fatal. Other formats have no rewrite and no field.
    let (rewrite_human, rewrite_data) = match rewrite_metadata(store_root, false) {
        Ok(MetadataRewrite::Unsupported) => (None, None),
        Ok(MetadataRewrite::Rewritten { before, after }) => (
            Some(format!(
                "Rewrote the metadata database: {before} to {after} pages."
            )),
            Some(
                serde_json::json!({ "status": "rewritten", "beforePages": before, "afterPages": after }),
            ),
        ),
        Ok(MetadataRewrite::Busy) => (None, Some(serde_json::json!({ "status": "busy" }))),
        Ok(MetadataRewrite::NotDue | MetadataRewrite::Paused) => {
            (None, Some(serde_json::json!({ "status": "notDue" })))
        }
        Err(error) => (
            Some(format!("Rewriting the metadata database failed: {error}")),
            Some(serde_json::json!({ "status": "failed", "error": error })),
        ),
    };
    let with_rewrite = |human: String| match &rewrite_human {
        Some(line) => format!("{human}\n{line}"),
        None => human,
    };
    let Passes {
        last: outcome,
        consolidated,
        failure,
    } = passes.map_err(|error| CliError::runtime(with_rewrite(error)))?;
    let with_data = |mut data: serde_json::Value| {
        if let Some(metadata) = &rewrite_data {
            data["metadata"] = metadata.clone();
        }
        if let Some(projections) = &projections {
            data["projections"] = projections.clone();
        }
        data
    };
    if consolidated > 0 {
        let mut human = format!("Consolidated {consolidated} packs.");
        if let Some(failure) = &failure {
            human.push_str(&format!("\nA later pass failed: {failure}"));
        }
        return Ok(Rendered {
            human: with_rewrite(human),
            data: with_data(serde_json::json!({
                "status": "consolidated",
                "packs": consolidated,
                "warning": failure,
            })),
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
            format!("Consolidated {packs} packs into one."),
            "consolidated",
            packs,
        ),
    };
    Ok(Rendered {
        human: with_rewrite(human),
        data: with_data(serde_json::json!({ "status": status, "packs": packs })),
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
