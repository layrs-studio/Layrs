use super::*;
use crate::secure_fs::{PinnedDirectory, PinnedFile};
use layrs_core::{
    projection::EntryState,
    scm::{
        Chunk, ChunkId, ChunkingDescriptor, FileChunk, FileManifest, FileManifestBuilder,
        TreeEntryTarget,
    },
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(super) fn persist(
    c: &mut Connection,
    store: &ProjectionStore,
    state: &VersionedProjection,
    scan: scan::Scan,
    faults: &dyn CaptureFaults,
) -> Result<CapturedPatch> {
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let patch = persist_in_transaction(&tx, store, state, scan)?;
    faults.check(CapturePoint::BeforeCommit)?;
    tx.commit()?;
    faults.check(CapturePoint::AfterCommit)?;
    Ok(patch)
}
pub(in crate::projection) fn persist_in_transaction(
    c: &Connection,
    store: &ProjectionStore,
    state: &VersionedProjection,
    scan: scan::Scan,
) -> Result<CapturedPatch> {
    let previous: Option<u64> =
        c.query_row("SELECT MAX(sequence) FROM projection_captures", [], |r| {
            r.get(0)
        })?;
    let parent = previous.map(|n| load(c, store, n)).transpose()?;
    if let Some(previous) = &parent {
        if previous.projection_digest == state.digest && previous.changes == scan.changes {
            return Ok(previous.clone());
        }
    }
    let sequence = previous
        .unwrap_or(0)
        .checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| invalid("capture sequence exhausted"))?;
    let mut patch = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence,
        previous_patch_digest: parent.map(|p| p.digest),
        projection_generation: state.generation,
        projection_digest: state.digest.clone(),
        identity: store.identity().clone(),
        changes: scan.changes.clone(),
        digest: String::new(),
    };
    patch.digest = patch.computed_digest()?;
    let payload = canonical_json(&patch).map_err(invalid)?;
    if payload.len() > MAX_DOCUMENT {
        return Err(invalid("capture document exceeds8MiB"));
    }
    for change in &patch.changes {
        if let Some(file) = &change.replacement {
            persist_file(c, store, &change.path, file)?;
        }
    }
    if scan::scan(store, &state.snapshot, false)?.files != scan.files {
        return Err(invalid("checkout changed while storing partial capture"));
    }
    c.execute("INSERT INTO projection_captures(sequence,digest,projection_generation,payload) VALUES(?1,?2,?3,?4)",params![sequence,patch.digest,state.generation,payload])?;
    // Includes hashes of existing deduplicated blobs: corruption never becomes a successful capture.
    validate(c, store, &patch)?;
    Ok(patch)
}
pub(in crate::projection) fn load(
    c: &Connection,
    store: &ProjectionStore,
    sequence: u64,
) -> Result<CapturedPatch> {
    load_inner(c, store, sequence, true)
}
pub(in crate::projection) fn load_metadata(
    c: &Connection,
    store: &ProjectionStore,
    sequence: u64,
) -> Result<CapturedPatch> {
    load_inner(c, store, sequence, false)
}
fn load_inner(
    c: &Connection,
    store: &ProjectionStore,
    sequence: u64,
    verify_bytes: bool,
) -> Result<CapturedPatch> {
    let length: u64 = c.query_row(
        "SELECT length(payload) FROM projection_captures WHERE sequence=?1",
        [sequence],
        |r| r.get(0),
    )?;
    if length > MAX_DOCUMENT as u64 {
        return Err(invalid("capture document exceeds8MiB"));
    }
    let (digest, generation, bytes): (String, u64, Vec<u8>) = c.query_row(
        "SELECT digest,projection_generation,payload FROM projection_captures WHERE sequence=?1",
        [sequence],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let patch: CapturedPatch = serde_json::from_slice(&bytes).map_err(invalid)?;
    if patch.sequence != sequence
        || patch.projection_generation != generation
        || patch.digest != digest
        || canonical_json(&patch).map_err(invalid)? != bytes
    {
        return Err(invalid("capture identity or canonical document mismatch"));
    }
    validate_with_bytes(c, store, &patch, verify_bytes)?;
    Ok(patch)
}
fn validate(c: &Connection, store: &ProjectionStore, patch: &CapturedPatch) -> Result<()> {
    validate_with_bytes(c, store, patch, true)
}
fn validate_with_bytes(
    c: &Connection,
    store: &ProjectionStore,
    patch: &CapturedPatch,
    verify_bytes: bool,
) -> Result<()> {
    if patch.schema_version != "layrs.projection-capture.v1"
        || patch.identity != *store.identity()
        || patch.sequence == 0
        || patch.digest != patch.computed_digest()?
        || patch.changes.len() > 1024
    {
        return Err(invalid("capture digest, identity or schema mismatch"));
    }
    let prior: Option<String> = if patch.sequence == 1 {
        None
    } else {
        Some(c.query_row(
            "SELECT digest FROM projection_captures WHERE sequence=?1",
            [patch.sequence - 1],
            |r| r.get(0),
        )?)
    };
    if prior != patch.previous_patch_digest {
        return Err(invalid("capture history chain mismatch"));
    }
    let length: u64 = c.query_row(
        "SELECT length(payload) FROM projection_snapshots WHERE generation=?1",
        [patch.projection_generation],
        |r| r.get(0),
    )?;
    if length > 64 * 1024 * 1024 {
        return Err(invalid("projection snapshot exceeds64MiB"));
    }
    let (digest, parent, bytes): (String, Option<String>, Vec<u8>) = c.query_row(
        "SELECT digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1",
        [patch.projection_generation],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if digest != patch.projection_digest
        || digest != database::digest(patch.projection_generation, parent.as_deref(), &bytes)
    {
        return Err(invalid("capture base projection changed"));
    }
    let snapshot: ProjectionSnapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
    if snapshot.canonical_bytes()? != bytes {
        return Err(invalid("capture base projection is not canonical"));
    }
    let reserved = snapshot.reserved_paths()?;
    let mut previous = None;
    let mut total = 0u64;
    for change in &patch.changes {
        let path = change.path.as_str();
        if path.len() > 4096 || previous.is_some_and(|p: &str| p >= path) {
            return Err(invalid("capture paths are not bounded, sorted and unique"));
        }
        previous = Some(path);
        reserved.check_path(path)?;
        let index = snapshot
            .entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .map_err(|_| invalid("capture references an unknown path"))?;
        let entry = &snapshot.entries[index];
        if !entry.writable || !matches!(entry.state, EntryState::Materialized | EntryState::Absent)
        {
            return Err(invalid("capture references a protected entry"));
        }
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some((file_id, entry.executable)),
            None if entry.state == EntryState::Absent => None,
            _ => return Err(invalid("capture expected target is not a regular file")),
        };
        if change.expected.as_ref().map(|f| (&f.file_id, f.executable)) != expected
            || change
                .replacement
                .as_ref()
                .map(|f| (&f.file_id, f.executable))
                == expected
        {
            return Err(invalid("capture expected/replacement target mismatch"));
        }
        if let Some(file) = &change.replacement {
            total = total
                .checked_add(file.bytes)
                .filter(|n| *n <= MAX_CAPTURE_BYTES)
                .ok_or_else(|| invalid("capture replacement bytes exceed4GiB"))?;
            if file.bytes > MAX_CAPTURE_FILE_BYTES {
                return Err(invalid("capture file exceeds1GiB"));
            }
            if verify_bytes {
                file_manifest(c, file)?;
            }
        }
    }
    Ok(())
}
fn persist_file(
    c: &Connection,
    store: &ProjectionStore,
    path: &RepoPath,
    file: &CapturedFile,
) -> Result<()> {
    let root = PinnedDirectory::open(&store.root)?;
    let (parent, name) = root.existing_parent(Path::new(path.as_str()))?;
    let pinned = parent.open_file(&name)?;
    persist_pinned_file(c, &pinned, file)?;
    parent.verify_path_identity()?;
    if !pinned.same_file(&parent.open_file(&name)?) {
        return Err(invalid("capture file replaced while storing"));
    }
    root.verify_path_identity()?;
    Ok(())
}
pub(in crate::projection) fn persist_pinned_file(
    c: &Connection,
    pinned: &PinnedFile,
    file: &CapturedFile,
) -> Result<()> {
    let mut input = pinned.file();
    input.seek(SeekFrom::Start(0))?;
    let meta = input.metadata()?;
    if meta.len() != file.bytes
        || meta.len() > MAX_CAPTURE_FILE_BYTES
        || scan::executable(&meta) != file.executable
    {
        return Err(invalid("capture file changed before storage"));
    }
    let legacy: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_capture_files WHERE file_id=?1)",
        [file.file_id.as_str()],
        |r| r.get(0),
    )?;
    let chunks: u64 = c.query_row(
        "SELECT COUNT(*) FROM projection_capture_chunks WHERE file_id=?1",
        [file.file_id.as_str()],
        |r| r.get(0),
    )?;
    let insert = !legacy && chunks == 0;
    if insert && file.bytes == 0 {
        c.execute(
            "INSERT INTO projection_capture_files(file_id,bytes) VALUES(?1,?2)",
            params![file.file_id.as_str(), Vec::<u8>::new()],
        )?;
    }
    let mut builder = FileManifestBuilder::new(
        ChunkingDescriptor::deterministic_fixed(CAPTURE_CHUNK_BYTES as u64).map_err(invalid)?,
        MAX_CAPTURE_FILE_BYTES,
        1024,
    )
    .map_err(invalid)?;
    let mut left = file.bytes;
    let mut ordinal = 0;
    while left > 0 {
        let size = left.min(CAPTURE_CHUNK_BYTES as u64) as usize;
        let mut bytes = vec![0u8; size];
        input.read_exact(&mut bytes)?;
        let chunk = Chunk::new(bytes).map_err(invalid)?;
        builder.push_chunk(&chunk).map_err(invalid)?;
        if insert {
            c.execute("INSERT INTO projection_capture_chunks(file_id,ordinal,chunk_id,bytes) VALUES(?1,?2,?3,?4)", params![file.file_id.as_str(), ordinal, chunk.id().as_str(), chunk.bytes()])?;
        }
        ordinal += 1;
        left -= size as u64;
    }
    if input.read(&mut [0u8; 1])? != 0 {
        return Err(invalid("capture file grew while storing"));
    }
    let produced = builder.finish().map_err(invalid)?;
    if produced.file_id() != &file.file_id || produced.byte_length() != file.bytes {
        return Err(invalid("capture file changed while storing"));
    }
    pinned.verify_parent()?;
    file_manifest(c, file)?;
    Ok(())
}
fn for_each_chunk(
    c: &Connection,
    file: &CapturedFile,
    mut visit: impl FnMut(usize, &[u8]) -> Result<()>,
) -> Result<()> {
    let legacy: Option<u64> = c
        .query_row(
            "SELECT length(bytes) FROM projection_capture_files WHERE file_id=?1",
            [file.file_id.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let chunk_count: u64 = c.query_row(
        "SELECT COUNT(*) FROM projection_capture_chunks WHERE file_id=?1",
        [file.file_id.as_str()],
        |r| r.get(0),
    )?;
    if legacy.is_some() && chunk_count != 0 {
        return Err(invalid(
            "captured file has conflicting storage representations",
        ));
    }
    if let Some(length) = legacy.filter(|_| chunk_count == 0) {
        if length != file.bytes || length > MAX_PATCH_BYTES as u64 {
            return Err(invalid("legacy capture length mismatch"));
        }
        let mut offset = 0u64;
        let mut ordinal = 0;
        while offset < length {
            let size = (length - offset).min(CAPTURE_CHUNK_BYTES as u64);
            let bytes: Vec<u8> = c.query_row(
                "SELECT substr(bytes,?2,?3) FROM projection_capture_files WHERE file_id=?1",
                params![file.file_id.as_str(), offset + 1, size],
                |r| r.get(0),
            )?;
            if bytes.len() as u64 != size {
                return Err(invalid("legacy capture chunk length changed"));
            }
            visit(ordinal, &bytes)?;
            offset += size;
            ordinal += 1;
        }
        return Ok(());
    }
    let mut statement = c.prepare("SELECT ordinal,chunk_id,length(bytes),bytes FROM projection_capture_chunks WHERE file_id=?1 AND length(bytes)<=1048576 AND length(chunk_id)=71 ORDER BY ordinal")?;
    let mut rows = statement.query([file.file_id.as_str()])?;
    let mut count = 0usize;
    while let Some(row) = rows.next()? {
        let ordinal: usize = row.get(0)?;
        let claimed: String = row.get(1)?;
        let length: u64 = row.get(2)?;
        if length > CAPTURE_CHUNK_BYTES as u64 {
            return Err(invalid("stored capture chunk exceeds1MiB"));
        }
        let bytes: Vec<u8> = row.get(3)?;
        let expected = file
            .bytes
            .saturating_sub((count as u64) * CAPTURE_CHUNK_BYTES as u64)
            .min(CAPTURE_CHUNK_BYTES as u64);
        if ordinal != count || expected == 0 || length != expected || bytes.len() as u64 != expected
        {
            return Err(invalid("capture chunks are not bounded and contiguous"));
        }
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-chunk-v1\0");
        hash.update(&bytes);
        if claimed != format!("blake3:{}", hash.finalize()) {
            return Err(invalid("capture chunk digest mismatch"));
        }
        visit(ordinal, &bytes)?;
        count += 1;
    }
    if count == 0
        || count as u64 != chunk_count
        || count != file.bytes.div_ceil(CAPTURE_CHUNK_BYTES as u64) as usize
    {
        return Err(invalid("capture chunks do not cover file"));
    }
    Ok(())
}
pub(in crate::projection) fn file_manifest(
    c: &Connection,
    file: &CapturedFile,
) -> Result<layrs_core::scm::FileManifest> {
    if file.bytes > MAX_CAPTURE_FILE_BYTES {
        return Err(invalid("capture file exceeds1GiB"));
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-file-v1\0");
    let mut chunks = Vec::new();
    let mut offset = 0u64;
    for_each_chunk(c, file, |_, bytes| {
        let mut chunk_hash = blake3::Hasher::new();
        chunk_hash.update(b"layrs-chunk-v1\0");
        chunk_hash.update(bytes);
        chunks.push(
            FileChunk::new(
                ChunkId::parse(format!("blake3:{}", chunk_hash.finalize())).map_err(invalid)?,
                offset,
                bytes.len() as u64,
            )
            .map_err(invalid)?,
        );
        hash.update(bytes);
        offset += bytes.len() as u64;
        Ok(())
    })?;
    if format!("blake3:{}", hash.finalize()) != file.file_id.as_str() || offset != file.bytes {
        return Err(invalid("captured file hash or length mismatch"));
    }
    FileManifest::new(
        file.file_id.clone(),
        file.bytes,
        ChunkingDescriptor::deterministic_fixed(CAPTURE_CHUNK_BYTES as u64).map_err(invalid)?,
        chunks,
    )
    .map_err(invalid)
}
pub(in crate::projection) fn stream_file(
    c: &Connection,
    file: &CapturedFile,
    out: &mut dyn Write,
) -> Result<()> {
    let manifest = file_manifest(c, file)?;
    let mut verifier = manifest.verifier();
    for_each_chunk(c, file, |_, bytes| {
        verifier.push(bytes).map_err(invalid)?;
        out.write_all(bytes)?;
        Ok(())
    })?;
    verifier.finish().map_err(invalid)?;
    Ok(())
}
pub(in crate::projection) fn read_chunk(
    c: &Connection,
    file: &CapturedFile,
    ordinal: usize,
    expected_chunk: &FileChunk,
    out: &mut dyn Write,
) -> Result<()> {
    if file.bytes > MAX_CAPTURE_FILE_BYTES {
        return Err(invalid("capture file exceeds1GiB"));
    }
    let offset = (ordinal as u64)
        .checked_mul(CAPTURE_CHUNK_BYTES as u64)
        .filter(|n| *n < file.bytes)
        .ok_or_else(|| invalid("capture chunk ordinal out of range"))?;
    let size = (file.bytes - offset).min(CAPTURE_CHUNK_BYTES as u64);
    let legacy: Option<u64> = c
        .query_row(
            "SELECT length(bytes) FROM projection_capture_files WHERE file_id=?1",
            [file.file_id.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let chunk_count: u64 = c.query_row(
        "SELECT COUNT(*) FROM projection_capture_chunks WHERE file_id=?1",
        [file.file_id.as_str()],
        |r| r.get(0),
    )?;
    let (claimed, bytes): (Option<String>, Vec<u8>) = if let Some(length) =
        legacy.filter(|_| chunk_count == 0)
    {
        if length != file.bytes || length > MAX_PATCH_BYTES as u64 {
            return Err(invalid("legacy capture length mismatch"));
        }
        // Legacy blobs did not persist per-chunk digests, so verify the file.
        file_manifest(c, file)?;
        (
            None,
            c.query_row(
                "SELECT substr(bytes,?2,?3) FROM projection_capture_files WHERE file_id=?1",
                params![file.file_id.as_str(), offset + 1, size],
                |r| r.get(0),
            )?,
        )
    } else {
        let value: (String, u64, Vec<u8>) = c.query_row(
            "SELECT chunk_id,length(bytes),bytes FROM projection_capture_chunks WHERE file_id=?1 AND ordinal=?2 AND length(bytes)<=1048576 AND length(chunk_id)=71",
            params![file.file_id.as_str(), ordinal], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        if value.1 != size {
            return Err(invalid("stored capture chunk length mismatch"));
        }
        (Some(value.0), value.2)
    };
    if bytes.len() as u64 != size {
        return Err(invalid("capture chunk length mismatch"));
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-chunk-v1\0");
    hash.update(&bytes);
    let computed = format!("blake3:{}", hash.finalize());
    if legacy.is_some() && chunk_count != 0 {
        return Err(invalid(
            "captured file has conflicting storage representations",
        ));
    }
    if claimed.is_some_and(|id| id != computed)
        || expected_chunk.chunk_id().as_str() != computed
        || expected_chunk.byte_length() != size
        || expected_chunk.offset() != offset
    {
        return Err(invalid("capture chunk digest mismatch"));
    }
    out.write_all(&bytes)?;
    Ok(())
}
