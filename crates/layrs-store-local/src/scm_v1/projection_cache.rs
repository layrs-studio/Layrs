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
//! 3. The form: complete (0) or delta (1).
//! 4. A complete projection: its entries, sorted by path (ADR 0028, compact).
//!    Each holds its path as the byte count it shares with the previous path
//!    plus its own suffix (varints), a kind byte (file, executable file or
//!    symlink), and then either the 32-byte FileId digest and the length
//!    (varint), or the target (symlink).
//!
//!    A delta: the root Tree of the complete projection it applies to and that
//!    file's own checksum (so it applies to those exact bytes only), then the
//!    paths it removes or sets, sorted, encoded as entries where a removed
//!    path has kind 0. Successive projections differ by a few paths: each is
//!    a few bytes, not another copy of every FileId.
//! 5. The blake3 checksum of everything before it.
use super::checkout::{CheckoutProjectionContent, CheckoutProjectionEntry};
use layrs_core::scm::{FileId, TreeId};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"LYRSPRJ1";
const VERSION: u32 = 4;
const DIRECTORY: &str = "projection-cache";
/// Bound on what a corrupt file can make a reader allocate.
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const KIND_FILE: u8 = 1;
const KIND_SYMLINK: u8 = 2;
const KIND_EXECUTABLE: u8 = 3;
/// In a delta: the path is absent from the result.
const KIND_REMOVED: u8 = 0;
const FORM_COMPLETE: u8 = 0;
const FORM_DELTA: u8 = 1;
/// A delta is written while it changes at most this share of the paths.
const DELTA_MAX_PERCENT: usize = 25;

fn file(store_root: &Path, tree: &TreeId) -> PathBuf {
    let name = blake3::hash(tree.as_str().as_bytes()).to_hex();
    store_root
        .join("metadata")
        .join(DIRECTORY)
        .join(format!("{name}.bin"))
}

/// The cached projection of `root_tree`, if a file exists whose binding,
/// structure and checksum are all intact (and, for a delta, those of the
/// exact complete projection it applies to). A file that fails is removed,
/// so the next `save` writes it again.
pub fn load(store_root: &Path, root_tree: &TreeId) -> Option<Vec<CheckoutProjectionEntry>> {
    let _span = crate::perf::span("projection_cache.load");
    let path = file(store_root, root_tree);
    let loaded = read(&path, root_tree).and_then(|parsed| match parsed.form {
        Form::Complete(entries) => Some((parsed.projection_tree, entries)),
        Form::Delta {
            base_root,
            base_checksum,
            changes,
        } => {
            let base_path = file(store_root, &base_root);
            let base = read(&base_path, &base_root)?;
            let Form::Complete(base_entries) = base.form else {
                return None;
            };
            (base.checksum == base_checksum)
                .then(|| apply(base_entries, changes))
                .flatten()
                .map(|entries| (parsed.projection_tree, entries))
        }
    });
    let Some((projection_tree, entries)) = loaded.filter(|(_, entries)| well_formed(entries))
    else {
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
        return None;
    };
    super::checkout::remember_projection_tree_id(&entries, &projection_tree);
    Some(entries)
}

enum Form {
    Complete(Vec<CheckoutProjectionEntry>),
    Delta {
        base_root: TreeId,
        base_checksum: [u8; 32],
        /// Sorted paths, `None` removed.
        changes: Vec<(String, Option<CheckoutProjectionContent>)>,
    },
}

struct Parsed {
    projection_tree: TreeId,
    form: Form,
    checksum: [u8; 32],
}

