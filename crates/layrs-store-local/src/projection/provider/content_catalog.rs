//! Catalog scopes for the local provider's shared content engine. The content
//! writer keeps its algorithms in `shared`; only this module opens provider SQL.
//! A hot journal is refused before any writable handle is opened.
use super::{
    artifacts::Artifacts,
    database_io::{self, BareConnection, ReadState, WriterCloseObserver},
    records::{
        capture::{ProviderCaptureStage, ProviderCaptureState},
        AllocationRecord, Record,
    },
    recovery::error::{invalid, ProviderBootstrapCause as Cause, Result as CauseResult},
    validation,
    validation::{PrivateCaptureMutationValidation, PrivateCaptureProof},
};
use crate::{
    lock::StoreLock,
    projection::{
        capture::CapturedFile,
        format,
        shared::provider_access::{
            provider_capture::operation, ActivationRegistration, FamilyAccess,
        },
        ProjectionStoreError, Result, SessionMode,
    },
};
use layrs_core::scm::{FileManifest, WorktreeId};
use rusqlite::{ffi, Connection};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::projection) struct ProviderContext {
    family: FamilyAccess,
    allocation: AllocationRecord,
    proof: ContextProof,
    artifacts: Artifacts,
    _owner: StoreLock,
}

enum ContextProof {
    Strict(PrivateCaptureProof),
    Publication(PrivateCaptureMutationValidation),
}
impl ContextProof {
    fn capture(&self) -> &PrivateCaptureProof {
        match self {
            Self::Strict(proof) => proof,
            Self::Publication(proof) => proof.capture(),
        }
    }
}
enum ContextAdmission {
    Strict,
    Publication(PrivateCaptureMutationValidation),
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

fn store(error: Cause) -> ProjectionStoreError {
    ProjectionStoreError::ProviderContent(Box::new(error))
}

fn cause(error: ProjectionStoreError) -> Cause {
    match error {
        ProjectionStoreError::Io(error) => Cause::Io(error),
        ProjectionStoreError::Database(error) => Cause::Sqlite(error),
        ProjectionStoreError::ProviderContent(error) => *error,
        other => Cause::Validation(Box::new(other)),
    }
}

impl ProviderContext {
    fn staged(
        &self,
        reservation: &str,
        expected_operation: Option<&str>,
    ) -> Result<&ProviderCaptureStage> {
        let mut found = None;
        for transaction in &self.proof.capture().transactions {
            let ProviderCaptureState::Staging { stage } = &transaction.state else {
                continue;
            };
            if stage.reservation != reservation {
                continue;
            }
            if expected_operation
                .is_some_and(|expected| operation(&transaction.transaction_id) != expected)
                || found.replace(stage).is_some()
            {
                return Err(store(invalid("provider content attempt identity differs")));
            }
        }
        found.ok_or_else(|| store(invalid("provider content has no matching staging intent")))
    }

    pub(in crate::projection) fn authorize_marker(&self, marker: &format::Marker) -> Result<()> {
        if marker.bytes()? != self.allocation.format.bytes()? {
            return Err(store(invalid("provider content owner marker differs")));
        }
        Ok(())
    }

    pub(in crate::projection) fn authorize_reservation(
        &self,
        id: &str,
        owner: &WorktreeId,
        operation: &str,
        bytes: u64,
    ) -> Result<()> {
        if owner != &self.allocation.format.identity.worktree_id {
            return Err(store(invalid("provider content reservation owner differs")));
        }
        let stage = self.staged(id, Some(operation))?;
        let files = stage
            .changes
            .iter()
            .filter_map(|change| change.replacement.as_ref())
            .cloned()
            .collect::<Vec<_>>();
        let budget = crate::projection::retention::budget_for(&files, 0)?;
        if bytes != budget {
            return Err(store(invalid(
                "provider content reservation charge differs from stage",
            )));
        }
        Ok(())
    }

    pub(in crate::projection) fn authorize_install(
        &self,
        expected: &CapturedFile,
        reservation: &str,
    ) -> Result<()> {
        let stage = self.staged(reservation, None)?;
        let matches = stage
            .changes
            .iter()
            .filter_map(|change| change.replacement.as_ref())
            .filter(|file| file.file_id == expected.file_id)
            .collect::<Vec<_>>();
        if matches.is_empty()
            || matches.iter().any(|file| file.bytes != expected.bytes)
            || !matches.iter().any(|file| *file == expected)
        {
            return Err(store(invalid(
                "provider content file is not exact staged replacement",
            )));
        }
        Ok(())
    }

