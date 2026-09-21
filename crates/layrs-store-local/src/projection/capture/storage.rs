use super::*;
use layrs_core::{projection::EntryState, scm::TreeEntryTarget};
use rusqlite::{Connection, params};

pub(super) fn persist(
    c: &mut Connection,
    store: &ProjectionStore,
    state: &VersionedProjection,
    scan: scan::Scan,
    faults: &dyn CaptureFaults,
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
        changes: scan.changes,
        digest: String::new(),
    };
    patch.digest = patch.computed_digest()?;
    let payload = canonical_json(&patch).map_err(invalid)?;
    if payload.len() > MAX_DOCUMENT {
        return Err(invalid("capture document exceeds8MiB"));
    }
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (id, bytes) in scan.bytes {
        if FileId::from_bytes(&bytes) != id {
            return Err(invalid("capture preparation hash mismatch"));
        }
        tx.execute("INSERT INTO projection_capture_files(file_id,bytes) VALUES(?1,?2) ON CONFLICT(file_id) DO NOTHING",params![id.as_str(),bytes])?;
    }
    tx.execute("INSERT INTO projection_captures(sequence,digest,projection_generation,payload) VALUES(?1,?2,?3,?4)",params![sequence,patch.digest,state.generation,payload])?;
    // Includes hashes of existing deduplicated blobs: corruption never becomes a successful capture.
    validate(&tx, store, &patch)?;
    faults.check(CapturePoint::BeforeCommit)?;
    tx.commit()?;
    faults.check(CapturePoint::AfterCommit)?;
    Ok(patch)
}
pub(in crate::projection) fn load(
    c: &Connection,
    store: &ProjectionStore,
    sequence: u64,
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
    validate(c, store, &patch)?;
    Ok(patch)
}
fn validate(c: &Connection, store: &ProjectionStore, patch: &CapturedPatch) -> Result<()> {
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
                .filter(|n| *n <= MAX_PATCH_BYTES as u64)
                .ok_or_else(|| invalid("capture replacement bytes exceed16MiB"))?;
            file_bytes(c, file)?;
        }
    }
    Ok(())
}
pub(super) fn file_bytes(c: &Connection, file: &CapturedFile) -> Result<Vec<u8>> {
    let len: u64 = c.query_row(
        "SELECT length(bytes) FROM projection_capture_files WHERE file_id=?1",
        [file.file_id.as_str()],
        |r| r.get(0),
    )?;
    if len != file.bytes || len > MAX_PATCH_BYTES as u64 {
        return Err(invalid("captured bytes length mismatch"));
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT bytes FROM projection_capture_files WHERE file_id=?1",
        [file.file_id.as_str()],
        |r| r.get(0),
    )?;
    if FileId::from_bytes(&bytes) != file.file_id {
        return Err(invalid("captured file hash mismatch"));
    }
    Ok(bytes)
}
