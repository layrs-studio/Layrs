//! Bounded resident content reads for the authenticated partial provider.
//! The caller owns the Store gates and a validated read-only catalog transaction.
//! This module never opens SQLite or attempts journal recovery.
use super::provider_access::{FamilyAccess, OperationLocks};
use crate::{
    projection::{
        capture::MAX_CAPTURE_FILE_BYTES,
        provider::recovery::error::{self, ProviderBootstrapCause as Cause},
    },
    secure_fs::PinnedDirectory,
};
use layrs_core::scm::{canonical_json, FileId, FileManifest, FileManifestId};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, OptionalExtension};
use std::{ffi::OsStr, io::Read};

const MAX_RANGE: usize = 256 * 1024;
const MAX_MANIFEST: u64 = 256 * 1024;
const MAX_CHUNK: u64 = 1024 * 1024;
const IO_SCRATCH: usize = 64 * 1024;
const MAX_CHUNKS: usize = 1024;

fn invalid(message: &'static str) -> Cause {
    error::invalid(message)
}

/// Returns `None` only when this family has no manifest for the FileId. The
/// caller must separately establish that no accepted protection/claim requires
/// that content. Its private/activation validation and observable SQL close
/// precede copying the returned bytes into a native callback buffer.
pub(in crate::projection) fn resident_range(
    family: &FamilyAccess,
    _locks: &OperationLocks,
    catalog: &Connection,
    file_id: &FileId,
    trusted_manifest_id: Option<&FileManifestId>,
    expected_bytes: u64,
    offset: u64,
    length: usize,
    cancel: &Cancellation,
) -> error::Result<Option<Vec<u8>>> {
    cancel.check()?;
    if expected_bytes > MAX_CAPTURE_FILE_BYTES || length > MAX_RANGE {
        return Err(invalid("provider range exceeds content budget"));
    }
    let end = offset
        .checked_add(length as u64)
        .ok_or_else(|| invalid("provider range overflows"))?;
    if end > expected_bytes {
        return Err(invalid("provider range exceeds file length"));
    }
    family.verify()?;
    let Some(manifest) = read_manifest(
        catalog,
        file_id,
        trusted_manifest_id,
        expected_bytes,
        cancel,
    )?
    else {
        family.verify()?;
        cancel.check()?;
        return Ok(None);
    };
    let root = PinnedDirectory::open(family.root())?;
    let objects = root.open_child_for_traversal(OsStr::new("objects"))?;
    let manifests = objects.open_child_for_traversal(OsStr::new("manifests"))?;
    let manifest_name = object_name(manifest.id().as_str())?;
    let pin = manifests.open_file(OsStr::new(manifest_name))?;
    if pin.file().metadata()?.len() > MAX_MANIFEST {
        return Err(invalid("provider manifest file exceeds bound"));
    }
    let bytes = read_bounded(pin.file(), MAX_MANIFEST as usize, cancel)?;
    if bytes != canonical_json(&manifest).map_err(|e| invalid_message(e))? {
        return Err(invalid("provider manifest file differs from catalog"));
    }
    pin.verify_parent()?;
    if !pin.same_file(&manifests.open_file(OsStr::new(manifest_name))?) {
        return Err(invalid("provider manifest path changed during read"));
    }
    let chunks = objects.open_child_for_traversal(OsStr::new("chunks"))?;
    let mut result = Vec::with_capacity(length);
    // The private descriptor is an exact source commitment. When the family
    // has installed another representation of the same FileId, a canonical
    // manifest and correct individual chunk hashes alone do not prove its
    // claimed FileId. Hash the whole file in that case, without buffering it.
    // This fallback may read up to the 1 GiB file bound per requested range;
    // cancellation is checked between bounded reads, not within an OS read.
    let mut full_hash = (Some(manifest.id()) != trusted_manifest_id).then(|| {
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-file-v1\0");
        hash
    });
    for part in manifest.chunks() {
        let part_start = part.offset();
        let part_end = part_start
            .checked_add(part.byte_length())
            .ok_or_else(|| invalid("provider chunk extent overflows"))?;
        let intersects = part_end > offset && part_start < end;
        if !intersects && full_hash.is_none() {
            continue;
        }
        cancel.check()?;
        let name = object_name(part.chunk_id().as_str())?;
        let pin = chunks.open_file(OsStr::new(name))?;
        let chunk_start = if intersects {
            offset.max(part_start) - part_start
        } else {
            0
        };
        let chunk_end = if intersects {
            end.min(part_end) - part_start
        } else {
            0
        };
        read_chunk_range(
            &pin,
            part.byte_length(),
            part.chunk_id().as_str(),
            chunk_start,
            chunk_end,
            &mut result,
            full_hash.as_mut(),
            cancel,
        )?;
        pin.verify_parent()?;
        if !pin.same_file(&chunks.open_file(OsStr::new(name))?) {
            return Err(invalid("provider chunk path changed during read"));
        }
        #[cfg(test)]
        test_hooks::after_verified_chunk();
    }
    if result.len() != length {
        return Err(invalid("provider range has incomplete chunk coverage"));
    }
    if full_hash.is_some_and(|hash| file_id.as_str() != format!("blake3:{}", hash.finalize())) {
        return Err(invalid("provider local representation has a false FileId"));
    }
    chunks.verify_path_identity()?;
    manifests.verify_path_identity()?;
    objects.verify_path_identity()?;
    root.verify_path_identity()?;
    family.verify()?;
    cancel.check()?;
    Ok(Some(result))
}

