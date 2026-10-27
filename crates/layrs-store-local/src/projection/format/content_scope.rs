//! Operation-local content authority, borrowed from format::Access only.
use super::*;
use rusqlite::Connection;

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