/// One cache file bound to `root_tree`, its checksum checked.
fn read(path: &Path, root_tree: &TreeId) -> Option<Parsed> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES || metadata.len() < 40 {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let (body, trailer) = bytes.split_at(bytes.len() - 32);
    let checksum = *blake3::hash(body).as_bytes();
    if checksum != trailer {
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
    let form = match reader.u8()? {
        FORM_COMPLETE => Form::Complete(
            read_entries(&mut reader, false)?
                .into_iter()
                .map(|(path, content)| {
                    content.map(|content| CheckoutProjectionEntry { path, content })
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        FORM_DELTA => {
            let base_root = TreeId::parse(reader.string()?).ok()?;
            let base_checksum: [u8; 32] = reader.take(32)?.try_into().ok()?;
            Form::Delta {
                base_root,
                base_checksum,
                changes: read_entries(&mut reader, true)?,
            }
        }
        _ => return None,
    };
    (reader.at == body.len()).then_some(Parsed {
        projection_tree,
        form,
        checksum,
    })
}

/// Sorted, prefix-shared entries; `removals` admits kind 0 (`None`).
fn read_entries(
    reader: &mut Reader<'_>,
    removals: bool,
) -> Option<Vec<(String, Option<CheckoutProjectionContent>)>> {
    let count = usize::try_from(reader.u32()?).ok()?;
    if count > MAX_ENTRIES {
        return None;
    }
    let mut entries = Vec::with_capacity(count);
    let mut previous: Vec<u8> = Vec::new();
    for index in 0..count {
        let shared = usize::try_from(reader.varint()?).ok()?;
        let suffix = usize::try_from(reader.varint()?).ok()?;
        if shared > previous.len() {
            return None;
        }
        let tail = reader.take(suffix)?;
        // Strictly increasing paths, checked as they are read.
        if index > 0 && tail <= &previous[shared..] {
            return None;
        }
        previous.truncate(shared);
        previous.extend_from_slice(tail);
        let path = std::str::from_utf8(&previous).ok()?.to_owned();
        let content = match reader.u8()? {
            kind @ (KIND_FILE | KIND_EXECUTABLE) => {
                let digest: [u8; 32] = reader.take(32)?.try_into().ok()?;
                Some(CheckoutProjectionContent::File {
                    file_id: FileId::from_digest_bytes(&digest),
                    byte_length: reader.varint()?,
                    executable: kind == KIND_EXECUTABLE,
                })
            }
            KIND_SYMLINK => Some(CheckoutProjectionContent::Symlink {
                target: reader.string()?.to_owned(),
            }),
            KIND_REMOVED if removals => None,
            _ => return None,
        };
        entries.push((path, content));
    }
    Some(entries)
}

/// `base` with every change applied (both sorted by path); a removal of an
/// absent path is malformed.
fn apply(
    base: Vec<CheckoutProjectionEntry>,
    changes: Vec<(String, Option<CheckoutProjectionContent>)>,
) -> Option<Vec<CheckoutProjectionEntry>> {
    let mut result = Vec::with_capacity(base.len() + changes.len());
    let mut base = base.into_iter().peekable();
    for (path, content) in changes {
        while base.peek().is_some_and(|entry| entry.path < path) {
            result.extend(base.next());
        }
        let replaced = base.next_if(|entry| entry.path == path).is_some();
        match content {
            Some(content) => result.push(CheckoutProjectionEntry { path, content }),
            None if replaced => {}
            None => return None,
        }
    }
    result.extend(base);
    Some(result)
}

/// The changes turning `base` into `next` (both sorted by path).
fn changes_between<'a>(
    base: &'a [CheckoutProjectionEntry],
    next: &'a [CheckoutProjectionEntry],
) -> Vec<(&'a str, Option<&'a CheckoutProjectionContent>)> {
    let mut changes = Vec::new();
    let (mut b, mut n) = (0, 0);
    while b < base.len() || n < next.len() {
        match (base.get(b), next.get(n)) {
            (Some(old), Some(new)) if old.path == new.path => {
                if old.content != new.content {
                    changes.push((new.path.as_str(), Some(&new.content)));
                }
                b += 1;
                n += 1;
            }
            (Some(old), Some(new)) if old.path < new.path => {
                changes.push((old.path.as_str(), None));
                b += 1;
            }
            (_, Some(new)) => {
                changes.push((new.path.as_str(), Some(&new.content)));
                n += 1;
            }
            (Some(old), None) => {
                changes.push((old.path.as_str(), None));
                b += 1;
            }
            (None, None) => break,
        }
    }
    changes
}

/// Records the projection of `root_tree`. Best effort: a cache that cannot
/// be written only makes the next command read the Store. A projection
/// already recorded is kept (a Tree's projection never changes; a damaged
/// file is removed by `load`). One close to the most recent complete
/// projection is recorded as a delta to it.
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
    let path = file(store_root, root_tree);
    if path.is_file() {
        // Recorded already: only marked recent, so pruning keeps it.
        if let Ok(file) = fs::OpenOptions::new().write(true).open(&path) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
        return;
    }
    let Some(directory) = path.parent().map(Path::to_path_buf) else {
        return;
    };
    let mut body = Vec::with_capacity(64 + entries.len() * 48);
    body.extend_from_slice(MAGIC);
    body.extend_from_slice(&VERSION.to_le_bytes());
    put_string(&mut body, root_tree.as_str());
    put_string(&mut body, projection_tree.as_str());
    let delta = newest_complete(&directory).and_then(|(base_root, base_path)| {
        let base = read(&base_path, &base_root)?;
        let Form::Complete(base_entries) = &base.form else {
            return None;
        };
        let changes = changes_between(base_entries, entries);
        (changes.len() * 100 <= entries.len() * DELTA_MAX_PERCENT)
            .then(|| (base_root, base.checksum, changes_owned(&changes)))
    });
    let complete = delta.is_none();
    match delta {
        Some((base_root, base_checksum, changes)) => {
            body.push(FORM_DELTA);
            put_string(&mut body, base_root.as_str());
            body.extend_from_slice(&base_checksum);
            put_entries(
                &mut body,
                changes
                    .iter()
                    .map(|(path, content)| (path.as_str(), content.as_ref())),
                changes.len(),
            );
        }
        None => {
            body.push(FORM_COMPLETE);
            put_entries(
                &mut body,
                entries
                    .iter()
                    .map(|entry| (entry.path.as_str(), Some(&entry.content))),
                entries.len(),
            );
        }
    }
    let checksum = blake3::hash(&body);
    body.extend_from_slice(checksum.as_bytes());
    let _ = (|| -> io::Result<()> {
        fs::create_dir_all(&directory)?;
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
        })?;
        // Pruning reads every file's header: only needed once a complete
        // projection was added (at most `KEPT_COMPLETE` stay) or more than
        // `KEPT` files are listed. A delta added within `KEPT` breaks
        // neither rule.
        if complete || listed(&directory).len() > KEPT {
            prune(&directory, &path);
        }
        Ok(())
    })();
}

