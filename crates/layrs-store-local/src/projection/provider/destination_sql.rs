//! Fresh EmptyIngress SQL only. The caller owns the pinned zero-byte database,
//! exclusive family/member locks, transaction, COMMIT and observable close.
use super::{
    cancellation, database_io, destination_validation,
    import_validation::{VerifiedImportClosure, copy_verified_prefix},
    records::{
        AllocationRecord, PreparedRecord, ProviderBootstrapSource, Record,
        activation::ProviderActivationRecord, import::ProviderImportPrefixRecord,
        transition::ProviderTransitionIntentRecord,
    },
    recovery::error::{Result, invalid},
    schema,
};
use crate::projection::{database, format, retention, selection};
use layrs_vfs::Cancellation;
use rusqlite::{Connection, params};

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
    initialize_empty_inner(
        c, marker, source, allocation, prepared, activation, prefix, intent, None, cancel,
    )
}

/// The current document belongs to this transition, outside the inherited
/// prefix rowset. Persist it atomically with the initial destination intent.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn initialize_empty_with_current(
    c: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    current: &super::destination_coverage::SourceCurrentDeniedCoverage<'_>,
    cancel: Option<&Cancellation>,
) -> Result<()> {
    initialize_empty_inner(
        c,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        intent,
        Some(current),
        cancel,
    )
}

#[allow(clippy::too_many_arguments)]
fn initialize_empty_inner(
    c: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    intent: &ProviderTransitionIntentRecord,
    current: Option<&super::destination_coverage::SourceCurrentDeniedCoverage<'_>>,
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
    match (intent.plan.denial_coverage.as_ref(), current) {
        (None, None) => {}
        (Some(_), Some(current)) => current.require_destination(intent, cancel)?,
        _ => {
            return Err(invalid(
                "destination initialization requires exact current document scope",
            ));
        }
    }
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
    if let Some(current) = current {
        current.copy_fresh_into(c, intent, cancel)?;
    }
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
    if intent.plan.denial_coverage.is_some() {
        let foreign_keys: u64 = destination.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
        if foreign_keys != 1 {
            return Err(invalid(
                "current document prefix copy requires foreign keys",
            ));
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum CopiedStagingEffect {
    Updated,
    AlreadyPresent,
}

/// Copying -> Staging changes only the destination intent row. The twelve
/// imported rowsets and head remain exact. The caller owns BEGIN/COMMIT,
/// rollback, both retained connections and every physical authority.
#[allow(clippy::too_many_arguments)]
pub(in crate::projection::provider) fn stage_copied(
    source_connection: &Connection,
    destination: &Connection,
    marker: &format::Marker,
    source: &ProviderBootstrapSource,
    allocation: &AllocationRecord,
    prepared: &PreparedRecord,
    activation: &ProviderActivationRecord,
    prefix: &ProviderImportPrefixRecord,
    copying: &ProviderTransitionIntentRecord,
    staging: &ProviderTransitionIntentRecord,
    proof: &VerifiedImportClosure,
    cancel: Option<&Cancellation>,
) -> Result<CopiedStagingEffect> {
    cancellation::check(cancel)?;
    if source_connection.is_autocommit()
        || destination.is_autocommit()
        || std::ptr::eq(source_connection, destination)
    {
        return Err(invalid(
            "copied staging requires distinct retained source and destination transactions",
        ));
    }
    proof.require_source_connection(source_connection)?;
    staging.require_transition(copying)?;
    if copying.role != super::records::transition::TransitionRole::Destination
        || copying.state != super::records::transition::TransitionState::Copying
        || copying.reservation.is_some()
        || copying.receipt_id.is_some()
        || staging.state != super::records::transition::TransitionState::Staging
        || staging.reservation.as_deref() != Some(staging.reservation_operation()?.as_str())
        || staging.receipt_id.is_some()
    {
        return Err(invalid("copied staging predecessor or reservation differs"));
    }
    super::import_validation::require_schema(
        destination,
        marker,
        super::import_validation::ImportDatabaseKind::Provider,
        cancel,
    )?;
    let count: u64 = destination.query_row(
        "SELECT count(*) FROM (SELECT 1 FROM projection_provider_transitions LIMIT 2)",
        [],
        |row| row.get(0),
    )?;
    if count != 1 {
        return Err(invalid("copied staging intent cardinality differs"));
    }
    let (is_copying, is_staging): (bool, bool) = destination.query_row(
        "SELECT state='copying',state='staging' FROM projection_provider_transitions",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if is_staging {
        destination_validation::read_staging(
            destination,
            marker,
            source,
            allocation,
            prepared,
            activation,
            prefix,
            staging,
            proof,
            cancel,
        )?;
        return Ok(CopiedStagingEffect::AlreadyPresent);
    }
    if !is_copying {
        return Err(invalid("copied staging intent phase differs"));
    }
    destination_validation::read_copied(
        destination,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        copying,
        proof,
        cancel,
    )?;
    let updated = destination.execute(
        "UPDATE projection_provider_transitions SET state='staging',reservation=?1,payload=?2,digest=?3 \
         WHERE id=?4 AND role='destination' AND state='copying' AND local_generation=0 \
         AND reservation IS NULL AND receipt_id IS NULL AND payload=?5 AND digest=?6",
        params![staging.reservation, staging.encode()?, staging.digest()?,
            copying.transaction_id, copying.encode()?, copying.digest()?],
    )?;
    if updated != 1 {
        return Err(invalid("copied staging exact predecessor CAS differs"));
    }
    destination_validation::read_staging(
        destination,
        marker,
        source,
        allocation,
        prepared,
        activation,
        prefix,
        staging,
        proof,
        cancel,
    )?;
    cancellation::check(cancel)?;
    Ok(CopiedStagingEffect::Updated)
}

#[cfg(test)]
#[path = "destination_sql/tests.rs"]
mod tests;
