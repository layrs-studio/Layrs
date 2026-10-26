//! Fresh EmptyIngress SQL only. The caller owns the pinned zero-byte database,
//! exclusive family/member locks, transaction, COMMIT and observable close.
use super::{
    cancellation, database_io, destination_validation,
    import_validation::{copy_verified_prefix, VerifiedImportClosure},
    records::{
        activation::ProviderActivationRecord, import::ProviderImportPrefixRecord,
        transition::ProviderTransitionIntentRecord, AllocationRecord, PreparedRecord,
        ProviderBootstrapSource, Record,
    },
    recovery::error::{invalid, Result},
    schema,
};
use crate::projection::{database, format, retention, selection};
use layrs_vfs::Cancellation;
use rusqlite::{params, Connection};

/// Insert only the empty schema-8 ingress. The caller must issue BEGIN
/// IMMEDIATE before this call and roll back on failure. No filesystem or
/// family state is created or adopted here.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn initialize_empty(
    c: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    cancellation::check(cancel)?;
    if c.is_autocommit() {
        return Err(invalid(
            "destination initializer requires caller transaction",
        ));
    }
    destination_validation::require_inputs(
        marker, source, allocation, prepared, activation, prefix, intent,
    )?;
    let foreign_keys: u64 = c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let version: u64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if foreign_keys != 1 || version != 0 || !database_io::schema_inventory(c)?.is_empty() {
        return Err(invalid(
            "destination initializer requires strict fresh empty schema",
        ));
    }
    cancellation::check(cancel)?;
    c.execute_batch(
        &[
            database::SQL,
            selection::SQL,
            retention::SQL,
            schema::SQL,
            "PRAGMA user_version=8;",
        ]
        .concat(),
    )?;
    let initial = source
        .initial()
        .canonical_bytes()
        .map_err(|e| invalid(e.to_string()))?;
    c.execute(
        "INSERT INTO projection_meta VALUES(1,?1)",
        [marker.bytes()?],
    )?;
    c.execute(
        "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
        params![marker.initial_snapshot_digest, initial],
    )?;
    c.execute("INSERT INTO projection_head VALUES(1,0)", [])?;
    selection::insert(c, 0, source.selection())?;
    c.execute(
        "INSERT INTO projection_backend VALUES(1,'ordinary',NULL)",
        [],
    )?;
    c.execute(
        "INSERT INTO projection_imports VALUES(1,?1,?2)",
        params![prefix.digest()?, prefix.encode()?],
    )?;
    c.execute(
        "INSERT INTO projection_provider_transitions\
        (id,role,state,local_generation,reservation,receipt_id,payload,digest)\
        VALUES(?1,'destination','copying',0,NULL,NULL,?2,?3)",
        params![intent.transaction_id, intent.encode()?, intent.digest()?],
    )?;
    destination_validation::read_empty(
        c, marker, source, allocation, prepared, activation, prefix, intent, cancel,
    )?;
    cancellation::check(cancel)?;
    Ok(())
}

/// Promote the exact EmptyIngress to CopiedPrefix in the caller's destination
/// transaction. A complete replay reads the same state without writing. The
/// source read transaction and its VerifiedImportClosure remain held through
/// copy and readback. Any error after an insert requires caller rollback.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn promote_or_replay_copied(
    source_connection: &Connection,
    destination: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    proof: &VerifiedImportClosure,
    cancel: Option<&Cancellation>,
) -> Result<destination_validation::CopiedPrefix> {
    cancellation::check(cancel)?;
    if source_connection.is_autocommit() || destination.is_autocommit() {
        return Err(invalid(
            "copied prefix requires retained source and destination transactions",
        ));
    }
    proof.require_source_connection(source_connection)?;
    // A typed reader can fail because of SQLite or an uncertain reference
    // close, not just because this is the other state. Never swallow its error.
    super::import_validation::require_schema(
        destination,
        marker,
        super::import_validation::ImportDatabaseKind::Provider,
        cancel,
    )?;
    let count: u64 = destination.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_head LIMIT 2)",
        [],
        |row| row.get(0),
    )?;
    if count != 1 {
        return Err(invalid("copied prefix head cardinality differs"));
    }
    let head: u64 = destination.query_row(
        "SELECT generation FROM projection_head WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let source_generation = match &prefix.source {
        super::records::import::ImportSource::Provider { version, .. } => version.generation,
        _ => return Err(invalid("copied prefix source differs")),
    };
    if head == source_generation {
        return destination_validation::read_copied(
            destination,
            marker,
            source,
            allocation,
            prepared,
            activation,
            prefix,
            intent,
            proof,
            cancel,
        );
    }
    if head != 0 {
        return Err(invalid("copied prefix head is outside transition boundary"));
    }
    destination_validation::read_empty(
        destination,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        intent,
        cancel,
    )?;
    copy_verified_prefix(
        source_connection,
        destination,
        marker,
        prefix,
        proof,
        cancel,
    )?;
    let updated = destination.execute(
        "UPDATE projection_head SET generation=?1 WHERE singleton=1 AND generation=0",
        [source_generation],
    )?;
    if updated != 1 {
        return Err(invalid("copied prefix head CAS differs"));
    }
    destination_validation::read_copied(
        destination,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        intent,
        proof,
        cancel,
    )
}

#[cfg(test)]
#[path = "destination_sql/tests.rs"]
mod tests;
