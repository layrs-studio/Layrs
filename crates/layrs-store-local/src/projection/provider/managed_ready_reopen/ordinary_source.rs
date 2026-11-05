//! Fresh B ownership and positively closed admitted provenance only. This is
//! deliberately independent of the ordinary current-frontier publication DATA.
//! No mutable opener, lease admission, settlement or source byte proof exists.
use super::*;
use crate::projection::provider::{
    admitted_ordinary,
    admitted_private_io::{AdmittedPreflight, DurableAdmission},
    records::{
        Record,
        import::{AdmittedSourceBinding, ImportSource, ImportSourceReferences},
    },
};
use rusqlite::Connection;

/// Narrow owning source capability for fresh adapters outside this module.
/// Its only constructor acquires Exclusive before a destination is opened;
/// no shared holder, DATA or lock upgrade can construct it.
pub(in crate::projection::provider) struct OrdinaryExclusiveSource {
    source: OrdinaryAdmissionSource,
}
impl OrdinaryExclusiveSource {
    pub(in crate::projection::provider) fn open(
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<Self> {
        Ok(Self {
            source: OrdinaryAdmissionSource::open_exclusive(root, cancel)?,
        })
    }
    pub(in crate::projection::provider) fn root(&self) -> &Path {
        self.source.pins.checkout_path()
    }
    pub(in crate::projection::provider) fn marker(&self) -> &crate::projection::format::Marker {
        &self.source.pins.marker
    }
    pub(in crate::projection::provider) fn database(&self) -> &database_io::DatabaseFile {
        &self.source.pins.database
    }
    pub(in crate::projection::provider) fn require_identity(
        &self,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.source.require_identity(cancel)
    }
    pub(in crate::projection::provider) fn require_snapshot(
        &self,
        c: &Connection,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.source.require_snapshot(c, cancel)
    }
    pub(in crate::projection::provider) fn require_ready_source(
        &self,
        ready: &ProviderTransitionIntentRecord,
    ) -> Result<()> {
        self.source.require_ready_source(ready)
    }
    pub(in crate::projection::provider) fn inspect_ready_destination(
        &self,
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<OrdinaryReadyInspection> {
        let pins = ManagedReadyPins::open(root, cancel)?;
        self.source.require_destination(&pins, cancel)?;
        OrdinaryReadyInspection::open_shared(pins, cancel)
    }

    /// Preliminary C resources only. Preserve the anti-alias/family checks
    /// before C ownership; actual Ready and complete family authentication are
    /// the separate fresh family driver's responsibility.
    pub(in crate::projection::provider) fn inspect_family_destination(
        &self,
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<OrdinaryFamilyResources> {
        let pins = ManagedReadyPins::open(root, cancel)?;
        self.source.require_destination(&pins, cancel)?;
        OrdinaryFamilyResources::open_shared(pins, cancel)
    }

    /// Fresh checkout population acquires a distinct C Exclusive immediately.
    pub(in crate::projection::provider) fn inspect_checkout_destination(
        &self,
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<OrdinaryCheckoutResources> {
        let pins = ManagedReadyPins::open(root, cancel)?;
        self.source.require_destination(&pins, cancel)?;
        OrdinaryCheckoutResources::open_exclusive(pins, cancel)
    }

    /// Private finalization needs its own Exclusive C acquisition from the
    /// start. Preserve the anti-alias/family preflight before that acquisition;
    /// no shared inspection or allocation can be upgraded/adopted here.
    pub(in crate::projection::provider) fn inspect_private_ready_destination(
        &self,
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<OrdinaryPrivateReadyResources> {
        let pins = ManagedReadyPins::open(root, cancel)?;
        self.source.require_destination(&pins, cancel)?;
        OrdinaryPrivateReadyResources::open_exclusive(pins, cancel)
    }
}

/// Nonforgeable fresh source capability, acquired before any destination owner.
/// The public constructor acquires a shared B owner. The private publication
/// constructor acquires an exclusive owner independently, never by upgrade.
/// Historical Published may precede B's current coherent suffix.
pub(in crate::projection::provider) struct OrdinaryAdmissionSource {
    pub(super) pins: ManagedReadyPins,
    baseline: DurableAdmission,
    _owner: StoreLock,
    _lifecycle: StoreLock,
}

impl OrdinaryAdmissionSource {
    pub(in crate::projection::provider) fn open(
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<Self> {
        Self::open_with_mode(root, SessionMode::Shared, cancel)
    }

    pub(super) fn open_exclusive(root: &Path, cancel: &Cancellation) -> Result<Self> {
        Self::open_with_mode(root, SessionMode::Exclusive, cancel)
    }

    fn open_with_mode(root: &Path, mode: SessionMode, cancel: &Cancellation) -> Result<Self> {
        cancel.check()?;
        let pins = ManagedReadyPins::open(root, cancel)?;
        let lifecycle = pins.family.lifecycle_lease()?;
        let owner = StoreLock::acquire_existing(&pins.metadata_path().join("layrs.lock"), mode)?;
        pins.verify(cancel)?;
        if pins.control.identity_token() != pins.marker.store_identity() {
            return Err(invalid(
                "ordinary admission source control differs from FORMAT",
            ));
        }
        let shared = pins
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary admission source requires shared FORMAT"))?;
        // No into_handle: this genuine clean RO preflight positively closes
        // before its immutable baseline can be retained by the capability.
        let (preflight, ()) = AdmittedPreflight::open_validated(
            pins.database.path(),
            &shared.database_identity,
            &pins.marker,
            |_| {
                pins.verify(cancel)?;
                cancel.check().map_err(Into::into)
            },
        )?;
        let baseline = preflight.admission();
        drop(preflight);
        let held = Self {
            pins,
            baseline,
            _owner: owner,
            _lifecycle: lifecycle,
        };
        held.require_identity(cancel)?;
        Ok(held)
    }

    /// Filesystem/binding checks only. Safe beneath the short catalog gate and
    /// after destination COMMIT before its mandatory writer close.
    pub(super) fn require_identity(&self, cancel: &Cancellation) -> Result<()> {
        cancel.check()?;
        #[cfg(test)]
        database_io::tests::fresh_ordinary_support::probe(
            self.pins.database.path(),
            database_io::tests::fresh_ordinary_support::ReadBoundary::OrdinarySourceIdentity,
        )?;
        self.pins.verify(cancel)?;
        let data = self.baseline.data();
        let binding = data.binding();
        let marker = &self.pins.marker;
        let shared = marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary admission source family binding absent"))?;
        let family = self.pins.family.writer_record()?;
        if data.marker() != marker
            || !matches!(data.private_schema(), 9 | 10)
            || self.pins.control.identity_token() != marker.store_identity()
            || shared.database_identity != binding.database_identity
            || marker.identity.worktree_id != binding.owner
            || self.pins.family.writer_identity(&binding.owner) != marker.identity
            || shared.root != self.pins.family.root()
            || shared.metadata_path != self.pins.family.member(&binding.owner)
            || shared.incarnation != family.incarnation
            || shared.root_identity != family.root_identity
            || binding.family_incarnation != family.incarnation
            || binding.family_root_identity != family.root_identity
            || binding.family_catalog_identity != family.catalog_file_identity
            || binding.family_marker_digest != self.pins.family.marker_digest()
        {
            return Err(invalid(
                "ordinary admission source physical admission binding differs",
            ));
        }
        cancel.check()?;
        Ok(())
    }

    /// Called only in the actual independently opened B RO snapshot. The
    /// detached baseline commits the immutable import, not the current head.
    pub(super) fn require_snapshot(&self, c: &Connection, cancel: &Cancellation) -> Result<()> {
        if c.is_autocommit() {
            return Err(invalid("ordinary admission source snapshot ended"));
        }
        let current = admitted_ordinary::inspect(c, &self.pins.marker, Some(cancel))?;
        self.baseline.data().require_same_import(&current)?;
        self.require_identity(cancel)
    }

    /// Bind the incoming C prefix to this genuinely inspected B admission.
    /// This is DATA equality only; commit must still observe actual Published
    /// through its own independent B RO connection.
    pub(super) fn require_ready_source(
        &self,
        ready: &ProviderTransitionIntentRecord,
    ) -> Result<()> {
        let data = self.baseline.data();
        let binding = AdmittedSourceBinding {
            admission_digest: data.admission_digest().to_owned(),
            incoming_prefix_id: data.prefix_id().to_owned(),
            incoming_receipt_id: data.receipt().digest()?,
            database_identity: data.binding().database_identity.clone(),
        };
        if !matches!(&ready.plan.source_prefix.source,
            ImportSource::Ordinary { private_schema, .. } if *private_schema == data.private_schema())
        {
            return Err(invalid(
                "fresh Ordinary destination source layout differs from B",
            ));
        }
        ready
            .plan
            .source_prefix
            .require_references(ImportSourceReferences::AdmittedOrdinary {
                marker: &self.pins.marker,
                admission: &binding,
            })?;
        Ok(())
    }

    /// Reject aliases before attempting C's owner, so opening C cannot
    /// reacquire/upgrade B's already held owner. Both must be genuine members
    /// of the same pinned family, with different checkout and private DBs.
    pub(super) fn require_destination(
        &self,
        pins: &ManagedReadyPins,
        cancel: &Cancellation,
    ) -> Result<()> {
        self.require_identity(cancel)?;
        pins.verify(cancel)?;
        if pins.root.identity_token() == self.pins.root.identity_token()
            || pins.marker.identity.worktree_id == self.pins.marker.identity.worktree_id
            || pins.database.path() == self.pins.database.path()
            || pins.metadata_path() == self.pins.metadata_path()
            || pins.family.root() != self.pins.family.root()
            || pins.family.marker_bytes() != self.pins.family.marker_bytes()
        {
            return Err(invalid(
                "ordinary admission requires distinct B/C in the same physical family",
            ));
        }
        let source = self
            .pins
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary admission source database binding absent"))?;
        let destination = pins
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("ordinary admission destination database binding absent"))?;
        if source.database_identity == destination.database_identity {
            return Err(invalid(
                "ordinary admission B/C private database identities alias",
            ));
        }
        Ok(())
    }
}
