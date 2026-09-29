use super::*;
use layrs_core::scm::{FileId, RepoPath, Symlink, TreeEntryTarget, TreeId};

/// Physical checkout identity without retaining file content in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckoutProjectionContent {
    File {
        file_id: FileId,
        byte_length: u64,
        executable: bool,
    },
    Symlink {
        target: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutProjectionEntry {
    pub path: String,
    pub content: CheckoutProjectionContent,
}

impl From<&CheckoutEntry> for CheckoutProjectionEntry {
    fn from(entry: &CheckoutEntry) -> Self {
        Self {
            path: entry.path.clone(),
            content: match &entry.content {
                CheckoutContent::File { bytes, executable } => CheckoutProjectionContent::File {
                    file_id: FileId::from_bytes(bytes),
                    byte_length: bytes.len() as u64,
                    executable: *executable,
                },
                CheckoutContent::Symlink { target } => CheckoutProjectionContent::Symlink {
                    target: target.clone(),
                },
            },
        }
    }
}

pub fn validate_streaming_projection(entries: &[CheckoutProjectionEntry]) -> io::Result<()> {
    // Path validation is content-independent; do not allocate placeholder file
    // buffers or duplicate the platform's case/device-name rules.
    super::super::checkout_paths::validate_paths(
        entries.iter().map(|e| e.path.as_str()),
        cfg!(windows),
    )
}

/// Same physical projection as entries_tree_id, with separate logical byte caps.
/// Empty directories and tombstones are absent from this physical projection.
pub fn projection_tree_id(entries: &[CheckoutProjectionEntry]) -> io::Result<TreeId> {
    if entries.len() > 100_000 {
        return Err(invalid("checkout entry limit exceeded"));
    }
    validate_streaming_projection(entries)?;
    let mut root = std::collections::BTreeMap::new();
    let mut total = 0u64;
    for entry in entries {
        let canonical = RepoPath::parse(&entry.path).map_err(|e| invalid(&e.to_string()))?;
        let parts: Vec<_> = canonical.as_str().split('/').collect();
        if canonical.as_str() != entry.path || parts.len() > 128 {
            return Err(invalid("invalid checkout projection path"));
        }
        let (target, executable, length) = match &entry.content {
            CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            } => (
                TreeEntryTarget::file(file_id.clone()),
                *executable,
                *byte_length,
            ),
            CheckoutProjectionContent::Symlink { target } => (
                TreeEntryTarget::symlink(
                    Symlink::new(target)
                        .map_err(|e| invalid(&e.to_string()))?
                        .id()
                        .clone(),
                ),
                false,
                target.len() as u64,
            ),
        };
        total = total
            .checked_add(length)
            .ok_or_else(|| invalid("checkout size overflow"))?;
        if length > 1024 * 1024 * 1024 || total > 4 * 1024 * 1024 * 1024 {
            return Err(invalid("streaming checkout byte limit exceeded"));
        }
        insert_projection(&mut root, &parts, Projection::Leaf(target, executable))?;
    }
    hash_projection(root)
}
