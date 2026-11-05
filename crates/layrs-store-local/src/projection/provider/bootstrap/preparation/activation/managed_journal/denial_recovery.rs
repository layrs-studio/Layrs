//! Recovery-only owner for an exact compact denial-start receipt. This never
//! opens an Active session, adopts a namespace, reconstructs an image, or grants
//! funding authority from a compact record.
use super::*;
use crate::projection::{
    provider::{
        artifacts::{Artifacts, RetainedControl},
        bootstrap::denial_receipt::{self, PublishedDeniedStartReceipt},
        capture_lock::CaptureAttemptLockError,
        records::Record,
        validation::PrivateActivePublicationValidation,
    },
    shared::{
        provider_access::FamilyAccess,
        source_companion::SourceCompanionFamilyContext,
        source_denial::{SourceDeniedCoveragePresence, SourceDeniedCoverageRelease},
    },
};
use layrs_core::scm::WorktreeId;
use std::{path::Path, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum DenialRecoveryEffect {
    NoReceipt,
    NoReservation,
    Released,
    RetainedActualFunding,
}

/// Reopen independently of ordinary Active admission, which intentionally
/// refuses an unproven reservation. The receipt exception is never exposed in
/// a returned holder. Checkout edits do not affect this release proof.
pub(in crate::projection::provider) fn recover(
    family_root: &Path,
    worktree: &WorktreeId,
    bootstrap_transaction: &str,
    expected: &ProviderBootstrapBinding,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> CauseResult<DenialRecoveryEffect> {
    cancel.check()?;
    let binding = ProviderBootstrapBinding::decode(&expected.encode()?)?;
    let family = FamilyAccess::open(family_root)?;
    let held = {
        let _locks = family.locks(worktree, false)?;
        let artifacts = Artifacts::open(&family, worktree, bootstrap_transaction)?;
        if artifacts.prepared.is_none()
            || artifacts.abandoned.is_some()
            || artifacts.binding()? != binding
        {
            return Err(invalid("denial recovery bootstrap binding differs"));
        }
        let RetainedControl::Final(guard) = artifacts.retain_control()? else {
            return Err(invalid("denial recovery requires final native control"));
        };
        guard.revalidate()?;
        artifacts.writer_require_guard(&guard)?;
        artifacts.verify(&family)?;
        outcome::PublicationPins {
            family,
            artifacts,
            binding,
            guard,
        }
    };
    // Same lock order as capture/start: capture lane before family gates.
    let attempt =
        CaptureAttemptLock::acquire(&held.artifacts, &held.family).map_err(
            |error| match error {
                CaptureAttemptLockError::Contended => Cause::Io(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "provider capture/recovery attempt already held",
                )),
                CaptureAttemptLockError::Cause(cause) => cause,
            },
        )?;
    let _locks = held.family.locks(worktree, true)?;
    let a = &held.artifacts.allocation;
    let p = held
        .artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("denial recovery P absent"))?;
    let receipt = denial_receipt::reopen(a, p, Some(cancel))?;
    let release = receipt
        .as_ref()
        .map(|receipt| SourceDeniedCoverageRelease::new(a, p, receipt.record()))
        .transpose()?;
    let mut retained_root = None;
    let effect = valid(database_io::read_cancellable_strict(
        &held.artifacts.database,
        cancel,
        |source| {
            let first = validation::private_active_publication_with_source(
                source,
                a,
                p,
                &held.binding,
                None,
                Some(cancel),
            )?;
            let ticket = first.active.mount.prepared.registration()?;
            retained_root = Some(ticket.guard_with_control_directory(
                Arc::new(first.active.source.vfs_catalog().clone()),
                &held.guard,
            )?);
            let root = retained_root
                .as_ref()
                .ok_or_else(|| invalid("denial recovery native guard absent"))?;
            let state = validation::private_active_publication_with_source(
                source,
                a,
                p,
                &held.binding,
                Some(root.registration().root_id()),
                Some(cancel),
            )?;
            if state.active.mount != first.active.mount
                || state.active.activation != first.active.activation
            {
                return Err(invalid("denial recovery native source changed"));
            }
            let boundary = || -> CauseResult<()> {
                cancel.check()?;
                attempt.verify(&held.artifacts, &held.family)?;
                super::super::boundary(&held, root)?;
                if let Some(receipt) = &receipt {
                    receipt.verify(Some(cancel))?;
                }
                Ok(())
            };
            boundary()?;
            let context = SourceCompanionFamilyContext {
                publication: &state,
                pending: None,
            };
            let normal_family = |c: &Connection| -> CauseResult<()> {
                if held
                    .family
                    .validate_source_companion_catalog(c, a, &context, Some(cancel))?
                    != ActivationRegistration::Ready
                {
                    return Err(invalid("denial recovery owner is not Ready"));
                }
                Ok(())
            };
            let Some(release) = &release else {
                valid(database_io::read_cancellable_strict(
                    &held.family.catalog,
                    cancel,
                    normal_family,
                )?)?;
                boundary()?;
                return Ok(DenialRecoveryEffect::NoReceipt);
            };
            if actual_funding(source, &state, release, receipt.as_ref().unwrap(), cancel)? {
                valid(database_io::read_cancellable_strict(
                    &held.family.catalog,
                    cancel,
                    normal_family,
                )?)?;
                boundary()?;
                return Ok(DenialRecoveryEffect::RetainedActualFunding);
            }
            let recovery_family = |c: &Connection| -> CauseResult<()> {
                if held.family.validate_source_denial_recovery_catalog(
                    c,
                    a,
                    &context,
                    release,
                    Some(cancel),
                )? != ActivationRegistration::Ready
                {
                    return Err(invalid("denial release owner is not Ready"));
                }
                Ok(())
            };
            let presence = valid(database_io::read_cancellable_strict(
                &held.family.catalog,
                cancel,
                |c| {
                    recovery_family(c)?;
                    release.read_presence(c, Some(cancel))
                },
            )?)?;
            boundary()?;
            if presence == SourceDeniedCoveragePresence::Reserved {
                let admission =
                    database_io::clean_admission(&held.family.catalog, false, recovery_family)?;
                boundary()?;
                let bare = admission.open(&held.family.catalog, close)?;
                let result = (|| -> CauseResult<()> {
                    admission.require_geometry(&bare)?;
                    let c = bare.connection();
                    c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
                    recovery_family(c)?;
                    boundary()?;
                    release.release(c, Some(cancel))?;
                    normal_family(c)?;
                    boundary()?;
                    c.execute_batch("COMMIT")?;
                    Ok(())
                })();
                let result = managed_destination::rollback(bare.connection(), result);
                bare.finish_writer(result, close)?;
            }
            // Any failed close stops above. The source absence snapshot remains
            // alive through the exact DELETE, its close and fresh catalog readback.
            valid(database_io::read_cancellable_strict(
                &held.family.catalog,
                cancel,
                normal_family,
            )?)?;
            boundary()?;
            Ok(if presence == SourceDeniedCoveragePresence::Reserved {
                DenialRecoveryEffect::Released
            } else {
                DenialRecoveryEffect::NoReservation
            })
        },
    )?)?;
    // The source reader must also close positively before deleting its receipt.
    if let Some(receipt) = receipt {
        receipt.cleanup(Some(cancel))?;
    }
    Ok(effect)
}

