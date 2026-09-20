use super::*;
use layrs_core::scm::{Chunk, ChunkingDescriptor, FileManifest, FileManifestBuilder};
use std::collections::BTreeSet;

const CHUNK_BYTES: usize = 1024 * 1024;

impl CheckoutReader {
    /// Optimistic two-pass capture, bounded to one 1MiB chunk at a time.
    /// The observer may receive duplicates or chunks from an unstable attempt;
    /// it must retain them only in a bounded private preparation, never publish.
    pub fn capture_streaming(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<(Vec<CheckoutProjectionEntry>, Vec<FileManifest>)> {
        let mut previous = None;
        for _ in 0..3 {
            progress()?;
            match self.scan_streaming(limits, tracked, observe, progress) {
                Ok(current) => {
                    if previous.as_ref() == Some(&current) {
                        return Ok(current);
                    }
                    previous = Some(current);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => previous = None,
                Err(e) => return Err(e),
            }
        }
        Err(unstable())
    }

    fn scan_streaming(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<(Vec<CheckoutProjectionEntry>, Vec<FileManifest>)> {
        self.directory.verify_path_identity()?;
        let mut scan = StreamScan {
            limits,
            tracked,
            observe,
            progress,
            ignored: super::super::checkout_ignore::IgnoreRules::load(&self.directory)?,
            entries: Vec::new(),
            manifests: Vec::new(),
            visited: 0,
            bytes: 0,
        };
        scan.walk(&self.directory, &self.root, "", 0)?;
        scan.entries.sort_by(|a, b| a.path.cmp(&b.path));
        scan.manifests.sort_by(|a, b| a.id().cmp(b.id()));
        scan.manifests.dedup_by(|a, b| a.id() == b.id());
        projection_tree_id(&scan.entries)?;
        self.directory.verify_path_identity()?;
        Ok((scan.entries, scan.manifests))
    }
}

struct StreamScan<'a> {
    limits: CheckoutLimits,
    tracked: &'a BTreeSet<String>,
    observe: &'a mut dyn FnMut(&Chunk) -> io::Result<()>,
    progress: &'a mut dyn FnMut() -> io::Result<()>,
    ignored: super::super::checkout_ignore::IgnoreRules,
    entries: Vec<CheckoutProjectionEntry>,
    manifests: Vec<FileManifest>,
    visited: usize,
    bytes: u64,
}

impl StreamScan<'_> {
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
        let before = list(disk, self.limits.max_entries.min(100_000))?;
        for name in &before {
            (self.progress)()?;
            if name.eq_ignore_ascii_case(".layrs") || name.eq_ignore_ascii_case(".git") {
                continue;
            }
            self.visited += 1;
            if self.visited > self.limits.max_entries.min(100_000) {
                return Err(invalid("checkout entry limit exceeded"));
            }
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
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
                self.add_bytes(target.len() as u64)?;
                if !same_metadata(&metadata, &fs::symlink_metadata(&full)?)
                    || fs::read_link(&full)?.to_str() != Some(&target)
                {
                    return Err(unstable());
                }
                self.entries.push(CheckoutProjectionEntry {
                    path,
                    content: CheckoutProjectionContent::Symlink { target },
                });
            } else if metadata.is_dir() {
                let child = directory.open_child_for_traversal(name.as_ref())?;
                self.walk(&child, &full, &path, depth + 1)?;
            } else if metadata.is_file() {
                let mut file = directory.open_file(name.as_ref())?;
                let first = file.file().metadata()?;
                self.add_bytes(first.len())?;
                let chunking = ChunkingDescriptor::deterministic_fixed(CHUNK_BYTES as u64)
                    .map_err(|e| invalid(&e.to_string()))?;
                let mut builder = FileManifestBuilder::new(chunking, first.len(), 1024)
                    .map_err(|e| invalid(&e.to_string()))?;
                let mut remaining = first.len();
                while remaining != 0 {
                    (self.progress)()?;
                    let count = remaining.min(CHUNK_BYTES as u64) as usize;
                    let mut bytes = vec![0; count];
                    file.file_mut().read_exact(&mut bytes).map_err(|e| {
                        if e.kind() == io::ErrorKind::UnexpectedEof {
                            unstable()
                        } else {
                            e
                        }
                    })?;
                    let chunk = Chunk::new(bytes).map_err(|e| invalid(&e.to_string()))?;
                    builder
                        .push_chunk(&chunk)
                        .map_err(|e| invalid(&e.to_string()))?;
                    (self.observe)(&chunk)?;
                    remaining -= count as u64;
                }
                if file.file_mut().read(&mut [0u8; 1])? != 0 {
                    return Err(unstable());
                }
                let reopened = directory.open_file(name.as_ref())?;
                if !file.same_file(&reopened)
                    || !same_metadata(&first, &file.file().metadata()?)
                    || !same_metadata(&first, &reopened.file().metadata()?)
                {
                    return Err(unstable());
                }
                let manifest = builder.finish().map_err(|e| invalid(&e.to_string()))?;
                self.entries.push(CheckoutProjectionEntry {
                    path,
                    content: CheckoutProjectionContent::File {
                        file_id: manifest.file_id().clone(),
                        byte_length: first.len(),
                        executable: executable(&first),
                    },
                });
                self.manifests.push(manifest);
            } else {
                return Err(invalid("unsupported checkout filesystem entry"));
            }
        }
        directory.verify_path_identity()?;
        if list(disk, self.limits.max_entries.min(100_000))? != before {
            return Err(unstable());
        }
        Ok(())
    }

    fn add_bytes(&mut self, count: u64) -> io::Result<()> {
        self.bytes = self
            .bytes
            .checked_add(count)
            .ok_or_else(|| invalid("checkout size overflow"))?;
        if count > self.limits.max_file_bytes.min(1024 * 1024 * 1024) as u64
            || self.bytes > self.limits.max_total_bytes.min(4 * 1024 * 1024 * 1024) as u64
        {
            return Err(invalid("streaming checkout byte limit exceeded"));
        }
        Ok(())
    }
}
