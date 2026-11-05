//! Private immutable checkpoints. A newly issued capability is always required;
//! neither bearer secrets nor capability/resume tokens are written to disk.
use super::*;
use layrs_store_local::download_cache::{
    DownloadCacheLease, DEFAULT_DOWNLOAD_CACHE_BYTES, MAX_CACHE_RECORDS, MAX_CACHE_RECORD_BYTES,
};
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;

const HEADER_LIMIT: usize = 2048;
const SEGMENT_LIMIT: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    binding: String,
    start: u64,
    end: u64,
    previous: String,
    digest: String,
    capability_digest: String,
}

pub(super) struct Resume {
    lease: Option<DownloadCacheLease>,
    root: PathBuf,
    binding: String,
    capability_digest: String,
    previous: String,
    count: usize,
    pub offset: u64,
}

impl NativeRemote {
    /// Dedicated owner-only cache directory. Its parent must already exist.
    /// An override is useful for isolated clients and tests; it must not point
    /// at a Worktree or an existing directory containing unrelated files.
    pub fn with_download_cache(mut self, path: impl Into<PathBuf>) -> Self {
        self.download_cache = Some(path.into());
        self
    }

    pub(super) fn resume_download(
        &self,
        cap: &DownloadCapability,
        spool: &mut File,
    ) -> RemoteResult<Option<Resume>> {
        // Encoded zstd representations may change across plans. Only identity
        // bytes have a stable representation suitable for persisted prefixes.
        if !cap.accept_ranges
            || cap.descriptor.compression != Compression::Identity
            || cap.descriptor.encoded_byte_len == 0
        {
            return Ok(None);
        }
        let binding = binding(self.origin.as_str(), cap)?;
        let root = match &self.download_cache {
            Some(path) => path.clone(),
            None => default_root()?,
        };
        // Probe only existence before opening the pinned private cache. Most
        // small objects complete in one response and need no durable writes.
        let lease = if root.join(format!("entry-{binding}")).try_exists()? {
            Some(DownloadCacheLease::open(
                &root,
                &binding,
                DEFAULT_DOWNLOAD_CACHE_BYTES,
            )?)
        } else {
            None
        };
        let mut resume = Resume {
            lease,
            root,
            binding,
            capability_digest: hash(cap.token.as_bytes()),
            previous: hash(b"layrs.download-prefix.v1"),
            count: 0,
            offset: 0,
        };
        if let Err(error) = resume.load(spool, cap.descriptor.encoded_byte_len) {
            // Refuse this attempt; a later explicitly retried transfer may
            // start fresh instead of repeatedly reading a corrupt prefix.
            if let Some(lease) = &resume.lease {
                lease.clear()?;
            }
            return Err(error);
        }
        Ok(Some(resume))
    }
}

fn default_root() -> RemoteResult<PathBuf> {
    #[cfg(windows)]
    let parent = std::env::var_os("LOCALAPPDATA");
    #[cfg(not(windows))]
    let parent = std::env::var_os("HOME");
    let parent = parent.ok_or(NativeRemoteError::Invalid(
        "private download cache location unavailable",
    ))?;
    Ok(PathBuf::from(parent).join(".layrs-download-resume-v1"))
}

fn binding(origin: &str, cap: &DownloadCapability) -> RemoteResult<String> {
    Ok(hash(&serde_json::to_vec(&(
        "layrs.download-binding.v1",
        origin,
        &cap.space_id,
        &cap.principal_id,
        cap.authorization_epoch,
        &cap.claim,
        &cap.descriptor,
    ))?))
}
fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}
fn invalid() -> NativeRemoteError {
    NativeRemoteError::Invalid("durable download checkpoint integrity mismatch")
}
impl Resume {
    pub(super) fn renewed(&mut self, cap: &DownloadCapability) {
        self.capability_digest = hash(cap.token.as_bytes());
    }
    fn load(&mut self, spool: &mut File, total: u64) -> RemoteResult<()> {
        let Some(lease) = &self.lease else {
            return Ok(());
        };
        let mut gap = false;
        for index in 0..MAX_CACHE_RECORDS {
            let Some(file) = lease.record(index)? else {
                gap = true;
                continue;
            };
            if gap {
                return Err(invalid());
            }
            let mut bytes = Vec::new();
            file.take(MAX_CACHE_RECORD_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() < 4 || bytes.len() > MAX_CACHE_RECORD_BYTES {
                return Err(invalid());
            }
            let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
            if size > HEADER_LIMIT || size + 4 > bytes.len() {
                return Err(invalid());
            }
            let header: Header =
                serde_json::from_slice(&bytes[4..4 + size]).map_err(|_| invalid())?;
            let data = &bytes[4 + size..];
            if header.version != 1
                || header.binding != self.binding
                || header.start != self.offset
                || header.end <= header.start
                || header.end > total
                || header.end - header.start != data.len() as u64
                || data.len() > SEGMENT_LIMIT
                || header.previous != self.previous
                || header.digest != hash(data)
                || header.capability_digest.len() != 64
                || !header
                    .capability_digest
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit())
            {
                return Err(invalid());
            }
            spool.write_all(data)?;
            self.previous = hash(&bytes);
            self.offset = header.end;
            self.count += 1;
        }
        Ok(())
    }

    pub fn save(&mut self, spool: &mut File, offset: u64) -> RemoteResult<()> {
        if offset > self.offset && self.lease.is_none() {
            let lease =
                DownloadCacheLease::open(&self.root, &self.binding, DEFAULT_DOWNLOAD_CACHE_BYTES)?;
            // Another process may have completed a prefix since the initial
            // probe. Never overwrite its immutable records with this attempt.
            if lease.record(0)?.is_some() {
                return Err(NativeRemoteError::Invalid(
                    "download checkpoint appeared concurrently; retry",
                ));
            }
            self.lease = Some(lease);
        }
        let position = spool.stream_position()?;
        while self.offset < offset {
            if self.count == MAX_CACHE_RECORDS {
                return Err(invalid());
            }
            let end = (self.offset + SEGMENT_LIMIT as u64).min(offset);
            let mut data = vec![0; (end - self.offset) as usize];
            spool.seek(SeekFrom::Start(self.offset))?;
            spool.read_exact(&mut data)?;
            let header = serde_json::to_vec(&Header {
                version: 1,
                binding: self.binding.clone(),
                start: self.offset,
                end,
                previous: self.previous.clone(),
                digest: hash(&data),
                capability_digest: self.capability_digest.clone(),
            })?;
            if header.len() > HEADER_LIMIT {
                return Err(invalid());
            }
            let mut record = Vec::with_capacity(4 + header.len() + data.len());
            record.extend_from_slice(&(header.len() as u32).to_be_bytes());
            record.extend_from_slice(&header);
            record.extend_from_slice(&data);
            self.lease
                .as_ref()
                .ok_or_else(invalid)?
                .append(self.count, &record)?;
            self.previous = hash(&record);
            self.offset = end;
            self.count += 1;
        }
        spool.seek(SeekFrom::Start(position))?;
        Ok(())
    }
    pub fn clear(self) -> RemoteResult<()> {
        if let Some(lease) = self.lease {
            lease.discard()?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "download_resume_tests.rs"]
mod tests;