/// Existing durable intent/body keeps B, even Cancelled/Published. Otherwise
/// prove both identities absent under this retained complete source snapshot.
fn actual_funding(
    source: &Connection,
    state: &PrivateActivePublicationValidation,
    release: &SourceDeniedCoverageRelease,
    receipt: &PublishedDeniedStartReceipt,
    cancel: &Cancellation,
) -> CauseResult<bool> {
    cancel.check()?;
    let binding = release.binding();
    let intent = state
        .source_transitions()
        .iter()
        .find(|row| row.transaction_id == binding.transaction_id);
    if let Some(intent) = intent {
        if intent.plan.digest()? != binding.plan_digest
            || intent.plan.source_prefix_id != binding.prefix_id
            || intent.plan.denial_coverage.as_ref() != Some(&binding.document)
        {
            return Err(invalid(
                "denial receipt conflicts with actual source intent",
            ));
        }
        let actual = state
            .capture_data()
            .denial_reservations()
            .iter()
            .find(|funding| funding.operation() == release.operation())
            .ok_or_else(|| invalid("denial receipt actual funding absent"))?;
        receipt.record().require_reservation(actual)?;
        return Ok(true);
    }
    let document: bool = source.query_row(
        "SELECT EXISTS(SELECT 1 FROM projection_observations WHERE digest=?1)",
        [&binding.document.id],
        |row| row.get(0),
    )?;
    if document
        || state
            .capture_data()
            .denial_reservations()
            .iter()
            .any(|funding| {
                funding.binding().transaction_id == binding.transaction_id
                    || funding.binding().document.id == binding.document.id
            })
    {
        return Err(invalid("denial recovery source document is not absent"));
    }
    Ok(false)
}
