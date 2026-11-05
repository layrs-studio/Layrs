//! Operation-local content authority, borrowed from format::Access only.
use super::*;
use rusqlite::Connection;

/// Authenticated fixed Ready frontier of a C14 admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::projection) struct DenialReady {
    pub generation: u64,
    pub capture_sequence: u64,
    pub digest: String,
}

pub(in crate::projection) struct ContentScope<'a> {
    marker: &'a Marker,
    root: &'a Path,
    _owner: &'a StoreLock,
    family: Option<&'a shared::Lease>,
    metadata: &'a PinnedDirectory,
    database: &'a super::super::file::DatabasePin,
    boundaries: &'a PreliminaryPins,
    recovery_binding: Option<&'a RecoveryBinding>,
    #[cfg(windows)]
    admission: Option<provider::admitted_private_io::DurableAdmission>,
}

impl<'a> ContentScope<'a> {
    #[cfg_attr(not(windows), allow(unused_variables))]
    pub(in crate::projection) fn preflight_local_capture(
        &self,
        c: &Connection,
        store: &ProjectionStore,
        state: &VersionedProjection,
        changes: &[capture::CapturedChange],
    ) -> Result<()> {
        self.validate(c, store)?;
        #[cfg(windows)]
        if let Some(admission) = &self.admission {
            admission.preflight_local_capture(c, self.marker, state, changes)?;
        }
        Ok(())
    }

    /// Fixed C14 Ready frontier as authenticated admission DATA. None for a
    /// legacy, non-denial or non-Windows scope. This performs no SQL.
    pub(in crate::projection) fn denial_ready(&self) -> Option<DenialReady> {
        #[cfg(windows)]
        if let Some(admission) = &self.admission {
            let data = admission.data();
            if data.ready().plan.denial_coverage.is_some() {
                return Some(DenialReady {
                    generation: data.ready_generation(),
                    capture_sequence: data.ready_capture_sequence(),
                    digest: data.ready().plan.placement_digest.clone(),
                });
            }
        }
        None
    }

    /// Fund a local C14 Restore's forced append before its first durable
    /// effect. Legacy and non-denial scopes keep their existing behavior.
    pub(in crate::projection) fn preflight_local_restore(
        &self,
        c: &Connection,
        store: &ProjectionStore,
        state: &VersionedProjection,
        plan: &restore::LocalRestorePlan,
    ) -> Result<()> {
        self.validate(c, store)?;
        #[cfg(windows)]
        if let Some(admission) = &self.admission {
            admission.preflight_local_restore(c, self.marker, state, plan)?;
        }
        #[cfg(not(windows))]
        let _ = (state, plan);
        Ok(())
    }

    pub(in crate::projection) fn has_live_export(
        &self,
        c: &Connection,
        store: &ProjectionStore,
    ) -> Result<bool> {
        self.validate(c, store)?;
        #[cfg(windows)]
        {
            if let Some(admission) = &self.admission {
                return admission.has_live(c, self.marker).map_err(Into::into);
            }
            provider::ordinary_transition::has_live(c)
        }
        #[cfg(not(windows))]
        Ok(false)
    }

    /// Observe a caller transaction's one explicit rollback and poison retained
    /// admitted authority on failure, as admission-owned snapshots already do.
    /// No validation query is issued by this cleanup operation.
    pub(in crate::projection) fn rollback_transaction(&self, c: &Connection) -> Result<()> {
        let result = c.execute_batch("ROLLBACK").map_err(Into::into);
        #[cfg(windows)]
        if result.is_err() {
            if let Some(admission) = &self.admission {
                admission.invalidate_after_failed_rollback();
            }
        }
        result
    }

    /// Recheck retained filesystem identities without starting validation SQL.
    /// This does not authenticate admission, recovery state or content bytes;
    /// those checks remain the enclosing operation's separate responsibility.
    pub(in crate::projection) fn verify_identity(&self, store: &ProjectionStore) -> Result<()> {
        if self.marker != &store.marker || self.root != store.root {
            return Err(invalid("content scope differs from owning projection"));
        }
        self.boundaries.verify(self.root, self.marker)?;
        verify_boundaries(self.root, self.marker, self.metadata, self.database)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        marker: &'a Marker,
        root: &'a Path,
        owner: &'a StoreLock,
        family: Option<&'a shared::Lease>,
        metadata: &'a PinnedDirectory,
        database: &'a super::super::file::DatabasePin,
        boundaries: &'a PreliminaryPins,
        recovery_binding: Option<&'a RecoveryBinding>,
        #[cfg(windows)] admission: Option<provider::admitted_private_io::DurableAdmission>,
    ) -> Self {
        Self {
            marker,
            root,
            _owner: owner,
            family,
            metadata,
            database,
            boundaries,
            recovery_binding,
            #[cfg(windows)]
            admission,
        }
    }

    pub(in crate::projection) fn validate(
        &self,
        c: &Connection,
        store: &ProjectionStore,
    ) -> Result<()> {
        if self.marker != &store.marker || self.root != store.root {
            return Err(invalid("content scope differs from owning projection"));
        }
        self.boundaries.verify(self.root, self.marker)?;
        verify_boundaries(self.root, self.marker, self.metadata, self.database)?;
        if let Some(expected) = self.recovery_binding {
            recovery::verify_binding(self.root, self.marker, self.database, self.family, expected)?;
        }
        #[cfg(windows)]
        if let Some(admission) = &self.admission {
            admission.validate(c, self.marker)?;
            let family = self
                .family
                .ok_or_else(|| invalid("admitted content scope lost family lease"))?;
            family.require_same_admission(admission.data())?;
            return family.verify();
        }
        provider::require_ordinary(c)?;
        if let Some(family) = self.family {
            if family.is_admitted() {
                return Err(invalid("ordinary content scope has an admitted family"));
            }
            family.verify()?;
        }
        Ok(())
    }

    pub(in crate::projection) fn verify(
        &self,
        c: &Connection,
        store: &ProjectionStore,
    ) -> Result<()> {
        self.validate(c, store)
    }
}

/// Keep legacy callers intact while requiring explicit authority for private9.
/// The borrowed form never re-acquires owner/lifecycle gates or a generic lease.
#[cfg_attr(not(windows), allow(clippy::large_enum_variant))]
pub(in crate::projection) enum ContentFamily<'a> {
    Borrowed(Option<&'a shared::Lease>),
    Legacy(Option<shared::Lease>),
}
impl ContentFamily<'_> {
    pub(in crate::projection) fn as_ref(&self) -> Option<&shared::Lease> {
        match self {
            Self::Borrowed(lease) => *lease,
            Self::Legacy(lease) => lease.as_ref(),
        }
    }
}

pub(in crate::projection) fn content_family<'a>(
    c: &Connection,
    store: &ProjectionStore,
    scope: Option<&'a ContentScope<'_>>,
) -> Result<ContentFamily<'a>> {
    if let Some(scope) = scope {
        scope.validate(c, store)?;
        Ok(ContentFamily::Borrowed(scope.family))
    } else {
        provider::require_ordinary(c)?;
        Ok(ContentFamily::Legacy(shared::for_store(store)?))
    }
}
