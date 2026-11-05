//! One retained checkpoint through durable provider capture and retry.
use super::*;
use crate::projection::provider::{
    capture_lock::CaptureAttemptLock,
    database_io::WriterCloseObserver,
    records::capture::{
        ProviderCaptureReceiptRecord, ProviderCaptureStage, ProviderCaptureState,
        ProviderCaptureTransactionRecord,
    },
    validation::CaptureContentProof,
};
use crate::projection::{capture::CapturedPatch, retention, shared};
use layrs_vfs::{Cancellation, Checkpoint};
use std::{
    collections::BTreeSet,
    error::Error,
    fmt,
    io::{self, Read},
};

#[cfg(test)]
#[path = "capture/tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderCaptureStep {
    Checkpoint,
    Hashing,
    Staging,
    Content,
    Protection,
    Acceptance,
    Cancellation,
    ReservationRelease,
}

/// Failure does not imply rollback. Retry the same ID to reobserve durable
/// state; uncertain family effects and their reservation remain retained.
#[derive(Debug)]
pub struct ProviderCaptureFailure {
    pub transaction_id: String,
    pub step: ProviderCaptureStep,
    pub cause: Cause,
}
impl fmt::Display for ProviderCaptureFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "provider capture {} at {:?}: {}",
            self.transaction_id, self.step, self.cause
        )
    }
}
impl Error for ProviderCaptureFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}
#[derive(Debug, Clone)]
pub struct ProviderCaptureAcknowledgement {
    pub transaction_id: String,
    pub receipt_digest: String,
    pub patch: CapturedPatch,
}
struct Close;
impl WriterCloseObserver for Close {
    fn before_close(&mut self, _: *mut rusqlite::ffi::sqlite3) -> Option<Cause> {
        None
    }
    fn after_close(&mut self, _: bool) -> Option<Cause> {
        None
    }
}
fn acknowledgement(
    record: &ProviderCaptureTransactionRecord,
) -> CauseResult<ProviderCaptureAcknowledgement> {
    let ProviderCaptureState::Accepted { patch, .. } = &record.state else {
        return Err(invalid("capture acknowledgement requires Accepted"));
    };
    let receipt = ProviderCaptureReceiptRecord::derive(record)?;
    Ok(ProviderCaptureAcknowledgement {
        transaction_id: record.transaction_id.clone(),
        receipt_digest: receipt.digest()?,
        patch: patch.clone(),
    })
}
fn lease(
    held: &outcome::PublicationPins,
    proof: validation::PrivateCaptureMutationValidation,
) -> CauseResult<shared::ProviderLease> {
    let family = FamilyAccess::open(held.family.root())?;
    let allocation = records::AllocationRecord::decode(&held.artifacts.allocation.encode()?)?;
    Ok(shared::provider_publication_content_lease(
        family, allocation, proof,
    )?)
}

fn reservation(record: &ProviderCaptureTransactionRecord) -> String {
    // Transaction IDs are private to an owner, whereas family reservation IDs
    // share one namespace across all worktrees.
    let mut hash = blake3::Hasher::new();
    hash.update(b"layrs-provider-capture-reservation-v1\0");
    hash.update(record.owner_identity_digest.as_bytes());
    hash.update(b"\0");
    hash.update(record.transaction_id.as_bytes());
    format!("capture-{}", hash.finalize().to_hex())
}
struct CancelReader<'a> {
    reader: &'a mut dyn Read,
    cancel: &'a Cancellation,
}
impl Read for CancelReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        let count = output.len().min(64 * 1024);
        let result = self.reader.read(&mut output[..count])?;
        self.cancel.check().map_err(io::Error::other)?;
        Ok(result)
    }
}

