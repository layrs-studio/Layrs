//! Retained ordinary C8 -> C10 handoff. Independent B/family read snapshots
//! span C COMMIT and writer close. Failures retain C ownership and byte pins;
//! no failure path reopens SQL or acknowledges the original B owner's close.
use super::{
    allocation::ManagedDestinationAllocation, checkout::OrdinaryCheckoutProof,
    protection::OrdinaryImportProtection, reservation, *,
};
use crate::{
    lock::StoreLock,
    projection::{
        provider::{
            database_io::{self, DatabaseFile, WriterCloseObserver},
            destination_ready::{read_ordinary_managed_ready, OwnedReadySnapshot},
            managed_admission::{
                admit_ordinary_ready, AdmissionWriteEffect, RequiredRepresentation,
            },
            published_handoff::read_ordinary_published_for_ready,
            records::Record,
            recovery::error::{
                invalid as cause_invalid, ProviderBootstrapCause as Cause, Result as CauseResult,
            },
        },
        shared::admitted_catalog::validate_terminal_ready_catalog,
    },
};

pub(in crate::projection::provider) use crate::projection::provider::managed_ready_reopen::AdmissionStep;

/// Fresh C10 and complete family DATA were read after positive writer, family
/// and independent B closes. The original B owner still needs its own close.
pub(in crate::projection::provider) struct OrdinaryAdmissionProgress {
    admission_digest: String,
}
impl OrdinaryAdmissionProgress {
    pub(in crate::projection::provider) fn admission_digest(&self) -> &str {
        &self.admission_digest
    }
    pub(in crate::projection::provider) fn step(&self) -> AdmissionStep {
        AdmissionStep::FreshVerified
    }
}

pub(in crate::projection::provider) struct OrdinaryAdmissionFailure<'a> {
    pub step: AdmissionStep,
    pub cause: Cause,
    pub held: OrdinaryReadyAdmission<'a>,
}

/// Borrows the already exclusive C allocation and immutable checkout/CAS
/// proofs. No owner lock is reacquired, and DATA cannot construct this holder.
pub(in crate::projection::provider) struct OrdinaryReadyAdmission<'a> {
    destination: &'a ManagedDestinationAllocation,
    protection: &'a OrdinaryImportProtection,
    checkout: &'a OrdinaryCheckoutProof,
    data: OwnedReadySnapshot,
    witness: Vec<RequiredRepresentation>,
    published: Option<ProviderTransitionIntentRecord>,
}

