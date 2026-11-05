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

#[cfg(all(windows, test))]
mod batch_tests;

#[allow(dead_code)]
pub(super) fn persist(
    c: &mut Connection,
    store: &ProjectionStore,
    state: &VersionedProjection,
    scan: scan::Scan,
    faults: &dyn CaptureFaults,
) -> Result<CapturedPatch> {
    persist_with_scope(c, store, state, scan, faults, None)
}

pub(super) fn persist_with_scope(
    c: &mut Connection,
    store: &ProjectionStore,
    state: &VersionedProjection,
    scan: scan::Scan,
    faults: &dyn CaptureFaults,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<CapturedPatch> {
    validate_content_scope(c, store, scope)?;
    if let Some(scope) = scope {
        scope.preflight_local_capture(c, store, state, &scan.changes)?;
    }
    let files = scan
        .changes
        .iter()
        .filter_map(|v| v.replacement.clone())
        .collect::<Vec<_>>();
    retention::reserve_with_scope(
        c,
        store,
        &format!("capture:{}", state.digest),
        &shared::digest(&scan.files)?,
        &files,
        0,
        scope,
    )?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let patch = persist_in_transaction_with_scope(&tx, store, state, scan, scope)?;
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
    persist_in_transaction_with_scope(c, store, state, scan, None)
}

pub(in crate::projection) fn persist_in_transaction_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    state: &VersionedProjection,
    scan: scan::Scan,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<CapturedPatch> {
    persist_transaction(c, store, state, scan, scope, true)
}

/// A new restore operation must advance the capture frontier even when its
/// file changes match the previous capture. Otherwise appending its receipt
/// could change an already committed import's historical rowset. Content
/// storage still deduplicates; replay of an existing restore returns earlier.
pub(in crate::projection) fn persist_restore_in_transaction_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    state: &VersionedProjection,
    scan: scan::Scan,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<CapturedPatch> {
    persist_transaction(c, store, state, scan, scope, false)
}

fn persist_transaction(
    c: &Connection,
    store: &ProjectionStore,
    state: &VersionedProjection,
    scan: scan::Scan,
    scope: Option<&format::ContentScope<'_>>,
    reuse_previous: bool,
) -> Result<CapturedPatch> {
    validate_content_scope(c, store, scope)?;
    let previous: Option<u64> =
        c.query_row("SELECT MAX(sequence) FROM projection_captures", [], |r| {
            r.get(0)
        })?;
    let parent = previous
        .map(|n| load_with_scope(c, store, n, scope))
        .transpose()?;
    if let Some(previous) = &parent {
        if reuse_previous
            && previous.identity == *store.identity()
            && previous.projection_digest == state.digest
            && previous.changes == scan.changes
        {
            retention::finish_budget(c, store)?;
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
            persist_file(c, store, &change.path, file, scope)?;
        }
    }
    if scan::scan_with_scope(store, c, &state.snapshot, false, scope)?.files != scan.files {
        return Err(invalid("checkout changed while storing partial capture"));
    }
    c.execute("INSERT INTO projection_captures(sequence,digest,projection_generation,payload) VALUES(?1,?2,?3,?4)",params![sequence,patch.digest,state.generation,payload])?;
    retention::finish_budget(c, store)?;
    // Final validation sees the committed budget state before COMMIT, so the
    // transient WRITE/active exception cannot admit unreferenced claims.
    // Existing deduplicated bytes are also hashed before acknowledgement.
    validate_with_bytes(c, store, &patch, true, scope)?;
    Ok(patch)
}
pub(in crate::projection) fn load(
    c: &Connection,
    store: &ProjectionStore,
    sequence: u64,
) -> Result<CapturedPatch> {
    load_with_scope(c, store, sequence, None)
}

pub(in crate::projection) fn load_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    sequence: u64,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<CapturedPatch> {
    load_inner(c, store, sequence, true, scope)
}
pub(in crate::projection) fn load_metadata(
    c: &Connection,
    store: &ProjectionStore,
    sequence: u64,
) -> Result<CapturedPatch> {
    load_inner(c, store, sequence, false, None)
}
fn load_inner(
    c: &Connection,
    store: &ProjectionStore,
    sequence: u64,
    verify_bytes: bool,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<CapturedPatch> {
    let patch = load_record(c, sequence)?;
    validate_with_bytes(c, store, &patch, verify_bytes, scope)?;
    Ok(patch)
}

/// Metadata interpretation with an origin supplied by an independently verified
/// import lineage. This performs no provenance lookup, content read, claim
/// acceptance or native authorization, so import validation cannot recurse back
/// into conversion::provenance through a capture loader.
pub(in crate::projection) fn load_metadata_for_origin(
    c: &Connection,
    sequence: u64,
    identity: &ProjectionIdentity,
    maximum_generation: u64,
) -> Result<CapturedPatch> {
    let patch = load_record(c, sequence)?;
    validate_record(
        c,
        &patch,
        || {
            if &patch.identity != identity || patch.projection_generation > maximum_generation {
                return Err(invalid("capture escaped its verified origin boundary"));
            }
            Ok(())
        },
        |_| Ok(()),
    )?;
    Ok(patch)
}

/// Stream a fixed-generation origin tail under the caller's retained snapshot.
/// The immutable snapshot, selection and Reserved paths are interpreted once;
/// every canonical patch, predecessor and change still receives full validation.
/// This carries DATA only and never dispatches through admission provenance.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(in crate::projection) fn visit_metadata_range_for_origin(
    c: &Connection,
    first: u64,
    last: u64,
    identity: &ProjectionIdentity,
    generation: u64,
    expected_digest: &str,
    check_cancel: impl FnMut() -> Result<()>,
    mut visit: impl FnMut(&CapturedPatch) -> Result<()>,
) -> Result<()> {
    visit_metadata_range_for_origin_with_base(
        c,
        first,
        last,
        identity,
        generation,
        expected_digest,
        check_cancel,
        |patch, _| visit(patch),
    )
}

/// The walk also lends the already interpreted fixed-generation base, so local
/// Restore journals are checked against it without a second snapshot load.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection) fn visit_metadata_range_for_origin_with_base(
    c: &Connection,
    first: u64,
    last: u64,
    identity: &ProjectionIdentity,
    generation: u64,
    expected_digest: &str,
    mut check_cancel: impl FnMut() -> Result<()>,
    mut visit: impl FnMut(&CapturedPatch, &ProjectionSnapshot) -> Result<()>,
) -> Result<()> {
    if first > last {
        return Ok(());
    }
    if c.is_autocommit() || first == 0 || last - first >= 100_000 {
        return Err(invalid("origin Capture range requires a retained snapshot"));
    }
    check_cancel()?;
    let (digest, snapshot) = load_capture_base(c, generation)?;
    if digest != expected_digest {
        return Err(invalid("capture base projection changed"));
    }
    let reserved = snapshot.reserved_paths()?;
    let mut previous = if first == 1 {
        None
    } else {
        Some(c.query_row(
            "SELECT digest FROM projection_captures WHERE sequence=?1",
            [first - 1],
            |r| r.get::<_, String>(0),
        )?)
    };
    let mut query = c.prepare(
        "SELECT sequence,length(payload),digest,projection_generation,payload FROM projection_captures WHERE sequence>=?1 AND sequence<=?2 ORDER BY sequence",
    )?;
    let mut rows = query.query(params![first, last])?;
    for sequence in first..=last {
        check_cancel()?;
        let row = rows
            .next()?
            .ok_or_else(|| invalid("capture origin range has a missing sequence"))?;
        let actual_sequence: u64 = row.get(0)?;
        let length: u64 = row.get(1)?;
        if actual_sequence != sequence || length > MAX_DOCUMENT as u64 {
            return Err(invalid(
                "capture origin range sequence or document bound differs",
            ));
        }
        let bytes: Vec<u8> = row.get(4)?;
        let patch = decode_record(sequence, row.get(2)?, row.get(3)?, &bytes)?;
        validate_patch_header(&patch)?;
        if &patch.identity != identity
            || patch.projection_generation != generation
            || patch.projection_digest != digest
            || patch.previous_patch_digest != previous
        {
            return Err(invalid("capture escaped its verified origin range"));
        }
        validate_changes(&patch, &snapshot, &reserved, |_| Ok(()))?;
        visit(&patch, &snapshot)?;
        previous = Some(patch.digest);
    }
    if rows.next()?.is_some() {
        return Err(invalid("capture origin range has extra sequences"));
    }
    check_cancel()
}

