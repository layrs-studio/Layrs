//! Persistent cache of checkout projections (council r4, B2), keyed by root
//! Tree: `status`, `step` and transitions need not re-read a Step's Trees and
//! file lengths on every command.
//!
//! A projection is a pure function of its root Tree (content-addressed, and
//! each FileId fixes its length), so a cached value never goes stale and a
//! capture may record it before its Step is admitted. Callers still check
//! the Step's admission; the cache only answers what the Tree projects to. It is derived data, like the change
//! index (ADR 0025) or git's index and commit-graph:
//!
//! - It is written atomically (rename) and never synced.
//! - A file whose binding, structure or trailing blake3 checksum is wrong is
//!   ignored and the projection is read from the Store again.
//! - Callers sample hits against a recomputation and delete a wrong file.
//!
//! The format, in order:
//!
//! 1. `LYRSPRJ1` and a format version.
//! 2. The binding: the root Tree id, then the projection's own Tree id (as
//!    `projection_tree_id` computes it; it differs from the root only when
//!    the root holds what a checkout cannot represent, such as Tombstones).
//! 3. The entries, sorted by path (ADR 0028, compact). Each holds its path as
//!    the byte count it shares with the previous path plus its own suffix
//!    (varints), a kind byte (file, executable file or symlink), and then
//!    either the 32-byte FileId digest and the length (varint), or the
//!    target (symlink).
//! 4. The blake3 checksum of everything before it.
use super::checkout::{CheckoutProjectionContent, CheckoutProjectionEntry};
use layrs_core::scm::{FileId, TreeId};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"LYRSPRJ1";
const VERSION: u32 = 3;
const DIRECTORY: &str = "projection-cache";
/// Bound on what a corrupt file can make a reader allocate.
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const KIND_FILE: u8 = 1;
const KIND_SYMLINK: u8 = 2;
const KIND_EXECUTABLE: u8 = 3;

fn file(store_root: &Path, tree: &TreeId) -> PathBuf {
    let name = blake3::hash(tree.as_str().as_bytes()).to_hex();
    store_root
        .join("metadata")
        .join(DIRECTORY)
        .join(format!("{name}.bin"))
}

