//! Clean admitted inspection, restore recovery and prejournal cancellation
//! admission under already-retained physical/owner scopes. Opening never
//! settles, migrates, rolls SQLite back or acquires an additional owner lock.
use super::*;
use crate::projection::provider::{
    admitted_private_io::{AdmittedPreflight, AdmittedPrivateHandle},
    recovery::error::{Result as CauseResult, invalid as rejected},
};
use rusqlite::Connection;

type DatabasePin = crate::projection::file::DatabasePin;

pub(super) fn inspection_reasons(access: &Access) -> Result<Vec<ProjectionRecoveryReason>> {
    let PrivateConnection::Admitted(handle) = &access.connection else {
        return Err(invalid("admitted inspection lost its typed private handle"));
    };
    handle.with_validation(|c| {
        validate_platform(&database::current(c)?.snapshot)?;
        let filesystem: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM projection_install) OR EXISTS(SELECT 1 FROM projection_refresh_pending) OR EXISTS(SELECT 1 FROM projection_namespace_pending) OR EXISTS(SELECT 1 FROM projection_restore_pending) OR EXISTS(SELECT 1 FROM projection_rebase_pending)", [], |r| r.get(0))?;
        let publication: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM projection_publication_pending)", [], |r| r.get(0))?;
        let mut reasons = Vec::new();
        if filesystem { reasons.push(ProjectionRecoveryReason::FilesystemOperation); }
        if publication { reasons.push(ProjectionRecoveryReason::Publication); }
        if !filesystem && !publication && super::reservation_pending(c)? {
            reasons.push(ProjectionRecoveryReason::Reservation);
        }
        Ok(reasons)
    }).map_err(Into::into)
}

pub(super) fn cancel_access(
    store: &ProjectionStore,
    family: Option<shared::Lease>,
    lock: StoreLock,
    metadata: PinnedDirectory,
    pin: DatabasePin,
    boundaries: PreliminaryPins,
    binding: RecoveryBinding,
) -> Result<Access> {
    let (access, _) = open_access(
        store,
        family,
        lock,
        metadata,
        pin,
        boundaries,
        binding,
        |preflight, data: &retention::RestoreCancellationData| {
            if !data.has_budget() {
                // Completed cancellation replay has no mutation to perform.
                preflight.into_handle(false)
            } else if data.requires_denial_capture_handle() {
                preflight.into_local_capture_cancel_handle(data)
            } else if data.requires_denial_restore_handle() {
                preflight.into_local_restore_cancel_handle(data)
            } else {
                preflight.into_handle(true)
            }
        },
        |c| retention::restore_cancellation_preflight(c, store).map_err(Into::into),
        |data, lease| {
            data.validate_family(lease, store)?;
            data.validate_staging(store)
        },
        |handle, baseline| {
            let fresh = handle.with_validation(|c| {
                retention::restore_cancellation_preflight(c, store).map_err(Into::into)
            })?;
            if fresh != *baseline {
                return Err(rejected(
                    "admitted cancellation preflight changed before recovery",
                ));
            }
            fresh.validate_staging(store).map_err(Into::into)
        },
    )?;
    Ok(access)
}

pub(super) fn inspection_access(
    store: &ProjectionStore,
    family: Option<shared::Lease>,
    lock: StoreLock,
    metadata: PinnedDirectory,
    pin: DatabasePin,
    boundaries: PreliminaryPins,
    binding: RecoveryBinding,
) -> Result<Access> {
    open_access(
        store,
        family,
        lock,
        metadata,
        pin,
        boundaries,
        binding,
        |preflight, ()| preflight.into_handle(false),
        |_| Ok(()),
        |(), _| Ok(()),
        |handle, ()| handle.with_validation(|_| Ok(())),
    )
    .map(|(access, ())| access)
}

pub(super) fn restore_access(
    store: &ProjectionStore,
    family: Option<shared::Lease>,
    lock: StoreLock,
    metadata: PinnedDirectory,
    pin: DatabasePin,
    boundaries: PreliminaryPins,
    binding: RecoveryBinding,
) -> Result<Access> {
    let (access, _) = open_access(
        store,
        family,
        lock,
        metadata,
        pin,
        boundaries,
        binding,
        |preflight, data: &restore::RestoreRecoveryData| {
            if data.requires_denial_restore_handle() {
                preflight.into_local_restore_recovery_handle(data)
            } else {
                preflight.into_handle(true)
            }
        },
        |c| restore::recovery_preflight(c, store).map_err(Into::into),
        |data, lease| data.validate_family(lease, store),
        |handle, baseline| {
            let fresh = handle
                .with_validation(|c| restore::recovery_preflight(c, store).map_err(Into::into))?;
            if fresh != *baseline {
                return Err(rejected(
                    "admitted restore preflight changed before recovery",
                ));
            }
            Ok(())
        },
    )?;
    Ok(access)
}

/// Preflight callback runs inside the clean private RO transaction; the family
/// callback runs only after that real connection has positively closed. All
/// validation preceding into_handle is read-only and preserves budgets.
#[allow(clippy::too_many_arguments)]
fn open_access<T>(
    store: &ProjectionStore,
    mut family: Option<shared::Lease>,
    lock: StoreLock,
    metadata: PinnedDirectory,
    pin: DatabasePin,
    boundaries: PreliminaryPins,
    binding: RecoveryBinding,
    open_handle: impl FnOnce(AdmittedPreflight, &T) -> CauseResult<AdmittedPrivateHandle>,
    validate_private: impl FnOnce(&Connection) -> CauseResult<T>,
    validate_family: impl FnOnce(&T, &shared::Lease) -> Result<()>,
    validate_fresh: impl FnOnce(&AdmittedPrivateHandle, &T) -> CauseResult<()>,
) -> Result<(Access, T)> {
    let path = store.marker.metadata_path(&store.root);
    let (preflight, data) = AdmittedPreflight::open_validated(
        &path.join(DATABASE),
        pin.identity(),
        &store.marker,
        validate_private,
    )?;
    let lease = family
        .as_mut()
        .ok_or_else(|| invalid("admitted recovery family absent"))?;
    lease.admit(preflight.data())?;
    lease.with_content_catalog(false, |c| {
        let valid: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM worktrees WHERE id=?1 AND marker=?2 AND state='ready' AND metadata_path=?3 AND database_identity=?4)",
            rusqlite::params![store.identity().worktree_id.as_str(), store.marker.bytes()?,
                path.to_string_lossy(), pin.identity()], |r| r.get(0),
        )?;
        if !valid { return Err(invalid("admitted recovery registration differs or is not ready")); }
        Ok(())
    })?;
    validate_family(&data, lease)?;
    boundaries.verify(&store.root, &store.marker)?;
    verify_boundaries(&store.root, &store.marker, &metadata, &pin)?;
    verify_binding(&store.root, &store.marker, &pin, family.as_ref(), &binding)?;
    let handle = open_handle(preflight, &data)?;
    if let Err(error) = validate_fresh(&handle, &data) {
        return handle.finish(Err(error)).map_err(Into::into);
    }
    let access = Access {
        connection: PrivateConnection::Admitted(handle),
        metadata,
        database: pin,
        root: store.root.clone(),
        marker: store.marker.clone(),
        recovery_binding: Some(binding),
        _boundaries: boundaries,
        _lock: lock,
        family,
    };
    if let Err(error) = access.verify() {
        return access.run(|_| Err(error));
    }
    Ok((access, data))
}