impl<'a> OrdinaryReadyAdmission<'a> {
    pub(in crate::projection::provider) fn from_retained(
        destination: &'a ManagedDestinationAllocation,
        protection: &'a OrdinaryImportProtection,
        checkout: &'a OrdinaryCheckoutProof,
        cancel: &Cancellation,
    ) -> CauseResult<Self> {
        cancel.check()?;
        destination.destination_database().verify()?;
        destination.family().verify()?;
        let data = {
            let family = destination.family();
            let _gate = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"),
                SessionMode::Shared,
            )?;
            reservation::valid(database_io::read_cancellable(
                destination.destination_database(),
                cancel,
                |c| {
                    reservation::valid(database_io::read_cancellable(
                        &family.catalog,
                        cancel,
                        |family_c| {
                            let data = read_ordinary_managed_ready(
                                c,
                                &destination.proof().format,
                                family_c,
                                Some(cancel),
                            )?
                            .into_snapshot()?;
                            if data.ready.destination.as_ref() != Some(destination.proof()) {
                                return Err(cause_invalid(
                                    "ordinary admission allocation differs from actual Ready",
                                ));
                            }
                            destination.require_published(&data.ready.plan)?;
                            let state = validate_terminal_ready_catalog(
                                family_c,
                                family.marker_bytes(),
                                &data.marker,
                                &data.binding,
                                &data.required,
                                protection.root(),
                                Some(cancel),
                            )?;
                            if state != data.installation {
                                return Err(cause_invalid(
                                    "ordinary admission retained family state differs",
                                ));
                            }
                            Ok(data)
                        },
                    )?)
                },
            )?)?
        };
        let mut witness: Vec<_> = data
            .required
            .iter()
            .map(|manifest| RequiredRepresentation {
                file_id: manifest.file_id().clone(),
                manifest_id: manifest.id().clone(),
                bytes: manifest.byte_length(),
            })
            .collect();
        witness.sort_unstable();
        let held = Self {
            destination,
            protection,
            checkout,
            data,
            witness,
            published: None,
        };
        // Namespace enumeration and complete physical-proof binding happen
        // only after both independent RO scopes have positively closed.
        held.require_full(cancel)?;
        Ok(held)
    }

    /// Resolve only this retained attempt, without B or a writer. Failure
    /// before observing actual B Published cannot have entered the C writer.
    pub(in crate::projection::provider) fn resolve(
        self,
        cancel: &Cancellation,
    ) -> std::result::Result<OrdinaryAdmissionProgress, OrdinaryAdmissionFailure<'a>> {
        let mut step = AdmissionStep::Validating;
        let result = (|| {
            self.require_full(cancel)?;
            let digest = self.read_admitted(cancel)?;
            step = AdmissionStep::ReadScopesClosed;
            self.require_full(cancel)?;
            if self.read_admitted(cancel)? != digest {
                return Err(cause_invalid(
                    "ordinary admission resolution digest changed",
                ));
            }
            self.require_full(cancel)?;
            step = AdmissionStep::FreshVerified;
            Ok(OrdinaryAdmissionProgress {
                admission_digest: digest,
            })
        })();
        result.map_err(|cause| OrdinaryAdmissionFailure {
            step,
            cause,
            held: self,
        })
    }

    fn require_identity(&self, cancel: &Cancellation) -> CauseResult<()> {
        cancel.check()?;
        self.destination.require_published(&self.data.ready.plan)?;
        self.protection.verify_identity(cancel)?;
        self.checkout
            .require_identity(&self.data.expected, &self.data.journal, cancel)
    }

    fn require_full(&self, cancel: &Cancellation) -> CauseResult<()> {
        self.require_identity(cancel)?;
        self.protection
            .require_parts(&self.data.binding, &self.data.required, cancel)?;
        self.checkout
            .require_parts(&self.data.expected, &self.data.journal, cancel)
    }

    fn require_family(&self, family_c: &Connection, cancel: &Cancellation) -> CauseResult<()> {
        let state = validate_terminal_ready_catalog(
            family_c,
            self.destination.family().marker_bytes(),
            &self.data.marker,
            &self.data.binding,
            &self.data.required,
            self.protection.root(),
            Some(cancel),
        )?;
        if state != self.data.installation {
            return Err(cause_invalid(
                "ordinary admission complete family state changed",
            ));
        }
        Ok(())
    }

    fn require_admitted(&self, c: &Connection, cancel: &Cancellation) -> CauseResult<String> {
        let published = self.published.as_ref().ok_or_else(|| {
            cause_invalid("ordinary admission has no retained actual SourcePublished")
        })?;
        let admitted = read_admitted_import(c, &self.data.marker, Some(cancel))?;
        if admitted.private_schema() != 10
            || admitted.ready() != &self.data.ready
            || admitted.receipt() != &self.data.receipt
            || admitted.source_published() != published
            || admitted.record().source_published_digest != published.digest()?
            || admitted.record().binding != self.data.binding
            || admitted.record().root_id != self.protection.root().id()
            || admitted.required() != self.witness
            || admitted.witness_column_bytes() != self.witness.len() as u64 * 150
            || admitted.archive().rows() != self.data.journal.rows()
            || admitted.expected().header() != self.data.expected.header()
        {
            return Err(cause_invalid(
                "ordinary admission immutable C10 evidence differs",
            ));
        }
        admitted.record().digest()
    }

    /// C10-only readback. Both RO connections close before the gate drops and
    /// before any full namespace check; a close error skips all later reads.
    fn read_admitted(&self, cancel: &Cancellation) -> CauseResult<String> {
        self.require_identity(cancel)?;
        let family = self.destination.family();
        let _gate = StoreLock::acquire_existing(
            &family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        reservation::valid(database_io::read_cancellable(
            self.destination.destination_database(),
            cancel,
            |c| {
                let digest = self.require_admitted(c, cancel)?;
                reservation::valid(database_io::read_cancellable(
                    &family.catalog,
                    cancel,
                    |family_c| {
                        self.require_family(family_c, cancel)?;
                        self.require_identity(cancel)?;
                        Ok(digest)
                    },
                )?)
            },
        )?)
    }

    fn commit_inner(
        &mut self,
        source_marker: &format::Marker,
        source_boundary: impl Fn() -> CauseResult<()>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> CauseResult<OrdinaryAdmissionProgress> {
        self.require_full(cancel)?;
        source_boundary()?;
        let source_binding = source_marker
            .shared
            .as_ref()
            .ok_or_else(|| cause_invalid("ordinary admission source lacks shared FORMAT"))?;
        // A new bound-file handle and independent RO SQLite connection: the
        // original B owner connection is never publication evidence here.
        let source_database = DatabaseFile::open(
            source_binding.metadata_path.join("projection.sqlite3"),
            &source_binding.database_identity,
        )?;
        let digest = {
            let family = self.destination.family();
            let _gate = StoreLock::acquire_existing(
                &family.root().join("metadata/catalog.lock"),
                SessionMode::Shared,
            )?;
            reservation::valid(database_io::read_cancellable(
                &source_database,
                cancel,
                |source_c| {
                    let published = read_ordinary_published_for_ready(
                        source_c,
                        source_marker,
                        &self.data.ready,
                        &self.data.receipt,
                        Some(cancel),
                    )?;
                    let actual =
                        ProviderTransitionIntentRecord::decode(&published.published().encode()?)?;
                    if self
                        .published
                        .as_ref()
                        .is_some_and(|prior| prior != &actual)
                    {
                        return Err(cause_invalid(
                            "ordinary admission actual SourcePublished changed",
                        ));
                    }
                    self.published = Some(actual);
                    reservation::valid(database_io::read_cancellable(
                        &family.catalog,
                        cancel,
                        |family_c| {
                            self.require_family(family_c, cancel)?;
                            self.require_identity(cancel)?;
                            source_boundary()?;
                            let admission = database_io::clean_admission(
                                self.destination.destination_database(),
                                false,
                                |c| {
                                    let fresh = read_ordinary_managed_ready(
                                        c,
                                        &self.data.marker,
                                        family_c,
                                        Some(cancel),
                                    )?
                                    .into_snapshot()?;
                                    self.data.require_same(&fresh)?;
                                    published.require(source_c, Some(cancel))?;
                                    self.require_identity(cancel)?;
                                    source_boundary()
                                },
                            )?;
                            self.require_identity(cancel)?;
                            source_boundary()?;
                            *step = AdmissionStep::WriterOpenAttempted;
                            let bare =
                                admission.open(self.destination.destination_database(), close)?;
                            let result = (|| {
                                admission.require_geometry(&bare)?;
                                let c = bare.connection();
                                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                                let ready = read_ordinary_managed_ready(
                                    c,
                                    &self.data.marker,
                                    family_c,
                                    Some(cancel),
                                )?;
                                let snapshot = read_ordinary_managed_ready(
                                    c,
                                    &self.data.marker,
                                    family_c,
                                    Some(cancel),
                                )?
                                .into_snapshot()?;
                                self.data.require_same(&snapshot)?;
                                *step = AdmissionStep::MutationAttempted;
                                if admit_ordinary_ready(
                                    c,
                                    &self.data.marker,
                                    family_c,
                                    source_c,
                                    &ready,
                                    &published,
                                    Some(cancel),
                                )? != AdmissionWriteEffect::Admitted
                                {
                                    return Err(cause_invalid(
                                        "ordinary physical admission did not migrate exact C8",
                                    ));
                                }
                                let digest = self.require_admitted(c, cancel)?;
                                self.require_family(family_c, cancel)?;
                                published.require(source_c, Some(cancel))?;
                                self.require_identity(cancel)?;
                                source_boundary()?;
                                cancel.check()?;
                                *step = AdmissionStep::CommitAttempted;
                                c.execute_batch("COMMIT")?;
                                *step = AdmissionStep::CommitReturned;
                                // No C8 proof, SQL validation or namespace scan follows
                                // COMMIT before the real C writer close is acknowledged.
                                self.require_identity(cancel)?;
                                source_boundary()?;
                                Ok(digest)
                            })();
                            let digest = bare.finish_writer(result, close)?;
                            *step = AdmissionStep::WriterClosed;
                            Ok(digest)
                        },
                    )?)
                },
            )?)?
        };
        *step = AdmissionStep::ReadScopesClosed;
        // C writer -> family RO -> independent B RO all closed positively.
        // Full physical checks and fresh SQL are success-only continuations.
        self.require_full(cancel)?;
        source_boundary()?;
        if self.read_admitted(cancel)? != digest {
            return Err(cause_invalid("ordinary admission fresh C10 digest differs"));
        }
        self.require_full(cancel)?;
        source_boundary()?;
        *step = AdmissionStep::FreshVerified;
        Ok(OrdinaryAdmissionProgress {
            admission_digest: digest,
        })
    }
}

impl OrdinaryManagedExport<'_> {
    /// The retained B owner stays open and autocommit. Only the independently
    /// opened read-only B snapshot supplies actual historical Published DATA.
    pub(in crate::projection::provider) fn admit_ready_destination<'a>(
        &mut self,
        mut held: OrdinaryReadyAdmission<'a>,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<OrdinaryAdmissionProgress, OrdinaryAdmissionFailure<'a>> {
        let mut step = AdmissionStep::Validating;
        let result = (|| -> CauseResult<OrdinaryAdmissionProgress> {
            cancel.check()?;
            let store = self.store;
            let image = &self.image;
            if held.data.ready.plan.source_prefix != self.prefix {
                return Err(cause_invalid(
                    "ordinary admission source differs from retained export",
                ));
            }
            let access = self.access.as_mut().ok_or_else(|| {
                cause_invalid("ordinary admission source owner already finalized")
            })?;
            if !access.connection.is_autocommit() {
                return Err(cause_invalid(
                    "ordinary admission original B owner has a transaction",
                ));
            }
            // Access's entry validation happens before either the family gate
            // or the independent B RO scope. There is no post-action SQL.
            access
                .with_content(|source, scope| {
                    let boundary = || -> CauseResult<()> {
                        cancel.check()?;
                        if !source.is_autocommit() {
                            return Err(cause_invalid(
                                "ordinary admission original B owner has a transaction",
                            ));
                        }
                        scope.verify_identity(store)?;
                        image.verify_identity(cancel)?;
                        Ok(())
                    };
                    boundary()?;
                    held.commit_inner(&store.marker, boundary, close, cancel, &mut step)
                        .map_err(Into::into)
                })
                .map_err(Cause::from)
        })();
        result.map_err(|cause| OrdinaryAdmissionFailure { step, cause, held })
    }
}