pub(super) fn run(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    checkpoint: &mut Checkpoint<'_>,
    attempt: &CaptureAttemptLock,
    transaction_id: &str,
    cancel: &Cancellation,
) -> std::result::Result<ProviderCaptureAcknowledgement, ProviderCaptureFailure> {
    let mut step = ProviderCaptureStep::Checkpoint;
    let result = (|| -> CauseResult<ProviderCaptureAcknowledgement> {
        attempt.verify(&held.artifacts, &held.family)?;
        cancel.check()?;
        checkpoint.revalidate_namespace()?;
        let initial = capture_sql::observe(held, root, mount, activation)?;
        let existing = initial
            .capture()
            .transactions
            .iter()
            .find(|record| record.transaction_id == transaction_id)
            .cloned();
        if let Some(record) = &existing {
            match &record.state {
                ProviderCaptureState::Accepted { .. } => {
                    step = ProviderCaptureStep::ReservationRelease;
                    return release_and_acknowledge(
                        held, root, mount, activation, checkpoint, attempt, record,
                    );
                }
                ProviderCaptureState::Cancelled { .. } => {
                    return Err(invalid("capture transaction was cancelled"));
                }
                _ => {}
            }
        }
        let mut record = if let Some(record) = existing {
            record
        } else {
            let record = ProviderCaptureTransactionRecord::begin(
                &held.artifacts.allocation,
                mount,
                activation,
                transaction_id.into(),
                initial.capture().patches.len() as u64,
                initial
                    .capture()
                    .patches
                    .last()
                    .map(|patch| patch.digest.clone()),
            )?;
            step = ProviderCaptureStep::Hashing;
            capture_sql::begin(held, root, mount, activation, &record, &mut Close)?;
            record
        };
        step = ProviderCaptureStep::Hashing;
        let image = capture_image::plan(
            checkpoint,
            &initial.active().source,
            &initial.active().activated.snapshot,
            cancel,
        )?;
        let proposed = ProviderCaptureStage {
            image_digest: image.image_digest,
            image_path_count: image.image_path_count,
            changes: image.changes,
            reservation: reservation(&record),
        };
        attempt.verify(&held.artifacts, &held.family)?;
        step = ProviderCaptureStep::Staging;
        match &record.state {
            ProviderCaptureState::Hashing => {
                let mut next = record.clone();
                next.state = ProviderCaptureState::Staging {
                    stage: proposed.clone(),
                };
                capture_sql::transition(
                    held, root, mount, activation, &record, &next, checkpoint, &mut Close,
                )?;
                record = next;
                #[cfg(test)]
                tests::crash::barrier("staging", held, root, mount, activation, &record);
            }
            ProviderCaptureState::Staging { stage } if stage == &proposed => {}
            _ => {
                return Err(invalid(
                    "capture checkpoint differs from durable staged image",
                ));
            }
        }
        finish(
            held, root, mount, activation, checkpoint, attempt, cancel, &record, proposed,
            &mut step,
        )
    })();
    result.map_err(|cause| ProviderCaptureFailure {
        transaction_id: transaction_id.into(),
        step,
        cause,
    })
}

fn finish(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    checkpoint: &mut Checkpoint<'_>,
    attempt: &CaptureAttemptLock,
    cancel: &Cancellation,
    record: &ProviderCaptureTransactionRecord,
    stage: ProviderCaptureStage,
    step: &mut ProviderCaptureStep,
) -> CauseResult<ProviderCaptureAcknowledgement> {
    let operation = format!(
        "provider-capture:{}",
        blake3::hash(record.transaction_id.as_bytes()).to_hex()
    );
    let files = stage
        .changes
        .iter()
        .filter_map(|change| change.replacement.clone())
        .collect::<Vec<_>>();
    *step = ProviderCaptureStep::Content;
    let proof = capture_sql::observe(held, root, mount, activation)?;
    let content = lease(held, proof)?;
    content.reserve_exact(
        &stage.reservation,
        &held.artifacts.allocation.format.identity.worktree_id,
        &operation,
        retention::budget_for(&files, 0)?,
    )?;
    let mut installed = Vec::new();
    let mut seen = BTreeSet::new();
    for change in &stage.changes {
        cancel.check()?;
        let Some(file) = &change.replacement else {
            continue;
        };
        if !seen.insert(file.file_id.clone()) {
            continue;
        }
        let mut callback_cause = None;
        let result = checkpoint.with_reader(&change.path, |reader| {
            content
                .install_file(
                    &mut CancelReader { reader, cancel },
                    file,
                    &stage.reservation,
                )
                .map_err(|error| {
                    let cause: Cause = error.into();
                    callback_cause = Some(match cause {
                        Cause::Io(error)
                            if matches!(
                                error
                                    .get_ref()
                                    .and_then(|e| e.downcast_ref::<layrs_vfs::Error>()),
                                Some(layrs_vfs::Error::Cancelled)
                            ) =>
                        {
                            Cause::Vfs(layrs_vfs::Error::Cancelled)
                        }
                        cause => cause,
                    });
                    layrs_vfs::Error::Io(io::Error::other("provider capture installer failed"))
                })
        });
        if let Some(cause) = callback_cause {
            return Err(cause);
        }
        let manifest = result?;
        installed.push(manifest);
    }
    drop(content); // release owner shared before private SQL CAS
    attempt.verify(&held.artifacts, &held.family)?;
    cancel.check()?;
    let rows = installed
        .iter()
        .map(|manifest| CaptureContentProof {
            file_id: manifest.file_id().as_str().into(),
            manifest_id: manifest.id().as_str().into(),
            bytes: manifest.byte_length(),
        })
        .collect::<Vec<_>>();
    capture_sql::publish_content(
        held, root, mount, activation, record, &rows, checkpoint, &mut Close,
    )?;
    *step = ProviderCaptureStep::Protection;
    let proof = capture_sql::observe(held, root, mount, activation)?;
    let content = lease(held, proof)?;
    let (root_id, _) =
        content.protect(&held.artifacts.allocation.format, &operation, &installed)?;
    drop(content);
    attempt.verify(&held.artifacts, &held.family)?;
    cancel.check()?;
    *step = ProviderCaptureStep::Acceptance;
    let proof = capture_sql::observe(held, root, mount, activation)?;
    let patch = match proof.capture().patches.last() {
        Some(previous) if previous.changes == stage.changes => previous.clone(),
        previous => {
            let mut patch = CapturedPatch {
                schema_version: "layrs.projection-capture.v1".into(),
                sequence: proof.capture().patches.len() as u64 + 1,
                previous_patch_digest: previous.map(|patch| patch.digest.clone()),
                projection_generation: activation.activated_generation,
                projection_digest: activation.activated_digest.clone(),
                identity: held.artifacts.allocation.format.identity.clone(),
                changes: stage.changes.clone(),
                digest: String::new(),
            };
            patch.digest = patch.computed_digest()?;
            patch
        }
    };
    let mut accepted = record.clone();
    accepted.state = ProviderCaptureState::Accepted { stage, patch };
    let receipt = ProviderCaptureReceiptRecord::derive(&accepted)?;
    capture_sql::accept(
        held, root, mount, activation, record, &accepted, &receipt, &root_id, checkpoint,
        &mut Close,
    )?;
    #[cfg(test)]
    tests::crash::barrier("accepted", held, root, mount, activation, &accepted);
    *step = ProviderCaptureStep::ReservationRelease;
    release_and_acknowledge(
        held, root, mount, activation, checkpoint, attempt, &accepted,
    )
}

