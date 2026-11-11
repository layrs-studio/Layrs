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
//! - Each projection is one value of the derived cache database
//!   (`derived_cache`), written without sync.
//! - A value whose binding, structure or trailing blake3 checksum is wrong
//!   is ignored and the projection is read from the Store again.
//! - Callers sample hits against a recomputation and delete a wrong value.
//!
//! The format, in order:
//!
//! 1. `LYRSPRJ1` and a format version.
//! 2. The binding: the root Tree id, then the projection's own Tree id (as
//!    `projection_tree_id` computes it; it differs from the root only when
//!    the root holds what a checkout cannot represent, such as Tombstones).
//! 3. The packs its entries name: their count, then each one's 32-byte
//!    registered digest.
//! 4. The form: complete (0) or delta (1).
//! 5. A complete projection: its entries, sorted by path (ADR 0028, compact).
//!    Each holds its path as the byte count it shares with the previous path
//!    plus its own suffix (varints), a kind byte, and then:
//!    - a File (plain or executable) its Store holds as a file record of a
//!      large pack: that pack's position in the table and the record's
//!      ordinal (varints; `FileRefs`). The record gives the FileId and
//!      length exactly, and a pack no longer registered leaves the value
//!      unreadable (removed, then read from the Store again);
//!    - any other File: the 32-byte FileId digest and the length (varint);
//!    - a symlink: its target.
//!
//!    A delta: the root Tree of the complete projection it applies to and that
//!    file's own checksum (so it applies to those exact bytes only), then the
//!    paths it removes or sets, sorted, encoded as entries where a removed
//!    path has kind 0. Successive projections differ by a few paths: each is
//!    a few bytes, not another copy of every FileId.
//! 6. The blake3 checksum of everything before it.
use super::checkout::{CheckoutProjectionContent, CheckoutProjectionEntry};
use super::derived_cache::{Access, Projections};
use layrs_core::scm::{FileId, TreeId};
use std::{
    fs,
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 8] = b"LYRSPRJ1";
const VERSION: u32 = 5;
const DIRECTORY: &str = "projection-cache";
/// Bound on what a corrupt file can make a reader allocate.
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const KIND_FILE: u8 = 1;
const KIND_SYMLINK: u8 = 2;
const KIND_EXECUTABLE: u8 = 3;
/// A File named by a file record (`FileRefs`).
const KIND_FILE_REF: u8 = 4;
const KIND_EXECUTABLE_REF: u8 = 5;
/// Packs one value may name.
const MAX_PACKS: usize = 1024;
/// In a delta: the path is absent from the result.
const KIND_REMOVED: u8 = 0;
const FORM_COMPLETE: u8 = 0;
const FORM_DELTA: u8 = 1;
/// A delta is written while it changes at most this share of the paths.
const DELTA_MAX_PERCENT: usize = 25;
/// Largest complete projection a process keeps in memory for its deltas
/// (about 400 000 paths).
const REMEMBERED_MAX_BYTES: usize = 16 * 1024 * 1024;

/// Exact positions naming Files in the Store (its large packs' file
/// records), so a projection holds a few bytes per File instead of its
/// 32-byte FileId. Without them, values that use positions are not read
/// (`load` answers `None` and keeps them).
pub trait FileRefs {
    /// A position naming `file_id` (whose length is `length`), if any.
    fn reference(&self, file_id: &FileId, length: u64) -> Option<([u8; 32], u32)>;
    /// The FileId and length a position names, while its pack is registered.
    fn resolve(&self, pack: &[u8; 32], ordinal: u32) -> Option<(FileId, u64)>;
}

/// How a value is decoded: with the Store's positions, if any, and whether
/// it needed them without having them.
struct Decoding<'a> {
    refs: Option<&'a dyn FileRefs>,
    needs_refs: std::cell::Cell<bool>,
}

/// Where projections were kept as files before the derived cache database;
/// removed when a projection is saved.
fn legacy_directory(store_root: &Path) -> PathBuf {
    store_root.join("metadata").join(DIRECTORY)
}

