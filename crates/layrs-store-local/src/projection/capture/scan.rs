use super::*;
use crate::secure_fs::{PinnedDirectory, PinnedFile};
use layrs_core::{
    projection::{EntryState, ProjectionEntry},
    scm::TreeEntryTarget,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
};

#[cfg(test)]
mod tests;

// Ordinary scans have no token. The Windows readonly adapter borrows its token
// through this callback for this call only, keeping VFS out of other platforms.
type CancellationCheck<'a> = Option<&'a dyn Fn() -> Result<()>>;

fn check_cancel(cancel: CancellationCheck<'_>) -> Result<()> {
    match cancel {
        Some(check) => check(),
        None => Ok(()),
    }
}

pub(in crate::projection) struct Scan {
    pub files: BTreeMap<String, Option<CapturedFile>>,
    pub changes: Vec<CapturedChange>,
}

pub(in crate::projection) fn scan(
    store: &ProjectionStore,
    c: &rusqlite::Connection,
    snapshot: &ProjectionSnapshot,
    _retain: bool,
) -> Result<Scan> {
    scan_with_scope(store, c, snapshot, _retain, None)
}

pub(in crate::projection) fn scan_with_scope(
    store: &ProjectionStore,
    c: &rusqlite::Connection,
    snapshot: &ProjectionSnapshot,
    _retain: bool,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<Scan> {
    scan_with_mode(store, c, snapshot, scope, LeafOpen::Capture, None)
}

/// A retained immutable export handle shares only read access. Every scanner
/// leaf open, including namespace checks and identity reopens, must therefore
/// avoid the DELETE right requested by the ordinary capture opener.
#[cfg(windows)]
pub(in crate::projection) fn scan_readonly_with_scope(
    store: &ProjectionStore,
    c: &rusqlite::Connection,
    snapshot: &ProjectionSnapshot,
    _retain: bool,
    scope: Option<&format::ContentScope<'_>>,
    cancel: &layrs_vfs::Cancellation,
) -> Result<Scan> {
    let check = || {
        cancel
            .check()
            .map_err(super::super::provider::recovery::ProviderBootstrapCause::from)
            .map_err(Into::into)
    };
    scan_with_mode(store, c, snapshot, scope, LeafOpen::Readonly, Some(&check))
}

#[derive(Clone, Copy)]
enum LeafOpen {
    Capture,
    #[cfg(windows)]
    Readonly,
}
impl LeafOpen {
    fn open(self, parent: &PinnedDirectory, name: &std::ffi::OsStr) -> io::Result<PinnedFile> {
        match self {
            Self::Capture => parent.open_file(name),
            #[cfg(windows)]
            Self::Readonly => parent.open_readonly_file(name),
        }
    }
}

fn scan_with_mode(
    store: &ProjectionStore,
    c: &rusqlite::Connection,
    snapshot: &ProjectionSnapshot,
    scope: Option<&format::ContentScope<'_>>,
    mode: LeafOpen,
    cancel: CancellationCheck<'_>,
) -> Result<Scan> {
    check_cancel(cancel)?;
    if let Some(scope) = scope {
        scope.validate(c, store)?;
    } else {
        super::super::provider::require_ordinary(c)?;
    }
    let selection = super::super::selection::for_scan(c, snapshot)?;
    let root = PinnedDirectory::open(&store.root)?;
    let present = namespace(&root, &store.root, snapshot, &selection, mode, cancel)?;
    let mut result = Scan {
        files: BTreeMap::new(),
        changes: Vec::new(),
    };
    let mut total = 0u64;
    let reserved = snapshot.reserved_paths()?;
    for entry in &snapshot.entries {
        check_cancel(cancel)?;
        if !selection.includes(&entry.path) {
            continue;
        }
        if matches!(entry.state, EntryState::Reserved | EntryState::Unavailable) {
            continue;
        }
        if matches!(entry.target, Some(TreeEntryTarget::Tree { .. })) {
            continue;
        }
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id, entry.executable)),
            None if entry.state == EntryState::Absent => None,
            _ => {
                return Err(invalid(
                    "partial capture currently requires regular-file leaves",
                ));
            }
        };
        let path = entry.path.as_str();
        let actual = if present.contains(path) {
            let (parent, name) = root.existing_parent(Path::new(path))?;
            let pinned = mode.open(&parent, &name)?;
            let mut input = pinned.file();
            let meta = input.metadata()?;
            if meta.len() > MAX_CAPTURE_FILE_BYTES {
                return Err(invalid("partial capture file exceeds 1GiB"));
            }
            total = total
                .checked_add(meta.len())
                .filter(|n| *n <= MAX_CAPTURE_BYTES)
                .ok_or_else(|| invalid("partial capture exceeds4GiB"))?;
            let executable = executable(&meta);
            let file_id = hash_file(&mut input, meta.len(), cancel)?;
            let descriptor = CapturedFile {
                file_id,
                executable,
                bytes: meta.len(),
            };
            if expected != Some((&descriptor.file_id, descriptor.executable)) && !entry.writable {
                return Err(invalid("read-only projection file was modified"));
            }
            // Changed bytes are reopened through the pinned checkout and
            // written to SQL in bounded chunks after both scans agree.
            pinned.verify_parent()?;
            parent.verify_path_identity()?;
            if !pinned.same_file(&mode.open(&parent, &name)?) {
                return Err(invalid("file replaced during capture"));
            }
            Some(descriptor)
        } else {
            None
        };
        let equal = actual.as_ref().map(|f| (&f.file_id, f.executable)) == expected;
        if !equal {
            if !entry.writable {
                return Err(invalid("read-only projection file was removed or modified"));
            }
            reserved.check_path(path)?;
            if path.len() > 4096 || result.changes.len() >= 1024 {
                return Err(invalid("partial patch exceeds path/change budget"));
            }
            result.changes.push(CapturedChange {
                path: entry.path.clone(),
                expected: expected.map(|(id, executable)| CapturedTarget {
                    file_id: id.clone(),
                    executable,
                }),
                replacement: actual.clone(),
            });
        }
        result.files.insert(path.to_owned(), actual);
    }
    if namespace(&root, &store.root, snapshot, &selection, mode, cancel)? != present {
        return Err(invalid("checkout namespace changed during capture"));
    }
    root.verify_path_identity()?;
    check_cancel(cancel)?;
    Ok(result)
}

