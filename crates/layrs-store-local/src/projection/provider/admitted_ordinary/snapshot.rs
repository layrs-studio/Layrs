use super::*;

/// Only this module's BEGIN is ended here. No PRAGMA, savepoint or connection
/// opening is hidden in inspection, and caller transactions survive failures.
pub(super) fn with_snapshot<T>(
    c: &Connection,
    cancel: Option<&Cancellation>,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    cancellation::check(cancel)?;
    if !c.is_autocommit() {
        return action();
    }
    c.execute_batch("BEGIN DEFERRED")?;
    let result = action().and_then(|value| {
        cancellation::check(cancel)?;
        Ok(value)
    });
    let close = c.execute_batch("ROLLBACK").map_err(Cause::from);
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
