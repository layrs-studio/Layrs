use crate::validation::{SyncResult, SyncValidationError};
use serde::{Deserialize, Serialize};

pub const SMALL_FILE_CHUNK_THRESHOLD: usize = 256 * 1024;
pub const CDC_MIN_CHUNK_BYTES: usize = 64 * 1024;
pub const CDC_TARGET_CHUNK_BYTES: usize = 1024 * 1024;
pub const CDC_MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChunkingStrategy {
    Single,
    DeterministicFixed,
    FastCdcPrepared,
}

impl ChunkingStrategy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::DeterministicFixed => "deterministic-fixed",
            Self::FastCdcPrepared => "fastcdc-prepared",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkSpan {
    pub offset: u64,
    pub byte_len: u64,
}

impl ChunkSpan {
    pub fn validate(&self) -> SyncResult<()> {
        if self.byte_len == 0 {
            return Err(SyncValidationError::new(
                "chunk_span.byte_len",
                "must be greater than zero",
            ));
        }
        Ok(())
    }
}

pub trait Chunker {
    fn strategy(&self) -> ChunkingStrategy;
    fn chunk(&self, bytes: &[u8]) -> Vec<ChunkSpan>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeterministicChunker {
    pub small_file_threshold: usize,
    pub min_chunk_bytes: usize,
    pub target_chunk_bytes: usize,
    pub max_chunk_bytes: usize,
}

impl DeterministicChunker {
    pub const fn new(
        small_file_threshold: usize,
        min_chunk_bytes: usize,
        target_chunk_bytes: usize,
        max_chunk_bytes: usize,
    ) -> Self {
        Self {
            small_file_threshold,
            min_chunk_bytes,
            target_chunk_bytes,
            max_chunk_bytes,
        }
    }
}

impl Default for DeterministicChunker {
    fn default() -> Self {
        Self::new(
            SMALL_FILE_CHUNK_THRESHOLD,
            CDC_MIN_CHUNK_BYTES,
            CDC_TARGET_CHUNK_BYTES,
            CDC_MAX_CHUNK_BYTES,
        )
    }
}

impl Chunker for DeterministicChunker {
    fn strategy(&self) -> ChunkingStrategy {
        ChunkingStrategy::DeterministicFixed
    }

    fn chunk(&self, bytes: &[u8]) -> Vec<ChunkSpan> {
        if bytes.is_empty() {
            return Vec::new();
        }

        if bytes.len() <= self.small_file_threshold {
            return vec![ChunkSpan {
                offset: 0,
                byte_len: bytes.len() as u64,
            }];
        }

        let target = self
            .target_chunk_bytes
            .clamp(self.min_chunk_bytes, self.max_chunk_bytes);
        let mut spans = Vec::new();
        let mut offset = 0_usize;

        while offset < bytes.len() {
            let remaining = bytes.len() - offset;
            let mut byte_len = remaining.min(target);
            let trailing = remaining.saturating_sub(byte_len);

            if trailing > 0 && trailing < self.min_chunk_bytes {
                byte_len = remaining.min(self.max_chunk_bytes);
            }

            spans.push(ChunkSpan {
                offset: offset as u64,
                byte_len: byte_len as u64,
            });
            offset += byte_len;
        }

        spans
    }
}
