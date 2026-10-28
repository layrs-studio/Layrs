//! Local immutable SnapshotSource and disconnected native session for an
//! authenticated Active/Ready owner. No network or acquisition is possible.
use super::*;
use layrs_core::scm::FileId;
use layrs_vfs::{Cancellation, Catalog, SnapshotSource};
use std::{
    error::Error,
    fmt, io,
    path::Path,
    sync::{Arc, OnceLock},
};

struct LocalSource {
    held: outcome::PublicationPins,
    root: layrs_vfs::ProviderRootGuard,
    record: ProviderActiveMountRecord,
    activation: ProviderActivationRecord,
    // This lease outlives Session::disconnect and every callback. Session's
    // exceptional failed-disconnect path leaks its source/context intentionally.
    lifecycle: OnceLock<crate::lock::StoreLock>,
}

impl LocalSource {
    fn new(
        held: outcome::PublicationPins,
        root: layrs_vfs::ProviderRootGuard,
        record: ProviderActiveMountRecord,
        activation: ProviderActivationRecord,
    ) -> Arc<Self> {
        Arc::new(Self {
            held,
            root,
            record,
            activation,
            lifecycle: OnceLock::new(),
        })
    }
    fn lease(&self) -> std::result::Result<(), ProviderActiveOpenFailure> {
        if self.lifecycle.get().is_none() {
            let lease = self.held.family.lifecycle_lease().map_err(|e| {
                ProviderActiveOpenFailure::from_failure(
                    "retain active callback lifecycle",
                    e.into(),
                )
            })?;
            let _ = self.lifecycle.set(lease);
        }
        Ok(())
    }
    fn validate(&self) -> std::result::Result<(), ProviderActiveOpenFailure> {
        if self.lifecycle.get().is_none() {
            return Err(ProviderActiveOpenFailure::from_failure(
                "validate active callback source",
                preparation_report::Failure::from(invalid(
                    "active callback lifecycle lease absent",
                )),
            ));
        }
        let _locks = self
            .held
            .family
            .locks(
                &self.held.artifacts.allocation.format.identity.worktree_id,
                false,
            )
            .map_err(|e| {
                ProviderActiveOpenFailure::from_failure("validate active callback source", e.into())
            })?;
        capture_state::require_exact_live(&self.held, &self.root, &self.record, &self.activation)
            .map_err(|e| {
            ProviderActiveOpenFailure::from_failure("validate active callback source", e)
        })?;
        Ok(())
    }
}

impl SnapshotSource for LocalSource {
    fn catalog(&self) -> &Catalog {
        self.root.catalog()
    }
    fn read_exact_at(
        &self,
        file_id: &FileId,
        offset: u64,
        output: &mut [u8],
        cancel: &Cancellation,
    ) -> layrs_vfs::Result<()> {
        if self.lifecycle.get().is_none() {
            return Err(layrs_vfs::Error::NeedsRepair(
                "active callback lifecycle lease absent",
            ));
        }
        ranges::read(
            &self.held,
            &self.root,
            &self.record,
            &self.activation,
            file_id,
            offset,
            output,
            cancel,
        )
        .map_err(map_range_failure)
    }
}

fn map_range_failure(error: ProviderActiveRangeFailure) -> layrs_vfs::Error {
    match error {
        ProviderActiveRangeFailure::NeedsHydration {
            file_id,
            offset,
            length,
        } => layrs_vfs::Error::MissingBytes {
            file_id,
            offset,
            length,
        },
        ProviderActiveRangeFailure::Cancelled => layrs_vfs::Error::Cancelled,
        ProviderActiveRangeFailure::InvalidRange => layrs_vfs::Error::InvalidRange,
        ProviderActiveRangeFailure::Unavailable(error) => {
            layrs_vfs::Error::Io(io::Error::other(error))
        }
    }
}

#[derive(Debug)]
pub enum ProviderActiveSessionCause {
    Store(ProviderActiveOpenFailure),
    Native(layrs_vfs::Error),
}
impl fmt::Display for ProviderActiveSessionCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => e.fmt(f),
            Self::Native(e) => e.fmt(f),
        }
    }
}
impl Error for ProviderActiveSessionCause {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            Self::Native(e) => Some(e),
        }
    }
}

