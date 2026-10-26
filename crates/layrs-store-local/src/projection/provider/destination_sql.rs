//! Fresh EmptyIngress SQL only. The caller owns the pinned zero-byte database,
//! exclusive family/member locks, transaction, COMMIT and observable close.
use super::{
    cancellation, database_io, destination_validation,
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

#[cfg(test)]
#[path = "destination_sql/tests.rs"]
mod tests;