fn changes_owned(
    changes: &[(&str, Option<&CheckoutProjectionContent>)],
) -> Vec<(String, Option<CheckoutProjectionContent>)> {
    changes
        .iter()
        .map(|(path, content)| ((*path).to_owned(), content.cloned()))
        .collect()
}

fn put_entries<'a>(
    body: &mut Vec<u8>,
    entries: impl Iterator<Item = (&'a str, Option<&'a CheckoutProjectionContent>)>,
    count: usize,
) {
    body.extend_from_slice(&(count as u32).to_le_bytes());
    let mut previous: &[u8] = &[];
    for (path, content) in entries {
        let path = path.as_bytes();
        let shared = path
            .iter()
            .zip(previous)
            .take_while(|(a, b)| a == b)
            .count();
        put_varint(body, shared as u64);
        put_varint(body, (path.len() - shared) as u64);
        body.extend_from_slice(&path[shared..]);
        previous = path;
        match content {
            Some(CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            }) => {
                body.push(if *executable {
                    KIND_EXECUTABLE
                } else {
                    KIND_FILE
                });
                body.extend_from_slice(&file_id.digest_bytes());
                put_varint(body, *byte_length);
            }
            Some(CheckoutProjectionContent::Symlink { target }) => {
                body.push(KIND_SYMLINK);
                put_string(body, target);
            }
            None => body.push(KIND_REMOVED),
        }
    }
}

/// What a cache file is, from its header alone: its root Tree, and for a
/// delta the root of the complete projection it needs.
struct Header {
    root: TreeId,
    base: Option<TreeId>,
}

