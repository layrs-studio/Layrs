use super::*;
use rusqlite::{Connection, OpenFlags, params};
const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const SQL: &str = r#"
PRAGMA user_version=5;
CREATE TABLE projection_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),identity BLOB NOT NULL) STRICT;
CREATE TABLE projection_snapshots(generation INTEGER PRIMARY KEY CHECK(generation>=0),digest TEXT NOT NULL UNIQUE,parent_digest TEXT,payload BLOB NOT NULL CHECK(length(payload)<=67108864),FOREIGN KEY(parent_digest) REFERENCES projection_snapshots(digest) ON DELETE RESTRICT) STRICT;
CREATE TABLE projection_head(singleton INTEGER PRIMARY KEY CHECK(singleton=1),generation INTEGER NOT NULL REFERENCES projection_snapshots(generation) ON DELETE RESTRICT) STRICT;
CREATE TABLE projection_install(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
CREATE TABLE projection_refresh_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
CREATE TABLE projection_refresh_history(generation INTEGER PRIMARY KEY REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
CREATE TABLE projection_capture_files(file_id TEXT PRIMARY KEY CHECK(length(file_id)=71),bytes BLOB NOT NULL CHECK(length(bytes)<=16777216)) STRICT;
CREATE TABLE projection_capture_chunks(file_id TEXT NOT NULL CHECK(length(file_id)=71),ordinal INTEGER NOT NULL CHECK(ordinal>=0 AND ordinal<1024),chunk_id TEXT NOT NULL CHECK(length(chunk_id)=71),bytes BLOB NOT NULL CHECK(length(bytes)>0 AND length(bytes)<=1048576),PRIMARY KEY(file_id,ordinal)) STRICT;
CREATE TABLE projection_captures(sequence INTEGER PRIMARY KEY CHECK(sequence>0),digest TEXT NOT NULL UNIQUE CHECK(length(digest)=71),projection_generation INTEGER NOT NULL REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
CREATE TABLE projection_publication_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),sequence INTEGER NOT NULL UNIQUE REFERENCES projection_captures(sequence) ON DELETE RESTRICT,projection_generation INTEGER NOT NULL REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,projection_digest TEXT NOT NULL CHECK(length(projection_digest)=64),capture_digest TEXT NOT NULL CHECK(length(capture_digest)=71),request_digest TEXT NOT NULL CHECK(length(request_digest)=71),request_bytes BLOB NOT NULL CHECK(length(request_bytes)<=25165824),receipt_digest TEXT CHECK(length(receipt_digest)=71),receipt_bytes BLOB CHECK(length(receipt_bytes)<=25165824),CHECK((receipt_digest IS NULL)=(receipt_bytes IS NULL))) STRICT;
CREATE TABLE projection_publications(sequence INTEGER PRIMARY KEY REFERENCES projection_captures(sequence) ON DELETE RESTRICT,request_digest TEXT NOT NULL CHECK(length(request_digest)=71),receipt_digest TEXT NOT NULL CHECK(length(receipt_digest)=71),receipt_bytes BLOB NOT NULL CHECK(length(receipt_bytes)<=25165824),next_snapshot BLOB CHECK(length(next_snapshot)<=67108864),projection_generation INTEGER NOT NULL REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,projection_digest TEXT NOT NULL CHECK(length(projection_digest)=64),after_state BLOB NOT NULL CHECK(length(after_state)<=68157440)) STRICT;
CREATE TABLE projection_namespace_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
CREATE TABLE projection_namespace_operations(preview_digest TEXT PRIMARY KEY CHECK(length(preview_digest)=71),capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
CREATE TABLE projection_restore_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
CREATE TABLE projection_restore_history(operation_id TEXT PRIMARY KEY,capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
CREATE TABLE projection_rebase_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
CREATE TABLE projection_rebase_history(operation_id TEXT PRIMARY KEY,generation INTEGER NOT NULL UNIQUE REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
CREATE TABLE projection_rebase_previews(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
"#;
const CHUNK_TABLE: &str = "CREATE TABLE projection_capture_chunks(file_id TEXT NOT NULL CHECK(length(file_id)=71),ordinal INTEGER NOT NULL CHECK(ordinal>=0 AND ordinal<1024),chunk_id TEXT NOT NULL CHECK(length(chunk_id)=71),bytes BLOB NOT NULL CHECK(length(bytes)>0 AND length(bytes)<=1048576),PRIMARY KEY(file_id,ordinal)) STRICT;";
pub(super) fn migrate(c: &mut Connection) -> Result<()> {
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == 5 || schema(c)?.is_empty() {
        return Ok(());
    }
    if version != 3 && version != 4 {
        return Err(invalid("unsupported projection schema migration"));
    }
    let v4_sql = SQL
        .replace("PRAGMA user_version=5;", "PRAGMA user_version=4;")
        .replace("CREATE TABLE projection_rebase_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;\n", "")
        .replace("CREATE TABLE projection_rebase_history(operation_id TEXT PRIMARY KEY,generation INTEGER NOT NULL UNIQUE REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;\n", "")
        .replace("CREATE TABLE projection_rebase_previews(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;\n", "");
    let old_sql = if version == 3 { v4_sql
        .replace("PRAGMA user_version=4;", "PRAGMA user_version=3;")
        .replace(&format!("{CHUNK_TABLE}\n"), "")
        .replace("projection_namespace_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=8388608))", "projection_namespace_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=65536))")
        .replace("projection_namespace_operations(preview_digest TEXT PRIMARY KEY CHECK(length(preview_digest)=71),capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=8388608))", "projection_namespace_operations(preview_digest TEXT PRIMARY KEY CHECK(length(preview_digest)=71),capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=65536))")
    } else { v4_sql };
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(&old_sql)?;
    if schema(c)? != schema(&reference)? {
        return Err(invalid("projection schema mismatch before migration"));
    }
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if version == 3 {
        tx.execute_batch(CHUNK_TABLE)?;
        tx.execute_batch("ALTER TABLE projection_namespace_pending RENAME TO projection_namespace_pending_old;
        CREATE TABLE projection_namespace_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
        INSERT INTO projection_namespace_pending SELECT * FROM projection_namespace_pending_old;
        DROP TABLE projection_namespace_pending_old;
        ALTER TABLE projection_namespace_operations RENAME TO projection_namespace_operations_old;
        CREATE TABLE projection_namespace_operations(preview_digest TEXT PRIMARY KEY CHECK(length(preview_digest)=71),capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
        INSERT INTO projection_namespace_operations SELECT * FROM projection_namespace_operations_old;
        DROP TABLE projection_namespace_operations_old;")?;
    }
    tx.execute_batch("CREATE TABLE projection_rebase_pending(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
        CREATE TABLE projection_rebase_history(operation_id TEXT PRIMARY KEY,generation INTEGER NOT NULL UNIQUE REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,payload BLOB NOT NULL CHECK(length(payload)<=67108864)) STRICT;
        CREATE TABLE projection_rebase_previews(singleton INTEGER PRIMARY KEY CHECK(singleton=1),payload BLOB NOT NULL CHECK(length(payload)<=8388608)) STRICT;
        PRAGMA user_version=5;")?;
    tx.commit()?;
    Ok(())
}
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
    if c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))? != 5
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
        migrate(c)?;
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
        c.query_row("SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_restore_pending) OR EXISTS(SELECT 1 FROM projection_rebase_pending)", [], |r| {
            r.get(0)
        })?;
    if pending {
        return Err(invalid(
            "projection filesystem operation pending; explicit resume required",
        ));
    }
    Ok(())
}
pub(super) fn require_mutable(c: &Connection) -> Result<()> {
    require_idle(c)?;
    let pending: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_publication_pending)",
        [],
        |r| r.get(0),
    )?;
    if pending {
        return Err(invalid(
            "publication pending; retry exact durable request before local mutation",
        ));
    }
    Ok(())
}
