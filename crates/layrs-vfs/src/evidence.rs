use layrs_core::scm::{FileId, RepoPath};

/// An observation, not an atomic Worktree snapshot. The caller must quiesce
/// writers and reconcile the entire namespace before publishing a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureEvidence {
    /// CFAPI identity, size and in-sync state establish this partial file's base.
    Immutable {
        file_id: FileId,
        size: u64,
        original_path: RepoPath,
    },
    /// Exact hashing is required, including for fully resident clean files.
    NeedsHash { base: Option<FileId>, size: u64 },
    /// Absent in an ALWAYS_FULL namespace (not a nonresident placeholder).
    Deleted { base: FileId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderEvidence {
    pub on_disk_bytes: u64,
    pub modified_bytes: u64,
    pub in_sync: bool,
    pub partial: bool,
    pub file_id: i64,
    pub sync_root_file_id: i64,
}

pub fn classify_placeholder(
    entry: &crate::CatalogEntry,
    size: u64,
    info: &PlaceholderEvidence,
) -> CaptureEvidence {
    if info.in_sync && info.modified_bytes == 0 && info.partial && size == entry.size {
        CaptureEvidence::Immutable {
            file_id: entry.file_id.clone(),
            size,
            original_path: entry.path.clone(),
        }
    } else {
        CaptureEvidence::NeedsHash {
            base: Some(entry.file_id.clone()),
            size,
        }
    }
}

/// Holds a read-access handle denying writes/deletes while evidence is consumed.
/// The provider opens the reparse point and performs metadata queries only;
/// read access is necessary for Windows sharing checks, not content hydration.
/// Dropping it releases that per-file guard. Namespace-level atomicity is the
/// upper layer's responsibility. This handle intentionally cannot read bytes.
#[derive(Debug)]
pub struct Inspection {
    pub path: RepoPath,
    pub evidence: CaptureEvidence,
    pub placeholder: Option<PlaceholderEvidence>,
    pub(crate) _guard: Option<std::fs::File>,
    pub(crate) _parents: Vec<std::fs::File>,
}