fn header(path: &Path) -> Option<Header> {
    use std::io::Read;
    let mut prefix = Vec::with_capacity(320);
    fs::File::open(path)
        .ok()?
        .take(320)
        .read_to_end(&mut prefix)
        .ok()?;
    let mut reader = Reader {
        bytes: &prefix,
        at: 0,
    };
    if reader.take(8)? != MAGIC || reader.u32()? != VERSION {
        return None;
    }
    let root = TreeId::parse(reader.string()?).ok()?;
    reader.string()?;
    let base = match reader.u8()? {
        FORM_COMPLETE => None,
        FORM_DELTA => Some(TreeId::parse(reader.string()?).ok()?),
        _ => return None,
    };
    Some(Header { root, base })
}

/// The most recently written complete projection: its root and file.
fn newest_complete(directory: &Path) -> Option<(TreeId, PathBuf)> {
    let mut files = listed(directory);
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().find_map(|(_, path)| {
        let header = header(&path)?;
        header.base.is_none().then_some((header.root, path))
    })
}

fn listed(directory: &Path) -> Vec<(std::time::SystemTime, PathBuf)> {
    let Ok(listing) = fs::read_dir(directory) else {
        return Vec::new();
    };
    listing
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            (path.extension()? == "bin").then_some(())?;
            Some((entry.metadata().ok()?.modified().ok()?, path))
        })
        .collect()
}

/// Projections kept: those of the Trees most recently written (each Worktree
/// and Room base, recent Steps), most of them small deltas.
const KEPT: usize = 8;
/// Complete projections kept, each about 40 bytes per path: the bases of a
/// project and two Rooms on other Steps keep theirs.
const KEPT_COMPLETE: usize = 3;