/// The cached projection of `root_tree`, if a file exists whose binding,
/// structure and checksum are all intact.
pub fn load(store_root: &Path, root_tree: &TreeId) -> Option<Vec<CheckoutProjectionEntry>> {
    let _span = crate::perf::span("projection_cache.load");
    let path = file(store_root, root_tree);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES || metadata.len() < 40 {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let (body, trailer) = bytes.split_at(bytes.len() - 32);
    if blake3::hash(body).as_bytes() != trailer {
        return None;
    }
    let mut reader = Reader { bytes: body, at: 0 };
    if reader.take(8)? != MAGIC || reader.u32()? != VERSION {
        return None;
    }
    if reader.string()? != root_tree.as_str() {
        return None;
    }
    let projection_tree = TreeId::parse(reader.string()?).ok()?;
    let count = usize::try_from(reader.u32()?).ok()?;
    if count > MAX_ENTRIES {
        return None;
    }
    let mut entries = Vec::with_capacity(count);
    let mut previous: Vec<u8> = Vec::new();
    for _ in 0..count {
        let shared = usize::try_from(reader.varint()?).ok()?;
        let suffix = usize::try_from(reader.varint()?).ok()?;
        if shared > previous.len() {
            return None;
        }
        previous.truncate(shared);
        previous.extend_from_slice(reader.take(suffix)?);
        let path = String::from_utf8(previous.clone()).ok()?;
        let content = match reader.u8()? {
            kind @ (KIND_FILE | KIND_EXECUTABLE) => {
                let digest: [u8; 32] = reader.take(32)?.try_into().ok()?;
                let file_id = FileId::parse(format!(
                    "blake3:{}",
                    blake3::Hash::from_bytes(digest).to_hex()
                ))
                .ok()?;
                CheckoutProjectionContent::File {
                    file_id,
                    byte_length: reader.varint()?,
                    executable: kind == KIND_EXECUTABLE,
                }
            }
            KIND_SYMLINK => CheckoutProjectionContent::Symlink {
                target: reader.string()?.to_owned(),
            },
            _ => return None,
        };
        entries.push(CheckoutProjectionEntry { path, content });
    }
    if reader.at != body.len() || !well_formed(&entries) {
        return None;
    }
    super::checkout::remember_projection_tree_id(&entries, &projection_tree);
    Some(entries)
}

/// Records the projection of `root_tree`. Best effort: a cache that cannot
/// be written only makes the next command read the Store.
pub fn save(
    store_root: &Path,
    root_tree: &TreeId,
    projection_tree: &TreeId,
    entries: &[CheckoutProjectionEntry],
) {
    let _span = crate::perf::span("projection_cache.save");
    if entries.len() > MAX_ENTRIES || !well_formed(entries) {
        return;
    }
    let mut body = Vec::with_capacity(64 + entries.len() * 48);
    body.extend_from_slice(MAGIC);
    body.extend_from_slice(&VERSION.to_le_bytes());
    put_string(&mut body, root_tree.as_str());
    put_string(&mut body, projection_tree.as_str());
    body.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let mut previous: &[u8] = &[];
    for entry in entries {
        let path = entry.path.as_bytes();
        let shared = path
            .iter()
            .zip(previous)
            .take_while(|(a, b)| a == b)
            .count();
        put_varint(&mut body, shared as u64);
        put_varint(&mut body, (path.len() - shared) as u64);
        body.extend_from_slice(&path[shared..]);
        previous = path;
        match &entry.content {
            CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            } => {
                let Some(digest) = file_id
                    .as_str()
                    .strip_prefix("blake3:")
                    .and_then(|hex| blake3::Hash::from_hex(hex).ok())
                else {
                    return;
                };
                body.push(if *executable {
                    KIND_EXECUTABLE
                } else {
                    KIND_FILE
                });
                body.extend_from_slice(digest.as_bytes());
                put_varint(&mut body, *byte_length);
            }
            CheckoutProjectionContent::Symlink { target } => {
                body.push(KIND_SYMLINK);
                put_string(&mut body, target);
            }
        }
    }
    let checksum = blake3::hash(&body);
    body.extend_from_slice(checksum.as_bytes());
    let path = file(store_root, root_tree);
    let _ = (|| -> io::Result<()> {
        let directory = path.parent().ok_or_else(|| io::Error::other("no parent"))?;
        fs::create_dir_all(directory)?;
        let staged = directory.join(format!(
            ".{}.{}.tmp",
            path.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        {
            let mut file = fs::File::create(&staged)?;
            file.write_all(&body)?;
        }
        fs::rename(&staged, &path).inspect_err(|_| {
            let _ = fs::remove_file(&staged);
        })
    })();
}

/// Forgets a cached projection found to differ from its recomputation.
pub fn remove(store_root: &Path, root_tree: &TreeId) {
    let _ = fs::remove_file(file(store_root, root_tree));
}

/// Structure a projection always has: non-empty relative paths without empty
/// components, strictly sorted (hence unique), and no path both a leaf and a
/// directory prefix of another leaf.
fn well_formed(entries: &[CheckoutProjectionEntry]) -> bool {
    let mut previous: Option<&str> = None;
    let leaves: std::collections::HashSet<&str> =
        entries.iter().map(|entry| entry.path.as_str()).collect();
    for entry in entries {
        let path = entry.path.as_str();
        if path.is_empty()
            || path.starts_with('/')
            || path.ends_with('/')
            || path.contains("//")
            || path.contains('\\')
            || previous.is_some_and(|previous| previous >= path)
            || path
                .match_indices('/')
                .any(|(at, _)| leaves.contains(&path[..at]))
        {
            return false;
        }
        previous = Some(path);
    }
    true
}

fn put_varint(body: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        body.push((value as u8) | 0x80);
        value >>= 7;
    }
    body.push(value as u8);
}

fn put_string(body: &mut Vec<u8>, value: &str) {
    body.extend_from_slice(&(value.len() as u32).to_le_bytes());
    body.extend_from_slice(value.as_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    /// Minimal varints only: one input, one projection.
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f).checked_shl(shift)?;
            if byte & 0x80 == 0 {
                return (shift == 0 || byte != 0).then_some(value);
            }
        }
        None
    }
    fn string(&mut self) -> Option<&'a str> {
        let length = usize::try_from(self.u32()?).ok()?;
        std::str::from_utf8(self.take(length)?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<CheckoutProjectionEntry> {
        let file = |path: &str, data: &[u8]| CheckoutProjectionEntry {
            path: path.into(),
            content: CheckoutProjectionContent::File {
                file_id: FileId::from_bytes(data),
                byte_length: data.len() as u64,
                executable: false,
            },
        };
        vec![
            file("a.txt", b"a"),
            file("d/b.txt", b"bb"),
            CheckoutProjectionEntry {
                path: "d/link".into(),
                content: CheckoutProjectionContent::Symlink {
                    target: "b.txt".into(),
                },
            },
            file("d/sub/c.txt", b"ccc"),
            CheckoutProjectionEntry {
                path: "d/sub/run.sh".into(),
                content: CheckoutProjectionContent::File {
                    file_id: FileId::from_bytes(&[7; 300]),
                    byte_length: 300,
                    executable: true,
                },
            },
        ]
    }

    #[test]
    fn roundtrip_binding_and_corruption_fall_back_to_none() {
        let root =
            std::env::temp_dir().join(format!("layrs-projection-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let tree = TreeId::parse(format!("blake3:{}", blake3::hash(b"tree"))).unwrap();
        save(&root, &tree, &tree, &entries());
        assert_eq!(load(&root, &tree), Some(entries()));
        // Another Tree never reads this file as its own, even renamed.
        let other = TreeId::parse(format!("blake3:{}", blake3::hash(b"other"))).unwrap();
        fs::copy(file(&root, &tree), file(&root, &other)).unwrap();
        assert_eq!(load(&root, &other), None);
        // Any flipped byte fails the checksum.
        let path = file(&root, &tree);
        let mut bytes = fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 1;
        fs::write(&path, &bytes).unwrap();
        assert_eq!(load(&root, &tree), None);
        remove(&root, &tree);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn malformed_projections_are_never_written_or_read() {
        let mut unsorted = entries();
        unsorted.swap(0, 1);
        assert!(!well_formed(&unsorted));
        let mut leaf_and_directory = entries();
        leaf_and_directory.insert(
            1,
            CheckoutProjectionEntry {
                path: "d".into(),
                content: CheckoutProjectionContent::Symlink { target: "x".into() },
            },
        );
        assert!(!well_formed(&leaf_and_directory));
        assert!(well_formed(&entries()));
    }
}
