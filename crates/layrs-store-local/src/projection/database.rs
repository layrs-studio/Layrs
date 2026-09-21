use super::*;
use rusqlite::{Connection, OpenFlags, params};
const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const SQL: &str = r#"
PRAGMA user_version=3;
CREATE TABLE projection_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),identity BLOB NOT NULL) STRICT;
CREATE TABLE projection_snapshots(generation INTEGER PRIMARY KEY CHECK(generation>=0),digest TEXT NOT NULL UNIQUE,parent_digest TEXT,payload BLOB NOT NULL CHECK(length(payload)<=67108864),FOREIGN KEY(parent_digest) REFERENCES projection_snapshots(digest) ON DELETE RESTRICT) STRICT;
CREATE TABLE projection_head(singleton INTEGER PRIMARY KEY CHECK(singleton=1),generation INTEGER NOT NULL REFERENCES projection_snapshots(generation) ON DELETE RESTRICT) STRICT;
CREATE TABLE projection_install(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
CREATE TABLE projection_capture_files(file_id TEXT PRIMARY KEY CHECK(length(file_id)=71),bytes BLOB NOT NULL CHECK(length(bytes)<=16777216)) STRICT;
CREATE TABLE projection_captures(sequence INTEGER PRIMARY KEY CHECK(sequence>0),digest TEXT NOT NULL UNIQUE CHECK(length(digest)=71),projection_generation INTEGER NOT NULL REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
"#;
pub(super) fn connect(path: &Path, write: bool) -> Result<Connection> {
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let c = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    c.busy_timeout(std::time::Duration::from_secs(2))?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")?;
    if write {
        c.execute_batch("PRAGMA synchronous=FULL;")?;
    }
    Ok(c)
}
fn schema(c: &Connection) -> Result<Vec<(String, String, String)>> {
    let (count,bytes):(u64,u64)=c.query_row("SELECT COUNT(*),COALESCE(SUM(length(sql)),0) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if count > 32 || bytes > 64 * 1024 {
        return Err(invalid("projection schema exceeds validation budget"));
    }
    let mut s = c.prepare(
        "SELECT type,name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
    )?;
    Ok(s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?)
}
pub(super) fn validate(c: &Connection, marker: &format::Marker) -> Result<()> {
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(SQL)?;
    if c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))? != 3
        || schema(c)? != schema(&reference)?
    {
        return Err(invalid(
            "projection schema mismatch; explicit migration required",
        ));
    }
    let length: u64 = c.query_row(
        "SELECT length(identity) FROM projection_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if length > 64 * 1024 {
        return Err(invalid("projection identity exceeds limit"));
    }
    let (count, bytes): (u64, Vec<u8>) =
        c.query_row("SELECT COUNT(*),identity FROM projection_meta", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    if count != 1 || bytes != marker.bytes()? {
        return Err(invalid("projection identity differs from FORMAT"));
    }
    let initial: String = c.query_row(
        "SELECT digest FROM projection_snapshots WHERE generation=0",
        [],
        |r| r.get(0),
    )?;
    if initial != marker.initial_snapshot_digest {
        return Err(invalid("initial projection differs from FORMAT"));
    }
    let failures: u64 = c.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
        r.get(0)
    })?;
    if failures != 0 {
        return Err(invalid("projection foreign key corruption"));
    }
    Ok(())
}
pub(super) fn initialize(
    c: &mut Connection,
    marker: &format::Marker,
    initial: &ProjectionSnapshot,
) -> Result<()> {
    let initial_bytes = initial.canonical_bytes()?;
    if digest(0, None, &initial_bytes) != marker.initial_snapshot_digest {
        return Err(invalid("bootstrap initial snapshot differs from FORMAT"));
    }
    if !schema(c)?.is_empty() {
        validate(c, marker)?;
        let state = current(c)?;
        if state.generation != 0 || state.snapshot != *initial {
            return Err(invalid(
                "bootstrap already has a different projection state",
            ));
        }
        return Ok(());
    }
    c.execute_batch("PRAGMA journal_mode=DELETE;")?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(SQL)?;
    tx.execute(
        "INSERT INTO projection_meta VALUES(1,?1)",
        [marker.bytes()?],
    )?;
    tx.execute(
        "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
        params![digest(0, None, &initial_bytes), initial_bytes],
    )?;
    tx.execute("INSERT INTO projection_head VALUES(1,0)", [])?;
    tx.commit()?;
    Ok(())
}
pub(super) fn digest(generation: u64, parent: Option<&str>, bytes: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"layrs-projection-state-v1\0");
    h.update(&generation.to_be_bytes());
    h.update(parent.unwrap_or("").as_bytes());
    h.update(&[0]);
    h.update(bytes);
    h.finalize().to_hex().to_string()
}
pub(super) fn current(c: &Connection) -> Result<VersionedProjection> {
    let (count,generation,max):(u64,u64,u64)=c.query_row("SELECT COUNT(*),generation,(SELECT MAX(generation) FROM projection_snapshots) FROM projection_head",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if count != 1 || generation != max {
        return Err(invalid("projection head is missing or rolled back"));
    }
    let length: u64 = c.query_row(
        "SELECT length(payload) FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| r.get(0),
    )?;
    if length > MAX_BYTES {
        return Err(invalid("projection payload too large"));
    }
    let (claimed, parent, bytes): (String, Option<String>, Vec<u8>) = c.query_row(
        "SELECT digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1",
        [generation],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if (generation == 0) != parent.is_none() {
        return Err(invalid("invalid projection parent"));
    }
    if generation != 0 {
        let expected: String = c.query_row(
            "SELECT digest FROM projection_snapshots WHERE generation=?1",
            [generation - 1],
            |r| r.get(0),
        )?;
        if parent.as_ref() != Some(&expected) {
            return Err(invalid("projection ancestry changed"));
        }
    }
    if bytes.len() as u64 != length || digest(generation, parent.as_deref(), &bytes) != claimed {
        return Err(invalid("projection snapshot digest mismatch"));
    }
    let snapshot: ProjectionSnapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
    if snapshot.canonical_bytes()? != bytes {
        return Err(invalid("projection snapshot is not canonical"));
    }
    Ok(VersionedProjection {
        generation,
        digest: claimed,
        snapshot,
    })
}

pub(super) fn require_idle(c: &Connection) -> Result<()> {
    let pending: bool =
        c.query_row("SELECT EXISTS(SELECT 1 FROM projection_install)", [], |r| {
            r.get(0)
        })?;
    if pending {
        return Err(invalid(
            "projection installation pending; explicit resume required",
        ));
    }
    Ok(())
}
