//! Read-only acquisition of checkout bytes through pinned directory handles.
//! No content is read through symlinks; a link is captured as its target text.
use crate::secure_fs::PinnedDirectory;
mod projection;
mod streaming;
pub use projection::{
    CheckoutProjectionContent, CheckoutProjectionEntry, projection_tree_id,
    validate_streaming_projection,
};
use std::{
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
pub struct CheckoutLimits {
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckoutContent {
    File { bytes: Vec<u8>, executable: bool },
    Symlink { target: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckoutEntry {
    pub path: String,
    pub content: CheckoutContent,
}

/// Read-only platform validation shared by previews and the materializer.
pub fn validate_projection(entries: &[CheckoutEntry]) -> io::Result<()> {
    super::checkout_paths::validate(entries)
}

/// Identity survives renames, but reopening requires the originally recorded path.
/// This is a capability for reading only, never an authority to overwrite files.
pub struct CheckoutReader {
    root: PathBuf,
    directory: PinnedDirectory,
}

impl CheckoutReader {
    pub fn open(root: impl AsRef<Path>, expected_identity: Option<&str>) -> io::Result<Self> {
        let root = std::path::absolute(root)?;
        if root.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(invalid("checkout path contains parent traversal"));
        }
        // Reject reparse/symlink ancestors as well as the checkout itself.
        let mut ancestor = PathBuf::new();
        for part in root.components() {
            ancestor.push(part);
            if matches!(part, Component::Normal(_)) {
                let metadata = fs::symlink_metadata(&ancestor)?;
                if metadata.file_type().is_symlink() || reparse(&metadata) {
                    return Err(invalid("checkout ancestor is a symlink or reparse point"));
                }
            }
        }
        let directory = PinnedDirectory::open(&root)?;
        if expected_identity.is_some_and(|expected| expected != directory.identity_token()) {
            return Err(invalid("checkout filesystem identity changed"));
        }
        Ok(Self { root, directory })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn identity(&self) -> String {
        self.directory.identity_token()
    }

    /// Metadata-only probe for directory-to-leaf writes. Ignored descendants
    /// must not disappear from previews merely because normal capture skips
    /// their bytes. The materializer will refuse nonempty obstructing folders.
    pub fn untracked_obstructions(
        &self,
        before: &[CheckoutEntry],
        after: &[CheckoutEntry],
        limits: CheckoutLimits,
    ) -> io::Result<Vec<String>> {
        super::checkout_paths::validate(before)?;
        super::checkout_paths::validate(after)?;
        self.untracked_obstruction_paths(before.iter().map(|e|e.path.as_str()).collect(),
            after.iter().map(|e|e.path.as_str()).collect(),limits)
    }

    pub fn untracked_obstructions_projection(&self, before:&[CheckoutProjectionEntry],
        after:&[CheckoutProjectionEntry], limits:CheckoutLimits)->io::Result<Vec<String>> {
        validate_streaming_projection(before)?;
        validate_streaming_projection(after)?;
        self.untracked_obstruction_paths(before.iter().map(|e|e.path.as_str()).collect(),
            after.iter().map(|e|e.path.as_str()).collect(),limits)
    }

    fn untracked_obstruction_paths(&self,known:std::collections::BTreeSet<&str>,
        after:Vec<&str>,limits:CheckoutLimits)->io::Result<Vec<String>> {
        let mut pending = Vec::new();
        for entry in after {
            let path = self.root.join(entry);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() && !reparse(&metadata) => {
                    let (parent, name) = self.directory.existing_parent(Path::new(entry))?;
                    pending.push((
                        entry.to_owned(),
                        parent.open_child_for_traversal(&name)?,
                        0usize,
                    ));
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        let mut visited = 0usize;
        let mut collisions = Vec::new();
        while let Some((relative, directory, depth)) = pending.pop() {
            if depth > limits.max_depth.min(128) {
                return Err(invalid("checkout obstruction depth limit exceeded"));
            }
            directory.verify_path_identity()?;
            for name in list(&self.root.join(&relative), limits.max_entries)? {
                visited += 1;
                if visited > limits.max_entries {
                    return Err(invalid("checkout obstruction entry limit exceeded"));
                }
                let path = format!("{relative}/{name}");
                let prefix = format!("{path}/");
                let tracked = known.contains(path.as_str())
                    || known
                        .range(prefix.as_str()..)
                        .next()
                        .is_some_and(|p| p.starts_with(&prefix));
                if !tracked {
                    collisions.push(path);
                    continue;
                }
                let metadata = fs::symlink_metadata(self.root.join(&path))?;
                if metadata.is_dir() && !reparse(&metadata) {
                    pending.push((
                        path,
                        directory.open_child_for_traversal(name.as_ref())?,
                        depth + 1,
                    ));
                }
            }
            directory.verify_path_identity()?;
        }
        self.directory.verify_path_identity()?;
        collisions.sort();
        collisions.dedup();
        Ok(collisions)
    }

    /// Two matching bounded scans provide an optimistic stable capture. A live
    /// application must still coordinate its own multi-file transactions.
    pub fn capture(&self, limits: CheckoutLimits) -> io::Result<Vec<CheckoutEntry>> {
        self.capture_tracked(limits, &std::collections::BTreeSet::new())
    }
    pub fn capture_tracked(
        &self,
        limits: CheckoutLimits,
        tracked: &std::collections::BTreeSet<String>,
    ) -> io::Result<Vec<CheckoutEntry>> {
        self.capture_tracked_with_progress(limits, tracked, &mut || Ok(()))
    }
    pub(crate) fn capture_tracked_with_progress(
        &self,
        limits: CheckoutLimits,
        tracked: &std::collections::BTreeSet<String>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Vec<CheckoutEntry>> {
        let mut previous = None;
        for _ in 0..3 {
            progress()?;
            match self.scan(limits, tracked, progress) {
                Ok(current) => {
                    if previous.as_ref() == Some(&current) {
                        return Ok(current);
                    }
                    previous = Some(current);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    previous = None;
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "checkout changed during capture; retry after writes settle",
        ))
    }

    fn scan(
        &self,
        limits: CheckoutLimits,
        tracked: &std::collections::BTreeSet<String>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Vec<CheckoutEntry>> {
        self.directory.verify_path_identity()?;
        let mut state = Scan {
            progress,
            ignored: super::checkout_ignore::IgnoreRules::load(&self.directory)?,
            tracked,
            limits,
            entries: Vec::new(),
            visited: 0,
            bytes: 0,
        };
        state.walk(&self.directory, &self.root, "", 0)?;
        state.entries.sort_by(|a, b| a.path.cmp(&b.path));
        super::checkout_paths::validate(&state.entries)?;
        self.directory.verify_path_identity()?;
        Ok(state.entries)
    }
}

struct Scan<'a> {
    progress: &'a mut dyn FnMut() -> io::Result<()>,
    ignored: super::checkout_ignore::IgnoreRules,
    tracked: &'a std::collections::BTreeSet<String>,
    limits: CheckoutLimits,
    entries: Vec<CheckoutEntry>,
    visited: usize,
    bytes: usize,
}
impl Scan<'_> {
    fn walk(
        &mut self,
        directory: &PinnedDirectory,
        disk: &Path,
        relative: &str,
        depth: usize,
    ) -> io::Result<()> {
        if depth > self.limits.max_depth.min(128) {
            return Err(invalid("checkout depth limit exceeded"));
        }
        directory.verify_path_identity()?;
        let before = list(disk, self.limits.max_entries)?;
        for name in &before {
            (self.progress)()?;
            if name.eq_ignore_ascii_case(".layrs") || name.eq_ignore_ascii_case(".git") {
                continue;
            }
            self.visited += 1;
            if self.visited > self.limits.max_entries {
                return Err(invalid("checkout entry limit exceeded"));
            }
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            // Validate even empty directories and prevent normalization collisions.
            let canonical =
                layrs_core::scm::RepoPath::parse(&path).map_err(|e| invalid(&e.to_string()))?;
            if canonical.as_str() != path {
                return Err(invalid("checkout path is not canonical NFC"));
            }
            let full = disk.join(name);
            let metadata = fs::symlink_metadata(&full)?;
            let prefix = format!("{path}/");
            let tracked = self.tracked.contains(&path)
                || self
                    .tracked
                    .range(prefix.clone()..)
                    .next()
                    .is_some_and(|p| p.starts_with(&prefix));
            if !tracked
                && self.ignored.ignores(&path, metadata.is_dir())
                && !(metadata.is_dir() && self.ignored.has_exceptions)
            {
                continue;
            }
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&full)?;
                let target = target
                    .to_str()
                    .ok_or_else(|| invalid("symlink target is not UTF-8"))?
                    .to_owned();
                if target.len() > self.limits.max_file_bytes {
                    return Err(invalid("symlink target exceeds limit"));
                }
                if !same_metadata(&metadata, &fs::symlink_metadata(&full)?)
                    || fs::read_link(&full)?.to_str() != Some(&target)
                {
                    return Err(unstable());
                }
                self.add(path, CheckoutContent::Symlink { target })?;
            } else if metadata.is_dir() {
                let child = directory.open_child_for_traversal(name.as_ref())?;
                self.walk(&child, &full, &path, depth + 1)?;
            } else if metadata.is_file() {
                let mut file = directory.open_file(name.as_ref())?;
                let first = file.file().metadata()?;
                if first.len() > self.limits.max_file_bytes as u64
                    || first.len() > self.limits.max_total_bytes.saturating_sub(self.bytes) as u64
                {
                    return Err(invalid("checkout byte limit exceeded"));
                }
                let mut bytes = Vec::new();
                file.file_mut()
                    .take(self.limits.max_file_bytes.saturating_add(1) as u64)
                    .read_to_end(&mut bytes)?;
                let reopened = directory.open_file(name.as_ref())?;
                if !file.same_file(&reopened)
                    || !same_metadata(&first, &file.file().metadata()?)
                    || !same_metadata(&first, &reopened.file().metadata()?)
                    || bytes.len() as u64 != first.len()
                {
                    return Err(unstable());
                }
                self.add(
                    path,
                    CheckoutContent::File {
                        bytes,
                        executable: executable(&first),
                    },
                )?;
            } else {
                return Err(invalid("unsupported checkout filesystem entry"));
            }
        }
        directory.verify_path_identity()?;
        if list(disk, self.limits.max_entries)? != before {
            return Err(unstable());
        }
        Ok(())
    }

    fn add(&mut self, path: String, content: CheckoutContent) -> io::Result<()> {
        let count = match &content {
            CheckoutContent::File { bytes, .. } => bytes.len(),
            CheckoutContent::Symlink { target } => target.len(),
        };
        self.bytes = self
            .bytes
            .checked_add(count)
            .ok_or_else(|| invalid("checkout size overflow"))?;
        if self.bytes > self.limits.max_total_bytes {
            return Err(invalid("checkout byte limit exceeded"));
        }
        self.entries.push(CheckoutEntry { path, content });
        Ok(())
    }
}

fn list(path: &Path, limit: usize) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| invalid("checkout filename is not UTF-8"))?;
        if names.len() >= limit {
            return Err(invalid("checkout directory entry limit exceeded"));
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}
fn same_metadata(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len()
        && a.modified().ok() == b.modified().ok()
        && a.created().ok() == b.created().ok()
        && a.file_type() == b.file_type()
        && a.permissions() == b.permissions()
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn unstable() -> io::Error {
    io::Error::new(io::ErrorKind::WouldBlock, "checkout changed during capture")
}
fn executable(metadata: &fs::Metadata) -> bool {
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
fn reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

/// Hash the complete physical projection without choosing a chunk layout.
pub fn entries_tree_id(entries: &[CheckoutEntry]) -> io::Result<layrs_core::scm::TreeId> {
    use layrs_core::scm::{FileId, RepoPath, Symlink, TreeEntryTarget};
    let mut root = std::collections::BTreeMap::new();
    let mut total = 0usize;
    if entries.len() > 100_000 {
        return Err(invalid("checkout entry limit exceeded"));
    }
    for entry in entries {
        let canonical = RepoPath::parse(&entry.path).map_err(|e| invalid(&e.to_string()))?;
        let parts: Vec<_> = canonical.as_str().split('/').collect();
        if canonical.as_str() != entry.path
            || parts.len() > 128
            || parts
                .iter()
                .any(|p| p.eq_ignore_ascii_case(".layrs") || p.eq_ignore_ascii_case(".git"))
        {
            return Err(invalid("invalid checkout projection path"));
        }
        let (target, executable, bytes) = match &entry.content {
            CheckoutContent::File { bytes, executable } => (
                TreeEntryTarget::file(FileId::from_bytes(bytes)),
                *executable,
                bytes.len(),
            ),
            CheckoutContent::Symlink { target } => (
                TreeEntryTarget::symlink(
                    Symlink::new(target)
                        .map_err(|e| invalid(&e.to_string()))?
                        .id()
                        .clone(),
                ),
                false,
                target.len(),
            ),
        };
        total = total
            .checked_add(bytes)
            .ok_or_else(|| invalid("checkout size overflow"))?;
        if bytes > 64 * 1024 * 1024 || total > 256 * 1024 * 1024 {
            return Err(invalid("checkout byte limit exceeded"));
        }
        insert_projection(&mut root, &parts, Projection::Leaf(target, executable))?;
    }
    hash_projection(root)
}

enum Projection {
    Directory(std::collections::BTreeMap<String, Projection>),
    Leaf(layrs_core::scm::TreeEntryTarget, bool),
}
fn insert_projection(
    directory: &mut std::collections::BTreeMap<String, Projection>,
    parts: &[&str],
    value: Projection,
) -> io::Result<()> {
    if parts.len() == 1 {
        if directory.insert(parts[0].into(), value).is_some() {
            return Err(invalid("duplicate checkout projection path"));
        }
        return Ok(());
    }
    match directory
        .entry(parts[0].into())
        .or_insert_with(|| Projection::Directory(Default::default()))
    {
        Projection::Directory(children) => insert_projection(children, &parts[1..], value),
        Projection::Leaf(..) => Err(invalid("file/directory projection collision")),
    }
}
fn hash_projection(
    directory: std::collections::BTreeMap<String, Projection>,
) -> io::Result<layrs_core::scm::TreeId> {
    use layrs_core::scm::{RepoPathComponent, Tree, TreeEntry, TreeEntryTarget};
    let mut entries = Vec::new();
    for (name, value) in directory {
        let (target, executable) = match value {
            Projection::Directory(children) => {
                (TreeEntryTarget::tree(hash_projection(children)?), false)
            }
            Projection::Leaf(target, executable) => (target, executable),
        };
        entries.push(TreeEntry::new(
            RepoPathComponent::parse(name).map_err(|e| invalid(&e.to_string()))?,
            target,
            executable,
        ));
    }
    Ok(Tree::new(entries)
        .map_err(|e| invalid(&e.to_string()))?
        .id()
        .clone())
}