/// The cached projection of `root_tree`, if a value exists whose binding,
/// structure and checksum are all intact (and, for a delta, those of the
/// exact complete projection it applies to). A value that fails is removed,
/// so the next `save` writes it again.
pub fn load(
    store_root: &Path,
    root_tree: &TreeId,
    refs: Option<&dyn FileRefs>,
) -> Option<Vec<CheckoutProjectionEntry>> {
    let _span = crate::perf::span("projection_cache.load");
    let cache = Projections::open(store_root, Access::Read);
    let decoding = Decoding {
        refs,
        needs_refs: std::cell::Cell::new(false),
    };
    let decoding = &decoding;
    let mut stored = false;
    let loaded = cache.as_ref().and_then(|cache| {
        let parsed = read(cache, root_tree, &mut stored, decoding)?;
        match parsed.form {
            Form::Complete(entries) => Some((parsed.projection_tree, entries)),
            Form::Delta {
                base_root,
                base_checksum,
                changes,
            } => {
                let base = match last_complete(&base_root, &base_checksum) {
                    Some(bytes) => parse(
                        &bytes[..bytes.len() - 32],
                        &base_root,
                        base_checksum,
                        decoding,
                    ),
                    None => read(cache, &base_root, &mut false, decoding),
                }?;
                let Form::Complete(base_entries) = base.form else {
                    return None;
                };
                (base.checksum == base_checksum)
                    .then(|| apply(base_entries, changes))
                    .flatten()
                    .map(|entries| (parsed.projection_tree, entries))
            }
        }
    });
    let checked = {
        let _span = crate::perf::span("projection_cache.well_formed");
        loaded.filter(|(_, entries)| well_formed(entries))
    };
    let Some((projection_tree, entries)) = checked else {
        if decoding.needs_refs.get() {
            // Sound for a caller with the Store's positions: kept.
            return None;
        }
        // Removed, then forgotten (council s9 r3): a read that saw the value
        // began before the forget, so it cannot remember it again.
        if let (Some(cache), true) = (&cache, stored) {
            cache.remove(root_tree.as_str());
        }
        forget_complete(root_tree);
        return None;
    };
    {
        let _span = crate::perf::span("projection_cache.remember");
        super::checkout::remember_projection_tree_id(&entries, &projection_tree);
    }
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

/// One cached value bound to `root_tree`, its checksum checked; `stored` is
/// set when a value was found at all.
fn read(
    cache: &Projections,
    root_tree: &TreeId,
    stored: &mut bool,
    decoding: &Decoding<'_>,
) -> Option<Parsed> {
    // Taken before the read: bytes read before a projection is forgotten
    // are never remembered after it (council s9 r2).
    let generation = memo_generation().load(std::sync::atomic::Ordering::Acquire);
    let bytes = {
        let _span = crate::perf::span("projection_cache.read_file");
        cache.get(root_tree.as_str(), MAX_BYTES)?
    };
    *stored = true;
    verified(bytes, root_tree, generation, decoding)
}

/// `bytes` parsed if their checksum holds; a complete projection's bytes are
/// remembered (`remember_complete`).
fn verified(
    bytes: Vec<u8>,
    root_tree: &TreeId,
    generation: u64,
    decoding: &Decoding<'_>,
) -> Option<Parsed> {
    if bytes.len() < 40 {
        return None;
    }
    let (body, trailer) = bytes.split_at(bytes.len() - 32);
    let checksum = {
        let _span = crate::perf::span("projection_cache.checksum");
        *blake3::hash(body).as_bytes()
    };
    if checksum != trailer {
        return None;
    }
    let parsed = parse(body, root_tree, checksum, decoding)?;
    if matches!(parsed.form, Form::Complete(_)) {
        remember_complete(root_tree, checksum, bytes, generation);
    }
    Some(parsed)
}

/// The verified bytes of the last complete projection this process read,
/// and when they were read.
type Verified = (
    TreeId,
    [u8; 32],
    std::sync::Arc<Vec<u8>>,
    std::time::Instant,
);

/// How long remembered bytes may answer a delta: the loads of one command,
/// not a long-lived process's later commands. Another process removing the
/// projection meanwhile is no more exposed than a read just before it.
const REMEMBERED_FOR: std::time::Duration = std::time::Duration::from_secs(1);

/// Incremented whenever a projection is forgotten.
fn memo_generation() -> &'static std::sync::atomic::AtomicU64 {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    &GENERATION
}

