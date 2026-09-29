use crate::{error::CliError, render::Rendered};
use layrs_store_local::scm_v1::compaction::{compact_metadata, resume_metadata_compaction};
use std::path::Path;

/// Discovery deliberately does not open the Store: a durable maintenance
/// journal fences normal opens until this explicit resume command finishes.
pub(super) fn compact(start: &Path, resume: bool) -> Result<Rendered, CliError> {
    let start = std::path::absolute(start).map_err(super::error)?;
    let mut root = None;
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(".layrs");
        if candidate.try_exists().map_err(super::error)? {
            root = Some(candidate);
            break;
        }
    }
    let root = root.ok_or_else(|| CliError::runtime("No native .layrs checkout found."))?;
    let report = if resume {
        resume_metadata_compaction(root)
    } else {
        compact_metadata(root)
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
