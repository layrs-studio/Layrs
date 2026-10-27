//! Narrow clean catalog I/O for the admitted shared policy. The caller owns
//! lifecycle/owner/catalog gates and filesystem pins. No recovery or private
//! database access is available through this adapter.
use super::{
    database_io::{self, BareConnection, DatabaseFile, WriterCloseObserver},
    recovery::error::{ProviderBootstrapCause as Cause, Result, invalid},
};
use rusqlite::{Connection, ffi};

struct ObserveClose;
impl WriterCloseObserver for ObserveClose {
    fn before_close(&mut self, _: *mut ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, _: bool) -> Option<Cause> {
        None
    }
}

fn conclude<T>(result: Result<T>, close: Result<()>) -> Result<T> {
    match (result, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(close)) => Err(close),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(close)) => Err(Cause::Cleanup {
            primary: Box::new(primary),
            close: Box::new(close),
        }),
    }
}

/// End only a read transaction begun here. In particular, the clean-admission
/// probe and any transaction already owned by an operation stay untouched.
fn validate_snapshot(
    c: &Connection,
    validate: &impl Fn(&Connection) -> Result<()>,
    rollback_failed: &mut bool,
) -> Result<()> {
    if !c.is_autocommit() {
        return validate(c);
    }
    c.execute_batch("BEGIN DEFERRED")?;
    let result = validate(c);
    let close = c.execute_batch("ROLLBACK").map_err(Cause::from);
    *rollback_failed |= close.is_err();
    conclude(result, close)
}

/// Positive clean RO admission precedes every writable open. The operation
/// receives an autocommit connection and may own its own BEGIN/COMMIT scopes.
/// Validation never commits or rolls back an operation's active transaction.
/// Before closing this adapter's connection, any unfinished operation is
/// explicitly rolled back and cannot be acknowledged as successful.
pub(in crate::projection) fn with_catalog<T>(
    database: &DatabaseFile,
    write: bool,
    validate: impl Fn(&Connection) -> Result<()>,
    operation: impl FnOnce(&mut Connection) -> Result<T>,
) -> Result<T> {
    let mut rollback_failed = false;
    let admitted = database_io::clean_admission(database, false, |c| {
        validate_snapshot(c, &validate, &mut rollback_failed)
    })?;
    let mut observer = ObserveClose;
    let mut bare = if write {
        admitted.open(database, &mut observer)?
    } else {
        BareConnection::open(database.path(), false)?
    };
    let result = (|| {
        admitted.require_geometry(&bare)?;
        // Geometry is established before any setup SQL. Keep the existing
        // ordinary content connection settings without enabling recovery.
        bare.connection_mut()
            .busy_timeout(std::time::Duration::from_secs(2))?;
        bare.connection()
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")?;
        if write {
            bare.connection()
                .execute_batch("PRAGMA synchronous=FULL;")?;
        }
        validate_snapshot(bare.connection(), &validate, &mut rollback_failed)?;
        let result = operation(bare.connection_mut());
        let result = conclude(
            result,
            validate_snapshot(bare.connection(), &validate, &mut rollback_failed),
        );
        if bare.connection().is_autocommit() {
            return result;
        }
        result.and_then(|_| {
            Err(invalid(
                "admitted content operation left an unfinished transaction",
            ))
        })
    })();
    // Setup or prevalidation can itself fail while a transaction remains
    // active; retain its error and the explicit rollback outcome before close.
    let result = if bare.connection().is_autocommit() || rollback_failed {
        result
    } else {
        conclude(
            result,
            bare.connection()
                .execute_batch("ROLLBACK")
                .map_err(Cause::from),
        )
    };
    if write {
        bare.finish_writer(result, &mut observer)
    } else {
        bare.finish(result)
    }
}
