//! Read-only diagnosis remains available when normal Store admission fails.
use super::{NativeClientError, Result};
use layrs_store_local::scm_v1::{self, FsckOptions, FsckSeverity};
use serde_json::{json, Value};
use std::path::Path;

pub fn diagnose_checkout(root: impl AsRef<Path>) -> Result<Value> {
    let root = std::path::absolute(root)?;
    let checkout = root
        .ancestors()
        .find(|path| path.join(".layrs").exists())
        .ok_or(NativeClientError::Missing("checkout .layrs directory"))?;
    scm_v1::checkout::CheckoutReader::open(checkout, None)?;
    let metadata = std::fs::symlink_metadata(checkout.join(".layrs"))?;
    let store_root = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        checkout.join(".layrs")
    } else {
        super::NativeWorkspace::resolve_store_for_audit(checkout)?
    };
    let report = scm_v1::fsck_existing(store_root, FsckOptions::default())
        .map_err(|e| NativeClientError::InvalidSnapshot(format!("integrity inspection: {e}")))?;
    Ok(json!({
        "clean": report.is_clean(), "complete": !report.is_truncated(),
        "checkedObjects": report.checked_objects, "checkedRows": report.checked_rows,
        "retentionRoots": report.gc_reachability.roots,
        "reachableObjects": report.gc_reachability.reachable_objects,
        "unreachableObjects": report.gc_reachability.unreachable_objects.len(),
        "reconstructibleProjections": report.projection_rebuild_plan.actions.len(),
        "findings": report.findings.iter().map(|finding| json!({
            "code": finding.code.as_str(),
            "severity": match finding.severity { FsckSeverity::Error => "error", FsckSeverity::Warning => "warning", FsckSeverity::Info => "info" },
            "path": finding.path, "objectId": finding.object_id, "entityId": finding.entity_id,
            "message": finding.message,
        })).collect::<Vec<_>>()
    }))
}
