use super::*;
use layrs_core::scm::{ChunkId, FileId};

/// All callbacks are local prepared I/O, never network operations.
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
    let _span = crate::perf::span("stage.files");
    let stage_one = |job: &StageJob<'_>,
                     write: &mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>|
     -> io::Result<PinnedFile> {
        // No other writer can open the staged file while we hold it: the
        // postcheck relies on that to vouch for the bytes proven here.
        let mut file = backup.create_file_denying_writers(format!("new-{}", job.index).as_ref())?;
        let mut verified = VerifiedWriter::new(file.file_mut(), job.file_id, job.length);
        write(job.path, &mut verified)?;
        verified.finish()?;
        Ok(file)
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
    let mut results = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let (next, stop, stage_one) = (&next, &stop, &stage_one);
                scope.spawn(move || {
                    let mut done = Vec::new();
                    let mut write = |path: &str, output: &mut dyn Write| shared(path, output);
                    while !stop.load(Ordering::Relaxed) {
                        let position = next.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(position) else { break };
                        let result = stage_one(job, &mut write);
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
        return Err(error("checkout staging stopped before every file was written"));
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
