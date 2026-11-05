use super::{CompactionResult, MetadataCompactionReport};
use crate::scm_v1::sqlite;
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use std::{fs, path::Path, time::Duration};

pub fn require_idle(db: &Connection) -> CompactionResult<()> {
    for query in [
        "SELECT EXISTS(SELECT 1 FROM recovery_operations WHERE state!='completed')",
        "SELECT EXISTS(SELECT 1 FROM materialization_leases WHERE released_at_ms IS NULL)",
        "SELECT EXISTS(SELECT 1 FROM pending_objects WHERE state NOT IN ('installed','abandoned'))",
        "SELECT EXISTS(SELECT 1 FROM pending_admissions WHERE state IN ('pending','validated'))",
        "SELECT EXISTS(SELECT 1 FROM pending_steps WHERE state IN ('pending','validated'))",
        "SELECT EXISTS(SELECT 1 FROM pending_ref_updates WHERE state IN ('pending','validated'))",
    ] {
        if db.query_row(query, [], |row| row.get::<_, bool>(0))? {
            return Err(
                "metadata compaction refused: recovery, lease or pending mutation in a Worktree"
                    .into(),
            );
        }
    }
    Ok(())
}

/// Multiset of typed SQL rows, independent from physical order/rowid. Explicit
/// IDs and all projection/state/payload columns remain part of the digest.
pub fn snapshot(db: &Connection) -> CompactionResult<(String, u64)> {
    let check: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err("compaction SQLite integrity check failed".into());
    }
    if db
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some()
    {
        return Err("compaction foreign key check failed".into());
    }
    let tables = db
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut digest = blake3::Hasher::new();
    let mut total = 0u64;
    for table in tables {
        if !table
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("invalid SQLite table name".into());
        }
        digest.update(&(table.len() as u64).to_le_bytes());
        digest.update(table.as_bytes());
        let count: u64 = db.query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |r| {
            r.get(0)
        })?;
        total = total
            .checked_add(count)
            .ok_or("compaction row count overflow")?;
        if total > 1_000_000 {
            return Err("compaction exceeds one million rows".into());
        }
        let mut statement = db.prepare(&format!("SELECT * FROM \"{table}\""))?;
        let columns = statement.column_count();
        let mut rows = statement.query([])?;
        let mut hashes = Vec::with_capacity(count as usize);
        while let Some(row) = rows.next()? {
            let mut hash = blake3::Hasher::new();
            for index in 0..columns {
                match row.get_ref(index)? {
                    ValueRef::Null => {
                        hash.update(&[0]);
                    }
                    ValueRef::Integer(v) => {
                        hash.update(&[1]);
                        hash.update(&v.to_le_bytes());
                    }
                    ValueRef::Real(v) => {
                        hash.update(&[2]);
                        hash.update(&v.to_bits().to_le_bytes());
                    }
                    value => {
                        let (kind, bytes) = match value {
                            ValueRef::Text(v) => (3, v),
                            ValueRef::Blob(v) => (4, v),
                            _ => unreachable!(),
                        };
                        if bytes.len() > 16 * 1024 * 1024 {
                            return Err("compaction SQL cell exceeds 16 MiB".into());
                        }
                        hash.update(&[kind]);
                        hash.update(&(bytes.len() as u64).to_le_bytes());
                        hash.update(bytes);
                    }
                }
            }
            hashes.push(*hash.finalize().as_bytes());
        }
        hashes.sort_unstable();
        digest.update(&count.to_le_bytes());
        for hash in hashes {
            digest.update(&hash);
        }
    }
    Ok((digest.finalize().to_hex().to_string(), total))
}

pub fn prepare_candidate(path: &Path) -> CompactionResult<()> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
    let (busy, _, _): (u32, u32, u32) =
        db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    if busy != 0 {
        return Err("compaction checkpoint busy".into());
    }
    drop(db);
    crate::layout::open_read_write_no_follow(path)?.sync_all()?;
    crate::layout::sync_directory(path.parent().ok_or("compaction candidate parent missing")?)?;
    require_empty_sidecars(path)
}

pub fn require_empty_sidecars(path: &Path) -> CompactionResult<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        match fs::symlink_metadata(&name) {
            Ok(m) if m.is_file() && !m.file_type().is_symlink() && m.len() == 0 => (),
            Ok(_) => return Err("compaction refuses nonempty or unsafe SQLite sidecar".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub fn verify(path: &Path, report: &MetadataCompactionReport) -> CompactionResult<()> {
    let db = sqlite::open_read_only(path, Duration::from_secs(2))?;
    sqlite::validate_store_identity(&db, &report.store_id)?;
    require_idle(&db)?;
    if snapshot(&db)? != (report.logical_digest.clone(), report.retained_rows) {
        return Err("compaction logical snapshot mismatch".into());
    }
    Ok(())
}
