use super::*;
use rusqlite::Connection;
type CompletedRow = (String, Vec<u8>, Option<Vec<u8>>, u64, String, Vec<u8>);
pub(super) fn json_digest(domain: &[u8], bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(invalid("publication JSON exceeds24MiB"));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(invalid)?;
    if !value.is_object() || canonical_json(&value).map_err(invalid)? != bytes {
        return Err(invalid(
            "publication request/receipt must be a canonical JSON object",
        ));
    }
    let mut h = blake3::Hasher::new();
    h.update(domain);
    h.update(bytes);
    Ok(format!("blake3:{}", h.finalize()))
}
pub(super) fn pending(
    c: &Connection,
    store: &ProjectionStore,
) -> Result<Option<PendingPublication>> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_publication_pending)",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let (length,receipt_length): (u64,Option<u64>) = c.query_row(
        "SELECT length(request_bytes),length(receipt_bytes) FROM projection_publication_pending WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?,r.get(1)?)),
    )?;
    if length > MAX_BYTES as u64 || receipt_length.is_some_and(|n| n > MAX_BYTES as u64) {
        return Err(invalid("pending publication exceeds24MiB"));
    }
    let (pending,receipt_digest)=c.query_row("SELECT sequence,projection_generation,projection_digest,capture_digest,request_digest,request_bytes,receipt_bytes,receipt_digest FROM projection_publication_pending WHERE singleton=1",[],|r|Ok((PendingPublication{sequence:r.get(0)?,projection_generation:r.get(1)?,projection_digest:r.get(2)?,capture_digest:r.get(3)?,request_digest:r.get(4)?,request_bytes:r.get(5)?,receipt_bytes:r.get(6)?},r.get::<_,Option<String>>(7)?)))?;
    let actual_digest = pending
        .receipt_bytes
        .as_deref()
        .map(|b| json_digest(b"layrs-projection-receipt-v1\0", b))
        .transpose()?;
    if actual_digest != receipt_digest {
        return Err(invalid("pending publication receipt corruption"));
    }
    let capture = super::super::capture::storage::load(c, store, pending.sequence)?;
    if pending.capture_digest != capture.digest
        || pending.projection_generation != capture.projection_generation
        || pending.projection_digest != capture.projection_digest
        || pending.request_digest
            != json_digest(b"layrs-projection-request-v1\0", &pending.request_bytes)?
    {
        return Err(invalid("pending publication integrity or base mismatch"));
    }
    Ok(Some(pending))
}
pub(super) fn completed(
    c: &Connection,
    sequence: u64,
    receipt: &[u8],
    next: Option<&[u8]>,
) -> Result<Option<VersionedProjection>> {
    let Some((record, stored_next)) = completed_record(c, sequence)? else {
        return Ok(None);
    };
    if record.receipt_bytes != receipt || stored_next.as_deref() != next {
        return Err(invalid("completed publication replay differs"));
    }
    Ok(Some(record.projection))
}
pub(super) fn completed_record(
    c: &Connection,
    sequence: u64,
) -> Result<Option<(CompletedPublication, Option<Vec<u8>>)>> {
    let exists: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_publications WHERE sequence=?1)",
        [sequence],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let (receipt_len,next_len,state_len):(u64,Option<u64>,u64)=c.query_row("SELECT length(receipt_bytes),length(next_snapshot),length(after_state) FROM projection_publications WHERE sequence=?1",[sequence],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if receipt_len > MAX_BYTES as u64
        || next_len.is_some_and(|n| n > 64 * 1024 * 1024)
        || state_len > 65 * 1024 * 1024
    {
        return Err(invalid("completed publication exceeds storage bounds"));
    }
    let (digest,stored_receipt,stored_next,generation,state_digest,state_bytes):CompletedRow=c.query_row("SELECT receipt_digest,receipt_bytes,next_snapshot,projection_generation,projection_digest,after_state FROM projection_publications WHERE sequence=?1",[sequence],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    if digest != json_digest(b"layrs-projection-receipt-v1\0", &stored_receipt)? {
        return Err(invalid("completed publication replay differs"));
    }
    let state: VersionedProjection = serde_json::from_slice(&state_bytes).map_err(invalid)?;
    if canonical_json(&state).map_err(invalid)? != state_bytes
        || state.generation != generation
        || state.digest != state_digest
    {
        return Err(invalid("completed publication state corrupted"));
    }
    let snapshot = state.snapshot.canonical_bytes()?;
    let snapshot_len: u64 = c.query_row(
        "SELECT length(payload) FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| r.get(0),
    )?;
    if snapshot_len > 64 * 1024 * 1024 {
        return Err(invalid("completed projection exceeds64MiB"));
    }
    let (stored_digest, parent, stored_bytes): (String, Option<String>, Vec<u8>) = c.query_row(
        "SELECT digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if stored_bytes != snapshot
        || stored_digest != state.digest
        || database::digest(generation, parent.as_deref(), &stored_bytes) != stored_digest
    {
        return Err(invalid("completed publication snapshot changed"));
    }
    Ok(Some((
        CompletedPublication {
            sequence,
            receipt_bytes: stored_receipt,
            projection: state,
        },
        stored_next,
    )))
}