fn verified_complete() -> &'static std::sync::Mutex<Option<Verified>> {
    static LAST: std::sync::OnceLock<std::sync::Mutex<Option<Verified>>> =
        std::sync::OnceLock::new();
    LAST.get_or_init(|| std::sync::Mutex::new(None))
}

/// Kept so a delta bound to these exact bytes (its base checksum) applies to
/// them without reading the file again: a command often loads two
/// projections that are deltas to the same complete one. Only bytes whose
/// checksum was verified, only one projection per process, and only up to
/// `REMEMBERED_MAX_BYTES` (a larger one is read again).
fn remember_complete(root_tree: &TreeId, checksum: [u8; 32], bytes: Vec<u8>, generation: u64) {
    if let Ok(mut last) = verified_complete().lock() {
        // Under the lock `forget_complete` takes: a projection forgotten
        // since these bytes were read is not remembered again.
        if memo_generation().load(std::sync::atomic::Ordering::Acquire) != generation {
            return;
        }
        *last = (bytes.len() <= REMEMBERED_MAX_BYTES).then(|| {
            (
                root_tree.clone(),
                checksum,
                std::sync::Arc::new(bytes),
                std::time::Instant::now(),
            )
        });
    }
}

/// Forgets the remembered projection of `root_tree` (council s9 r1): a
/// projection removed as wrong must not keep answering its deltas.
fn forget_complete(root_tree: &TreeId) {
    let mut last = verified_complete()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    memo_generation().fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    if last.as_ref().is_some_and(|(tree, ..)| tree == root_tree) {
        *last = None;
    }
}

/// The remembered bytes of `root_tree`'s complete projection, if they are
/// exactly those of `checksum` and were read within `REMEMBERED_FOR`.
fn last_complete(root_tree: &TreeId, checksum: &[u8; 32]) -> Option<std::sync::Arc<Vec<u8>>> {
    let mut last = verified_complete().lock().ok()?;
    let (tree, sum, bytes, read_at) = last.as_ref()?;
    if read_at.elapsed() >= REMEMBERED_FOR {
        // Expired: a long-lived process does not keep it (council s9 r3).
        *last = None;
        return None;
    }
    (tree == root_tree && sum == checksum).then(|| {
        crate::perf::count("projection_cache.base_remembered", 1);
        bytes.clone()
    })
}