    pub(in crate::projection) fn authorize_accepted_release(
        &self,
        reservation: &str,
        owner: &WorktreeId,
        transaction_id: &str,
    ) -> Result<()> {
        if owner != &self.allocation.format.identity.worktree_id
            || !self.proof.capture().transactions.iter().any(|record| {
                record.transaction_id == transaction_id
                    && matches!(&record.state, ProviderCaptureState::Accepted { stage, .. } if stage.reservation == reservation)
            })
        {
            return Err(store(invalid("provider reservation release requires exact accepted capture")));
        }
        Ok(())
    }

    pub(in crate::projection) fn authorize_protection(
        &self,
        marker: &format::Marker,
        expected_operation: &str,
        manifests: &[FileManifest],
    ) -> Result<()> {
        self.authorize_marker(marker)?;
        let matches = self
            .proof
            .capture()
            .transactions
            .iter()
            .filter_map(|transaction| {
                let ProviderCaptureState::Staging { stage } = &transaction.state else {
                    return None;
                };
                (operation(&transaction.transaction_id) == expected_operation).then_some(stage)
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(store(invalid(
                "provider content protection operation differs",
            )));
        }
        let stage = matches[0];
        let mut expected = BTreeMap::new();
        for file in stage
            .changes
            .iter()
            .filter_map(|change| change.replacement.as_ref())
        {
            if expected
                .insert(file.file_id.as_str(), file.bytes)
                .is_some_and(|bytes| bytes != file.bytes)
            {
                return Err(store(invalid(
                    "provider content staged file length differs",
                )));
            }
        }
        let content = self
            .proof
            .capture()
            .content
            .iter()
            .map(|row| (row.file_id.as_str(), row))
            .collect::<BTreeMap<_, _>>();
        if content.len() != self.proof.capture().content.len() || manifests.len() > expected.len() {
            return Err(store(invalid(
                "provider content protection set exceeds stage",
            )));
        }
        let mut required = BTreeSet::new();
        for (file_id, bytes) in &expected {
            let row = content.get(file_id).ok_or_else(|| {
                store(invalid("provider content protection lacks private content"))
            })?;
            if row.bytes != *bytes {
                return Err(store(invalid("provider content protection length differs")));
            }
            required.insert(row.manifest_id.as_str());
        }
        let mut actual = BTreeSet::new();
        for manifest in manifests {
            let file_id = manifest.file_id().as_str();
            let row = content.get(file_id).ok_or_else(|| {
                store(invalid("provider content protection includes foreign file"))
            })?;
            if expected.get(file_id) != Some(&manifest.byte_length())
                || row.bytes != manifest.byte_length()
                || row.manifest_id != manifest.id().as_str()
                || !actual.insert(manifest.id().as_str())
            {
                return Err(store(invalid(
                    "provider content protection manifest differs from private stage",
                )));
            }
        }
        if actual != required {
            return Err(store(invalid(
                "provider content protection manifest set is incomplete",
            )));
        }
        Ok(())
    }

    /// Caller already holds the family's shared lifecycle lock. Keep the
    /// target owner shared for every content step and native checkpoint gap.
    pub(in crate::projection) fn new(
        family: FamilyAccess,
        allocation: AllocationRecord,
        _proof: PrivateCaptureProof,
    ) -> Result<Self> {
        Self::new_mode(family, allocation, ContextAdmission::Strict)
    }

    /// Explicit capture context retaining the COMPLETE publication-aware gate.
    /// Re-read after acquiring ownerShared, and reject stale/substituted state.
    pub(in crate::projection) fn new_publication(
        family: FamilyAccess,
        allocation: AllocationRecord,
        proof: PrivateCaptureMutationValidation,
    ) -> Result<Self> {
        Self::new_mode(family, allocation, ContextAdmission::Publication(proof))
    }

    fn new_mode(
        family: FamilyAccess,
        allocation: AllocationRecord,
        admission: ContextAdmission,
    ) -> Result<Self> {
        family.require_allocation(&allocation).map_err(store)?;
        let owner = StoreLock::acquire_existing(
            &family
                .member(&allocation.format.identity.worktree_id)
                .join("layrs.lock"),
            SessionMode::Shared,
        )
        .map_err(|error| store(Cause::Store(error)))?;
        family.verify().map_err(store)?;
        let artifacts = Artifacts::open(
            &family,
            &allocation.format.identity.worktree_id,
            &allocation.transaction_id,
        )
        .map_err(store)?;
        if artifacts.allocation.encode()? != allocation.encode()? {
            return Err(store(invalid(
                "provider content allocation receipt differs",
            )));
        }
        let prepared = artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| store(invalid("provider content prepared receipt absent")))?;
        let binding = artifacts.binding().map_err(store)?;
        let proof = match database_io::read(&artifacts.database, |c| match &admission {
            ContextAdmission::Strict => Ok(ContextProof::Strict(
                validation::private_active_readable_with_source(
                    c,
                    &artifacts.allocation,
                    prepared,
                    &binding,
                    None,
                    None,
                )?
                .require_no_active_transition()?
                .capture,
            )),
            ContextAdmission::Publication(expected) => {
                let current = validation::private_active_capture_mutation_with_source(
                    c,
                    &artifacts.allocation,
                    prepared,
                    &binding,
                    None,
                    None,
                )?;
                current.require_same_state(expected)?;
                Ok(ContextProof::Publication(current))
            }
        })
        .map_err(store)?
        {
            ReadState::Valid(proof) => proof,
            ReadState::Rejected { cause, .. }
            | ReadState::ReadButUnavailable(cause)
            | ReadState::Unavailable(cause) => return Err(store(cause)),
            ReadState::Hot(hot) => return Err(store(Cause::ReadonlyRollback(hot.trigger))),
            ReadState::UnboundRollback { trigger, cause } => {
                return Err(store(Cause::Cleanup {
                    primary: Box::new(Cause::ReadonlyRollback(trigger)),
                    close: Box::new(cause),
                }));
            }
            ReadState::Uninitialized => {
                return Err(store(invalid(
                    "provider content private database is unavailable",
                )));
            }
        };
        artifacts.verify(&family).map_err(store)?;
        Ok(Self {
            family,
            allocation,
            proof,
            artifacts,
            _owner: owner,
        })
    }