/// Retains the original authenticated owner and native root proof after a
/// refused session construction. Retry revalidates both databases and READY.
pub struct ProviderActiveSessionWithheld {
    source: Arc<LocalSource>,
    cause: ProviderActiveSessionCause,
}
impl ProviderActiveSessionWithheld {
    pub fn root(&self) -> &Path {
        self.source.root.root()
    }
    pub fn binding(&self) -> &ProviderBootstrapBinding {
        &self.source.held.binding
    }
    pub fn cause(&self) -> &ProviderActiveSessionCause {
        &self.cause
    }
    pub fn retry(self) -> std::result::Result<ProviderActiveSession, Self> {
        build(self.source)
    }
}

/// A disconnected native Session whose source retains the Store lifecycle
/// gate. Explicit disconnect drains callbacks before normal source release.
pub struct ProviderActiveSession {
    session: layrs_vfs::Session,
    source: Arc<LocalSource>,
}
impl ProviderActiveSession {
    /// Internal durable handoff adapter. The ordinary opener is a separate
    /// operation after this consumes the destination's exclusive Ready pins.
    pub(in crate::projection::provider) fn admit_ready_destination(
        &self,
        destination: crate::projection::provider::managed_ready_reopen::ManagedReadyAdmission,
        close: &mut impl crate::projection::provider::database_io::WriterCloseObserver,
        cancel: &Cancellation,
    ) -> std::result::Result<
        String,
        crate::projection::provider::managed_ready_reopen::AdmissionFailure,
    > {
        use crate::projection::provider::{
            managed_ready_reopen::{AdmissionFailure, AdmissionStep},
            published_handoff::PublicationSource,
        };
        let held = &self.source.held;
        let prepare = (|| -> CauseResult<_> {
            cancel.check()?;
            let lifecycle = held.family.writer_lifecycle(false)?;
            let owner = crate::lock::StoreLock::acquire_existing(
                &held
                    .family
                    .member(&held.artifacts.allocation.format.identity.worktree_id)
                    .join("layrs.lock"),
                crate::projection::SessionMode::Shared,
            )?;
            let prepared = held
                .artifacts
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("admission source prepared receipt absent"))?;
            Ok((lifecycle, owner, prepared))
        })();
        let (_source_lifecycle, _source_owner, prepared) = match prepare {
            Ok(value) => value,
            Err(cause) => {
                return Err(AdmissionFailure {
                    step: AdmissionStep::Validating,
                    cause,
                    held: destination,
                })
            }
        };
        destination.commit(
            &held.artifacts.database,
            PublicationSource {
                allocation: &held.artifacts.allocation,
                prepared,
                binding: &held.binding,
                root_id: self.source.root.registration().root_id(),
            },
            || {
                cancel.check()?;
                held.artifacts.verify(&held.family)?;
                self.source.root.revalidate()?;
                Ok(())
            },
            close,
            cancel,
        )
    }

    pub fn root(&self) -> &Path {
        self.source.root.root()
    }
    pub fn is_connected(&self) -> bool {
        self.session.is_connected()
    }
    pub fn callback_failures(&self) -> Vec<layrs_vfs::CallbackFailure> {
        self.session.callback_failures()
    }
    pub fn hydration_requests(&self) -> Vec<layrs_vfs::HydrationRequest> {
        self.session.hydration_requests()
    }
    pub fn connect(&mut self) -> std::result::Result<(), ProviderActiveSessionCause> {
        self.source
            .validate()
            .map_err(ProviderActiveSessionCause::Store)?;
        self.session
            .connect()
            .map_err(ProviderActiveSessionCause::Native)
    }
    pub fn disconnect(&mut self) -> layrs_vfs::Result<()> {
        self.session.disconnect()
    }
    pub fn checkpoint(&self) -> layrs_vfs::Result<layrs_vfs::Checkpoint<'_>> {
        self.session.checkpoint()
    }
    /// Explicit portable recovery export. Callers keep every external editor,
    /// build tool and agent quiescent through the response. This does not admit
    /// the exported files into managed capture or change the native source.
    pub fn export_quiesced(
        &mut self,
        parent: &Path,
        cancel: &Cancellation,
    ) -> std::result::Result<ProviderRecoveryExportOutcome, ProviderRecoveryExportFailure> {
        recovery_export::run(
            &self.session,
            self.source.as_ref(),
            &self.source.held,
            &self.source.root,
            &self.source.record,
            &self.source.activation,
            parent,
            cancel,
        )
    }
    /// Capture a complete selected image. The caller keeps external namespace
    /// mutation quiescent. The same transaction ID resumes or returns its
    /// durable acknowledgement; a changed staged image is refused.
    pub fn capture(
        &mut self,
        transaction_id: &str,
        cancel: &Cancellation,
    ) -> std::result::Result<ProviderCaptureAcknowledgement, ProviderCaptureFailure> {
        let failure = |cause| ProviderCaptureFailure {
            transaction_id: transaction_id.into(),
            step: ProviderCaptureStep::Checkpoint,
            cause,
        };
        if !self.is_connected() {
            return Err(failure(invalid("capture requires a connected session")));
        }
        use crate::projection::provider::capture_lock::{
            CaptureAttemptLock, CaptureAttemptLockError,
        };
        let attempt =
            CaptureAttemptLock::acquire(&self.source.held.artifacts, &self.source.held.family)
                .map_err(|error| {
                    failure(match error {
                        CaptureAttemptLockError::Contended => Cause::Io(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "another provider capture holds the allocation",
                        )),
                        CaptureAttemptLockError::Cause(cause) => cause,
                    })
                })?;
        let mut checkpoint = self
            .session
            .checkpoint()
            .map_err(|error| failure(error.into()))?;
        capture::run(
            &self.source.held,
            &self.source.root,
            &self.source.record,
            &self.source.activation,
            &mut checkpoint,
            &attempt,
            transaction_id,
            cancel,
        )
    }

    /// Durably abandon an unfinished attempt while retaining its staged
    /// content and protection. Accepted patches cannot be cancelled.
    pub fn cancel_capture(
        &mut self,
        transaction_id: &str,
    ) -> std::result::Result<(), ProviderCaptureFailure> {
        use crate::projection::provider::capture_lock::{
            CaptureAttemptLock, CaptureAttemptLockError,
        };
        let failure = |cause| ProviderCaptureFailure {
            transaction_id: transaction_id.into(),
            step: ProviderCaptureStep::Cancellation,
            cause,
        };
        let attempt =
            CaptureAttemptLock::acquire(&self.source.held.artifacts, &self.source.held.family)
                .map_err(|error| {
                    failure(match error {
                        CaptureAttemptLockError::Contended => Cause::Io(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "another provider capture holds the allocation",
                        )),
                        CaptureAttemptLockError::Cause(cause) => cause,
                    })
                })?;
        let checkpoint = self
            .session
            .checkpoint()
            .map_err(|error| failure(error.into()))?;
        capture::cancel(
            &self.source.held,
            &self.source.root,
            &self.source.record,
            &self.source.activation,
            &checkpoint,
            &attempt,
            transaction_id,
        )
    }
}