/// A cache file's body (its checksum already verified) bound to `root_tree`.
fn parse(
    body: &[u8],
    root_tree: &TreeId,
    checksum: [u8; 32],
    decoding: &Decoding<'_>,
) -> Option<Parsed> {
    let _span = crate::perf::span("projection_cache.parse");
    let mut reader = Reader { bytes: body, at: 0 };
    if reader.take(8)? != MAGIC || reader.u32()? != VERSION {
        return None;
    }
    if reader.string()? != root_tree.as_str() {
        return None;
    }
    let projection_tree = TreeId::parse(reader.string()?).ok()?;
    let pack_count = usize::try_from(reader.u32()?).ok()?;
    if pack_count > MAX_PACKS {
        return None;
    }
    let packs = (0..pack_count)
        .map(|_| reader.take(32)?.try_into().ok())
        .collect::<Option<Vec<[u8; 32]>>>()?;
    let form = match reader.u8()? {
        FORM_COMPLETE => Form::Complete(
            read_entries(&mut reader, false, &packs, decoding)?
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
                changes: read_entries(&mut reader, true, &packs, decoding)?,
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
    packs: &[[u8; 32]],
    decoding: &Decoding<'_>,
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
            kind @ (KIND_FILE_REF | KIND_EXECUTABLE_REF) => {
                let pack = packs.get(usize::try_from(reader.varint()?).ok()?)?;
                let ordinal = u32::try_from(reader.varint()?).ok()?;
                let Some(refs) = decoding.refs else {
                    decoding.needs_refs.set(true);
                    return None;
                };
                let (file_id, byte_length) = refs.resolve(pack, ordinal)?;
                Some(CheckoutProjectionContent::File {
                    file_id,
                    byte_length,
                    executable: kind == KIND_EXECUTABLE_REF,
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
    refs: Option<&dyn FileRefs>,
) {
    let _span = crate::perf::span("projection_cache.save");
    if entries.len() > MAX_ENTRIES || !well_formed(entries) {
        return;
    }
    let Some(cache) = Projections::open(store_root, Access::Create) else {
        return;
    };
    if cache.touch(root_tree.as_str()) {
        // Recorded already: only marked recent, so pruning keeps it.
        return;
    }
    let decoding = Decoding {
        refs,
        needs_refs: std::cell::Cell::new(false),
    };
    let mut packs: Vec<[u8; 32]> = Vec::new();
    let mut form = Vec::with_capacity(16 + entries.len() * 16);
    let delta = cache
        .newest_complete(MAX_BYTES)
        .and_then(|(base_root, bytes)| {
            let base_root = TreeId::parse(base_root).ok()?;
            let generation = memo_generation().load(std::sync::atomic::Ordering::Acquire);
            let base = verified(bytes, &base_root, generation, &decoding)?;
            let Form::Complete(base_entries) = &base.form else {
                return None;
            };
            let changes = changes_between(base_entries, entries);
            (changes.len() * 100 <= entries.len() * DELTA_MAX_PERCENT)
                .then(|| (base_root, base.checksum, changes_owned(&changes)))
        });
    let complete = delta.is_none();
    let base = delta
        .as_ref()
        .map(|(base_root, ..)| base_root.as_str().to_owned());
    match delta {
        Some((base_root, base_checksum, changes)) => {
            form.push(FORM_DELTA);
            put_string(&mut form, base_root.as_str());
            form.extend_from_slice(&base_checksum);
            put_entries(
                &mut form,
                changes
                    .iter()
                    .map(|(path, content)| (path.as_str(), content.as_ref())),
                changes.len(),
                refs,
                &mut packs,
            );
        }
        None => {
            form.push(FORM_COMPLETE);
            put_entries(
                &mut form,
                entries
                    .iter()
                    .map(|entry| (entry.path.as_str(), Some(&entry.content))),
                entries.len(),
                refs,
                &mut packs,
            );
        }
    }
    let mut body = Vec::with_capacity(96 + packs.len() * 32 + form.len());
    body.extend_from_slice(MAGIC);
    body.extend_from_slice(&VERSION.to_le_bytes());
    put_string(&mut body, root_tree.as_str());
    put_string(&mut body, projection_tree.as_str());
    body.extend_from_slice(&(packs.len() as u32).to_le_bytes());
    for pack in &packs {
        body.extend_from_slice(pack);
    }
    body.extend_from_slice(&form);
    let checksum = blake3::hash(&body);
    body.extend_from_slice(checksum.as_bytes());
    if cache
        .insert(root_tree.as_str(), base.as_deref(), &body)
        .is_err()
    {
        return;
    }
    // Pruning is only needed once a complete projection was added (at most
    // `KEPT_COMPLETE` stay) or more than `KEPT` are recorded. A delta added
    // within `KEPT` breaks neither rule.
    if complete || cache.count() > KEPT {
        prune(&cache, root_tree.as_str());
    }
    let legacy = legacy_directory(store_root);
    if fs::symlink_metadata(&legacy).is_ok() {
        let _ = fs::remove_dir_all(&legacy);
    }
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
    refs: Option<&dyn FileRefs>,
    packs: &mut Vec<[u8; 32]>,
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
                let named = refs
                    .and_then(|refs| refs.reference(file_id, *byte_length))
                    .and_then(|(pack, ordinal)| {
                        let at = match packs.iter().position(|known| known == &pack) {
                            Some(at) => at,
                            None if packs.len() < MAX_PACKS => {
                                packs.push(pack);
                                packs.len() - 1
                            }
                            None => return None,
                        };
                        Some((at, ordinal))
                    });
                match named {
                    Some((at, ordinal)) => {
                        body.push(if *executable {
                            KIND_EXECUTABLE_REF
                        } else {
                            KIND_FILE_REF
                        });
                        put_varint(body, at as u64);
                        put_varint(body, u64::from(ordinal));
                    }
                    None => {
                        body.push(if *executable {
                            KIND_EXECUTABLE
                        } else {
                            KIND_FILE
                        });
                        body.extend_from_slice(&file_id.digest_bytes());
                        put_varint(body, *byte_length);
                    }
                }
            }
            Some(CheckoutProjectionContent::Symlink { target }) => {
                body.push(KIND_SYMLINK);
                put_string(body, target);
            }
            None => body.push(KIND_REMOVED),
        }
    }
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
fn prune(cache: &Projections, kept: &str) {
    let mut rows = cache.rows();
    if let Some(at) = rows.iter().position(|row| row.root == kept) {
        let just = rows.remove(at);
        rows.insert(0, just);
    }
    // The newest complete projections, by root.
    let complete: Vec<&str> = rows
        .iter()
        .filter(|row| row.base.is_none())
        .map(|row| row.root.as_str())
        .take(KEPT_COMPLETE)
        .collect();
    // Deltas fill what the complete projections leave of `KEPT`.
    let mut retained = complete.len();
    for row in &rows {
        let keep = match &row.base {
            Some(base) => {
                let keep = complete.contains(&base.as_str()) && retained < KEPT;
                retained += usize::from(keep);
                keep
            }
            None => complete.contains(&row.root.as_str()),
        };
        if !keep && row.root != kept {
            cache.remove(&row.root);
        }
    }
}

/// Forgets a cached projection found to differ from its recomputation.
pub fn remove(store_root: &Path, root_tree: &TreeId) {
    // Removed, then forgotten (council s9 r3): a read that saw the value
    // began before the forget, so it cannot remember it again.
    if let Some(cache) = Projections::open(store_root, Access::Write) {
        cache.remove(root_tree.as_str());
    }
    forget_complete(root_tree);
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
        save(&root, &tree, &tree, &entries(), None);
        assert_eq!(load(&root, &tree, None), Some(entries()));
        // Another Tree never reads this value as its own, even copied.
        let other = TreeId::parse(format!("blake3:{}", blake3::hash(b"other"))).unwrap();
        let cache = Projections::open(&root, Access::Read).unwrap();
        let bytes = cache.get(tree.as_str(), MAX_BYTES).unwrap();
        cache.insert(other.as_str(), None, &bytes).unwrap();
        assert_eq!(load(&root, &other, None), None);
        // Any flipped byte fails the checksum, and the value is removed.
        let mut damaged = bytes.clone();
        let middle = damaged.len() / 2;
        damaged[middle] ^= 1;
        cache.insert(tree.as_str(), None, &damaged).unwrap();
        assert_eq!(load(&root, &tree, None), None);
        assert!(cache.get(tree.as_str(), MAX_BYTES).is_none());
        save(&root, &tree, &tree, &entries(), None);
        remove(&root, &tree);
        assert!(cache.get(tree.as_str(), MAX_BYTES).is_none());
        drop(cache);
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
            save(&root, tree, tree, &entries(), None);
        }
        assert!(Projections::open(&root, Access::Read).unwrap().count() <= KEPT);
        assert_eq!(load(&root, trees.last().unwrap(), None), Some(entries()));
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
        save(&root, &tree(0), &tree(0), &base, None);
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
        save(&root, &tree(1), &tree(1), &next, None);
        let cache = Projections::open(&root, Access::Read).unwrap();
        let size = |n: u8| cache.get(tree(n).as_str(), MAX_BYTES).unwrap().len();
        assert!(size(1) * 20 < size(0), "{} vs {}", size(1), size(0));
        assert_eq!(load(&root, &tree(1), None), Some(next.clone()));
        assert_eq!(load(&root, &tree(0), None), Some(base.clone()));
        // Most paths changed: a complete projection.
        let far = many(400, 2);
        save(&root, &tree(2), &tree(2), &far, None);
        assert!(
            cache
                .rows()
                .iter()
                .any(|row| row.root == tree(2).as_str() && row.base.is_none())
        );
        assert_eq!(load(&root, &tree(2), None), Some(far));
        // A delta never applies to other bytes than its base's: a base
        // rewritten (same Tree, other bytes) leaves it unreadable and removed.
        let mut altered = base.clone();
        altered.truncate(399);
        cache.remove(tree(0).as_str());
        save(&root, &tree(0), &tree(0), &altered, None);
        assert_eq!(load(&root, &tree(1), None), None);
        assert!(cache.get(tree(1).as_str(), MAX_BYTES).is_none());
        drop(cache);
        let _ = fs::remove_dir_all(&root);
    }

    /// A complete projection removed as wrong no longer answers the deltas
    /// bound to it from this process's memory (council s9 r1).
    #[test]
    fn a_removed_base_no_longer_answers_its_deltas() {
        let root = std::env::temp_dir().join(format!(
            "layrs-projection-cache-forget-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let tree = |n: u8| TreeId::parse(format!("blake3:{}", blake3::hash(&[n, 77]))).unwrap();
        let base = many(300, 5);
        save(&root, &tree(0), &tree(0), &base, None);
        let mut next = base.clone();
        next.remove(7);
        save(&root, &tree(1), &tree(1), &next, None);
        // The base is read (and remembered), then removed.
        assert_eq!(load(&root, &tree(0), None), Some(base));
        remove(&root, &tree(0));
        assert_eq!(load(&root, &tree(1), None), None);
        let _ = fs::remove_dir_all(&root);
    }

    /// Positions handed out by a test: (pack 7, ordinal n) names the n-th
    /// FileId of `files`, while `alive`.
    struct Positions {
        files: Vec<(FileId, u64)>,
        alive: std::cell::Cell<bool>,
    }

    impl FileRefs for Positions {
        fn reference(&self, file_id: &FileId, length: u64) -> Option<([u8; 32], u32)> {
            let at = self
                .files
                .iter()
                .position(|(known, known_length)| known == file_id && *known_length == length)?;
            Some(([7; 32], at as u32))
        }

        fn resolve(&self, pack: &[u8; 32], ordinal: u32) -> Option<(FileId, u64)> {
            (self.alive.get() && pack == &[7; 32])
                .then(|| self.files.get(ordinal as usize).cloned())
                .flatten()
        }
    }

    #[test]
    fn files_named_by_position_are_smaller_and_need_the_positions() {
        let root = std::env::temp_dir().join(format!(
            "layrs-projection-cache-refs-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let tree = |n: u8| TreeId::parse(format!("blake3:{}", blake3::hash(&[n, 55]))).unwrap();
        let projection = many(300, 3);
        let files: Vec<(FileId, u64)> = projection
            .iter()
            .filter_map(|entry| match &entry.content {
                CheckoutProjectionContent::File {
                    file_id,
                    byte_length,
                    ..
                } => Some((file_id.clone(), *byte_length)),
                _ => None,
            })
            .collect();
        let positions = Positions {
            files,
            alive: std::cell::Cell::new(true),
        };
        // Complete forms both: without the positions, the first is no base.
        save(&root, &tree(1), &tree(1), &projection, Some(&positions));
        save(&root, &tree(0), &tree(0), &projection, None);
        let cache = Projections::open(&root, Access::Read).unwrap();
        let size = |n: u8| cache.get(tree(n).as_str(), MAX_BYTES).unwrap().len();
        assert!(size(1) * 3 < size(0), "{} vs {}", size(1), size(0));
        assert_eq!(
            load(&root, &tree(1), Some(&positions)),
            Some(projection.clone())
        );
        // Without the positions: not read, and kept.
        assert_eq!(load(&root, &tree(1), None), None);
        assert!(cache.get(tree(1).as_str(), MAX_BYTES).is_some());
        // A position whose pack is gone: unreadable, and removed.
        positions.alive.set(false);
        assert_eq!(load(&root, &tree(1), Some(&positions)), None);
        assert!(cache.get(tree(1).as_str(), MAX_BYTES).is_none());
        // Values without positions read as before, with or without them.
        assert_eq!(
            load(&root, &tree(0), Some(&positions)),
            Some(projection.clone())
        );
        assert_eq!(load(&root, &tree(0), None), Some(projection));
        drop(cache);
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
