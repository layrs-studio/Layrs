//! Exact family catalog schema-2 migration inside a caller-owned transaction.
//! No opener, lock, filesystem effect, COMMIT, or journal recovery lives here.
use super::SQL as SQL1;
use crate::projection::provider::{
    cancellation,
    database_io::{schema_inventory, with_reference},
    recovery::error::{Result, invalid},
};
use layrs_vfs::Cancellation;
use rusqlite::Connection;

// An explicit migration may refuse a very large legacy catalog. The copy
// itself is INSERT..SELECT; no unbounded collection is materialized in Rust.
const MAX_MANIFESTS: u64 = 1_000_000;
const MAX_RELATIONS: u64 = 16_777_216;
const MAX_MANIFEST_PAYLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection) enum MigrationEffect {
    Migrated,
    AlreadyV2,
}

/// Exact reference produced through the same SQL-1 base and transformation as
/// a real database. SQLite's ALTER TABLE quotes renamed identifiers in its
/// stored DDL; a hand-written final schema would have a different fingerprint.
pub(in crate::projection) fn reference_inventory() -> Result<Vec<(String, String, String)>> {
    with_reference(SQL1, |reference| {
        reference.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
        transform(reference)?;
        reference.execute_batch("COMMIT")?;
        schema_inventory(reference)
    })
}

/// Construct the same schema-2 reference on a caller-opened fresh memory DB.
/// Ordinary callers keep their own SQLite opening/profile contract. This must
/// never receive a business database or an already-open caller transaction.
pub(super) fn initialize_reference(c: &Connection) -> Result<()> {
    if !c.is_autocommit()
        || c.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))? != 0
        || !schema_inventory(c)?.is_empty()
        || !c.query_row(
            "SELECT file='' FROM pragma_database_list WHERE name='main'",
            [],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Err(invalid(
            "family schema2 reference requires fresh memory database",
        ));
    }
    c.execute_batch(SQL1)?;
    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
    transform(c)?;
    c.execute_batch("COMMIT")?;
    Ok(())
}

/// Provider test fixtures keep the same fresh-memory-only preconditions.
#[cfg(test)]
pub(in crate::projection) fn initialize_test_reference(c: &Connection) -> Result<()> {
    initialize_reference(c)
}

/// Caller retains a fresh, FK-enabled connection and its already-open SQL
/// transaction. Caller alone decides COMMIT/ROLLBACK and observable close.
/// The caller must hold the family's exclusive lifecycle/catalog gates and
/// prove clean admission before opening the writer. Do not nest another
/// SQLite progress hook on this connection.
pub(in crate::projection) fn migrate(
    connection: &Connection,
    cancel: &Cancellation,
) -> Result<MigrationEffect> {
    cancellation::with_progress(connection, cancel, |c| {
        if c.is_autocommit()
            || c.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, u64>(0))? != 1
        {
            return Err(invalid(
                "family migration requires caller transaction and FK ON",
            ));
        }
        let version: u32 = c.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        let observed = schema_inventory(c)?;
        match version {
            2 => {
                if observed != reference_inventory()? {
                    return Err(invalid("family schema2 fingerprint differs"));
                }
                require_foreign_keys(c)?;
                Ok(MigrationEffect::AlreadyV2)
            }
            1 => {
                let original = with_reference(SQL1, schema_inventory)?;
                if observed != original {
                    return Err(invalid("family schema1 fingerprint differs"));
                }
                preflight(c)?;
                require_foreign_keys(c)?;
                cancellation::check(Some(cancel))?;
                transform(c)?;
                if schema_inventory(c)? != reference_inventory()? {
                    return Err(invalid("family schema2 migration fingerprint differs"));
                }
                require_foreign_keys(c)?;
                cancellation::check(Some(cancel))?;
                Ok(MigrationEffect::Migrated)
            }
            _ => Err(invalid("unsupported family schema migration version")),
        }
    })
}

fn require_foreign_keys(c: &Connection) -> Result<()> {
    if c.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |row| row.get::<_, bool>(0),
    )? {
        return Err(invalid("family migration foreign key violation"));
    }
    Ok(())
}

fn count_bounded(c: &Connection, table: &'static str, max: u64) -> Result<()> {
    // Names are literals from this module, never an external SQL identifier.
    let count: u64 = c.query_row(
        &format!("SELECT count(*) FROM (SELECT 1 FROM {table} LIMIT ?1)"),
        [max + 1],
        |row| row.get(0),
    )?;
    if count > max {
        return Err(invalid("family migration row count exceeds bound"));
    }
    Ok(())
}

