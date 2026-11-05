//! Fresh Ordinary C8 -> C10 admission from a real independently owned B.
//! No retained export driver, mutable ordinary opener, owner upgrade, implicit
//! retry or recovery is involved. Every failure retains the attempt's pins.
use super::*;
use crate::projection::{
    provider::{
        database_io::WriterCloseObserver,
        destination_ready::read_ordinary_managed_ready,
        managed_admission::{
            AdmissionWriteEffect, RequiredRepresentation, admit_ordinary_ready,
            read_admitted_import,
        },
        published_handoff::read_ordinary_published_for_ready,
        records::Record,
    },
    shared::{
        admitted_catalog::validate_terminal_ready_catalog, exact_protection::ExactProtection,
    },
};
use rusqlite::Connection;

pub(in crate::projection::provider) struct OrdinaryFreshAdmissionFailure<'b> {
    pub step: AdmissionStep,
    pub cause: Cause,
    pub held: OrdinaryFreshAdmission<'b>,
}

/// Construction borrows an already acquired B capability before acquiring a
/// new exclusive C owner. No Marker or detached DATA can construct this holder.
/// A failed COMMIT/close is never reported as rollback or acknowledged success.
pub(in crate::projection::provider) struct OrdinaryFreshAdmission<'b> {
    source: &'b OrdinaryAdmissionSource,
    inspection: OrdinaryReadyInspection,
    protection: ExactProtection,
    witness: Vec<RequiredRepresentation>,
    published: Option<ProviderTransitionIntentRecord>,
}

impl<'b> OrdinaryFreshAdmission<'b> {
    pub(in crate::projection::provider) fn open(
        source: &'b OrdinaryAdmissionSource,
        root: &Path,
        cancel: &Cancellation,
    ) -> Result<Self> {
        source.require_identity(cancel)?;
        let pins = ManagedReadyPins::open(root, cancel)?;
        // Reject B aliases BEFORE attempting any second owner acquisition.
        source.require_destination(&pins, cancel)?;
        let inspection = OrdinaryReadyInspection::open_exclusive(pins, cancel)?;
        let data = &inspection.data;
        source.require_ready_source(&data.ready)?;
        let protection = ExactProtection::new(
            &data.binding.family_incarnation,
            &data.binding.owner,
            &data.binding.database_identity,
            &data.ready.transaction_id,
            &data.required,
            Some(cancel),
        )?;
        let mut witness: Vec<_> = data
            .required
            .iter()
            .map(|m| RequiredRepresentation {
                file_id: m.file_id().clone(),
                manifest_id: m.id().clone(),
                bytes: m.byte_length(),
            })
            .collect();
        witness.sort_unstable();
        source.require_identity(cancel)?;
        Ok(Self {
            source,
            inspection,
            protection,
            witness,
            published: None,
        })
    }

    pub(in crate::projection::provider) fn commit(
        mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<String, OrdinaryFreshAdmissionFailure<'b>> {
        let mut step = AdmissionStep::Validating;
        let result = self.commit_inner(close, cancel, &mut step);
        result.map_err(|cause| OrdinaryFreshAdmissionFailure {
            step,
            cause,
            held: self,
        })
    }

