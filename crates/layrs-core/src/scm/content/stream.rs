//! Incremental file identity construction/verification. Only chunk descriptors
//! are retained; content bytes belong to the caller and are never accumulated.
use super::*;

fn hasher(domain: &str) -> blake3::Hasher {
    let mut value = blake3::Hasher::new();
    value.update(domain.as_bytes());
    value.update(&[0]);
    value
}
fn invalid(reason: &str) -> ScmError {
    ScmError::InvalidField {
        field: "fileStream",
        reason: reason.into(),
    }
}

pub struct FileManifestBuilder {
    chunking: ChunkingDescriptor,
    chunks: Vec<FileChunk>,
    hash: blake3::Hasher,
    bytes: u64,
    max_bytes: u64,
    max_chunks: usize,
    failed: bool,
}
impl FileManifestBuilder {
    pub fn new(chunking: ChunkingDescriptor, max_bytes: u64, max_chunks: usize) -> Result<Self> {
        chunking.validate()?;
        Ok(Self {
            chunking,
            chunks: Vec::new(),
            hash: hasher(FILE_DOMAIN),
            bytes: 0,
            max_bytes,
            max_chunks,
            failed: false,
        })
    }
    pub fn push_chunk(&mut self, chunk: &Chunk) -> Result<()> {
        if self.failed {
            return Err(invalid("builder previously failed"));
        }
        self.failed = true;
        let next = self
            .bytes
            .checked_add(chunk.byte_length())
            .ok_or_else(|| invalid("file length overflow"))?;
        if next > self.max_bytes || self.chunks.len() >= self.max_chunks {
            return Err(invalid("file stream exceeds caller budget"));
        }
        let entry = FileChunk::new(chunk.id().clone(), self.bytes, chunk.byte_length())?;
        self.hash.update(chunk.bytes());
        self.chunks.push(entry);
        self.bytes = next;
        self.failed = false;
        Ok(())
    }
    pub fn finish(self) -> Result<FileManifest> {
        if self.failed {
            return Err(invalid("builder previously failed"));
        }
        FileManifest::new(
            FileId::from_digest(self.hash.finalize()),
            self.bytes,
            self.chunking,
            self.chunks,
        )
    }
}

pub struct FileManifestVerifier<'a> {
    manifest: &'a FileManifest,
    hash: blake3::Hasher,
    next: usize,
    bytes: u64,
    failed: bool,
}
impl FileManifestVerifier<'_> {
    /// One complete chunk, in manifest order. An error permanently invalidates
    /// this verifier; callers cannot ignore a bad chunk and finish successfully.
    pub fn push(&mut self, bytes: &[u8]) -> Result<()> {
        if self.failed {
            return Err(invalid("verifier previously failed"));
        }
        self.failed = true;
        let entry = self
            .manifest
            .chunks
            .get(self.next)
            .ok_or_else(|| invalid("extra chunk after file end"))?;
        if bytes.len() as u64 != entry.byte_length {
            return Err(ScmError::InvalidChunk(entry.chunk_id.to_string()));
        }
        if entry.offset != self.bytes {
            return Err(invalid("chunk offset differs from manifest"));
        }
        let mut chunk_hash = hasher(CHUNK_DOMAIN);
        chunk_hash.update(bytes);
        if ChunkId::from_digest(chunk_hash.finalize()) != entry.chunk_id {
            return Err(ScmError::InvalidChunk(entry.chunk_id.to_string()));
        }
        let next = self
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| invalid("file length overflow"))?;
        if next > self.manifest.byte_length {
            return Err(invalid("file length exceeds manifest"));
        }
        self.hash.update(bytes);
        self.bytes = next;
        self.next += 1;
        self.failed = false;
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        if self.failed {
            return Err(invalid("verifier previously failed"));
        }
        if self.next != self.manifest.chunks.len() || self.bytes != self.manifest.byte_length {
            return Err(invalid("file ended before manifest coverage"));
        }
        let computed = FileId::from_digest(self.hash.finalize());
        if computed != self.manifest.file_id {
            return Err(ScmError::IdentityMismatch {
                claimed: self.manifest.file_id.to_string(),
                computed: computed.to_string(),
            });
        }
        Ok(())
    }
}
impl FileManifest {
    pub fn verifier(&self) -> FileManifestVerifier<'_> {
        FileManifestVerifier {
            manifest: self,
            hash: hasher(FILE_DOMAIN),
            next: 0,
            bytes: 0,
            failed: false,
        }
    }
    /// Reads at most one owned chunk at a time. The resolver must separately
    /// enforce its I/O, authorization and individual allocation budgets.
    pub fn verify_chunks<F>(&self, mut resolve: F) -> Result<()>
    where
        F: FnMut(&ChunkId) -> Option<Vec<u8>>,
    {
        let mut verifier = self.verifier();
        for entry in &self.chunks {
            let bytes = resolve(&entry.chunk_id)
                .ok_or_else(|| ScmError::MissingChunk(entry.chunk_id.to_string()))?;
            verifier.push(&bytes)?;
        }
        verifier.finish()
    }
}
