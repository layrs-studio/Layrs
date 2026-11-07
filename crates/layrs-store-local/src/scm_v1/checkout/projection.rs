use super::*;
use layrs_core::scm::{FileId, Symlink, TreeEntryTarget, TreeId};

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

/// Content fingerprint of a projection (every path, kind, id, flag and
/// length, unambiguously framed): the key of `projection_tree_id`'s memo.
fn projection_fingerprint(entries: &[CheckoutProjectionEntry]) -> [u8; 32] {
    // The same byte stream, hashed in large blocks: one `update` per tiny
    // field costs more than hashing the whole stream.
    const BLOCK: usize = 64 * 1024;
    let mut hasher = blake3::Hasher::new();
    let mut stream = Vec::with_capacity(BLOCK + 512);
    stream.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for entry in entries {
        stream.extend_from_slice(&(entry.path.len() as u64).to_le_bytes());
        stream.extend_from_slice(entry.path.as_bytes());
        match &entry.content {
            CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            } => {
                stream.extend_from_slice(&[1, u8::from(*executable)]);
                stream.extend_from_slice(&(file_id.as_str().len() as u64).to_le_bytes());
                stream.extend_from_slice(file_id.as_str().as_bytes());
                stream.extend_from_slice(&byte_length.to_le_bytes());
            }
            CheckoutProjectionContent::Symlink { target } => {
                stream.push(2);
                stream.extend_from_slice(&(target.len() as u64).to_le_bytes());
                stream.extend_from_slice(target.as_bytes());
            }
        }
        if stream.len() >= BLOCK {
            hasher.update(&stream);
            stream.clear();
        }
    }
    hasher.update(&stream);
    *hasher.finalize().as_bytes()
}

fn projection_memo() -> &'static std::sync::Mutex<std::collections::HashMap<[u8; 32], TreeId>> {
    static MEMO: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<[u8; 32], TreeId>>,
    > = std::sync::OnceLock::new();
    MEMO.get_or_init(Default::default)
}

/// Records that `entries` is a valid projection whose Tree is `tree` (as
/// `projection_tree_id` would compute), e.g. from a projection cache entry
/// written with its computed Tree. Bounded; forgotten when the process ends.
pub fn remember_projection_tree_id(entries: &[CheckoutProjectionEntry], tree: &TreeId) {
    let key = projection_fingerprint(entries);
    if let Ok(mut memo) = projection_memo().lock() {
        if memo.len() >= 64 {
            memo.clear();
        }
        memo.insert(key, tree.clone());
    }
}

/// Same physical projection as entries_tree_id, with separate logical byte caps.
/// Empty directories and tombstones are absent from this physical projection.
/// Memoized by content within a process: a transition validates the same
/// before/after projections several times.
pub fn projection_tree_id(entries: &[CheckoutProjectionEntry]) -> io::Result<TreeId> {
    let key = projection_fingerprint(entries);
    if let Some(tree) = projection_memo()
        .lock()
        .ok()
        .and_then(|memo| memo.get(&key).cloned())
    {
        crate::perf::count("projection.tree_id_memo", 1);
        return Ok(tree);
    }
    let tree = projection_tree_id_uncached(entries)?;
    remember_projection_tree_id(entries, &tree);
    Ok(tree)
}

fn projection_tree_id_uncached(entries: &[CheckoutProjectionEntry]) -> io::Result<TreeId> {
    let _span = crate::perf::span("projection.tree_id");
    if entries.len() > 100_000 {
        return Err(invalid("checkout entry limit exceeded"));
    }
    validate_streaming_projection(entries)?;
    let mut root = std::collections::BTreeMap::new();
    let mut total = 0u64;
    for entry in entries {
        // Validated above; a valid path is its own canonical form.
        let parts: Vec<&str> = entry.path.split('/').collect();
        if parts.len() > 128 {
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
        // Sizes are logical here (ADR 0027): content streams elsewhere.
        total = total
            .checked_add(length)
            .ok_or_else(|| invalid("checkout size overflow"))?;
        insert_projection(&mut root, &parts, Projection::Leaf(target, executable))?;
    }
    hash_projection(root)
}

#[cfg(test)]
mod bench_tree_id {
    use super::*;

    fn sample(count: usize) -> Vec<CheckoutProjectionEntry> {
        let mut entries: Vec<_> = (0..count)
            .map(|i| CheckoutProjectionEntry {
                path: format!("d{}/f{i}.txt", i % 10),
                content: CheckoutProjectionContent::File {
                    file_id: FileId::from_bytes(format!("line {i}").as_bytes()),
                    byte_length: 10,
                    executable: false,
                },
            })
            .collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    #[test]
    #[ignore = "timing probe, run with --ignored --nocapture"]
    fn timing_projection_tree_id() {
        let entries = sample(10_000);
        for _ in 0..3 {
            let start = std::time::Instant::now();
            validate_streaming_projection(&entries).unwrap();
            let validate = start.elapsed();
            let start = std::time::Instant::now();
            for entry in &entries {
                layrs_core::scm::RepoPath::parse(&entry.path).unwrap();
            }
            let parse = start.elapsed();
            let start = std::time::Instant::now();
            projection_tree_id_uncached(&entries).unwrap();
            let total = start.elapsed();
            let start = std::time::Instant::now();
            projection_fingerprint(&entries);
            let fingerprint = start.elapsed();
            eprintln!(
                "validate {validate:?} parse {parse:?} uncached total {total:?} fingerprint {fingerprint:?}"
            );
        }
    }
}
