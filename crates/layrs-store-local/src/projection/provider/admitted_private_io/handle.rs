use super::*;

/// References bridge existing ordinary APIs; this adapter exposes no raw handle
/// or ownership-transfer API. Callers must not replace the borrowed Connection
/// and must finish explicitly even after operation failure.
/// An abandoned handle follows BareConnection's existing quarantine-on-Drop
/// contract and never silently acknowledges a successful close.
#[must_use = "admitted private handles require an explicit finish on every path"]
pub(in crate::projection) struct AdmittedPrivateHandle {
    bare: BareConnection,
    write: bool,
    admission: DurableAdmission,
}
impl AdmittedPrivateHandle {
    pub(super) fn new(bare: BareConnection, write: bool, admission: DurableAdmission) -> Self {
        Self {
            bare,
            write,
            admission,
        }
    }
    pub(super) fn bare(&self) -> &BareConnection {
        &self.bare
    }
    pub(super) fn admission_ref(&self) -> &DurableAdmission {
        &self.admission
    }

    /// Borrowing permits existing transaction APIs. Moving/replacing through
    /// the bridge is forbidden; verify/finish reject a changed main filename.
    pub(in crate::projection) fn connection(&self) -> &Connection {
        self.bare.connection()
    }
    pub(in crate::projection) fn connection_mut(&mut self) -> &mut Connection {
        self.bare.connection_mut()
    }
    pub(in crate::projection) fn data(&self) -> &AdmittedOrdinaryData {
        self.admission.data()
    }
    pub(in crate::projection) fn admission(&self) -> DurableAdmission {
        self.admission.clone()
    }
    pub(in crate::projection) fn verify(&self) -> Result<()> {
        self.admission
            .validate(self.connection(), self.data().marker())
    }

    /// Read DATA in the same snapshot that revalidates this durable import.
    /// Transaction ownership and rollback-failure poisoning remain internal.
    pub(in crate::projection) fn with_validation<T>(
        &self,
        callback: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        validation::validate_with(
            &self.admission,
            self.connection(),
            self.data().marker(),
            callback,
        )
    }

    /// Success requires an autocommit connection, immutable private9 provenance
    /// and a positive native close. No COMMIT occurs here. An unfinished caller
    /// transaction is refused and rolled back once; rollback errors invalidate
    /// all token clones and are preserved alongside the original operation.
    pub(in crate::projection) fn finish<T>(self, result: Result<T>) -> Result<T> {
        let unfinished = !self.connection().is_autocommit();
        let result = if unfinished {
            result.and_then(|_| {
                Err(invalid(
                    "admitted private operation left an unfinished transaction",
                ))
            })
        } else {
            result
        };
        let result = conclude(result, self.verify());
        self.close_result(result)
    }

    /// Close after a failed enclosing operation without starting admission SQL.
    /// The error cannot become success. Rollback, pin verification and observed
    /// native close still run through the same cleanup/quarantine machinery.
    pub(in crate::projection) fn close_failed_operation<T>(self, cause: Cause) -> Result<T> {
        self.close_result(Err(cause))
    }

    /// Setup may have failed its geometry check, before SQL was permitted.
    /// Preserve that original error and close without starting validation SQL.
    pub(super) fn fail_setup(self, cause: Cause) -> Result<Self> {
        self.close_result(Err(cause))
    }

    fn close_result<T>(self, result: Result<T>) -> Result<T> {
        let result = if self.connection().is_autocommit()
            || self.admission.inner.rollback_failed.load(Ordering::Acquire)
        {
            result
        } else {
            let rollback = self
                .connection()
                .execute_batch("ROLLBACK")
                .map_err(Cause::from);
            if rollback.is_err() {
                self.admission
                    .inner
                    .rollback_failed
                    .store(true, Ordering::Release);
            }
            conclude(result, rollback)
        };
        let result = conclude(result, self.admission.inner.database.verify());
        if self.write {
            self.bare.finish_writer(result, &mut ObserveClose)
        } else {
            self.bare.finish(result)
        }
    }
}