fn hash_file(input: &mut dyn Read, size: u64, cancel: CancellationCheck<'_>) -> Result<FileId> {
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut buffer = [0u8; 64 * 1024];
    let mut count = 0u64;
    loop {
        check_cancel(cancel)?;
        let n = input.read(&mut buffer)?;
        // Cancellation during either a data read or EOF must not yield a
        // successful descriptor or proceed to another chunk.
        check_cancel(cancel)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > size {
            return Err(invalid("file changed during capture"));
        }
        hash.update(&buffer[..n]);
    }
    if count != size {
        return Err(invalid("file changed during capture"));
    }
    check_cancel(cancel)?;
    FileId::parse(format!("blake3:{}", hash.finalize())).map_err(invalid)
}

fn namespace(
    root: &PinnedDirectory,
    base: &Path,
    snapshot: &ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
    mode: LeafOpen,
    cancel: CancellationCheck<'_>,
) -> Result<BTreeSet<String>> {
    check_cancel(cancel)?;
    let traversal = selection.traversal(snapshot.path_semantics)?;
    let mut entries = BTreeMap::<&str, &ProjectionEntry>::new();
    let mut directories = BTreeSet::new();
    for e in &snapshot.entries {
        check_cancel(cancel)?;
        entries.insert(e.path.as_str(), e);
        for (i, _) in e.path.as_str().match_indices('/') {
            directories.insert(&e.path.as_str()[..i]);
        }
        if matches!(e.target, Some(TreeEntryTarget::Tree { .. }))
            && !matches!(e.state, EntryState::Reserved | EntryState::Unavailable)
        {
            directories.insert(e.path.as_str());
        }
    }
    let mut pending = vec![(String::new(), PinnedDirectory::open(base)?)];
    let mut seen = BTreeSet::new();
    let mut count = 0;
    while let Some((relative, directory)) = pending.pop() {
        check_cancel(cancel)?;
        directory.verify_path_identity()?;
        for item in fs::read_dir(base.join(&relative))? {
            check_cancel(cancel)?;
            let item = item?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| invalid("non-UTF8 projection path"))?;
            if relative.is_empty() && name == ".layrs" {
                continue;
            }
            count += 1;
            if count > 100_000 {
                return Err(invalid("partial checkout namespace exceeds100000 entries"));
            }
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let entry = entries.get(path.as_str());
            // Excluded paths are not opened, hashed, or interpreted as deletions.
            // Ancestors and case aliases of selected paths still get verified.
            if !traversal.intersects(&path)? {
                continue;
            }
            if entry
                .is_some_and(|e| matches!(e.state, EntryState::Reserved | EntryState::Unavailable))
            {
                return Err(invalid(format!(
                    "reserved projection path exists on disk: {path}"
                )));
            }
            if directories.contains(path.as_str()) {
                pending.push((path, directory.open_child_for_traversal(name.as_ref())?));
            } else {
                if entry.is_none() {
                    return Err(invalid(format!(
                        "path has no observed write authority: {path}"
                    )));
                }
                // Pinned open rejects symlinks, reparse points and directories.
                mode.open(&directory, name.as_ref())?;
                seen.insert(path);
            }
        }
        check_cancel(cancel)?;
        directory.verify_path_identity()?;
    }
    root.verify_path_identity()?;
    check_cancel(cancel)?;
    Ok(seen)
}
pub(in crate::projection) fn executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}
