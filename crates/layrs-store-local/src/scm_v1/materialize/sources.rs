use super::*;
use layrs_core::scm::{ChunkId, FileId};

/// All callbacks are local prepared I/O, never network operations.
#[allow(clippy::type_complexity)]
pub struct CheckoutSources<'a> {
    pub chunks: &'a [(ChunkId, u64)],
    pub read_chunk: &'a mut dyn FnMut(&ChunkId) -> layrs_store::scm::Result<Vec<u8>>,
    pub write_file: &'a mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>,
    /// Called once after the last `write_file`, before any journal/commit
    /// phase: a source may hold a shared Store capability while staging and
    /// must release it here (never across exclusive phases).
    pub end_staging: Option<&'a mut dyn FnMut()>,
    /// Optional thread-safe equivalent of `write_file`. When present, files
    /// are staged concurrently (reads, verification and writes overlap).
    pub write_file_shared: Option<&'a (dyn Fn(&str, &mut dyn Write) -> io::Result<()> + Sync)>,
}

/// One regular file to stage as `new-{index}` in the backup directory.
pub(super) struct StageJob<'a> {
    pub index: usize,
    pub path: &'a str,
    pub file_id: &'a FileId,
    pub length: u64,
}

/// Creates and writes every staged file, each through `VerifiedWriter`
/// (exclusive creation, bytes proven against the FileId and length). With a
/// shared source the files are staged concurrently; claims are in order and
/// the first failure in staging order is reported. No per-file flush: see the
/// checkout durability contract in `materialize`.
pub(super) fn stage_files(
    backup: &PinnedDirectory,
    jobs: &[StageJob<'_>],
    sources: &mut CheckoutSources<'_>,
) -> io::Result<std::collections::BTreeMap<usize, PinnedFile>> {
    // No other writer can open the staged file while we hold it: the
    // postcheck relies on that to vouch for the bytes proven here.
    let create = |job: &StageJob<'_>| {
        backup.create_file_denying_writers(format!("new-{}", job.index).as_ref())
    };
    write_files(&create, &|_, file| Ok(file), jobs, sources)
}

/// The directory part of a checkout path (`""` at the root).
fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Positions of the items whose directories are `parents`, in an order that
/// takes the directories in turn, one item each: consecutive positions are
/// in different directories as long as several remain.
pub(super) fn spread_by_directory<'p>(parents: impl Iterator<Item = &'p str>) -> Vec<usize> {
    let mut groups: std::collections::BTreeMap<&str, std::collections::VecDeque<usize>> =
        std::collections::BTreeMap::new();
    let mut count = 0;
    for (position, parent) in parents.enumerate() {
        groups.entry(parent).or_default().push_back(position);
        count += 1;
    }
    let mut queues: Vec<_> = groups.into_values().collect();
    let mut order = Vec::with_capacity(count);
    while !queues.is_empty() {
        queues.retain_mut(|queue| {
            order.extend(queue.pop_front());
            !queue.is_empty()
        });
    }
    order
}

/// A freshly created file the staging writer fills.
pub(super) trait CreatedFile: Send {
    fn output(&mut self) -> &mut dyn Write;
}
impl CreatedFile for PinnedFile {
    fn output(&mut self) -> &mut dyn Write {
        self.file_mut()
    }
}
impl CreatedFile for fs::File {
    fn output(&mut self) -> &mut dyn Write {
        self
    }
}

