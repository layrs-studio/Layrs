//! Clean admitted inspection and restore recovery under already-retained
//! physical/owner scopes. No settlement, migration, private rollback recovery,
//! reservation cancellation or additional owner lock is available here.
use super::*;
use crate::projection::provider::{
    admitted_private_io::{AdmittedPreflight, AdmittedPrivateHandle},
    recovery::error::{invalid as rejected, Result as CauseResult},
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
        Ok(reasons)
    }).map_err(Into::into)
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
        false,
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
        true,
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
fn open_access<T>(
    store: &ProjectionStore,
    mut family: Option<shared::Lease>,
    lock: StoreLock,
    metadata: PinnedDirectory,
    pin: DatabasePin,
    boundaries: PreliminaryPins,
    binding: RecoveryBinding,
    write: bool,
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
    let handle = preflight.into_handle(write)?;
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
