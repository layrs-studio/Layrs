use super::*;
use rusqlite::Connection;

/// Validate immutable refresh intents against their exact parent/result snapshots.
/// Quarantined bytes are historical user data; this check never deletes them.
pub(in crate::projection) fn verify_history(c: &Connection) -> Result<u64> {
    let mut statement = c.prepare(
        "SELECT generation,length(payload) FROM projection_refresh_history ORDER BY generation",
    )?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let generation: u64 = row.get(0)?;
        let length: u64 = row.get(1)?;
        if generation == 0 || length > 64 * 1024 * 1024 {
            return Err(invalid("invalid refresh history generation or length"));
        }
        let bytes: Vec<u8> = c.query_row(
            "SELECT payload FROM projection_refresh_history WHERE generation=?1",
            [generation],
            |r| r.get(0),
        )?;
        let journal: Journal = serde_json::from_slice(&bytes).map_err(invalid)?;
        if bytes.len() as u64 != length || encode(&journal)? != bytes || !journal.ready {
            return Err(invalid("invalid canonical refresh history"));
        }
        let before = snapshot(c, generation - 1)?;
        let after = snapshot(c, generation)?;
        recovery::validate_snapshot(&before, &journal)?;
        if after.snapshot != journal.target {
            return Err(invalid(
                "refresh history result differs from admitted snapshot",
            ));
        }
        count += 1;
    }
    Ok(count)
}
fn snapshot(c: &Connection, generation: u64) -> Result<VersionedProjection> {
    let length: u64 = c.query_row(
        "SELECT length(payload) FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| r.get(0),
    )?;
    if length > 64 * 1024 * 1024 {
        return Err(invalid("oversized historical refresh snapshot"));
    }
    let (digest, parent, bytes): (String, Option<String>, Vec<u8>) = c.query_row(
        "SELECT digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let snapshot: ProjectionSnapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
    if bytes.len() as u64 != length
        || snapshot.canonical_bytes()? != bytes
        || database::digest(generation, parent.as_deref(), &bytes) != digest
    {
        return Err(invalid("corrupt historical refresh snapshot"));
    }
    Ok(VersionedProjection {
        generation,
        digest,
        snapshot,
    })
}
