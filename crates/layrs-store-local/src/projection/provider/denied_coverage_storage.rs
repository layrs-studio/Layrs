//! Exact document storage inside a caller-owned transaction. DATA only.
//!
//! The caller must authenticate schema/identity, retain the source authority,
//! account for the full document and atomically persist its REQUIRED versioned
//! Planned link in the same Immediate transaction. This module neither opens
//! nor commits/closes a connection and cannot grant Ready or acceptance. On
//! error the caller must stop and explicitly roll back, observing close errors.
use super::{
    cancellation,
    records::{
        Record,
        denied_coverage::{
            DeniedCoverageRef, DerivedDeniedCoverage, TransitionDeniedCoverageRecord,
        },
    },
    recovery::error::{Result, invalid},
};
use layrs_vfs::Cancellation;
use rusqlite::{
    Connection, DatabaseName, OptionalExtension, TransactionState, params, types::ValueRef,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InsertedDocument {
    Inserted,
    AlreadyPresent,
}

fn require_snapshot(c: &Connection) -> Result<()> {
    if c.is_autocommit() {
        return Err(invalid(
            "denied coverage requires a caller-owned SQL transaction",
        ));
    }
    Ok(())
}

fn require_write_transaction(c: &Connection) -> Result<()> {
    require_snapshot(c)?;
    if c.transaction_state(Some(DatabaseName::Main))? != TransactionState::Write {
        return Err(invalid(
            "denied coverage writer requires an active main write transaction",
        ));
    }
    let query_only: i64 = c.query_row("PRAGMA query_only", [], |row| row.get(0))?;
    if query_only != 0 {
        return Err(invalid(
            "denied coverage writer refuses query-only connection",
        ));
    }
    Ok(())
}

// Both probes belong to the same caller-owned SQL snapshot. The first probe
// bounds payload size before Rust materializes it; the second borrows the BLOB
// during decoding. Schema validation is an enclosing reader's responsibility.
fn read_optional(
    c: &Connection,
    expected: &DeniedCoverageRef,
    cancel: Option<&Cancellation>,
) -> Result<Option<TransitionDeniedCoverageRecord>> {
    cancellation::check(cancel)?;
    require_snapshot(c)?;
    expected.validate()?;
    let shape: Option<(String, i64)> = c
        .query_row(
            "SELECT typeof(payload),length(payload) FROM projection_observations WHERE digest=?1",
            [&expected.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((kind, bytes)) = shape else {
        return Ok(None);
    };
    if kind != "blob" || bytes <= 0 || bytes > TransitionDeniedCoverageRecord::MAX_BYTES as i64 {
        return Err(invalid(
            "denied coverage stored body type/length exceeds bounds",
        ));
    }
    cancellation::check(cancel)?;
    let mut statement = c.prepare(
        "SELECT payload FROM projection_observations WHERE digest=?1 AND typeof(payload)='blob' AND length(payload)=?2",
    )?;
    let mut rows = statement.query(params![expected.id, bytes])?;
    let row = rows
        .next()?
        .ok_or_else(|| invalid("denied coverage body disappeared from SQL snapshot"))?;
    let ValueRef::Blob(payload) = row.get_ref(0)? else {
        return Err(invalid("denied coverage body is not a BLOB"));
    };
    if payload.len() != bytes as usize {
        return Err(invalid("denied coverage stored body length differs"));
    }
    let value = TransitionDeniedCoverageRecord::read_body(&expected.id, payload, expected)?;
    cancellation::check(cancel)?;
    Ok(Some(value))
}

/// Authenticate an actual local canonical body against a supplied DATA link.
/// This alone does not authenticate the link's plan, receipt, source or phase.
pub(super) fn read_required(
    c: &Connection,
    expected: &DeniedCoverageRef,
    cancel: Option<&Cancellation>,
) -> Result<TransitionDeniedCoverageRecord> {
    read_optional(c, expected, cancel)?
        .ok_or_else(|| invalid("required denied coverage document is missing"))
}

/// Insert observation-derived canonical bytes, or accept an exact existing
/// document without mutation. Require an active main write transaction and a
/// writable connection before encoding or reading any body. Never repair,
/// overwrite, delete or silently ignore a conflicting row.
/// The enclosing Immediate transaction must also write the versioned plan
/// and perform its full inventory/budget checks before COMMIT and observed close.
pub(super) fn insert_exact(
    c: &Connection,
    value: &DerivedDeniedCoverage,
    expected: &DeniedCoverageRef,
    cancel: Option<&Cancellation>,
) -> Result<InsertedDocument> {
    cancellation::check(cancel)?;
    require_write_transaction(c)?;
    let foreign_keys: i64 = c.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(invalid("denied coverage writer requires foreign keys"));
    }
    expected.validate()?;
    let value = value.record();
    let payload = value.encode()?;
    // Validate the caller's entire reference BEFORE the first possible write.
    if value.reference()? != *expected {
        return Err(invalid("denied coverage insertion reference differs"));
    }
    if let Some(actual) = read_optional(c, expected, cancel)? {
        if actual != *value {
            return Err(invalid("denied coverage existing body conflicts"));
        }
        return Ok(InsertedDocument::AlreadyPresent);
    }
    cancellation::check(cancel)?;
    if c.execute(
        "INSERT INTO projection_observations(digest,payload) VALUES(?1,?2)",
        params![expected.id, payload],
    )? != 1
    {
        return Err(invalid(
            "denied coverage insertion affected an unexpected row count",
        ));
    }
    // No further SQL probe follows any insert/read/validation failure.
    if read_required(c, expected, cancel)? != *value {
        return Err(invalid("denied coverage inserted body readback differs"));
    }
    Ok(InsertedDocument::Inserted)
}

#[cfg(test)]
mod tests;
