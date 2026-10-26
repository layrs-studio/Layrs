//! Bounded byte transfer for one exact current-image representation.
use super::*;

const SCRATCH: usize = 64 * 1024;

pub(super) fn copy_verified(
    manifest: &FileManifest,
    output: &mut dyn Write,
    cancel: &Cancellation,
    mut read: impl FnMut(&mut [u8]) -> Result<()>,
) -> Result<()> {
    cancel.check()?;
    let mut file_hash = blake3::Hasher::new();
    file_hash.update(b"layrs-file-v1\0");
    let mut offset = 0u64;
    let mut scratch = [0u8; SCRATCH];
    for chunk in manifest.chunks() {
        cancel.check()?;
        if chunk.offset() != offset {
            return Err(invalid("managed copy chunk offset differs"));
        }
        let mut chunk_hash = blake3::Hasher::new();
        chunk_hash.update(b"layrs-chunk-v1\0");
        let mut remaining = chunk.byte_length();
        while remaining > 0 {
            cancel.check()?;
            let count = remaining.min(SCRATCH as u64) as usize;
            read(&mut scratch[..count])?;
            cancel.check()?;
            chunk_hash.update(&scratch[..count]);
            file_hash.update(&scratch[..count]);
            output.write_all(&scratch[..count])?;
            offset = offset
                .checked_add(count as u64)
                .ok_or_else(|| invalid("managed copy offset overflow"))?;
            remaining -= count as u64;
        }
        if format!("blake3:{}", chunk_hash.finalize()) != chunk.chunk_id().as_str() {
            return Err(invalid("managed copy chunk bytes differ"));
        }
    }
    cancel.check()?;
    if offset != manifest.byte_length()
        || format!("blake3:{}", file_hash.finalize()) != manifest.file_id().as_str()
    {
        return Err(invalid("managed copy full file bytes differ"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "stream/tests.rs"]
mod tests;
