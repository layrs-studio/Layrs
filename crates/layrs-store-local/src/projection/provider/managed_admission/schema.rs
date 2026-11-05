//! Exact admitted private9/private10 footprints. Never append either to a
//! private8 database outside its caller-transaction admission writer.
use super::*;
use crate::projection::{database, retention, selection};

pub(super) const SQL: &str = r#"
CREATE TABLE projection_managed_admission(singleton INTEGER PRIMARY KEY CHECK(singleton=1),digest TEXT NOT NULL CHECK(length(CAST(digest AS BLOB))=71),source_published_digest TEXT NOT NULL CHECK(length(CAST(source_published_digest AS BLOB))=71),source_published BLOB NOT NULL CHECK(length(source_published)>0 AND length(source_published)<=262144),receipt_id TEXT NOT NULL REFERENCES projection_provider_transition_receipts(id) ON DELETE RESTRICT,ready_generation INTEGER NOT NULL REFERENCES projection_snapshots(generation) ON DELETE RESTRICT,ready_capture_sequence INTEGER NOT NULL REFERENCES projection_captures(sequence) ON DELETE RESTRICT,archive_count INTEGER NOT NULL CHECK(archive_count>=2 AND archive_count<=200002),archive_digest TEXT NOT NULL CHECK(length(CAST(archive_digest AS BLOB))=71),payload BLOB NOT NULL CHECK(length(payload)>0 AND length(payload)<=65536)) STRICT;
CREATE TABLE projection_import_checkout(path TEXT PRIMARY KEY CHECK(length(CAST(path AS BLOB))>0 AND length(CAST(path AS BLOB))<=128),operation TEXT NOT NULL CHECK(length(CAST(operation AS BLOB))>0 AND length(CAST(operation AS BLOB))<=64),digest TEXT NOT NULL CHECK(length(CAST(digest AS BLOB))=71),payload BLOB NOT NULL CHECK(length(payload)>0 AND length(payload)<=8192),state TEXT NOT NULL CHECK(state='owned')) STRICT;
"#;

/// Normalized-column maxima, excluding SQLite pages/indices/journal geometry.
/// 4 digest/reference TEXTs * 71 + 4 INTEGERs * 8 + both bounded BLOBs.
pub(super) const MAX_ADMISSION_COLUMN_BYTES: u64 = 4 * 71 + 4 * 8 + 262144 + 65536;
pub(super) const MAX_ARCHIVE_COLUMN_BYTES: u64 = 64 * 1024 * 1024;

/// Ordinary admission adds its own immutable witness; SQL above is the exact
/// historical private9 DDL and must remain unchanged.
pub(super) const SQL10_WITNESS: &str = r#"
CREATE TABLE projection_import_representations(file_id TEXT NOT NULL CHECK(length(CAST(file_id AS BLOB))=71 AND substr(file_id,1,7)='blake3:' AND substr(file_id,8) NOT GLOB '*[^0-9a-f]*'),manifest_id TEXT NOT NULL CHECK(length(CAST(manifest_id AS BLOB))=71 AND substr(manifest_id,1,7)='blake3:' AND substr(manifest_id,8) NOT GLOB '*[^0-9a-f]*'),bytes INTEGER NOT NULL CHECK(bytes>=0 AND bytes<=1073741824),PRIMARY KEY(file_id,manifest_id)) STRICT;
"#;
pub(super) const MAX_REPRESENTATIONS: u64 = 100_000;
pub(super) const REPRESENTATION_COLUMN_BYTES: u64 = 71 + 71 + 8;
pub(super) const MAX_REPRESENTATION_COLUMN_BYTES: u64 = 15_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum AdmittedLayout {
    Provider9,
    Ordinary10,
}
impl AdmittedLayout {
    pub(in crate::projection::provider) fn private_schema(self) -> u32 {
        match self {
            Self::Provider9 => 9,
            Self::Ordinary10 => 10,
        }
    }
}

/// Closed actual-schema token. It is not admission, ACK durability or bytes.
pub(in crate::projection::provider) struct AdmittedSchemaPermit<'a> {
    connection: &'a Connection,
    marker: format::Marker,
    layout: AdmittedLayout,
}
impl AdmittedSchemaPermit<'_> {
    pub(in crate::projection::provider) fn layout(&self) -> AdmittedLayout {
        self.layout
    }
    pub(in crate::projection::provider) fn private_schema(&self) -> u32 {
        self.layout.private_schema()
    }
    pub(in crate::projection::provider) fn require(
        &self,
        c: &Connection,
        marker: &format::Marker,
        cancel: Option<&Cancellation>,
    ) -> Result<()> {
        cancellation::check(cancel)?;
        if c.is_autocommit() || !std::ptr::eq(c, self.connection) || &self.marker != marker {
            return Err(invalid(
                "admitted schema permit transaction or FORMAT differs",
            ));
        }
        // A borrow cannot prevent transaction restart or transactional DDL.
        if require_schema(c, marker, cancel)?.layout != self.layout {
            return Err(invalid("admitted schema permit layout changed"));
        }
        Ok(())
    }
}

pub(in crate::projection::provider) fn require_schema<'a>(
    c: &'a Connection,
    marker: &format::Marker,
    cancel: Option<&Cancellation>,
) -> Result<AdmittedSchemaPermit<'a>> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid("admitted import requires retained transaction"));
    }
    marker.validate()?;
    if marker.shared.is_none() || marker.import_digest.is_none() {
        return Err(invalid("admitted import requires shared imported FORMAT"));
    }
    let version: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let inventory = super::super::database_io::schema_inventory(c)?;
    let layout = match version {
        9 => AdmittedLayout::Provider9,
        10 => AdmittedLayout::Ordinary10,
        _ => {
            return Err(invalid(
                "admitted import requires exact private schema9 or schema10",
            ))
        }
    };
    let mut ddl = [
        database::SQL,
        selection::SQL,
        retention::SQL,
        super::super::schema::SQL,
        SQL,
    ]
    .concat();
    if layout == AdmittedLayout::Ordinary10 {
        ddl.push_str(SQL10_WITNESS);
    }
    super::super::database_io::with_reference(&ddl, |reference| {
        if inventory != super::super::database_io::schema_inventory(reference)? {
            return Err(invalid("admitted import schema fingerprint differs"));
        }
        Ok(())
    })?;
    if layout == AdmittedLayout::Ordinary10 {
        super::representations::preflight(c, cancel)?;
    }
    let (count, bytes): (u64, u64) = c.query_row(
        "SELECT count(*),coalesce(max(length(identity)),0) FROM projection_meta",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if count != 1 || bytes > 65536 {
        return Err(invalid("admitted import identity row missing or oversized"));
    }
    let bytes: Vec<u8> = c.query_row(
        "SELECT identity FROM projection_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if bytes != marker.bytes()? {
        return Err(invalid("admitted import identity differs from FORMAT"));
    }
    cancellation::check(cancel)?;
    Ok(AdmittedSchemaPermit {
        connection: c,
        marker: marker.clone(),
        layout,
    })
}
