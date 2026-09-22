//! Read-only verification and bounded history of the distinct partial Store.
use super::*;
use layrs_core::scm::FileId;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionCheck {
    pub snapshots: u64,
    pub captures: u64,
    pub captured_files: u64,
    pub captured_bytes: u64,
    pub completed_publications: u64,
    pub publication_pending: bool,
    pub namespace_operations: u64,
    pub refresh_operations: u64,
    pub restore_operations: u64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionHistoryPage {
    pub captures: Vec<CapturedPatch>,
    pub next_before: Option<u64>,
}
impl ProjectionStore {
    /// Check all immutable history. Does not alter or validate live checkout edits.
    pub fn fsck(&self) -> Result<ProjectionCheck> {
        let access = format::access(self, SessionMode::Shared)?;
        database::require_idle(&access.connection)?;
        let c = &access.connection;
        let integrity: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(invalid("projection SQLite integrity failure"));
        }
        let current = database::current(c)?;
        let (count, max): (u64, u64) = c.query_row(
            "SELECT COUNT(*),MAX(generation) FROM projection_snapshots",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if count
            != current
                .generation
                .checked_add(1)
                .ok_or_else(|| invalid("generation overflow"))?
            || max != current.generation
        {
            return Err(invalid(
                "projection snapshot history has gaps or detached rows",
            ));
        }
        let mut previous = None;
        for generation in 0..count {
            let len: u64 = c.query_row(
                "SELECT length(payload) FROM projection_snapshots WHERE generation=?1",
                [generation],
                |r| r.get(0),
            )?;
            if len > 64 * 1024 * 1024 {
                return Err(invalid("oversized projection snapshot"));
            }
            let (claimed, parent, bytes): (String, Option<String>, Vec<u8>) = c.query_row(
                "SELECT digest,parent_digest,payload FROM projection_snapshots WHERE generation=?1",
                [generation],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let snapshot: ProjectionSnapshot = serde_json::from_slice(&bytes).map_err(invalid)?;
            if parent != previous
                || database::digest(generation, parent.as_deref(), &bytes) != claimed
                || snapshot.canonical_bytes()? != bytes
            {
                return Err(invalid("historical projection corruption"));
            }
            previous = Some(claimed);
        }
        let captures: u64 =
            c.query_row("SELECT COUNT(*) FROM projection_captures", [], |r| r.get(0))?;
        let mut completed_publications = 0;
        for sequence in 1..=captures {
            capture::storage::load(c, self, sequence)?;
            if publication::storage::completed_record(c, self, sequence)?.is_some() {
                completed_publications += 1;
            }
        }
        let mut statement = c.prepare(
            "SELECT file_id,length(bytes) FROM projection_capture_files ORDER BY file_id",
        )?;
        let mut rows = statement.query([])?;
        let mut captured_files = 0;
        let mut captured_bytes = 0u64;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let len: u64 = row.get(1)?;
            if len > MAX_PATCH_BYTES as u64 {
                return Err(invalid("oversized captured file"));
            }
            let bytes: Vec<u8> = c.query_row(
                "SELECT bytes FROM projection_capture_files WHERE file_id=?1",
                [&id],
                |r| r.get(0),
            )?;
            if FileId::from_bytes(&bytes).as_str() != id || bytes.len() as u64 != len {
                return Err(invalid("captured file corruption"));
            }
            captured_files += 1;
            captured_bytes = captured_bytes
                .checked_add(len)
                .ok_or_else(|| invalid("capture size overflow"))?;
        }
        let publication_pending = publication::storage::pending(c, self)?.is_some();
        let namespace_operations = namespace::verify_history(c, self)?;
        let refresh_operations = refresh::verify_history(c)?;
        let restore_operations = restore::verify_history(c, self)?;
        access.verify()?;
        Ok(ProjectionCheck {
            snapshots: count,
            captures,
            captured_files,
            captured_bytes,
            completed_publications,
            publication_pending,
            namespace_operations,
            refresh_operations,
            restore_operations,
        })
    }
    /// Descending immutable local captures; the exclusive cursor is a sequence.
    pub fn capture_history(
        &self,
        before: Option<u64>,
        limit: u16,
    ) -> Result<ProjectionHistoryPage> {
        if !(1..=100).contains(&limit) {
            return Err(invalid("capture history limit must be 1..100"));
        }
        let access = format::access(self, SessionMode::Shared)?;
        database::require_idle(&access.connection)?;
        let mut statement=access.connection.prepare(
            "SELECT sequence FROM projection_captures WHERE sequence<?1 ORDER BY sequence DESC LIMIT ?2")?;
        let sequences = statement
            .query_map(
                rusqlite::params![before.unwrap_or(i64::MAX as u64), u64::from(limit) + 1],
                |r| r.get::<_, u64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_before =
            (sequences.len() > usize::from(limit)).then(|| sequences[usize::from(limit) - 1]);
        let mut captures = Vec::new();
        for sequence in sequences.into_iter().take(usize::from(limit)) {
            captures.push(capture::storage::load(&access.connection, self, sequence)?);
        }
        access.verify()?;
        Ok(ProjectionHistoryPage {
            captures,
            next_before,
        })
    }
}
