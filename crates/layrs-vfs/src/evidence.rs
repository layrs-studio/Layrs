use layrs_core::scm::{FileId, RepoPath};

/// An observation, not an atomic Worktree snapshot. The caller must quiesce
/// writers and reconcile the entire namespace before publishing a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureEvidence {
    /// CFAPI identity, size and in-sync state establish this file's base,
    /// resident or not: TRACK_ALL clears in-sync on any change (ADR 0025).
    Immutable {
        file_id: FileId,
        size: u64,
        original_path: RepoPath,
    },
    /// Exact hashing is required (new, modified or relocated content).
    NeedsHash { base: Option<FileId>, size: u64 },
    /// Absent in an ALWAYS_FULL namespace (not a nonresident placeholder).
    Deleted { base: FileId },
    /// Content the caller proved earlier and vouches for from the directory
    /// listing alone (a change index, ADR 0025). The file was not opened.
    Vouched { file_id: FileId, size: u64 },
}

/// What a directory enumeration reports about a regular file, without
/// opening it. Times are Windows FILETIME values (100 ns units since 1601).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListedFile {
    pub native_index: u64,
    pub size: u64,
    pub attributes: u32,
    pub last_write: i64,
    pub creation: i64,
    pub change: i64,
    pub placeholder: Option<ListedPlaceholder>,
}

impl ListedFile {
    /// Equal for change detection: same file, size, write time and
    /// placeholder state. Hydration (which only moves the change time and
    /// residency of an in-sync placeholder) is not a change.
    pub fn same_version(&self, other: &Self) -> bool {
        self.native_index == other.native_index
            && self.size == other.size
            && self.last_write == other.last_write
            && self.creation == other.creation
            && self.placeholder.map(|p| p.in_sync) == other.placeholder.map(|p| p.in_sync)
    }
}

/// CFAPI state of a listed placeholder, from its attributes and reparse tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListedPlaceholder {
    /// Cleared by the platform (TRACK_ALL) on any data or metadata change.
    pub in_sync: bool,
    pub partial: bool,
}

/// A trust hook's verdict on a listed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vouch {
    /// The in-sync placeholder created for this path: its catalog base. The
    /// provider still requires it in sync, at its catalog path and size.
    Base,
    /// Content proven earlier by the caller, unchanged since.
    Content(FileId),
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
    if info.in_sync && info.modified_bytes == 0 && size == entry.size {
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
    /// Listing facts of an entry admitted without a guard; revalidation
    /// requires the same version at the same path.
    pub(crate) listed: Option<ListedFile>,
}