/// Exact canonical payload a subsequent ordinary Capture would append, or None
/// for its existing same-image deduplication branch. The caller supplies only
/// already scanned changes and retains/validates the actual private snapshot.
/// This plans bytes and row growth; it grants no writer or content authority.
pub(in crate::projection) fn local_capture_append_payload(
    c: &Connection,
    state: &VersionedProjection,
    identity: &ProjectionIdentity,
    changes: &[CapturedChange],
) -> Result<Option<Vec<u8>>> {
    append_payload(c, state, identity, changes, true)
}

/// Exact canonical payload a local Restore finish appends. Restore always
/// advances the capture frontier (`persist_restore_in_transaction_with_scope`
/// never reuses the previous patch), so this never returns a dedup branch.
pub(in crate::projection) fn local_restore_append_payload(
    c: &Connection,
    state: &VersionedProjection,
    identity: &ProjectionIdentity,
    changes: &[CapturedChange],
) -> Result<Vec<u8>> {
    append_payload(c, state, identity, changes, false)?
        .ok_or_else(|| invalid("local Restore append cannot deduplicate"))
}

fn append_payload(
    c: &Connection,
    state: &VersionedProjection,
    identity: &ProjectionIdentity,
    changes: &[CapturedChange],
    deduplicate: bool,
) -> Result<Option<Vec<u8>>> {
    let sequence: Option<u64> =
        c.query_row("SELECT MAX(sequence) FROM projection_captures", [], |r| {
            r.get(0)
        })?;
    let previous = sequence.map(|n| load_record(c, n)).transpose()?;
    if deduplicate
        && previous.as_ref().is_some_and(|patch| {
            &patch.identity == identity
                && patch.projection_digest == state.digest
                && patch.changes.as_slice() == changes
        })
    {
        return Ok(None);
    }
    let sequence = sequence
        .unwrap_or(0)
        .checked_add(1)
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| invalid("capture sequence exhausted"))?;
    let mut patch = CapturedPatch {
        schema_version: "layrs.projection-capture.v1".into(),
        sequence,
        previous_patch_digest: previous.map(|patch| patch.digest),
        projection_generation: state.generation,
        projection_digest: state.digest.clone(),
        identity: identity.clone(),
        changes: changes.to_vec(),
        digest: String::new(),
    };
    patch.digest = patch.computed_digest()?;
    let payload = canonical_json(&patch).map_err(invalid)?;
    if payload.len() > MAX_DOCUMENT {
        return Err(invalid("capture document exceeds8MiB"));
    }
    Ok(Some(payload))
}

