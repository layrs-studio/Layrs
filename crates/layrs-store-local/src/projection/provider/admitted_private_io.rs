//! Clean private9 I/O for future format::Access integration. Caller retains
//! lifecycle, owner, FORMAT, namespace and family scopes; none are acquired here.
//! Every usable authority originates in a real bound-file RO preflight followed
//! by positive close. No generic Connection can construct an admission token.
use super::{
    admitted_ordinary::{self, AdmittedOrdinaryData},
    database_io::{self, BareConnection, CleanWriteAdmission, DatabaseFile, WriterCloseObserver},
    recovery::error::{ProviderBootstrapCause as Cause, Result, invalid},
};
use crate::projection::format;
use rusqlite::{Connection, ffi};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
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
    /// A caller-owned transaction failed its explicit rollback. Invalidate all
    /// retained clones before any later validation or cleanup can retry it.
    pub(in crate::projection) fn invalidate_after_failed_rollback(&self) {
        self.inner.rollback_failed.store(true, Ordering::Release);
    }

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

    /// Authenticate the immutable ingress and outgoing partition in one retained
    /// snapshot. A denial ingress is never decoded as an outgoing intent.
    pub(in crate::projection) fn has_live(
        &self,
        c: &Connection,
        marker: &format::Marker,
    ) -> Result<bool> {
        validation::validate_with(self, c, marker, |c| {
            let actual = super::managed_admission::read_admitted_import(c, marker, None)?;
            use super::records::transition::TransitionState;
            Ok(actual.outgoing().iter().any(|row| {
                matches!(
                    row.state,
                    TransitionState::Planned
                        | TransitionState::Copying
                        | TransitionState::Staging
                        | TransitionState::Ready
                )
            }))
        })
    }

    pub(in crate::projection) fn preflight_local_capture(
        &self,
        c: &Connection,
        marker: &format::Marker,
        state: &crate::projection::VersionedProjection,
        changes: &[crate::projection::capture::CapturedChange],
    ) -> Result<()> {
        validation::validate_with(self, c, marker, |c| {
            super::managed_admission::read_admitted_import(c, marker, None)?
                .preflight_local_capture(state, changes, None)
        })
    }

    /// Forced-append funding for a local C14 Restore, in the writer's current
    /// snapshot and before any reservation, staging or filesystem effect.
    pub(in crate::projection) fn preflight_local_restore(
        &self,
        c: &Connection,
        marker: &format::Marker,
        state: &crate::projection::VersionedProjection,
        plan: &crate::projection::restore::LocalRestorePlan,
    ) -> Result<()> {
        validation::validate_with(self, c, marker, |c| {
            super::managed_admission::read_admitted_import(c, marker, None)?
                .preflight_local_restore(state, plan, None)
        })
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
        self.into_handle_for(write, AccessPurpose::General)
    }

    /// Only the explicit local Capture route may request a C14 writer. The
    /// preceding full preflight enforces its fixed-Ready capture suffix.
    pub(in crate::projection) fn into_local_capture_handle(self) -> Result<AdmittedPrivateHandle> {
        self.into_handle_for(true, AccessPurpose::LocalCapture)
    }

    pub(in crate::projection) fn into_local_capture_cancel_handle(
        self,
        data: &crate::projection::retention::RestoreCancellationData,
    ) -> Result<AdmittedPrivateHandle> {
        if !data.matches_local_capture(self.data())
            || self.data().ready().plan.denial_coverage.is_none()
        {
            return Err(invalid(
                "local Capture cancellation requires authenticated C14 preflight",
            ));
        }
        self.into_handle_for(true, AccessPurpose::LocalCapture)
    }

    /// Only the explicit local Restore apply route may request a C14 Restore
    /// writer. The preceding full preflight authenticated the local suffix;
    /// apply still funds its forced append before any durable effect.
    pub(in crate::projection) fn into_local_restore_handle(self) -> Result<AdmittedPrivateHandle> {
        self.into_handle_for(true, AccessPurpose::LocalRestore)
    }

    /// Accepted local Restore journal recovery. The scope is DATA minted only
    /// from authenticated admission and the pending journal's own operation.
    pub(in crate::projection) fn into_local_restore_recovery_handle(
        self,
        data: &crate::projection::restore::RestoreRecoveryData,
    ) -> Result<AdmittedPrivateHandle> {
        if !data
            .local_scope()
            .is_some_and(|scope| scope.matches(self.data()))
            || self.data().ready().plan.denial_coverage.is_none()
        {
            return Err(invalid(
                "local Restore recovery requires authenticated C14 preflight",
            ));
        }
        self.into_handle_for(true, AccessPurpose::LocalRestore)
    }

    /// Pre-journal local Restore cancellation (budget/prejournal/staging only).
    pub(in crate::projection) fn into_local_restore_cancel_handle(
        self,
        data: &crate::projection::retention::RestoreCancellationData,
    ) -> Result<AdmittedPrivateHandle> {
        if !data.matches_local_restore(self.data())
            || self.data().ready().plan.denial_coverage.is_none()
        {
            return Err(invalid(
                "local Restore cancellation requires authenticated C14 preflight",
            ));
        }
        self.into_handle_for(true, AccessPurpose::LocalRestore)
    }

    fn into_handle_for(self, write: bool, purpose: AccessPurpose) -> Result<AdmittedPrivateHandle> {
        if write
            && matches!(purpose, AccessPurpose::General)
            && (self.data().ready().plan.denial_coverage.is_some()
                || self.data().receipt().denial_coverage.is_some()
                || self
                    .data()
                    .source_published()
                    .plan
                    .denial_coverage
                    .is_some())
        {
            return Err(invalid("admitted denial coverage remains read-only"));
        }
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

enum AccessPurpose {
    General,
    LocalCapture,
    LocalRestore,
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