/// Keeps the `KEPT` most recently written projections (`kept`, just
/// written, always) and the complete projections they apply to, with at most
/// `KEPT_COMPLETE` complete ones; a delta whose complete projection goes goes
/// too. Best effort: a removed projection is read from the Store when needed.
fn prune(directory: &Path, kept: &Path) {
    let mut files = listed(directory);
    files.sort_by(|a, b| b.0.cmp(&a.0));
    if let Some(at) = files.iter().position(|(_, path)| path == kept) {
        let just = files.remove(at);
        files.insert(0, just);
    }
    let headers: Vec<(PathBuf, Option<Header>)> = files
        .into_iter()
        .map(|(_, path)| {
            let header = header(&path);
            (path, header)
        })
        .collect();
    // The newest complete projections, by root.
    let complete: Vec<&TreeId> = headers
        .iter()
        .filter_map(|(_, header)| {
            header
                .as_ref()
                .filter(|h| h.base.is_none())
                .map(|h| &h.root)
        })
        .take(KEPT_COMPLETE)
        .collect();
    // Deltas fill what the complete projections leave of `KEPT`.
    let mut retained = complete.len();
    for (path, header) in &headers {
        let keep = match header {
            Some(Header {
                base: Some(base), ..
            }) => {
                let keep = complete.contains(&base) && retained < KEPT;
                retained += usize::from(keep);
                keep
            }
            Some(Header { root, base: None }) => complete.contains(&root),
            None => false,
        };
        if !keep && path != kept {
            let _ = fs::remove_file(path);
        }
    }
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
    // Strictly sorted (checked below), so a leaf is found by bisection.
    let is_leaf = |candidate: &str| {
        entries
            .binary_search_by(|entry| entry.path.as_str().cmp(candidate))
            .is_ok()
    };
    // The directory of the previous entry, already proven to hold no leaf
    // among its ancestors: most entries share it.
    let mut checked_directory: Option<&str> = None;
    for entry in entries {
        let path = entry.path.as_str();
        if path.is_empty()
            || path.starts_with('/')
            || path.ends_with('/')
            || path.contains("//")
            || path.contains('\\')
            || previous.is_some_and(|previous| previous >= path)
        {
            return false;
        }
        let directory = path.rsplit_once('/').map(|(directory, _)| directory);
        if directory.is_some() && directory != checked_directory {
            if path.match_indices('/').any(|(at, _)| is_leaf(&path[..at])) {
                return false;
            }
            checked_directory = directory;
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
    fn only_the_most_recent_projections_are_kept() {
        let root = std::env::temp_dir().join(format!(
            "layrs-projection-cache-prune-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let trees: Vec<TreeId> = (0..KEPT + 4)
            .map(|n| TreeId::parse(format!("blake3:{}", blake3::hash(&[n as u8]))).unwrap())
            .collect();
        for tree in &trees {
            save(&root, tree, tree, &entries());
        }
        let directory = file(&root, &trees[0]).parent().unwrap().to_path_buf();
        assert!(fs::read_dir(&directory).unwrap().count() <= KEPT);
        assert_eq!(load(&root, trees.last().unwrap()), Some(entries()));
        let _ = fs::remove_dir_all(&root);
    }

    fn many(count: usize, seed: u8) -> Vec<CheckoutProjectionEntry> {
        (0..count)
            .map(|n| {
                (
                    format!("d{}/f{n:05}.txt", n % 7),
                    CheckoutProjectionContent::File {
                        file_id: FileId::from_bytes(&[seed, n as u8, (n >> 8) as u8]),
                        byte_length: n as u64,
                        executable: false,
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_iter()
            .map(|(path, content)| CheckoutProjectionEntry { path, content })
            .collect::<Vec<_>>()
    }

    #[test]
    fn close_projections_are_deltas_bound_to_their_exact_base() {
        let root = std::env::temp_dir().join(format!(
            "layrs-projection-cache-delta-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let tree = |n: u8| TreeId::parse(format!("blake3:{}", blake3::hash(&[n, 99]))).unwrap();
        let base = many(400, 1);
        save(&root, &tree(0), &tree(0), &base);
        // A few paths changed, one removed, one added: a delta, much smaller.
        let mut next = base.clone();
        next.remove(3);
        next[10].content = CheckoutProjectionContent::Symlink {
            target: "elsewhere".into(),
        };
        next.push(CheckoutProjectionEntry {
            path: "zz/new.txt".into(),
            content: CheckoutProjectionContent::File {
                file_id: FileId::from_bytes(b"new"),
                byte_length: 3,
                executable: true,
            },
        });
        save(&root, &tree(1), &tree(1), &next);
        let size = |n: u8| fs::metadata(file(&root, &tree(n))).unwrap().len();
        assert!(size(1) * 20 < size(0), "{} vs {}", size(1), size(0));
        assert_eq!(load(&root, &tree(1)), Some(next.clone()));
        assert_eq!(load(&root, &tree(0)), Some(base.clone()));
        // Most paths changed: a complete projection.
        let far = many(400, 2);
        save(&root, &tree(2), &tree(2), &far);
        assert!(header(&file(&root, &tree(2))).unwrap().base.is_none());
        assert_eq!(load(&root, &tree(2)), Some(far));
        // A delta never applies to other bytes than its base's: a base
        // rewritten (same Tree, other bytes) leaves it unreadable and removed.
        let mut altered = base.clone();
        altered.truncate(399);
        fs::remove_file(file(&root, &tree(0))).unwrap();
        save(&root, &tree(0), &tree(0), &altered);
        assert_eq!(load(&root, &tree(1)), None);
        assert!(!file(&root, &tree(1)).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn applying_the_changes_between_two_projections_gives_the_second() {
        // A small deterministic generator: many random pairs, no sampling.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let pick = |seed: u64, count: u64| -> Vec<CheckoutProjectionEntry> {
            let mut map = std::collections::BTreeMap::new();
            let mut value = seed;
            for _ in 0..count {
                value = value
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let path = format!("d{}/f{}", value % 5, (value >> 8) % 40);
                let content = match (value >> 20) % 3 {
                    0 => CheckoutProjectionContent::Symlink {
                        target: format!("t{}", (value >> 24) % 3),
                    },
                    kind => CheckoutProjectionContent::File {
                        file_id: FileId::from_bytes(&((value >> 26) % 4).to_le_bytes()),
                        byte_length: (value >> 30) % 4,
                        executable: kind == 2,
                    },
                };
                map.insert(path, content);
            }
            map.into_iter()
                .map(|(path, content)| CheckoutProjectionEntry { path, content })
                .collect()
        };
        for _ in 0..500 {
            let (a, b) = (next(), next());
            let base = pick(a, a % 60);
            let target = pick(b, b % 60);
            let changes = changes_owned(&changes_between(&base, &target));
            assert_eq!(apply(base.clone(), changes), Some(target.clone()));
            assert!(changes_between(&target, &target).is_empty());
        }
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