    /// Explicit destination-only resolution after this attempt actually saw
    /// Published. In particular no source boundary or SQL B is consulted: B
    /// may be unavailable after an uncertain acknowledgement. C8 still refuses.
    pub(in crate::projection::provider) fn resolve(
        self,
        cancel: &Cancellation,
    ) -> std::result::Result<String, OrdinaryFreshAdmissionFailure<'b>> {
        let mut step = AdmissionStep::Validating;
        let result = (|| {
            if self.published.is_none() {
                return Err(invalid(
                    "fresh Ordinary resolution has not observed actual Published",
                ));
            }
            self.inspection.require_physical(cancel)?;
            let digest = self.read_admitted(cancel)?;
            step = AdmissionStep::ReadScopesClosed;
            self.inspection.require_physical(cancel)?;
            if self.read_admitted(cancel)? != digest {
                return Err(invalid("fresh Ordinary resolution digest changed"));
            }
            self.inspection.require_physical(cancel)?;
            step = AdmissionStep::FreshVerified;
            Ok(digest)
        })();
        result.map_err(|cause| OrdinaryFreshAdmissionFailure {
            step,
            cause,
            held: self,
        })
    }

    fn require_family(&self, c: &Connection, cancel: &Cancellation) -> Result<()> {
        let held = &self.inspection;
        let data = &held.data;
        let state = validate_terminal_ready_catalog(
            c,
            held.pins.family.marker_bytes(),
            &data.marker,
            &data.binding,
            &data.required,
            &self.protection,
            Some(cancel),
        )?;
        if state != data.installation {
            return Err(invalid(
                "fresh Ordinary admission complete family state changed",
            ));
        }
        Ok(())
    }

    fn require_admitted(&self, c: &Connection, cancel: &Cancellation) -> Result<String> {
        let data = &self.inspection.data;
        let published = self
            .published
            .as_ref()
            .ok_or_else(|| invalid("fresh Ordinary admission lacks observed actual Published"))?;
        let admitted = read_admitted_import(c, &data.marker, Some(cancel))?;
        if admitted.private_schema() != 10
            || admitted.ready() != &data.ready
            || admitted.receipt() != &data.receipt
            || admitted.source_published() != published
            || admitted.record().source_published_digest != published.digest()?
            || admitted.record().binding != data.binding
            || admitted.record().root_id != self.protection.id()
            || admitted.required() != self.witness
            || admitted.witness_column_bytes() != self.witness.len() as u64 * 150
            || admitted.archive().rows() != data.journal.rows()
            || admitted.expected().header() != data.expected.header()
        {
            return Err(invalid(
                "fresh Ordinary admission immutable C10 evidence differs",
            ));
        }
        let digest = admitted.record().digest()?;
        // Preserve the stricter terminal fence and fixed quota envelope, not
        // merely the generic admitted reader which permits ordinary tails.
        let (_, expected, journal) =
            admitted.into_ordinary_terminal(&data.required, data.installation, Some(cancel))?;
        if expected.header() != data.expected.header() || journal.rows() != data.journal.rows() {
            return Err(invalid("fresh Ordinary admission terminal archive changed"));
        }
        Ok(digest)
    }

    fn read_admitted(&self, cancel: &Cancellation) -> Result<String> {
        let held = &self.inspection;
        held.require_identity(cancel)?;
        let _catalog = StoreLock::acquire_existing(
            &held.pins.family.root().join("metadata/catalog.lock"),
            SessionMode::Shared,
        )?;
        valid(database_io::read_cancellable_strict(
            &held.pins.database,
            cancel,
            |c| {
                let digest = self.require_admitted(c, cancel)?;
                valid(database_io::read_cancellable_strict(
                    &held.pins.family.catalog,
                    cancel,
                    |family| {
                        self.require_family(family, cancel)?;
                        held.require_identity(cancel)?;
                        Ok(digest)
                    },
                )?)
            },
        )?)
    }

    fn commit_inner(
        &mut self,
        close: &mut impl WriterCloseObserver,
        cancel: &Cancellation,
        step: &mut AdmissionStep,
    ) -> Result<String> {
        self.inspection.require(cancel)?;
        self.source.require_identity(cancel)?;
        // This new bound-file reader has its own RO SQLite connection. The
        // closed B preflight and detached baseline never stand in for Published.
        let source = self.source;
        let source_binding = source
            .pins
            .marker
            .shared
            .as_ref()
            .ok_or_else(|| invalid("fresh Ordinary source shared binding absent"))?;
        let source_database = database_io::DatabaseFile::open(
            source.pins.database.path().to_owned(),
            &source_binding.database_identity,
        )?;
        let digest = {
            let _catalog = StoreLock::acquire_existing(
                &self
                    .inspection
                    .pins
                    .family
                    .root()
                    .join("metadata/catalog.lock"),
                SessionMode::Shared,
            )?;
            valid(database_io::read_cancellable_strict(
                &source_database,
                cancel,
                |source_c| {
                    source.require_snapshot(source_c, cancel)?;
                    let published = read_ordinary_published_for_ready(
                        source_c,
                        &source.pins.marker,
                        &self.inspection.data.ready,
                        &self.inspection.data.receipt,
                        Some(cancel),
                    )?;
                    let actual =
                        ProviderTransitionIntentRecord::decode(&published.published().encode()?)?;
                    if self
                        .published
                        .as_ref()
                        .is_some_and(|prior| prior != &actual)
                    {
                        return Err(invalid("fresh Ordinary observed Published changed"));
                    }
                    self.published = Some(actual);
                    valid(database_io::read_cancellable_strict(
                        &self.inspection.pins.family.catalog,
                        cancel,
                        |family| {
                            self.require_family(family, cancel)?;
                            self.inspection.require_identity(cancel)?;
                            source.require_identity(cancel)?;
                            let admission = database_io::clean_admission(
                                &self.inspection.pins.database,
                                false,
                                |c| {
                                    let fresh = read_ordinary_managed_ready(
                                        c,
                                        &self.inspection.data.marker,
                                        family,
                                        Some(cancel),
                                    )?
                                    .into_snapshot()?;
                                    self.inspection.data.require_same(&fresh)?;
                                    source.require_snapshot(source_c, cancel)?;
                                    published.require(source_c, Some(cancel))?;
                                    self.inspection.require_identity(cancel)?;
                                    source.require_identity(cancel)
                                },
                            )?;
                            self.inspection.require_identity(cancel)?;
                            source.require_identity(cancel)?;
                            *step = AdmissionStep::WriterOpenAttempted;
                            let bare = admission.open(&self.inspection.pins.database, close)?;
                            let result = (|| {
                                admission.require_geometry(&bare)?;
                                let c = bare.connection();
                                c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                                let ready = read_ordinary_managed_ready(
                                    c,
                                    &self.inspection.data.marker,
                                    family,
                                    Some(cancel),
                                )?;
                                let fresh = read_ordinary_managed_ready(
                                    c,
                                    &self.inspection.data.marker,
                                    family,
                                    Some(cancel),
                                )?
                                .into_snapshot()?;
                                self.inspection.data.require_same(&fresh)?;
                                source.require_snapshot(source_c, cancel)?;
                                *step = AdmissionStep::MutationAttempted;
                                if admit_ordinary_ready(
                                    c,
                                    &self.inspection.data.marker,
                                    family,
                                    source_c,
                                    &ready,
                                    &published,
                                    Some(cancel),
                                )? != AdmissionWriteEffect::Admitted
                                {
                                    return Err(invalid(
                                        "fresh Ordinary admission did not migrate exact C8",
                                    ));
                                }
                                let digest = self.require_admitted(c, cancel)?;
                                self.require_family(family, cancel)?;
                                source.require_snapshot(source_c, cancel)?;
                                published.require(source_c, Some(cancel))?;
                                self.inspection.require_identity(cancel)?;
                                source.require_identity(cancel)?;
                                cancel.check()?;
                                *step = AdmissionStep::CommitAttempted;
                                c.execute_batch("COMMIT")?;
                                *step = AdmissionStep::CommitReturned;
                                // Identity only after COMMIT, until the actual writer
                                // close is acknowledged. No old C8 SQL proof is reused.
                                self.inspection.require_identity(cancel)?;
                                source.require_identity(cancel)?;
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
        // C writer, family RO and B RO have ALL closed positively. No failed
        // or unknown close reaches the full namespace checks or fresh SQL.
        self.inspection.require_physical(cancel)?;
        source.require_identity(cancel)?;
        if self.read_admitted(cancel)? != digest {
            return Err(invalid("fresh Ordinary admission readback digest differs"));
        }
        self.inspection.require_physical(cancel)?;
        source.require_identity(cancel)?;
        *step = AdmissionStep::FreshVerified;
        Ok(digest)
    }
}