fn load_record(c: &Connection, sequence: u64) -> Result<CapturedPatch> {
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
    decode_record(sequence, digest, generation, &bytes)
}

fn decode_record(
    sequence: u64,
    digest: String,
    generation: u64,
    bytes: &[u8],
) -> Result<CapturedPatch> {
    let patch: CapturedPatch = serde_json::from_slice(bytes).map_err(invalid)?;
    if patch.sequence != sequence
        || patch.projection_generation != generation
        || patch.digest != digest
        || canonical_json(&patch).map_err(invalid)?.as_slice() != bytes
    {
        return Err(invalid("capture identity or canonical document mismatch"));
    }
    Ok(patch)
}
fn validate_with_bytes(
    c: &Connection,
    store: &ProjectionStore,
    patch: &CapturedPatch,
    verify_bytes: bool,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<()> {
    if verify_bytes {
        validate_content_scope(c, store, scope)?;
    }
    validate_record(
        c,
        patch,
        || conversion::validate_capture_identity(c, store, patch),
        |file| {
            if verify_bytes {
                file_manifest_with_scope(c, store, file, scope)?;
            }
            Ok(())
        },
    )
}

fn validate_content_scope(
    c: &Connection,
    store: &ProjectionStore,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<()> {
    if let Some(scope) = scope {
        scope.validate(c, store)
    } else {
        super::super::provider::require_ordinary(c)
    }
}

fn validate_record(
    c: &Connection,
    patch: &CapturedPatch,
    validate_origin: impl FnOnce() -> Result<()>,
    validate_content: impl FnMut(&CapturedFile) -> Result<()>,
) -> Result<()> {
    validate_patch_header(patch)?;
    validate_origin()?;
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
    let (digest, snapshot) = load_capture_base(c, patch.projection_generation)?;
    if digest != patch.projection_digest {
        return Err(invalid("capture base projection changed"));
    }
    let reserved = snapshot.reserved_paths()?;
    validate_changes(patch, &snapshot, &reserved, validate_content)
}

fn validate_patch_header(patch: &CapturedPatch) -> Result<()> {
    if patch.schema_version != "layrs.projection-capture.v1"
        || patch.sequence == 0
        || patch.digest != patch.computed_digest()?
        || patch.changes.len() > 1024
    {
        return Err(invalid("capture digest, identity or schema mismatch"));
    }
    Ok(())
}

fn load_capture_base(c: &Connection, generation: u64) -> Result<(String, ProjectionSnapshot)> {
    let length: u64 = c.query_row(
        "SELECT length(payload) FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| r.get(0),
    )?;
    if length > 64 * 1024 * 1024 {
        return Err(invalid("projection snapshot exceeds64MiB"));
    }
    let (digest, parent, bytes): (String, Option<String>, Vec<u8>) = c.query_row(
        "SELECT digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if digest != database::digest(generation, parent.as_deref(), &bytes) {
        return Err(invalid("capture base projection changed"));
    }
    let snapshot: ProjectionSnapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
    if snapshot.canonical_bytes()? != bytes {
        return Err(invalid("capture base projection is not canonical"));
    }
    // Capture replay is checked against the selection at its immutable base,
    // never against a later expansion or the current live checkout.
    selection::at(c, generation)?.validate_snapshot(&snapshot)?;
    Ok((digest, snapshot))
}

fn validate_changes(
    patch: &CapturedPatch,
    snapshot: &ProjectionSnapshot,
    reserved: &layrs_core::projection::ReservedPaths,
    mut validate_content: impl FnMut(&CapturedFile) -> Result<()>,
) -> Result<()> {
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
            validate_content(file)?;
        }
    }
    Ok(())
}
fn persist_file(
    c: &Connection,
    store: &ProjectionStore,
    path: &RepoPath,
    file: &CapturedFile,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<()> {
    let root = PinnedDirectory::open(&store.root)?;
    let (parent, name) = root.existing_parent(Path::new(path.as_str()))?;
    let pinned = parent.open_file(&name)?;
    persist_pinned_file_with_scope(c, store, &pinned, file, scope)?;
    parent.verify_path_identity()?;
    if !pinned.same_file(&parent.open_file(&name)?) {
        return Err(invalid("capture file replaced while storing"));
    }
    root.verify_path_identity()?;
    Ok(())
}
pub(in crate::projection) fn persist_pinned_file(
    c: &Connection,
    store: &ProjectionStore,
    pinned: &PinnedFile,
    file: &CapturedFile,
) -> Result<()> {
    persist_pinned_file_with_scope(c, store, pinned, file, None)
}

pub(in crate::projection) fn persist_pinned_file_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    pinned: &PinnedFile,
    file: &CapturedFile,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<()> {
    validate_content_scope(c, store, scope)?;
    if store.marker.shared.is_some() {
        return retention::retain_pinned_with_scope(
            c,
            store,
            pinned,
            file,
            "captured-content",
            file.file_id.as_str(),
            scope,
        );
    }
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
    file_manifest_with_scope(c, store, file, scope)?;
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
    store: &ProjectionStore,
    file: &CapturedFile,
) -> Result<layrs_core::scm::FileManifest> {
    file_manifest_with_scope(c, store, file, None)
}

pub(in crate::projection) fn file_manifest_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    file: &CapturedFile,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<layrs_core::scm::FileManifest> {
    validate_content_scope(c, store, scope)?;
    if store.marker.shared.is_some() {
        return retention::manifest_with_scope(c, store, file, scope);
    }
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
    store: &ProjectionStore,
    file: &CapturedFile,
    out: &mut dyn Write,
) -> Result<()> {
    stream_file_with_scope(c, store, file, out, None)
}

pub(in crate::projection) fn stream_file_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    file: &CapturedFile,
    out: &mut dyn Write,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<()> {
    let manifest = file_manifest_with_scope(c, store, file, scope)?;
    let family = format::content_family(c, store, scope)?;
    if let Some(lease) = family.as_ref() {
        return lease.stream(&manifest, out);
    }
    let mut verifier = manifest.verifier();
    for_each_chunk(c, file, |_, bytes| {
        verifier.push(bytes).map_err(invalid)?;
        out.write_all(bytes)?;
        Ok(())
    })?;
    verifier.finish().map_err(invalid)?;
    Ok(())
}
#[allow(dead_code)]
pub(in crate::projection) fn read_chunk(
    c: &Connection,
    store: &ProjectionStore,
    file: &CapturedFile,
    ordinal: usize,
    expected_chunk: &FileChunk,
    out: &mut dyn Write,
) -> Result<()> {
    read_chunk_with_scope(c, store, file, ordinal, expected_chunk, out, None)
}

pub(in crate::projection) fn read_chunk_with_scope(
    c: &Connection,
    store: &ProjectionStore,
    file: &CapturedFile,
    ordinal: usize,
    expected_chunk: &FileChunk,
    out: &mut dyn Write,
    scope: Option<&format::ContentScope<'_>>,
) -> Result<()> {
    let family = format::content_family(c, store, scope)?;
    if let Some(lease) = family.as_ref() {
        let manifest = retention::manifest_with_scope(c, store, file, scope)?;
        if manifest.chunks().get(ordinal) != Some(expected_chunk) {
            return Err(invalid("shared capture representation changed"));
        }
        return lease.stream_chunk(&manifest, ordinal, out);
    }
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
        file_manifest_with_scope(c, store, file, scope)?;
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
