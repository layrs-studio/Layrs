//! A scoped, connection-local SQLite progress hook for provider read probes.
//! The borrowed token is live only while the hook is installed; removal is
//! explicit before the observable SQLite close.
use super::recovery::error::{ProviderBootstrapCause as Cause, Result};
use layrs_vfs::Cancellation;
use rusqlite::{ffi, Connection};
use std::{ffi::c_void, os::raw::c_int};

const PROGRESS_OPCODES: c_int = 128;

unsafe extern "C" fn interrupted(context: *mut c_void) -> c_int {
    // SAFETY: with_progress pins this borrowed Cancellation for the full hook
    // lifetime, removes the hook before it returns, and invokes SQL on this
    // connection only on this thread. is_cancelled is a panic-free atomic load.
    let cancel = unsafe { &*(context.cast::<Cancellation>()) };
    c_int::from(cancel.is_cancelled())
}

struct Progress<'a> {
    connection: &'a Connection,
}
impl Drop for Progress<'_> {
    fn drop(&mut self) {
        // SAFETY: the connection remains open for this scope, and no statement
        // is active after the validation closure has returned.
        unsafe {
            ffi::sqlite3_progress_handler(self.connection.handle(), 0, None, std::ptr::null_mut())
        };
    }
}

pub(in crate::projection) fn with_progress<T>(
    connection: &Connection,
    cancel: &Cancellation,
    action: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    cancel.check().map_err(Cause::Vfs)?;
    // SAFETY: only fresh provider-owned connections use this helper; there is
    // no previous progress hook to replace. The pointer is borrowed and never
    // escapes the synchronous action. Progress::drop unregisters it first.
    unsafe {
        ffi::sqlite3_progress_handler(
            connection.handle(),
            PROGRESS_OPCODES,
            Some(interrupted),
            (cancel as *const Cancellation).cast_mut().cast::<c_void>(),
        )
    };
    let guard = Progress { connection };
    let result = action(connection);
    drop(guard);
    match result {
        Err(Cause::Sqlite(rusqlite::Error::SqliteFailure(code, _)))
            if code.code == rusqlite::ErrorCode::OperationInterrupted && cancel.is_cancelled() =>
        {
            Err(Cause::Vfs(layrs_vfs::Error::Cancelled))
        }
        Ok(value) => {
            cancel.check().map_err(Cause::Vfs)?;
            Ok(value)
        }
        other => other,
    }
}

pub(in crate::projection) fn check(cancel: Option<&Cancellation>) -> Result<()> {
    if let Some(cancel) = cancel {
        cancel.check().map_err(Cause::Vfs)?;
    }
    Ok(())
}

#[cfg(test)]
#[test]
#[ignore = "owned audited Windows SQLite process; scoped progress handler"]
fn cancellation_interrupts_recursive_sql_and_clears_handler() {
    use super::{database_io::BareConnection, process_profile::tests::child};
    child::run(
        concat!(
            module_path!(),
            "::cancellation_interrupts_recursive_sql_and_clears_handler"
        ),
        120,
        || {
            child::declare();
            let connection = BareConnection::reference().unwrap();
            let cancel = Cancellation::default();
            let signal = cancel.clone();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    signal.cancel();
                });
                let result = with_progress(connection.connection(), &cancel, |c| {
                    c.query_row(
                        "WITH RECURSIVE x(n) AS (SELECT 0 UNION ALL SELECT n+1 FROM x WHERE n<1000000000) SELECT count(*) FROM x",
                        [],
                        |row| row.get::<_, u64>(0),
                    )?;
                    Ok(())
                });
                assert!(matches!(
                    result,
                    Err(Cause::Vfs(layrs_vfs::Error::Cancelled))
                ));
            });
            // More than one progress interval. If the hook still points at the
            // cancelled token, this query is interrupted instead of returning.
            assert_eq!(connection.connection().query_row(
                "WITH RECURSIVE x(n) AS (SELECT 0 UNION ALL SELECT n+1 FROM x WHERE n<1000) SELECT count(*) FROM x",
                [],
                |r| r.get::<_, u64>(0),
            ).unwrap(), 1001);
            connection.finish(Ok(())).unwrap();
        },
    );
}