    fn validate(&self, c: &Connection) -> CauseResult<()> {
        self.family.verify()?;
        self.artifacts.verify(&self.family)?;
        if self
            .family
            .validate_capture_catalog(c, &self.allocation, self.proof.capture(), None)?
            != ActivationRegistration::Ready
        {
            return Err(invalid(
                "provider content requires ready family capture state",
            ));
        }
        self.artifacts.verify(&self.family)?;
        self.family.verify()
    }

    /// One catalog gate and one observable SQLite close per call. The proof is
    /// fixed for this lease; a new private transition requires a new context.
    pub(in crate::projection) fn with_catalog<T>(
        &self,
        write: bool,
        operation: impl FnOnce(&mut Connection) -> Result<T>,
    ) -> Result<T> {
        let gate = StoreLock::acquire_existing(
            &self.family.root().join("metadata/catalog.lock"),
            if write {
                SessionMode::Exclusive
            } else {
                SessionMode::Shared
            },
        )
        .map_err(|error| store(Cause::Store(error)))?;
        self.family.verify().map_err(store)?;
        let admitted =
            database_io::clean_admission(&self.family.catalog, false, |c| self.validate(c))
                .map_err(store)?;
        let result = if write {
            let mut observer = ObserveClose;
            let mut bare = admitted
                .open(&self.family.catalog, &mut observer)
                .map_err(store)?;
            let result = (|| -> CauseResult<T> {
                admitted.require_geometry(&bare)?;
                bare.connection().execute_batch("BEGIN DEFERRED")?;
                self.validate(bare.connection())?;
                bare.connection().execute_batch("COMMIT")?;
                let value = operation(bare.connection_mut()).map_err(cause)?;
                self.validate(bare.connection())?;
                Ok(value)
            })();
            bare.finish_writer(result, &mut observer)
        } else {
            let mut bare =
                BareConnection::open(self.family.catalog.path(), false).map_err(store)?;
            let result = (|| -> CauseResult<T> {
                admitted.require_geometry(&bare)?;
                bare.connection().execute_batch("BEGIN DEFERRED")?;
                self.validate(bare.connection())?;
                let value = operation(bare.connection_mut()).map_err(cause)?;
                self.validate(bare.connection())?;
                Ok(value)
            })();
            bare.finish(result)
        }
        .map_err(store)?;
        drop(gate);
        self.family.verify().map_err(store)?;
        Ok(result)
    }
}