/// As `stage_files`, each file exclusively created by `create`, and what
/// `finish` makes of it once its bytes are proven kept (on its writer thread).
pub(super) fn write_files<H: CreatedFile, T: Send>(
    create: &(dyn Fn(&StageJob<'_>) -> io::Result<H> + Sync),
    finish: &(dyn Fn(&StageJob<'_>, H) -> io::Result<T> + Sync),
    jobs: &[StageJob<'_>],
    sources: &mut CheckoutSources<'_>,
) -> io::Result<std::collections::BTreeMap<usize, T>> {
    let _span = crate::perf::span("stage.files");
    let stage_one = |job: &StageJob<'_>,
                     write: &mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>|
     -> io::Result<T> {
        let created = crate::perf::span("stage.create");
        let mut file = create(job)?;
        drop(created);
        let _write = crate::perf::span("stage.write");
        let mut verified = VerifiedWriter::new(file.output(), job.file_id, job.length);
        write(job.path, &mut verified)?;
        verified.finish()?;
        finish(job, file)
    };
    let mut staged = std::collections::BTreeMap::new();
    let Some(shared) = sources.write_file_shared else {
        for job in jobs {
            staged.insert(job.index, stage_one(job, &mut *sources.write_file)?);
        }
        return Ok(staged);
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8)
        .min(jobs.len().max(1));
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    // Concurrent workers write into different directories: files created
    // at once in one NTFS directory serialize on it (8 threads, 1,000 files
    // each: about 3.7 times slower than in separate directories).
    let order = spread_by_directory(jobs.iter().map(|job| parent_of(job.path)));
    let mut results = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let (next, stop, stage_one, order) = (&next, &stop, &stage_one, &order);
                scope.spawn(move || {
                    let mut done = Vec::new();
                    let mut write = |path: &str, output: &mut dyn Write| shared(path, output);
                    while !stop.load(Ordering::Relaxed) {
                        let claimed = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&position) = order.get(claimed) else {
                            break;
                        };
                        let result = stage_one(&jobs[position], &mut write);
                        if result.is_err() {
                            stop.store(true, Ordering::Relaxed);
                        }
                        done.push((position, result));
                    }
                    done
                })
            })
            .collect::<Vec<_>>();
        let mut results = Vec::with_capacity(jobs.len());
        let mut panicked = false;
        for worker in workers {
            match worker.join() {
                Ok(done) => results.extend(done),
                Err(_) => panicked = true,
            }
        }
        if panicked {
            return Err(error("checkout staging worker panicked"));
        }
        Ok(results)
    })?;
    results.sort_by_key(|(position, _)| *position);
    for (position, result) in results {
        staged.insert(jobs[position].index, result?);
    }
    if staged.len() != jobs.len() {
        return Err(error(
            "checkout staging stopped before every file was written",
        ));
    }
    Ok(staged)
}

pub(super) struct VerifiedWriter<'a> {
    output: &'a mut dyn Write,
    hasher: blake3::Hasher,
    expected: &'a FileId,
    length: u64,
    written: u64,
    failed: bool,
}

impl<'a> VerifiedWriter<'a> {
    pub(super) fn new(output: &'a mut dyn Write, expected: &'a FileId, length: u64) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"layrs-file-v1\0");
        Self {
            output,
            hasher,
            expected,
            length,
            written: 0,
            failed: false,
        }
    }
    pub(super) fn finish(self) -> io::Result<()> {
        if self.failed
            || self.written != self.length
            || self.expected.as_str() != format!("blake3:{}", self.hasher.finalize().to_hex())
        {
            return Err(error(
                "staged file does not match its expected FileId/length",
            ));
        }
        Ok(())
    }
}

impl Write for VerifiedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed || bytes.len() as u64 > self.length.saturating_sub(self.written) {
            self.failed = true;
            return Err(error("staged file exceeds expected length"));
        }
        match self.output.write(bytes) {
            Ok(n) => {
                self.hasher.update(&bytes[..n]);
                self.written += n as u64;
                Ok(n)
            }
            Err(e) => {
                self.failed = true;
                Err(e)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.output.flush().inspect_err(|_| self.failed = true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_positions_spread_over_directories() {
        let parents = ["a", "a", "a", "b", "b", "", "c"];
        let order = spread_by_directory(parents.iter().copied());
        assert_eq!(order, vec![5, 0, 3, 6, 1, 4, 2]);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..parents.len()).collect::<Vec<_>>());
    }
    #[test]
    fn staged_writer_rejects_corruption_truncation_overrun_and_ignored_errors() {
        let id = FileId::from_bytes(b"abcd");
        for bytes in [&b"abc"[..], &b"abce"[..]] {
            let mut sink = Vec::new();
            let mut writer = VerifiedWriter::new(&mut sink, &id, 4);
            writer.write_all(bytes).unwrap();
            assert!(writer.finish().is_err());
        }
        let mut sink = Vec::new();
        let mut writer = VerifiedWriter::new(&mut sink, &id, 4);
        writer.write_all(b"abcd").unwrap();
        assert!(writer.write(b"e").is_err());
        assert!(
            writer.finish().is_err(),
            "ignoring the write error must not admit the file"
        );
        assert_eq!(sink, b"abcd");
        let mut sink = Vec::new();
        let mut writer = VerifiedWriter::new(&mut sink, &id, 4);
        writer.write_all(b"ab").unwrap();
        writer.write_all(b"cd").unwrap();
        writer.finish().unwrap();
    }
}
