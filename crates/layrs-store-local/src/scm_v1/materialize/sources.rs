use super::*;
use layrs_core::scm::{ChunkId, FileId};

/// All callbacks are local prepared I/O, never network operations.
pub struct CheckoutSources<'a> {
    pub chunks: &'a [(ChunkId, u64)],
    pub read_chunk: &'a mut dyn FnMut(&ChunkId) -> layrs_store::scm::Result<Vec<u8>>,
    pub write_file: &'a mut dyn FnMut(&str, &mut dyn Write) -> io::Result<()>,
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
