//! Physical fresh Ready8 -> Admitted9 handoff. The source is independently
//! opened read-only here; source/family snapshots span destination COMMIT and
//! writer close. This is not the later ordinary opener or a GC capability.
use super::*;
use crate::projection::{
    provider::{
        database_io::WriterCloseObserver,
        managed_admission::{
            admit_denial_ready, admit_ready, read_admitted_import,
            require_ready_admission_capacity, AdmissionCoverage, AdmissionWriteEffect,
        },
        published_handoff::{read_published_for_ready, PublicationSource},
        records::Record,
    },
    shared::{exact_content::validate_protected_installations, exact_protection::ExactProtection},
};
use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum AdmissionStep {
    Validating,
    WriterOpenAttempted,
    MutationAttempted,
    CommitAttempted,
    CommitReturned,
    WriterClosed,
    ReadScopesClosed,
    FreshVerified,
}

/// Failure retains checkout/CAS pins and the exclusive destination owner. In
/// particular CommitAttempted is unknown, and CommitReturned may still have
/// failed its acknowledgement. No failure claims that migration was undone.
pub(in crate::projection::provider) struct AdmissionFailure {
    pub step: AdmissionStep,
    pub cause: Cause,
    pub held: ManagedReadyAdmission,
}

/// Constructed only by independently acquiring an exclusive destination owner;
/// a read-only ManagedReadyInspection cannot be upgraded into this object.
pub(in crate::projection::provider) struct ManagedReadyAdmission {
    inspection: ManagedReadyInspection,
}
impl ManagedReadyAdmission {
    pub(in crate::projection::provider) fn open(
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<Self> {
        Ok(Self {
            inspection: ManagedReadyInspection::open_with_mode(
                root,
                SessionMode::Exclusive,
                cancel,
            )?,
        })
    }

    /// Resolve a possibly committed handoff using destination evidence alone.
    /// In particular this never invokes the source publisher or opens a writer.
    /// A still-Ready8 destination refuses and remains held for explicit retry.
    pub(in crate::projection::provider) fn resolve(
        self,
        cancel: &Cancellation,
    ) -> std::result::Result<String, AdmissionFailure> {
        let mut step = AdmissionStep::Validating;
        let result = (|| {
            self.require_physical(cancel)?;
            let held = &self.inspection;
            let _catalog = StoreLock::acquire_existing(
                &held.pins.family.root().join("metadata/catalog.lock"),
                SessionMode::Shared,
            )?;
            let read = || -> Result<String> {
                valid(database_io::read_cancellable(
                    &held.pins.database,
                    cancel,
                    |c| {
                        let digest = self.require_admitted(c, cancel)?;
                        valid(database_io::read_cancellable(
                            &held.pins.family.catalog,
                            cancel,
                            |family_c| {
                                self.require_family(family_c, cancel)?;
                                self.require_physical(cancel)?;
                                Ok(digest)
                            },
                        )?)
                    },
                )?)
            };
            let digest = read()?;
            step = AdmissionStep::ReadScopesClosed;
            self.require_physical(cancel)?;
            if read()? != digest {
                return Err(invalid("physical admission resolution ACK changed"));
            }
            self.require_physical(cancel)?;
            cancel.check()?;
            step = AdmissionStep::FreshVerified;
            Ok(digest)
        })();
        result.map_err(|cause| AdmissionFailure {
            step,
            cause,
            held: self,
        })
    }

    /// The bootstrap adapter owns the genuine source owner/native pins and
    /// supplies only their identity revalidation here. No source content callback
    /// or namespace enumeration is invoked while a catalog gate is held.
    pub(in crate::projection::provider) fn commit(
        self,
        source_file: &database_io::DatabaseFile,
        source: PublicationSource<'_>,
        source_boundary: impl Fn() -> Result<()>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<String, AdmissionFailure> {
        self.commit_coverage(
            source_file,
            source,
            source_boundary,
            close,
            cancel,
            AdmissionCoverage::Legacy,
        )
    }

    pub(in crate::projection::provider) fn commit_denial(
        self,
        source_file: &database_io::DatabaseFile,
        source: PublicationSource<'_>,
        source_boundary: impl Fn() -> Result<()>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<String, AdmissionFailure> {
        self.commit_coverage(
            source_file,
            source,
            source_boundary,
            close,
            cancel,
            AdmissionCoverage::Denial,
        )
    }

    fn commit_coverage(
        self,
        source_file: &database_io::DatabaseFile,
        source: PublicationSource<'_>,
        source_boundary: impl Fn() -> Result<()>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        coverage: AdmissionCoverage,
    ) -> std::result::Result<String, AdmissionFailure> {
        let mut step = AdmissionStep::Validating;
        let result = self.commit_inner(
            source_file,
            source,
            source_boundary,
            close,
            cancel,
            &mut step,
            coverage,
        );
        match result {
            Ok(digest) => Ok(digest),
            Err(cause) => Err(AdmissionFailure {
                step,
                cause,
                held: self,
            }),
        }
    }

    fn commit_inner(
        &self,
        source_file: &database_io::DatabaseFile,
        source: PublicationSource<'_>,
        source_boundary: impl Fn() -> Result<()>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
        coverage: AdmissionCoverage,
    ) -> Result<String> {
        let held = &self.inspection;
        coverage.require(&held.data.ready, &held.data.receipt)?;
        held.require(cancel)?;
        source_boundary()?;
        // Rebind the actual file inode to the independently interpreted source
        // identity. A copied SQL database at another inode cannot be nominated.
        let source_database = database_io::DatabaseFile::open(
            source_file.path().to_owned(),
            &source.binding.recovery.database_identity,
        )?;
        let _catalog = StoreLock::acquire_existing(
            &held.pins.family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        let digest = valid(database_io::read_cancellable(
            &source_database,
            cancel,
            |source_c| {
                let published = read_published_for_ready(
                    source_c,
                    source,
                    &held.data.ready,
                    &held.data.receipt,
                    Some(cancel),
                )?;
                valid(database_io::read_cancellable(
                    &held.pins.family.catalog,
                    cancel,
                    |family_c| {
                        self.require_family(family_c, cancel)?;
                        self.require_physical(cancel)?;
                        source_boundary()?;
                        let admission =
                            database_io::clean_admission(&held.pins.database, false, |c| {
                                let fresh = read_managed_ready(
                                    c,
                                    &held.pins.marker,
                                    family_c,
                                    Some(cancel),
                                )?
                                .into_snapshot()?;
                                coverage.require(&fresh.ready, &fresh.receipt)?;
                                held.data.require_same(&fresh)?;
                                require_ready_admission_capacity(&fresh, Some(cancel))?;
                                coverage.require(published.published(), published.receipt())?;
                                published.require(source_c, Some(cancel))
                            })?;
                        self.require_physical(cancel)?;
                        source_boundary()?;
                        *step = AdmissionStep::WriterOpenAttempted;
                        let bare = admission.open(&held.pins.database, close)?;
                        let result = (|| {
                            admission.require_geometry(&bare)?;
                            let c = bare.connection();
                            c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                            let ready =
                                read_managed_ready(c, &held.pins.marker, family_c, Some(cancel))?;
                            *step = AdmissionStep::MutationAttempted;
                            let writer = match coverage {
                                AdmissionCoverage::Legacy => admit_ready,
                                AdmissionCoverage::Denial => admit_denial_ready,
                            };
                            if writer(
                                c,
                                &held.pins.marker,
                                family_c,
                                source_c,
                                &ready,
                                &published,
                                Some(cancel),
                            )? != AdmissionWriteEffect::Admitted
                            {
                                return Err(invalid(
                                    "fresh physical admission did not migrate Ready8",
                                ));
                            }
                            let digest = self.require_admitted(c, cancel)?;
                            self.require_family(family_c, cancel)?;
                            published.require(source_c, Some(cancel))?;
                            self.require_physical(cancel)?;
                            source_boundary()?;
                            cancel.check()?;
                            *step = AdmissionStep::CommitAttempted;
                            c.execute_batch("COMMIT")?;
                            *step = AdmissionStep::CommitReturned;
                            // Only retained identity/byte proofs after COMMIT. The old
                            // Ready8 SQL proof must never be reused against schema9.
                            self.require_physical(cancel)?;
                            source_boundary()?;
                            Ok(digest)
                        })();
                        // A failed transaction is rolled back by the mandatory real
                        // close; a failed COMMIT remains reported as indeterminate.
                        let digest = bare.finish_writer(result, close)?;
                        *step = AdmissionStep::WriterClosed;
                        Ok(digest)
                    },
                )?)
            },
        )?)?;
        *step = AdmissionStep::ReadScopesClosed;
        source_boundary()?;
        self.require_physical(cancel)?;
        let fresh = valid(database_io::read_cancellable(
            &held.pins.database,
            cancel,
            |c| {
                valid(database_io::read_cancellable(
                    &held.pins.family.catalog,
                    cancel,
                    |family_c| {
                        self.require_family(family_c, cancel)?;
                        self.require_admitted(c, cancel)
                    },
                )?)
            },
        )?)?;
        if fresh != digest {
            return Err(invalid("physical admission fresh ACK differs"));
        }
        self.require_physical(cancel)?;
        source_boundary()?;
        *step = AdmissionStep::FreshVerified;
        Ok(digest)
    }

    fn require_physical(&self, cancel: &Cancellation) -> Result<()> {
        let held = &self.inspection;
        held.pins.require_data(&held.data, cancel)?;
        held.cas
            .require(&held.data.binding, &held.data.required, cancel)?;
        held.checkout
            .require(&held.data.expected, &held.data.journal, cancel)
    }

    fn require_family(&self, c: &Connection, cancel: &Cancellation) -> Result<()> {
        let held = &self.inspection;
        if crate::projection::shared::provider_access::require_provider_catalog_schema(
            c,
            Some(cancel),
        )? != crate::projection::shared::CatalogSchema::V2
        {
            return Err(invalid("physical admission requires exact family schema2"));
        }
        let marker: Vec<u8> = c.query_row(
            "SELECT marker FROM family WHERE singleton=1 AND length(marker)<=65536",
            [],
            |r| r.get(0),
        )?;
        if marker != held.pins.family.marker_bytes() {
            return Err(invalid("physical admission family marker changed"));
        }
        let marker: Vec<u8> = c.query_row(
            "SELECT marker FROM worktrees WHERE id=?1 AND length(marker)<=65536 AND state='ready'",
            [held.pins.marker.identity.worktree_id.as_str()],
            |r| r.get(0),
        )?;
        if marker != held.pins.marker.bytes()? {
            return Err(invalid("physical admission family member changed"));
        }
        let binding = held
            .pins
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("physical admission family binding absent"))?;
        if !c.query_row(
            "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND metadata_path=?2 AND database_identity=?3)",
            rusqlite::params![held.pins.marker.identity.worktree_id.as_str(), binding.metadata_path.to_string_lossy(), binding.database_identity],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(invalid("physical admission family member binding changed"));
        }
        let protection = ExactProtection::new(
            &held.data.binding.family_incarnation,
            &held.data.binding.owner,
            &held.data.binding.database_identity,
            &held.data.ready.transaction_id,
            &held.data.required,
            Some(cancel),
        )?;
        let current = validate_protected_installations(
            c,
            &held.data.binding,
            &held.data.required,
            &protection,
            Some(cancel),
        )?;
        if protection.inspect(c, Some(cancel))?
            != crate::projection::shared::exact_protection::ProtectionPresence::Present
        {
            return Err(invalid("physical admission protection root absent"));
        }
        if current != held.data.installation {
            return Err(invalid("physical admission family reservation changed"));
        }
        Ok(())
    }

    fn require_admitted(&self, c: &Connection, cancel: &Cancellation) -> Result<String> {
        let held = &self.inspection;
        let admitted = read_admitted_import(c, &held.pins.marker, Some(cancel))?;
        let protection = ExactProtection::new(
            &held.data.binding.family_incarnation,
            &held.data.binding.owner,
            &held.data.binding.database_identity,
            &held.data.ready.transaction_id,
            &held.data.required,
            Some(cancel),
        )?;
        if admitted.ready() != &held.data.ready
            || admitted.receipt() != &held.data.receipt
            || admitted.record().binding != held.data.binding
            || admitted.record().root_id != protection.id()
            || admitted.archive().rows() != held.data.journal.rows()
            || admitted.expected().header() != held.data.expected.header()
        {
            return Err(invalid("physical admission immutable import changed"));
        }
        admitted.record().digest()
    }
}
