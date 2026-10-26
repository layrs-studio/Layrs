//! Bounded byte transfer for one exact current-image representation.
use super::*;

const SCRATCH: usize = 64 * 1024;

pub(super) fn exact_chunk(
    manifest: &FileManifest,
    ordinal: usize,
) -> Result<&layrs_core::scm::FileChunk> {
    let chunk = manifest
        .chunks()
        .get(ordinal)
        .ok_or_else(|| invalid("managed copy chunk ordinal absent"))?;
    let end = chunk
        .offset()
        .checked_add(chunk.byte_length())
        .filter(|end| *end <= manifest.byte_length())
        .ok_or_else(|| invalid("managed copy chunk range differs"))?;
    let expected_offset = if ordinal == 0 {
        0
    } else {
        let prior = &manifest.chunks()[ordinal - 1];
        prior
            .offset()
            .checked_add(prior.byte_length())
            .ok_or_else(|| invalid("managed copy prior chunk offset overflow"))?
    };
    if chunk.offset() != expected_offset
        || (ordinal == manifest.chunks().len() - 1 && end != manifest.byte_length())
    {
        return Err(invalid("managed copy chunk offset differs"));
    }
    Ok(chunk)
}

/// Verify exactly one chunk using absolute file offsets, without claiming a
/// full-file digest or reading any bytes outside the selected chunk.
pub(super) fn copy_verified_chunk(
    manifest: &FileManifest,
    ordinal: usize,
    output: &mut dyn Write,
    cancel: &Cancellation,
    mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
) -> Result<()> {
    cancel.check()?;
    let chunk = exact_chunk(manifest, ordinal)?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-chunk-v1\0");
    let mut offset = chunk.offset();
    let mut remaining = chunk.byte_length();
    let mut scratch = [0u8; SCRATCH];
    while remaining > 0 {
        cancel.check()?;
        let count = remaining.min(SCRATCH as u64) as usize;
        read(offset, &mut scratch[..count])?;
        cancel.check()?;
        hash.update(&scratch[..count]);
        output.write_all(&scratch[..count])?;
        cancel.check()?;
        offset = offset
            .checked_add(count as u64)
            .ok_or_else(|| invalid("managed copy chunk offset overflow"))?;
        remaining -= count as u64;
    }
    if format!("blake3:{}", hash.finalize()) != chunk.chunk_id().as_str() {
        return Err(invalid("managed copy chunk bytes differ"));
    }
    cancel.check()?;
    Ok(())
}

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