fn read_manifest(
    catalog: &Connection,
    file_id: &FileId,
    trusted_manifest_id: Option<&FileManifestId>,
    expected_bytes: u64,
    cancel: &Cancellation,
) -> error::Result<Option<FileManifest>> {
    cancel.check()?;
    let Some(id) = select_manifest_id(catalog, file_id, trusted_manifest_id)? else {
        return Ok(None);
    };
    let bounds: Option<(u64, u64, u64, u64)> = catalog
        .query_row(
            "SELECT length(CAST(id AS BLOB)),length(CAST(file_id AS BLOB)),length(payload),bytes FROM manifests WHERE id=?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((id_length, file_length, payload_length, declared_bytes)) = bounds else {
        return Err(invalid("provider selected manifest disappeared"));
    };
    if id_length != 71
        || file_length != 71
        || payload_length > MAX_MANIFEST
        || declared_bytes != expected_bytes
    {
        return Err(invalid("provider manifest catalog bounds or length differ"));
    }
    let (catalog_file, payload): (String, Vec<u8>) = catalog.query_row(
        "SELECT file_id,payload FROM manifests WHERE id=?1",
        [id.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    object_name(id.as_str())?;
    let manifest: FileManifest = serde_json::from_slice(&payload).map_err(invalid_message)?;
    if canonical_json(&manifest).map_err(invalid_message)? != payload
        || manifest.id() != &id
        || catalog_file != file_id.as_str()
        || manifest.file_id() != file_id
        || manifest.byte_length() != expected_bytes
        || manifest.chunks().len() > MAX_CHUNKS
    {
        return Err(invalid("provider manifest identity differs"));
    }
    let index: (u64, u64) = catalog.query_row(
        "SELECT count(*),coalesce(max(length(CAST(chunk_id AS BLOB))),0) FROM (SELECT chunk_id FROM manifest_chunks WHERE manifest_id=?1 LIMIT 1025)",
        [id.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if index.0 != manifest.chunks().len() as u64 || index.0 > MAX_CHUNKS as u64 || index.1 > 71 {
        return Err(invalid(
            "provider manifest index cardinality or bound differs",
        ));
    }
    let mut query = catalog.prepare(
        "SELECT mc.ordinal,mc.chunk_id,c.bytes FROM manifest_chunks mc LEFT JOIN chunks c ON c.id=mc.chunk_id WHERE mc.manifest_id=?1 ORDER BY mc.ordinal LIMIT 1025",
    )?;
    let mut rows = query.query([id.as_str()])?;
    for (ordinal, part) in manifest.chunks().iter().enumerate() {
        cancel.check()?;
        let Some(row) = rows.next()? else {
            return Err(invalid("provider manifest index ends early"));
        };
        let actual_ordinal: u64 = row.get(0)?;
        let actual_id: String = row.get(1)?;
        let actual_bytes: Option<u64> = row.get(2)?;
        if actual_ordinal != ordinal as u64
            || actual_id != part.chunk_id().as_str()
            || actual_bytes != Some(part.byte_length())
            || part.byte_length() == 0
            || part.byte_length() > MAX_CHUNK
        {
            return Err(invalid("provider manifest chunk index differs"));
        }
    }
    if rows.next()?.is_some() {
        return Err(invalid("provider manifest index has extra rows"));
    }
    cancel.check()?;
    Ok(Some(manifest))
}

fn select_manifest_id(
    catalog: &Connection,
    file_id: &FileId,
    trusted_manifest_id: Option<&FileManifestId>,
) -> error::Result<Option<FileManifestId>> {
    if let Some(trusted) = trusted_manifest_id {
        let present: bool = catalog.query_row(
            "SELECT EXISTS(SELECT 1 FROM manifests WHERE id=?1)",
            [trusted.as_str()],
            |r| r.get(0),
        )?;
        if present {
            return Ok(Some(trusted.clone()));
        }
    }
    let key_bytes: Option<u64> = catalog
        .query_row(
            "SELECT length(CAST(id AS BLOB)) FROM manifests WHERE file_id=?1 ORDER BY id COLLATE BINARY LIMIT 1",
            [file_id.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(key_bytes) = key_bytes else {
        return Ok(None);
    };
    if key_bytes != 71 {
        return Err(invalid("provider manifest candidate key exceeds bound"));
    }
    let id: String = catalog.query_row(
        "SELECT id FROM manifests WHERE file_id=?1 ORDER BY id COLLATE BINARY LIMIT 1",
        [file_id.as_str()],
        |r| r.get(0),
    )?;
    Ok(Some(FileManifestId::parse(id).map_err(invalid_message)?))
}

#[cfg(test)]
#[path = "provider_content/exact_tests.rs"]
mod exact_tests;

fn object_name(id: &str) -> error::Result<&str> {
    let Some(name) = id.strip_prefix("blake3:") else {
        return Err(invalid("provider content key is invalid"));
    };
    if name.len() != 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("provider content key is invalid"));
    }
    Ok(name)
}

fn invalid_message(error: impl std::fmt::Display) -> Cause {
    error::invalid(error.to_string())
}

fn read_bounded(file: &std::fs::File, max: usize, cancel: &Cancellation) -> error::Result<Vec<u8>> {
    let mut reader = file;
    let mut result = Vec::new();
    let mut scratch = [0u8; IO_SCRATCH];
    loop {
        cancel.check()?;
        let read = reader.read(&mut scratch)?;
        if read == 0 {
            break;
        }
        if result
            .len()
            .checked_add(read)
            .is_none_or(|length| length > max)
        {
            return Err(invalid("provider content file exceeds bound"));
        }
        result.extend_from_slice(&scratch[..read]);
    }
    cancel.check()?;
    Ok(result)
}

fn read_chunk_range(
    pin: &crate::secure_fs::PinnedFile,
    expected: u64,
    id: &str,
    start: u64,
    end: u64,
    result: &mut Vec<u8>,
    mut full_hash: Option<&mut blake3::Hasher>,
    cancel: &Cancellation,
) -> error::Result<()> {
    if expected == 0 || expected > MAX_CHUNK || start > end || end > expected {
        return Err(invalid("provider chunk range is invalid"));
    }
    if pin.file().metadata()?.len() != expected {
        return Err(invalid("provider chunk file length differs"));
    }
    let mut reader = pin.file();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-chunk-v1\0");
    let mut consumed = 0u64;
    let mut scratch = [0u8; IO_SCRATCH];
    while consumed < expected {
        cancel.check()?;
        let want = (expected - consumed).min(IO_SCRATCH as u64) as usize;
        let read = reader.read(&mut scratch[..want])?;
        if read == 0 {
            return Err(invalid("provider chunk ends early"));
        }
        hasher.update(&scratch[..read]);
        if let Some(file_hash) = full_hash.as_mut() {
            file_hash.update(&scratch[..read]);
        }
        let next = consumed + read as u64;
        if start < next && end > consumed {
            let begin = (start.max(consumed) - consumed) as usize;
            let finish = (end.min(next) - consumed) as usize;
            result.extend_from_slice(&scratch[begin..finish]);
        }
        consumed = next;
    }
    cancel.check()?;
    let mut extra = [0u8; 1];
    if reader.read(&mut extra)? != 0 {
        return Err(invalid("provider chunk grew during read"));
    }
    if id != format!("blake3:{}", hasher.finalize()) {
        return Err(invalid("provider chunk digest differs"));
    }
    cancel.check()?;
    Ok(())
}

#[cfg(test)]
pub(in crate::projection) mod test_hooks {
    use std::sync::{Mutex, OnceLock};

    type Hook = Box<dyn FnOnce() + Send + 'static>;
    fn slot() -> &'static Mutex<Option<Hook>> {
        static HOOK: OnceLock<Mutex<Option<Hook>>> = OnceLock::new();
        HOOK.get_or_init(|| Mutex::new(None))
    }
    pub(in crate::projection) fn install(hook: impl FnOnce() + Send + 'static) {
        let mut slot = slot().lock().unwrap();
        assert!(slot.is_none(), "range checkpoint already installed");
        *slot = Some(Box::new(hook));
    }
    pub(super) fn after_verified_chunk() {
        let callback = slot().lock().unwrap().take();
        if let Some(callback) = callback {
            callback();
        }
    }
}
