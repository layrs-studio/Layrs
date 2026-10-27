//! Internal borrowing bridge. Ownership never leaves the operation finalizer.
use super::*;
use std::ops::{Deref, DerefMut};

pub(in crate::projection) enum PrivateConnection {
    Legacy(rusqlite::Connection),
    #[cfg(windows)]
    Admitted(provider::admitted_private_io::AdmittedPrivateHandle),
}

impl Deref for PrivateConnection {
    type Target = rusqlite::Connection;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Legacy(c) => c,
            #[cfg(windows)]
            Self::Admitted(handle) => handle.connection(),
        }
    }
}
impl DerefMut for PrivateConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Legacy(c) => c,
            #[cfg(windows)]
            Self::Admitted(handle) => handle.connection_mut(),
        }
    }
}

impl PrivateConnection {
    pub(super) fn finish<T>(self, result: Result<T>) -> Result<T> {
        match self {
            #[cfg(windows)]
            Self::Admitted(handle) => handle
                .finish(result.map_err(Into::into))
                .map_err(store_cause),
            Self::Legacy(c) => {
                let result = if c.is_autocommit() {
                    result
                } else {
                    let result = result.and_then(|_| {
                        Err(invalid("operation left an unfinished private transaction"))
                    });
                    conclude(result, c.execute_batch("ROLLBACK").map_err(Into::into))
                };
                let closed = c.close().map_err(|(c, error)| {
                    // Preserve the legacy handle's cleanup behavior on failure;
                    // the admitted route uses its own quarantine/profile policy.
                    drop(c);
                    ProjectionStoreError::Database(error)
                });
                conclude(result, closed)
            }
        }
    }
}

#[cfg(windows)]
fn store_cause(cause: provider::recovery::error::ProviderBootstrapCause) -> ProjectionStoreError {
    use provider::recovery::error::ProviderBootstrapCause as Cause;
    match cause {
        Cause::Validation(error) => *error,
        Cause::Io(error) => ProjectionStoreError::Io(error),
        Cause::Sqlite(error) => ProjectionStoreError::Database(error),
        other => ProjectionStoreError::ProviderContent(Box::new(other)),
    }
}

fn conclude<T>(result: Result<T>, close: Result<()>) -> Result<T> {
    match (result, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(close)) => {
            #[cfg(windows)]
            {
                Err(provider::recovery::error::ProviderBootstrapCause::Cleanup {
                    primary: Box::new(primary.into()),
                    close: Box::new(close.into()),
                }
                .into())
            }
            #[cfg(not(windows))]
            {
                Err(invalid(format!(
                    "{primary}; private close also failed: {close}"
                )))
            }
        }
    }
}