impl ProviderActive {
    /// Consumes this Active holder into a disconnected local-only VFS session.
    pub fn into_disconnected_session(
        self,
    ) -> std::result::Result<ProviderActiveSession, ProviderActiveSessionWithheld> {
        build(LocalSource::new(
            self.held,
            self.root,
            self.record,
            self.activation,
        ))
    }
}
impl ProviderActiveReopened {
    /// Consumes this durably reopened Active holder into a disconnected session.
    pub fn into_disconnected_session(
        self,
    ) -> std::result::Result<ProviderActiveSession, ProviderActiveSessionWithheld> {
        build(LocalSource::new(
            self.held,
            self.root,
            self.record,
            self.activation,
        ))
    }
}

fn build(
    source: Arc<LocalSource>,
) -> std::result::Result<ProviderActiveSession, ProviderActiveSessionWithheld> {
    let constructed =
        (|| -> std::result::Result<layrs_vfs::Session, ProviderActiveSessionCause> {
            source.lease().map_err(ProviderActiveSessionCause::Store)?;
            source
                .validate()
                .map_err(ProviderActiveSessionCause::Store)?;
            let proof = source
                .root
                .try_clone()
                .map_err(ProviderActiveSessionCause::Native)?;
            let snapshot: Arc<dyn SnapshotSource> = source.clone();
            layrs_vfs::Session::reconnect_disconnected_with_control_directory(proof, snapshot)
                .map_err(ProviderActiveSessionCause::Native)
        })();
    match constructed {
        Ok(session) => Ok(ProviderActiveSession { session, source }),
        Err(cause) => Err(ProviderActiveSessionWithheld { source, cause }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cold_content_maps_to_exact_missing_bytes() {
        let id = FileId::from_bytes(b"missing");
        match map_range_failure(ProviderActiveRangeFailure::NeedsHydration {
            file_id: id.clone(),
            offset: 7,
            length: 9,
        }) {
            layrs_vfs::Error::MissingBytes {
                file_id,
                offset: 7,
                length: 9,
            } => assert_eq!(file_id, id),
            _ => panic!("cold range did not preserve missing-byte coordinates"),
        }
    }
}

#[cfg(test)]
#[path = "session/managed_image_tests.rs"]
mod managed_image_tests;

#[cfg(test)]
#[path = "session/managed_source_tests.rs"]
mod managed_source_tests;

#[cfg(test)]
#[path = "session/managed_chunk_tests.rs"]
mod managed_chunk_tests;

#[cfg(test)]
#[path = "session/managed_allocation_tests.rs"]
mod managed_allocation_tests;

#[cfg(test)]
#[path = "session/managed_destination_tests.rs"]
mod managed_destination_tests;

#[cfg(test)]
#[path = "session/managed_staging_tests.rs"]
mod managed_staging_tests;

#[cfg(test)]
mod capture_immutable_tests;
#[cfg(test)]
mod managed_admission_tests;
#[cfg(test)]
mod managed_admitted_export_tests;
#[cfg(test)]
mod managed_checkout_tests;
#[cfg(test)]
#[path = "session/managed_content_tests.rs"]
mod managed_content_tests;
#[cfg(test)]
mod managed_exact_tests;
#[cfg(test)]
mod managed_family_publication_tests;
#[cfg(test)]
mod managed_live_read_tests;
#[cfg(test)]
#[path = "session/managed_ordinary_admission_tests.rs"]
mod managed_ordinary_admission_tests;
#[cfg(all(test, windows))]
mod managed_ordinary_allocation_tests;
#[cfg(test)]
#[path = "session/managed_ordinary_autonomy_tests.rs"]
mod managed_ordinary_autonomy_tests;
#[cfg(test)]
#[path = "session/managed_ordinary_checkout_tests.rs"]
mod managed_ordinary_checkout_tests;
#[cfg(test)]
mod managed_ordinary_export_owner_tests;
#[cfg(test)]
#[path = "session/managed_ordinary_family_ready_tests.rs"]
mod managed_ordinary_family_ready_tests;
#[cfg(all(test, windows))]
#[path = "session/managed_ordinary_fresh_admission_tests.rs"]
mod managed_ordinary_fresh_admission_tests;
#[cfg(test)]
#[path = "session/managed_ordinary_installing_tests.rs"]
mod managed_ordinary_installing_tests;
#[cfg(test)]
mod managed_ordinary_metadata_tests;
#[cfg(test)]
#[path = "session/managed_ordinary_ready_tests.rs"]
mod managed_ordinary_ready_tests;
#[cfg(test)]
#[path = "session/managed_ordinary_reservation_tests.rs"]
mod managed_ordinary_reservation_tests;
#[cfg(test)]
mod managed_ordinary_source_intent_tests;
#[cfg(test)]
#[path = "session/managed_ordinary_source_publication_tests.rs"]
mod managed_ordinary_source_publication_tests;
#[cfg(test)]
mod managed_ordinary_staging_tests;
#[cfg(test)]
mod managed_post_publication_capture_tests;
#[cfg(test)]
mod managed_protection_tests;
#[cfg(test)]
mod managed_ready_reopen_tests;
#[cfg(test)]
mod managed_ready_tests;
#[cfg(test)]
mod managed_restore_cancellation_tests;
#[cfg(test)]
mod managed_restore_frontier_tests;
#[cfg(test)]
mod managed_restore_recovery_tests;
#[cfg(test)]
mod managed_source_publication_tests;

#[cfg(test)]
#[path = "session/managed_journal_tests.rs"]
mod managed_journal_tests;