fn release_and_acknowledge(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    checkpoint: &Checkpoint<'_>,
    attempt: &CaptureAttemptLock,
    accepted: &ProviderCaptureTransactionRecord,
) -> CauseResult<ProviderCaptureAcknowledgement> {
    let ProviderCaptureState::Accepted { stage, .. } = &accepted.state else {
        return Err(invalid("capture reservation release requires Accepted"));
    };
    let proof = capture_sql::observe(held, root, mount, activation)?;
    let content = lease(held, proof)?;
    content.release_accepted_reservation(
        &stage.reservation,
        &held.artifacts.allocation.format.identity.worktree_id,
        &accepted.transaction_id,
    )?;
    drop(content);
    attempt.verify(&held.artifacts, &held.family)?;
    checkpoint.revalidate_namespace()?;
    acknowledgement(accepted)
}

/// Cancellation records abandonment without deleting any retained content or
/// family protection. A different transaction may then capture a changed image.
pub(super) fn cancel(
    held: &outcome::PublicationPins,
    root: &layrs_vfs::ProviderRootGuard,
    mount: &ProviderActiveMountRecord,
    activation: &ProviderActivationRecord,
    checkpoint: &Checkpoint<'_>,
    attempt: &CaptureAttemptLock,
    transaction_id: &str,
) -> std::result::Result<(), ProviderCaptureFailure> {
    let result = (|| -> CauseResult<()> {
        attempt.verify(&held.artifacts, &held.family)?;
        let state = capture_sql::observe(held, root, mount, activation)?;
        let prior = state
            .capture()
            .transactions
            .iter()
            .find(|row| row.transaction_id == transaction_id)
            .ok_or_else(|| invalid("capture cancellation transaction absent"))?;
        let stage = match &prior.state {
            ProviderCaptureState::Hashing => None,
            ProviderCaptureState::Staging { stage } => Some(stage.clone()),
            ProviderCaptureState::Cancelled { .. } => return Ok(()),
            ProviderCaptureState::Accepted { .. } => {
                return Err(invalid("accepted capture cannot be cancelled"));
            }
        };
        let mut next = prior.clone();
        next.state = ProviderCaptureState::Cancelled { stage };
        capture_sql::transition(
            held, root, mount, activation, prior, &next, checkpoint, &mut Close,
        )?;
        attempt.verify(&held.artifacts, &held.family)
    })();
    result.map_err(|cause| ProviderCaptureFailure {
        transaction_id: transaction_id.into(),
        step: ProviderCaptureStep::Cancellation,
        cause,
    })
}
