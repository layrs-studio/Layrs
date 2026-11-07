use super::*;
use layrs_core::scm::{Chunk, FileManifest, FileManifestBuilder};
use std::collections::BTreeSet;

impl CheckoutReader {
    /// Optimistic two-pass capture, bounded to one chunk at a time per worker
    /// (`fixed_chunk_bytes`: 1 MiB, up to 16 MiB for files above 1 GiB).
    /// The observer may receive duplicates or chunks from an unstable attempt;
    /// it must retain them only in a bounded private preparation, never publish.
    pub fn capture_streaming(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<(Vec<CheckoutProjectionEntry>, Vec<FileManifest>)> {
        self.capture_streaming_expecting(limits, tracked, None, observe, progress)
    }

    /// As `capture_streaming`, given the projection the checkout is expected
    /// to hold (e.g. its base Step). A first pass exactly equal to `expected`
    /// is accepted alone, by the argument of `matches_projection`; any other
    /// result needs two equal passes as before.
    pub fn capture_streaming_expecting(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        expected: Option<&[CheckoutProjectionEntry]>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<(Vec<CheckoutProjectionEntry>, Vec<FileManifest>)> {
        let _span = crate::perf::span("checkout.capture_streaming");
        let mut previous = None;
        for attempt in 0..3 {
            progress()?;
            // A pass equal to the previous one was validated with it.
            let validated = previous
                .as_ref()
                .map(|(entries, _): &(Vec<CheckoutProjectionEntry>, _)| entries.as_slice())
                .or(expected);
            match self.scan_streaming(limits, tracked, observe, None, validated, progress) {
                Ok(current) => {
                    if previous.as_ref() == Some(&current)
                        || (attempt == 0 && expected == Some(current.0.as_slice()))
                    {
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

    /// As `capture_streaming_expecting`, where `trust` may vouch for a regular
    /// file from its listing metadata alone (a change index, ADR 0025): such a
    /// file is not opened. Every other file, listing and identity is checked
    /// as before. Returns the listing metadata of each file actually hashed,
    /// observed before it was read, so the caller can record what it proved.
    pub fn capture_streaming_trusting(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        expected: Option<&[CheckoutProjectionEntry]>,
        trust: &mut dyn FnMut(&str, &fs::Metadata) -> Option<TrustedFile>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<TrustedCapture> {
        self.capture_trusting_passes(
            limits, tracked, expected, trust, observe, progress, false, true,
        )
    }

    /// As `capture_streaming_trusting`, accepting the first complete pass
    /// (council r5, D4): for a capture that only records content, as git
    /// add does. Every hashed file is still one stable observation, every
    /// directory is relisted and an unstable pass is retried; only the
    /// second pass's cross-file consistency is not asked for. Never use it
    /// for the verification preceding a deletion or replacement.
    pub fn capture_streaming_trusting_single_pass(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        expected: Option<&[CheckoutProjectionEntry]>,
        trust: &mut dyn FnMut(&str, &fs::Metadata) -> Option<TrustedFile>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<TrustedCapture> {
        self.capture_trusting_passes(
            limits, tracked, expected, trust, observe, progress, true, true,
        )
    }

    /// A read-only observation of the folder for `status`, as git status
    /// makes it: one pass, each hashed file still one stable read, but the
    /// directories are not listed a second time to prove the pass atomic.
    /// Only per-file facts may be recorded from it (a file's own stable read
    /// against its own listing, as `status` refreshes racy index entries);
    /// never a result about the whole folder: never use it for a capture, or
    /// before deleting or replacing anything.
    pub fn observe_streaming_trusting(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        expected: Option<&[CheckoutProjectionEntry]>,
        trust: &mut dyn FnMut(&str, &fs::Metadata) -> Option<TrustedFile>,
    ) -> io::Result<TrustedCapture> {
        self.capture_trusting_passes(
            limits,
            tracked,
            expected,
            trust,
            &mut |_| Ok(()),
            &mut || Ok(()),
            true,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_trusting_passes(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        expected: Option<&[CheckoutProjectionEntry]>,
        trust: &mut dyn FnMut(&str, &fs::Metadata) -> Option<TrustedFile>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        progress: &mut dyn FnMut() -> io::Result<()>,
        single_pass: bool,
        relist: bool,
    ) -> io::Result<TrustedCapture> {
        let _span = crate::perf::span("checkout.capture_trusting");
        let mut previous: Option<TrustedCapture> = None;
        for attempt in 0..3 {
            progress()?;
            // A pass equal to the previous one was validated with it.
            let validated = previous
                .as_ref()
                .map(|previous| previous.entries.as_slice())
                .or(expected);
            match self.scan_streaming_listed(
                limits,
                tracked,
                observe,
                Some(&mut *trust),
                validated,
                progress,
                relist,
            ) {
                Ok(current) => {
                    let same = previous.as_ref().is_some_and(|previous| {
                        previous.entries == current.entries
                            && previous.manifests == current.manifests
                    });
                    if single_pass
                        || same
                        || (attempt == 0 && expected == Some(current.entries.as_slice()))
                    {
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

    /// Verification against a known projection. Two equal passes are what
    /// make a capture with no expectation meaningful; here the expectation is
    /// given, every file read is itself checked stable (same inode, metadata
    /// and listing before/after), so one pass equal to `expected` proves the
    /// match at that point. Any mismatch or instability falls back to the
    /// full two-pass capture, so a refusal decides exactly as before.
    pub fn matches_projection(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        expected: &[CheckoutProjectionEntry],
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        self.matches_projection_trusting(limits, tracked, expected, None, progress)
    }

    /// As `matches_projection`, where `trust` may vouch for a regular file
    /// without it being opened by path: the caller wrote that file itself
    /// (bytes proven while written) and still holds it, and answers only if
    /// its own handle shows the file unchanged and reachable at exactly that
    /// path. Every other entry, the listings and the identities are checked
    /// as before; a mismatch falls back to the untrusting two-pass capture.
    #[allow(clippy::type_complexity)]
    pub fn matches_projection_trusting(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        expected: &[CheckoutProjectionEntry],
        trust: Option<&mut dyn FnMut(&str, &fs::Metadata) -> Option<TrustedFile>>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        let _span = crate::perf::span("checkout.matches_projection");
        progress()?;
        match self.scan_streaming(
            limits,
            tracked,
            &mut |_| Ok(()),
            trust,
            Some(expected),
            progress,
        ) {
            Ok((entries, _)) if entries == expected => return Ok(true),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        Ok(self
            .capture_streaming(limits, tracked, &mut |_| Ok(()), progress)?
            .0
            == expected)
    }

    #[allow(clippy::type_complexity)]
    fn scan_streaming(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        trust: Option<&mut dyn FnMut(&str, &fs::Metadata) -> Option<TrustedFile>>,
        expected: Option<&[CheckoutProjectionEntry]>,
        progress: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<(Vec<CheckoutProjectionEntry>, Vec<FileManifest>)> {
        let scanned =
            self.scan_streaming_listed(limits, tracked, observe, trust, expected, progress, true)?;
        Ok((scanned.entries, scanned.manifests))
    }

    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn scan_streaming_listed(
        &self,
        limits: CheckoutLimits,
        tracked: &BTreeSet<String>,
        observe: &mut dyn FnMut(&Chunk) -> io::Result<()>,
        trust: Option<&mut dyn FnMut(&str, &fs::Metadata) -> Option<TrustedFile>>,
        expected: Option<&[CheckoutProjectionEntry]>,
        progress: &mut dyn FnMut() -> io::Result<()>,
        relist: bool,
    ) -> io::Result<TrustedCapture> {
        let _span = crate::perf::span("checkout.scan_pass");
        self.directory.verify_path_identity()?;
        let mut scan = StreamScan {
            limits,
            tracked,
            observe,
            // Re-coerce so the trait object lives as long as the scan.
            trust: match trust {
                Some(trust) => Some(trust),
                None => None,
            },
            progress,
            ignored: super::super::checkout_ignore::IgnoreRules::load(&self.directory)?,
            entries: Vec::new(),
            manifests: Vec::new(),
            visited: 0,
            bytes: 0,
            directories: Vec::new(),
            completed: 0,
            files: Vec::new(),
            hashed: Vec::new(),
            trusted: 0,
            relist,
        };
        scan.walk(self.directory.try_clone()?, self.root.clone(), "", 0)?;
        scan.flush()?;
        scan.entries.sort_by(|a, b| a.path.cmp(&b.path));
        scan.manifests.sort_by(|a, b| a.id().cmp(b.id()));
        scan.manifests.dedup_by(|a, b| a.id() == b.id());
        // A pass equal to the expected projection (read from an admitted Step,
        // where it was validated) is representable; any other is validated.
        if expected != Some(scan.entries.as_slice()) {
            projection_tree_id(&scan.entries)?;
        }
        self.directory.verify_path_identity()?;
        Ok(TrustedCapture {
            entries: scan.entries,
            manifests: scan.manifests,
            hashed: scan.hashed,
            trusted: scan.trusted,
        })
    }
}

/// Result of `capture_streaming_trusting`.
pub struct TrustedCapture {
    pub entries: Vec<CheckoutProjectionEntry>,
    pub manifests: Vec<FileManifest>,
    /// Listing metadata of every file hashed by the returned pass.
    pub hashed: Vec<(String, fs::Metadata)>,
    /// Regular files vouched for by the trust hook (not opened).
    pub trusted: usize,
}

/// One pass walks and classifies every entry in order, reading links and
/// recording regular files. In bounded batches it then hashes the recorded
/// files on a few worker threads (each file read is checked stable exactly as
/// before) and rechecks the identity and listing of every directory whose
/// subtree is complete, children before parents, closing its handle. Each
/// directory's listing is thus compared over a window covering all reads
/// below it, as in a sequential walk, and only the current directory chain
/// plus a bounded batch of directories stay open.
#[allow(clippy::type_complexity)]
struct StreamScan<'a> {
    limits: CheckoutLimits,
    tracked: &'a BTreeSet<String>,
    observe: &'a mut dyn FnMut(&Chunk) -> io::Result<()>,
    trust: Option<&'a mut dyn FnMut(&str, &fs::Metadata) -> Option<TrustedFile>>,
    progress: &'a mut dyn FnMut() -> io::Result<()>,
    ignored: super::super::checkout_ignore::IgnoreRules,
    entries: Vec<CheckoutProjectionEntry>,
    manifests: Vec<FileManifest>,
    visited: usize,
    bytes: u64,
    /// Walked directories in walk order; `None` once rechecked and closed.
    directories: Vec<Option<ScannedDirectory>>,
    /// Directories whose subtree is walked but not yet rechecked.
    completed: usize,
    /// Regular files to hash, in walk order.
    files: Vec<FileTask>,
    /// Listing metadata of hashed files, in walk order.
    hashed: Vec<(String, fs::Metadata)>,
    trusted: usize,
    /// Whether each completed directory is listed again (captures).
    relist: bool,
}

struct ScannedDirectory {
    handle: PinnedDirectory,
    disk: std::path::PathBuf,
    before: Vec<String>,
    complete: bool,
}

/// Batch bounds: pending files to hash, and walked directories kept open.
const FLUSH_FILES: usize = 2_048;
const FLUSH_DIRECTORIES: usize = 32;

/// A regular file vouched for by `matches_projection_trusting`'s caller.
pub struct TrustedFile {
    pub file_id: layrs_core::scm::FileId,
    pub length: u64,
    pub executable: bool,
}

struct FileTask {
    directory: usize,
    name: String,
    full: std::path::PathBuf,
    path: String,
    listed: fs::Metadata,
}

struct HashedFile {
    manifest: FileManifest,
    length: u64,
    executable: bool,
}

impl StreamScan<'_> {
    fn walk(
        &mut self,
        directory: PinnedDirectory,
        disk: std::path::PathBuf,
        relative: &str,
        depth: usize,
    ) -> io::Result<()> {
        if depth > self.limits.max_depth.min(128) {
            return Err(invalid("checkout depth limit exceeded"));
        }
        directory.verify_path_identity()?;
        let list_span = crate::perf::span("scan.list");
        let listed = list_with_metadata(&disk, self.limits.max_entries.min(100_000))?;
        drop(list_span);
        let _entries_span = crate::perf::span("scan.entries");
        let index = self.directories.len();
        self.directories.push(Some(ScannedDirectory {
            handle: directory,
            disk: disk.clone(),
            // Kept only to list the directory again (captures).
            before: if self.relist {
                listed.iter().map(|(name, _)| name.clone()).collect()
            } else {
                Vec::new()
            },
            complete: false,
        }));
        for (name, metadata) in listed {
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
            // A tracked path is a path of the validated base projection, so
            // already canonical; any other is validated here.
            let exact = self.tracked.contains(&path);
            if !exact {
                let canonical =
                    layrs_core::scm::RepoPath::parse(&path).map_err(|e| invalid(&e.to_string()))?;
                if canonical.as_str() != path {
                    return Err(invalid("checkout path is not canonical NFC"));
                }
            }
            // Only a directory can hold tracked descendants.
            let tracked = exact
                || (metadata.is_dir() && {
                    let prefix = format!("{path}/");
                    self.tracked
                        .range(prefix.clone()..)
                        .next()
                        .is_some_and(|p| p.starts_with(&prefix))
                });
            if !tracked
                && self.ignored.ignores(&path, metadata.is_dir())
                && !(metadata.is_dir() && self.ignored.has_exceptions)
            {
                continue;
            }
            if metadata.file_type().is_symlink() {
                let full = disk.join(&name);
                // Entry metadata only classifies; a link is re-read freshly.
                let metadata = &fs::symlink_metadata(&full)?;
                if !metadata.file_type().is_symlink() {
                    return Err(unstable());
                }
                let target = fs::read_link(&full)?;
                let target = target
                    .to_str()
                    .ok_or_else(|| invalid("symlink target is not UTF-8"))?
                    .to_owned();
                self.add_bytes(target.len() as u64)?;
                if !same_metadata(metadata, &fs::symlink_metadata(&full)?)
                    || fs::read_link(&full)?.to_str() != Some(&target)
                {
                    return Err(unstable());
                }
                self.entries.push(CheckoutProjectionEntry {
                    path,
                    content: CheckoutProjectionContent::Symlink { target },
                });
            } else if metadata.is_dir() {
                let full = disk.join(&name);
                let child = self.directories[index]
                    .as_ref()
                    .ok_or_else(|| io::Error::other("walked directory closed early"))?
                    .handle
                    .open_child_for_traversal(name.as_ref())
                    .map_err(|e| retype_error(e, &full, &metadata))?;
                self.walk(child, full, &path, depth + 1)?;
            } else if metadata.is_file() {
                if let Some(trusted) = self
                    .trust
                    .as_mut()
                    .and_then(|trust| trust(&path, &metadata))
                {
                    self.trusted += 1;
                    self.add_bytes(trusted.length)?;
                    self.entries.push(CheckoutProjectionEntry {
                        path,
                        content: CheckoutProjectionContent::File {
                            file_id: trusted.file_id,
                            byte_length: trusted.length,
                            executable: trusted.executable,
                        },
                    });
                    continue;
                }
                self.files.push(FileTask {
                    directory: index,
                    full: disk.join(&name),
                    name,
                    path,
                    listed: metadata,
                });
            } else {
                return Err(invalid("unsupported checkout filesystem entry"));
            }
        }
        if let Some(directory) = self.directories[index].as_mut() {
            directory.complete = true;
        }
        self.completed += 1;
        if self.files.len() >= FLUSH_FILES || self.completed >= FLUSH_DIRECTORIES {
            self.flush()?;
        }
        Ok(())
    }

    /// Hashes every pending file, then rechecks and closes every directory
    /// whose subtree is complete (children before parents).
    fn flush(&mut self) -> io::Result<()> {
        self.hash_files()?;
        let limit = self.limits.max_entries.min(100_000);
        for slot in self.directories.iter_mut().rev() {
            let Some(directory) = slot.as_ref().filter(|d| d.complete) else {
                continue;
            };
            (self.progress)()?;
            directory.handle.verify_path_identity()?;
            if self.relist {
                let _relist = crate::perf::span("scan.relist");
                if list(&directory.disk, limit)? != directory.before {
                    return Err(unstable());
                }
            }
            *slot = None;
        }
        self.completed = 0;
        Ok(())
    }

    /// Hashes recorded files concurrently. Chunks are forwarded to the
    /// observer on this thread; results are applied in walk order and the
    /// first failing file in walk order is reported, as a sequential pass
    /// would (files are claimed in order and every claimed file completes).
    fn hash_files(&mut self) -> io::Result<()> {
        use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
        let _span = crate::perf::span("checkout.hash_files");
        let tasks = std::mem::take(&mut self.files);
        if tasks.is_empty() {
            return Ok(());
        }
        // Measured on NTFS (2026-11-04): warm files scale to ~2 threads and
        // contend beyond; freshly written files (antivirus scan on first
        // open) keep scaling. Four balances both.
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, 4)
            .min(tasks.len());
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let observer_failed = AtomicBool::new(false);
        let limits = self.limits;
        // Running total charged by each worker before it reads a file.
        let total = AtomicU64::new(self.bytes);
        let total = &total;
        let directories = &self.directories;
        let observe = &mut self.observe;
        let progress = &mut self.progress;
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Chunk>(threads * 2);
        let (mut results, failure) = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    let sender = sender.clone();
                    let (tasks, next, stop, observer_failed) =
                        (&tasks, &next, &stop, &observer_failed);
                    scope.spawn(move || {
                        let mut done = Vec::new();
                        while !stop.load(Ordering::Relaxed) {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(task) = tasks.get(index) else { break };
                            let result = match &directories[task.directory] {
                                Some(directory) => hash_file(
                                    &directory.handle,
                                    task,
                                    limits,
                                    total,
                                    &sender,
                                    observer_failed,
                                ),
                                None => Err(io::Error::other("walked directory closed early")),
                            };
                            if result.is_err() {
                                stop.store(true, Ordering::Relaxed);
                            }
                            done.push((index, result));
                        }
                        done
                    })
                })
                .collect();
            drop(sender);
            let mut failure = None;
            loop {
                match receiver.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(chunk) => {
                        if failure.is_none() {
                            if let Err(e) = observe(&chunk) {
                                failure = Some(e);
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
                if failure.is_none() {
                    if let Err(e) = progress() {
                        failure = Some(e);
                    }
                }
                if failure.is_some() {
                    stop.store(true, Ordering::Relaxed);
                    observer_failed.store(true, Ordering::Relaxed);
                }
            }
            let mut results = Vec::with_capacity(tasks.len());
            for worker in workers {
                match worker.join() {
                    Ok(done) => results.extend(done),
                    Err(_) => {
                        failure.get_or_insert_with(|| {
                            io::Error::other("checkout hash worker panicked")
                        });
                    }
                }
            }
            (results, failure)
        });
        if let Some(failure) = failure {
            return Err(failure);
        }
        self.bytes = total.load(Ordering::Relaxed);
        results.sort_by_key(|(index, _)| *index);
        for (index, result) in results {
            let hashed = result?;
            self.hashed
                .push((tasks[index].path.clone(), tasks[index].listed.clone()));
            self.entries.push(CheckoutProjectionEntry {
                path: tasks[index].path.clone(),
                content: CheckoutProjectionContent::File {
                    file_id: hashed.manifest.file_id().clone(),
                    byte_length: hashed.length,
                    executable: hashed.executable,
                },
            });
            self.manifests.push(hashed.manifest);
        }
        Ok(())
    }

    fn add_bytes(&mut self, count: u64) -> io::Result<()> {
        self.bytes = self
            .bytes
            .checked_add(count)
            .ok_or_else(|| invalid("checkout size overflow"))?;
        if count > self.limits.max_file_bytes as u64
            || self.bytes > self.limits.max_total_bytes as u64
        {
            return Err(invalid("streaming checkout byte limit exceeded"));
        }
        Ok(())
    }
}

/// Reads one regular file as one stable observation: same file reopened by
/// name, unchanged metadata, exact length, end of file. Chunks go to `sink`.
fn hash_file(
    directory: &PinnedDirectory,
    task: &FileTask,
    limits: CheckoutLimits,
    total: &std::sync::atomic::AtomicU64,
    sink: &std::sync::mpsc::SyncSender<Chunk>,
    observer_failed: &std::sync::atomic::AtomicBool,
) -> io::Result<HashedFile> {
    let mut file = directory
        .open_file(task.name.as_ref())
        .map_err(|e| retype_error(e, &task.full, &task.listed))?;
    let first = file.file().metadata()?;
    // Both bounds are charged before any byte of this file is read.
    let charged = total
        .fetch_add(first.len(), std::sync::atomic::Ordering::Relaxed)
        .saturating_add(first.len());
    if first.len() > limits.max_file_bytes as u64 || charged > limits.max_total_bytes as u64 {
        return Err(invalid("streaming checkout byte limit exceeded"));
    }
    let chunk_bytes = layrs_core::scm::fixed_chunk_bytes(first.len());
    let chunks = usize::try_from(first.len().div_ceil(chunk_bytes))
        .map_err(|_| invalid("file chunk count exceeds this platform"))?;
    let mut builder = FileManifestBuilder::new(
        layrs_core::scm::fixed_chunking(first.len()),
        first.len(),
        chunks,
    )
    .map_err(|e| invalid(&e.to_string()))?;
    let mut remaining = first.len();
    while remaining != 0 {
        if observer_failed.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(io::Error::other("checkout scan stopped"));
        }
        let count = remaining.min(chunk_bytes) as usize;
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
        sink.send(chunk)
            .map_err(|_| io::Error::other("checkout scan stopped"))?;
        remaining -= count as u64;
    }
    if file.file_mut().read(&mut [0u8; 1])? != 0 {
        return Err(unstable());
    }
    let reopened = directory.open_file(task.name.as_ref())?;
    if !file.same_file(&reopened)
        || !same_metadata(&first, &file.file().metadata()?)
        || !same_metadata(&first, &reopened.file().metadata()?)
    {
        return Err(unstable());
    }
    let manifest = builder.finish().map_err(|e| invalid(&e.to_string()))?;
    Ok(HashedFile {
        manifest,
        length: first.len(),
        executable: executable(&first),
    })
}

/// The listing's entry metadata classifies each name without another path
/// stat. If opening by that class fails because the entry changed kind since
/// the listing, the pass is unstable (retried), as a fresh stat would have
/// classified it differently; any other failure is reported unchanged.
fn retype_error(error: io::Error, full: &Path, listed: &fs::Metadata) -> io::Error {
    match fs::symlink_metadata(full) {
        Ok(fresh) if fresh.file_type() != listed.file_type() => unstable(),
        _ => error,
    }
}
