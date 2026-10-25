use super::*;
use crate::secure_fs::PinnedDirectory;
use layrs_core::{
    projection::{EntryState, ProjectionEntry},
    scm::TreeEntryTarget,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
};

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
    super::super::provider::require_ordinary(c)?;
    let selection = super::super::selection::for_scan(c, snapshot)?;
    let root = PinnedDirectory::open(&store.root)?;
    let present = namespace(&root, &store.root, snapshot, &selection)?;
    let mut result = Scan {
        files: BTreeMap::new(),
        changes: Vec::new(),
    };
    let mut total = 0u64;
    let reserved = snapshot.reserved_paths()?;
    for entry in &snapshot.entries {
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
            let pinned = parent.open_file(&name)?;
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
            let mut hash = blake3::Hasher::new();
            hash.update(b"layrs-file-v1\0");
            let mut buffer = [0u8; 64 * 1024];
            let mut count = 0u64;
            loop {
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                count += n as u64;
                if count > meta.len() {
                    return Err(invalid("file changed during capture"));
                }
                hash.update(&buffer[..n]);
            }
            if count != meta.len() {
                return Err(invalid("file changed during capture"));
            }
            let file_id = FileId::parse(format!("blake3:{}", hash.finalize())).map_err(invalid)?;
            let descriptor = CapturedFile {
                file_id,
                executable,
                bytes: count,
            };
            if expected != Some((&descriptor.file_id, descriptor.executable)) && !entry.writable {
                return Err(invalid("read-only projection file was modified"));
            }
            // Changed bytes are reopened through the pinned checkout and
            // written to SQL in bounded chunks after both scans agree.
            pinned.verify_parent()?;
            parent.verify_path_identity()?;
            if !pinned.same_file(&parent.open_file(&name)?) {
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
    if namespace(&root, &store.root, snapshot, &selection)? != present {
        return Err(invalid("checkout namespace changed during capture"));
    }
    root.verify_path_identity()?;
    Ok(result)
}
fn namespace(
    root: &PinnedDirectory,
    base: &Path,
    snapshot: &ProjectionSnapshot,
    selection: &layrs_core::projection::ProjectionSelection,
) -> Result<BTreeSet<String>> {
    let traversal = selection.traversal(snapshot.path_semantics)?;
    let entries: BTreeMap<&str, &ProjectionEntry> = snapshot
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();
    let mut directories = BTreeSet::new();
    for e in &snapshot.entries {
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
        directory.verify_path_identity()?;
        for item in fs::read_dir(base.join(&relative))? {
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
                directory.open_file(name.as_ref())?;
                seen.insert(path);
            }
        }
        directory.verify_path_identity()?;
    }
    root.verify_path_identity()?;
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
