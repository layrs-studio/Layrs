//! Explicit, quiescent family-catalog upgrade before a new managed import.
//! No provider connection, private mutation, cleanup or journal recovery occurs.
use super::{
    artifacts::Artifacts,
    database_io::{self, ReadState, WriterCloseObserver},
    records::capture::ProviderCaptureState,
    recovery::error::{invalid, ProviderBootstrapCause as Cause, Result},
    validation::{self, PrivateActiveCaptureValidation},
};
use crate::projection::shared::{
    provider_access::{ActivationRegistration, FamilyAccess},
    schema_v2::{self, MigrationEffect},
};
use layrs_core::scm::WorktreeId;
use layrs_vfs::Cancellation;
use rusqlite::Connection;
use std::path::Path;

fn valid<T>(state: ReadState<T>) -> Result<T> {
    match state {
        ReadState::Valid(value) => Ok(value),
        ReadState::Uninitialized => Err(invalid("family upgrade database uninitialized")),
        ReadState::Hot(hot) => Err(Cause::ReadonlyRollback(hot.trigger)),
        ReadState::UnboundRollback { trigger, cause } => Err(Cause::Cleanup {
            primary: Box::new(Cause::ReadonlyRollback(trigger)),
            close: Box::new(cause),
        }),
        ReadState::Rejected { cause, .. }
        | ReadState::ReadButUnavailable(cause)
        | ReadState::Unavailable(cause) => Err(cause),
    }
}

fn boundary(family: &FamilyAccess, artifacts: &Artifacts, cancel: &Cancellation) -> Result<()> {
    cancel.check()?;
    artifacts.verify(family)?;
    family.verify()
}

fn source(
    c: &Connection,
    artifacts: &Artifacts,
    cancel: &Cancellation,
) -> Result<PrivateActiveCaptureValidation> {
    let prepared = artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("family upgrade source prepared receipt absent"))?;
    let proof = validation::private_active_readable_with_source(
        c,
        &artifacts.allocation,
        prepared,
        &artifacts.binding()?,
        None,
        Some(cancel),
    )?
    .require_no_active_transition()?;
    if proof.capture.transactions.iter().any(|transaction| {
        matches!(
            transaction.state,
            ProviderCaptureState::Hashing | ProviderCaptureState::Staging { .. }
        )
    }) {
        return Err(invalid("family upgrade source capture is not terminal"));
    }
    Ok(proof)
}

fn family_version(
    c: &Connection,
    family: &FamilyAccess,
    artifacts: &Artifacts,
    proof: &PrivateActiveCaptureValidation,
    cancel: &Cancellation,
) -> Result<u32> {
    boundary(family, artifacts, cancel)?;
    if family.validate_capture_catalog(c, &artifacts.allocation, &proof.capture, Some(cancel))?
        != ActivationRegistration::Ready
    {
        return Err(invalid("family upgrade source owner is not Ready"));
    }
    let version = c.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !matches!(version, 1 | 2) {
        return Err(invalid("family upgrade catalog version differs"));
    }
    Ok(version)
}

/// This retains the source read transaction until the nested action and the
/// source's observable read close have both completed. A close failure cannot
/// be mistaken for an acknowledgement or a reason to enter another path.
fn with_source<T>(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    cancel: &Cancellation,
    action: impl FnOnce(&PrivateActiveCaptureValidation) -> Result<T>,
) -> Result<T> {
    boundary(family, artifacts, cancel)?;
    valid(database_io::read_cancellable(
        &artifacts.database,
        cancel,
        |c| {
            let proof = source(c, artifacts, cancel)?;
            boundary(family, artifacts, cancel)?;
            let value = action(&proof)?;
            boundary(family, artifacts, cancel)?;
            Ok(value)
        },
    )?)
}

fn migrate_family(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    proof: &PrivateActiveCaptureValidation,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> Result<MigrationEffect> {
    let check = |c: &Connection| family_version(c, family, artifacts, proof, cancel);
    let prior = valid(database_io::read_cancellable(
        &family.catalog,
        cancel,
        check,
    )?)?;
    if prior == 2 {
        return Ok(MigrationEffect::AlreadyV2);
    }
    database_io::validate_native_filename(family.catalog.path())?;
    let admission = database_io::clean_admission(&family.catalog, false, |c| {
        if check(c)? != 1 {
            return Err(invalid("family upgrade predecessor version changed"));
        }
        Ok(())
    })?;
    boundary(family, artifacts, cancel)?;
    let bare = admission.open(&family.catalog, close)?;
    let result = (|| -> Result<MigrationEffect> {
        admission.require_geometry(&bare)?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")?;
        if check(c)? != 1 {
            return Err(invalid("family upgrade writer predecessor changed"));
        }
        let effect = schema_v2::migrate(c, cancel)?;
        if effect != MigrationEffect::Migrated || check(c)? != 2 {
            return Err(invalid("family upgrade prospective schema differs"));
        }
        boundary(family, artifacts, cancel)?;
        c.execute_batch("COMMIT")?;
        Ok(effect)
    })();
    bare.finish_writer(result, close)
}

/// Upgrade only after all Active/session/checkpoint holders of this family
/// have been dropped. The nonblocking exclusive lifecycle acquisition refuses
/// retained session/checkpoint leases before opening either SQLite database.
/// Bare Active holders have no permanent lifecycle lease: dropping those is
/// a caller precondition, not something this gate independently proves. FORMAT,
/// catalog inode, source receipts and private SQL are retained unchanged.
pub(super) fn upgrade_source_family(
    family_root: &Path,
    worktree: &WorktreeId,
    transaction_id: &str,
    close: &mut impl WriterCloseObserver,
    cancel: &Cancellation,
) -> Result<MigrationEffect> {
    cancel.check()?;
    let family = FamilyAccess::open(family_root)?;
    let _lifecycle = family.writer_lifecycle(true)?;
    let _owner = family.writer_owner(worktree)?;
    let _catalog = family.writer_catalog()?;
    let artifacts = Artifacts::open(&family, worktree, transaction_id)?;
    boundary(&family, &artifacts, cancel)?;
    let effect = with_source(&family, &artifacts, cancel, |proof| {
        migrate_family(&family, &artifacts, proof, close, cancel)
    })?;
    // No operation follows an uncertain writer/source close. This fresh read
    // occurs only after the complete source-retained mutation step succeeded.
    with_source(&family, &artifacts, cancel, |proof| {
        let version = valid(database_io::read_cancellable(
            &family.catalog,
            cancel,
            |c| family_version(c, &family, &artifacts, proof, cancel),
        )?)?;
        if version != 2 {
            return Err(invalid("family upgrade durable schema2 readback differs"));
        }
        Ok(())
    })?;
    boundary(&family, &artifacts, cancel)?;
    Ok(effect)
}
