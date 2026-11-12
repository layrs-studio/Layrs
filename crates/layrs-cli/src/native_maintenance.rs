use crate::{error::CliError, render::Rendered};
use layrs_client_core::native_scm::NativeWorkspace;
use layrs_store_local::scm_v1::{MaintenanceOutcome, maintain as maintain_store};
use std::path::Path;

/// One maintenance pass over the Store at `store_root` (council s17, B2b):
/// consolidates the pack files batches leave out, building outside the
/// Store lock.
pub(super) fn maintain(store_root: &Path) -> Result<Rendered, CliError> {
    let outcome = maintain_store(store_root).map_err(CliError::runtime)?;
    let (human, status, packs) = match outcome {
        MaintenanceOutcome::Busy => ("Another maintenance pass is running.".to_owned(), "busy", 0),
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