fn preflight(c: &Connection) -> Result<()> {
    count_bounded(c, "manifests", MAX_MANIFESTS)?;
    count_bounded(c, "manifest_chunks", MAX_RELATIONS)?;
    count_bounded(c, "root_manifests", MAX_RELATIONS)?;
    let payload: u64 = c.query_row(
        "SELECT coalesce(sum(length(payload)),0) FROM manifests",
        [],
        |row| row.get(0),
    )?;
    if payload > MAX_MANIFEST_PAYLOAD_BYTES {
        return Err(invalid("family migration manifest metadata exceeds bound"));
    }
    let bad: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM manifests WHERE length(CAST(id AS BLOB))!=71 OR length(CAST(file_id AS BLOB))!=71 OR bytes<0 OR bytes>1073741824 OR length(payload)>262144)",
        [],
        |row| row.get(0),
    )?;
    if bad {
        return Err(invalid("family migration manifest metadata malformed"));
    }
    // Legacy STRICT TEXT columns do not bound their byte lengths. Bound the
    // external keys before INSERT..SELECT duplicates their storage, even when
    // all foreign keys are valid. Manifest keys are independently bounded above.
    for sql in [
        "SELECT EXISTS(SELECT 1 FROM manifest_chunks WHERE length(CAST(manifest_id AS BLOB))!=71 OR length(CAST(chunk_id AS BLOB))!=71)",
        "SELECT EXISTS(SELECT 1 FROM root_manifests WHERE length(CAST(manifest_id AS BLOB))!=71 OR length(CAST(root_id AS BLOB))>128)",
    ] {
        if c.query_row(sql, [], |row| row.get::<_, bool>(0))? {
            return Err(invalid("family migration relation key exceeds bound"));
        }
    }
    Ok(())
}

/// SQL-2 schema constructor. The legacy reservation/FileId installations
/// remain untouched, including in-flight intents. New exact installations
/// have a separate manifest-ID key and are empty after migration.
fn transform(c: &Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE files(file_id TEXT PRIMARY KEY CHECK(length(CAST(file_id AS BLOB))=71),bytes INTEGER NOT NULL CHECK(bytes>=0 AND bytes<=1073741824),UNIQUE(file_id,bytes)) STRICT;
         CREATE TABLE manifests_v2(id TEXT PRIMARY KEY,file_id TEXT NOT NULL,bytes INTEGER NOT NULL,payload BLOB NOT NULL CHECK(length(payload)<=262144),FOREIGN KEY(file_id,bytes) REFERENCES files(file_id,bytes)) STRICT;
         CREATE TABLE manifest_chunks_v2(manifest_id TEXT NOT NULL REFERENCES manifests_v2(id),ordinal INTEGER NOT NULL,chunk_id TEXT NOT NULL REFERENCES chunks(id),PRIMARY KEY(manifest_id,ordinal)) STRICT;
         CREATE TABLE root_manifests_v2(root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,manifest_id TEXT NOT NULL REFERENCES manifests_v2(id),PRIMARY KEY(root_id,manifest_id)) STRICT;",
    )?;
    c.execute(
        "INSERT INTO files(file_id,bytes) SELECT file_id,bytes FROM manifests",
        [],
    )?;
    c.execute(
        "INSERT INTO manifests_v2(id,file_id,bytes,payload) SELECT id,file_id,bytes,payload FROM manifests",
        [],
    )?;
    c.execute(
        "INSERT INTO manifest_chunks_v2(manifest_id,ordinal,chunk_id) SELECT manifest_id,ordinal,chunk_id FROM manifest_chunks",
        [],
    )?;
    c.execute(
        "INSERT INTO root_manifests_v2(root_id,manifest_id) SELECT root_id,manifest_id FROM root_manifests",
        [],
    )?;
    c.execute_batch(
        "DROP TABLE manifest_chunks;
         DROP TABLE root_manifests;
         DROP TABLE manifests;
         ALTER TABLE manifests_v2 RENAME TO manifests;
         ALTER TABLE manifest_chunks_v2 RENAME TO manifest_chunks;
         ALTER TABLE root_manifests_v2 RENAME TO root_manifests;
         CREATE INDEX manifests_by_file ON manifests(file_id,id);
         CREATE TABLE exact_installations(reservation TEXT NOT NULL REFERENCES reservations(id),manifest_id TEXT NOT NULL CHECK(length(CAST(manifest_id AS BLOB))=71),file_id TEXT NOT NULL CHECK(length(CAST(file_id AS BLOB))=71),bytes INTEGER NOT NULL CHECK(bytes>=0 AND bytes<=1073741824),payload BLOB NOT NULL CHECK(length(payload)<=8192),manifest BLOB CHECK(length(manifest)<=262144),published INTEGER NOT NULL CHECK(published IN (0,1)),PRIMARY KEY(reservation,manifest_id)) STRICT;
         PRAGMA user_version=2;",
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "schema_v2/tests.rs"]
mod tests;
