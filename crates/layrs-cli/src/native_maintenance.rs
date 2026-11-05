use crate::{error::CliError, render::Rendered};
use layrs_client_core::native_scm::NativeWorkspace;
use std::path::Path;

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
