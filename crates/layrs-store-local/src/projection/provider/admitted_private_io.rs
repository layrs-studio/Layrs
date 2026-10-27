//! Clean private9 I/O for future format::Access integration. Caller retains
//! lifecycle, owner, FORMAT, namespace and family scopes; none are acquired here.
//! Every usable authority originates in a real bound-file RO preflight followed
//! by positive close. No generic Connection can construct an admission token.
use super::{
    admitted_ordinary::{self, AdmittedOrdinaryData},
    database_io::{self, BareConnection, CleanWriteAdmission, DatabaseFile, WriterCloseObserver},
    recovery::error::{invalid, ProviderBootstrapCause as Cause, Result},
};
use crate::projection::format;
use rusqlite::{ffi, Connection};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

mod handle;
mod validation;
pub(in crate::projection) use handle::AdmittedPrivateHandle;

struct DurableInner {
    database: DatabaseFile,
    data: AdmittedOrdinaryData,
    // A failed explicit rollback invalidates all clones. In particular finish
    // must not repeat that rollback on a still-active connection and must not
    // acknowledge success through a different clone of this authority.
    rollback_failed: AtomicBool,
}

/// Durable means previously positively closed private9 evidence, not a current
/// transaction, family/CAS proof, owner lease or permission to recover/GC.
#[derive(Clone)]
pub(in crate::projection) struct DurableAdmission {
    inner: Arc<DurableInner>,
}
impl DurableAdmission {
    pub(in crate::projection) fn data(&self) -> &AdmittedOrdinaryData {
        &self.inner.data
    }

    /// Check a current caller connection against the bound native filename,
    /// retained inode and immutable baseline. Existing caller transactions are
    /// never ended. An autocommit connection receives a short owned RO scope.
    pub(in crate::projection) fn validate(
        &self,
        c: &Connection,
        marker: &format::Marker,
    ) -> Result<()> {
        validation::validate(self, c, marker)
    }

    fn require_usable(&self) -> Result<()> {
        if self.inner.rollback_failed.load(Ordering::Acquire) {
            return Err(invalid(
                "admitted private authority has a failed rollback; fresh preflight required",
            ));
        }
        self.inner.database.verify()
    }
}

/// Operation-local clean geometry/snapshot gate plus detached immutable DATA.
/// Consume this object exactly once, while the caller's gates remain held.
#[must_use = "a private preflight grants no access until into_handle succeeds"]
pub(in crate::projection) struct AdmittedPreflight {
    admission: DurableAdmission,
    clean: CleanWriteAdmission,
}
impl AdmittedPreflight {
    pub(in crate::projection) fn open(
        path: &Path,
        identity: &str,
        marker: &format::Marker,
    ) -> Result<Self> {
        Self::open_validated(path, identity, marker, |_| Ok(())).map(|(preflight, ())| preflight)
    }

    /// Run the operation-specific preflight in the SAME genuine clean RO
    /// transaction, after private9 provenance has been interpreted. Neither
    /// the admission token nor callback DATA escapes before positive close.
    pub(in crate::projection) fn open_validated<T>(
        path: &Path,
        identity: &str,
        marker: &format::Marker,
        validate: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<(Self, T)> {
        database_io::validate_writer_filename(path)?;
        marker.validate()?;
        let shared = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("admitted private preflight requires shared FORMAT"))?;
        if identity != shared.database_identity
            || !validation::same_native_filename(
                path,
                &shared.metadata_path.join("projection.sqlite3"),
            )
        {
            return Err(invalid(
                "admitted private preflight differs from FORMAT database binding",
            ));
        }
        let database = DatabaseFile::open(path.to_owned(), identity)?;
        let mut verified = None;
        let clean = database_io::clean_admission(&database, false, |c| {
            validation::require_filename(c, database.path())?;
            let data = admitted_ordinary::inspect(c, marker, None)?;
            let value = validate(c)?;
            if c.is_autocommit() {
                return Err(invalid(
                    "admitted preflight callback ended its read transaction",
                ));
            }
            verified = Some((data, value));
            Ok(())
        })?; // Includes the one real RO connection's positive close.
        let (data, value) = verified
            .ok_or_else(|| invalid("admitted private preflight did not interpret private9"))?;
        database.verify()?;
        Ok((
            Self {
                admission: DurableAdmission {
                    inner: Arc::new(DurableInner {
                        database,
                        data,
                        rollback_failed: AtomicBool::new(false),
                    }),
                },
                clean,
            },
            value,
        ))
    }

    pub(in crate::projection) fn data(&self) -> &AdmittedOrdinaryData {
        self.admission.data()
    }
    pub(in crate::projection) fn admission(&self) -> DurableAdmission {
        self.admission.clone()
    }

    /// Every error after an owned connection exists runs its mandatory actual
    /// close. Geometry precedes busy timeout, PRAGMAs and private inspection.
    pub(in crate::projection) fn into_handle(self, write: bool) -> Result<AdmittedPrivateHandle> {
        self.admission.require_usable()?;
        let mut observer = ObserveClose;
        let bare = if write {
            self.clean
                .open(&self.admission.inner.database, &mut observer)?
        } else {
            BareConnection::open(self.admission.inner.database.path(), false)?
        };
        let mut handle = AdmittedPrivateHandle::new(bare, write, self.admission);
        let setup = (|| {
            self.clean.require_geometry(handle.bare())?;
            handle.admission().require_usable()?;
            validation::require_filename(
                handle.connection(),
                handle.admission_ref().inner.database.path(),
            )?;
            handle
                .connection_mut()
                .busy_timeout(std::time::Duration::from_secs(2))?;
            handle
                .connection()
                .execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")?;
            if write {
                handle
                    .connection()
                    .execute_batch("PRAGMA synchronous=FULL;")?;
            }
            handle.verify()
        })();
        match setup {
            Ok(()) => Ok(handle),
            // A geometry failure must close before any validation/setup SQL.
            Err(cause) => handle.fail_setup(cause),
        }
    }
}

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
