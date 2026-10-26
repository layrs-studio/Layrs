//! Two exact private metadata CAS transitions. Caller continuously owns the
//! lifecycle/owner/catalog gates and the authenticated A/P/control pins.
use super::*;
use rusqlite::{Connection, params};

#[derive(Clone, Copy)]
pub(super) enum Predecessor<'a> {
    Published(&'a MountRecord),
    Pending(&'a ProviderPreparationRecord),
}

fn boundary(
    held: &outcome::PublicationPins,
    native: Option<&layrs_vfs::ProviderRootGuard>,
) -> CauseResult<()> {
    held.artifacts.verify(&held.family)?;
    held.artifacts.writer_require_guard(&held.guard)?;
    match native {
        None => held.guard.revalidate_bootstrap_root().map_err(Cause::from),
        Some(root) => {
            held.guard.revalidate()?;
            root.revalidate()?;
            if root.control_directory().identity() != held.guard.identity() {
                return Err(invalid(
                    "native provider root lost the retained control binding",
                ));
            }
            Ok(())
        }
    }
}

fn mount_row(c: &Connection) -> CauseResult<(String, String, String, Vec<u8>, String)> {
    Ok(c.query_row(
        "SELECT id,catalog_id,phase,payload,digest FROM projection_provider_mounts",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?)
}

fn validate_old(
    c: &Connection,
    held: &outcome::PublicationPins,
    expected: Predecessor<'_>,
    next: &ProviderPreparationRecord,
    root_id: [u8; 16],
) -> CauseResult<()> {
    let a = &held.artifacts.allocation;
    let p = held
        .artifacts
        .prepared
        .as_ref()
        .ok_or_else(|| invalid("preparation requires retained P"))?;
    match expected {
        Predecessor::Published(mount) => {
            let inspected = validation::private_with_source(c, a, Some(p))?;
            let validation::PrivateSourceValidation::Initialized {
                mount: actual,
                source,
            } = inspected
            else {
                return Err(invalid(
                    "preparation requires initialized published metadata",
                ));
            };
            if actual != *mount || actual.phase != MountPhase::ControlPublished {
                return Err(invalid("published preparation predecessor changed"));
            }
            let old = mount_row(c)?;
            let row = records::preparation::MountRowData {
                id: &old.0,
                catalog_id: &old.1,
                phase: &old.2,
                payload: &old.3,
                digest: &old.4,
            };
            next.require_references(a, p, &source, root_id)?;
            next.require_predecessor_row(row)?;
        }
        Predecessor::Pending(pending) => {
            let inspected =
                validation::private_preparation_with_source(c, a, p, &held.binding, root_id)?;
            if inspected.mount != *pending || inspected.source.commitment() != &next.source {
                return Err(invalid("pending preparation predecessor changed"));
            }
            let old = mount_row(c)?;
            let row = records::preparation::MountRowData {
                id: &old.0,
                catalog_id: &old.1,
                phase: &old.2,
                payload: &old.3,
                digest: &old.4,
            };
            pending.require_row(row)?;
            next.require_pending(pending)?;
            next.require_references(a, p, &inspected.source, root_id)?;
        }
    }
    Ok(())
}

pub(super) fn transition(
    held: &outcome::PublicationPins,
    expected: Predecessor<'_>,
    next: &ProviderPreparationRecord,
    root_id: [u8; 16],
    native: Option<&layrs_vfs::ProviderRootGuard>,
    context: &mut report::Context,
) -> std::result::Result<(), report::Failure> {
    let phase = match expected {
        Predecessor::Published(_)
            if next.state == records::preparation::PreparationState::RegistrationPending
                && native.is_none() =>
        {
            report::ProviderPreparationSqlPhase::RegistrationPending
        }
        Predecessor::Pending(_)
            if matches!(
                next.state,
                records::preparation::PreparationState::ProviderPrepared { .. }
            ) && native.is_some() =>
        {
            report::ProviderPreparationSqlPhase::ProviderPrepared
        }
        _ => {
            return Err(report::Failure::private(invalid(
                "invalid provider preparation transition",
            )));
        }
    };
    boundary(held, native).map_err(report::Failure::private)?;
    let database = &held.artifacts.database;
    let admission = database_io::clean_admission(database, false, |c| {
        validate_old(c, held, expected, next, root_id)
    })
    .map_err(report::Failure::private)?;
    boundary(held, native).map_err(report::Failure::private)?;
    let open = context
        .arm(
            report::ProviderPreparationWriteStep::SqlOpen(phase),
            database.path(),
        )
        .map_err(report::Failure::private)?;
    let bare = {
        let mut observed = report::CloseLedger::new(context, phase, database.path());
        admission
            .open(database, &mut observed)
            .map_err(report::Failure::private)?
    };
    let result = (|| -> CauseResult<()> {
        admission.require_geometry(&bare)?;
        context.confirm(open)?;
        let transaction = context.arm(
            report::ProviderPreparationWriteStep::SqlTransaction(phase),
            database.path(),
        )?;
        let c = bare.connection();
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
        validate_old(c, held, expected, next, root_id)?;
        let old = mount_row(c)?;
        let changed = c.execute(
            "UPDATE projection_provider_mounts SET phase=?1,payload=?2,digest=?3 WHERE id=?4 AND catalog_id=?5 AND phase=?6 AND digest=?7 AND payload=?8",
            params![next.sql_phase(), next.encode()?, next.digest()?, old.0, old.1, old.2, old.4, old.3],
        )?;
        if changed != 1 {
            return Err(invalid(
                "provider preparation CAS changed no exact mount row",
            ));
        }
        let readback = validation::private_preparation_with_source(
            c,
            &held.artifacts.allocation,
            held.artifacts
                .prepared
                .as_ref()
                .ok_or_else(|| invalid("preparation P absent"))?,
            &held.binding,
            root_id,
        )?;
        if readback.mount != *next || readback.source.commitment() != &next.source {
            return Err(invalid("provider preparation SQL readback changed"));
        }
        context.confirm(transaction)?;
        boundary(held, native)?;
        let commit = context.arm(
            report::ProviderPreparationWriteStep::SqlCommit(phase),
            database.path(),
        )?;
        c.execute_batch("COMMIT")?;
        context.confirm(commit)?;
        Ok(())
    })();
    let mut observed = report::CloseLedger::new(context, phase, database.path());
    bare.finish_writer(result, &mut observed)
        .map_err(report::Failure::private)
}

pub(super) fn final_check(
    held: &outcome::PublicationPins,
    expected: &ProviderPreparationRecord,
    native: &layrs_vfs::ProviderRootGuard,
    root_id: [u8; 16],
) -> std::result::Result<(), report::Failure> {
    final_check_inner(held, expected, native, root_id, None)
}

pub(super) fn final_check_cancellable(
    held: &outcome::PublicationPins,
    expected: &ProviderPreparationRecord,
    native: &layrs_vfs::ProviderRootGuard,
    root_id: [u8; 16],
    cancel: &layrs_vfs::Cancellation,
) -> std::result::Result<(), report::Failure> {
    final_check_inner(held, expected, native, root_id, Some(cancel))
}

fn final_check_inner(
    held: &outcome::PublicationPins,
    expected: &ProviderPreparationRecord,
    native: &layrs_vfs::ProviderRootGuard,
    root_id: [u8; 16],
    cancel: Option<&layrs_vfs::Cancellation>,
) -> std::result::Result<(), report::Failure> {
    crate::projection::provider::cancellation::check(cancel).map_err(report::Failure::from)?;
    boundary(held, Some(native)).map_err(report::Failure::private)?;
    crate::projection::provider::cancellation::check(cancel).map_err(report::Failure::from)?;
    let family_read = |c: &Connection| match cancel {
        Some(cancel) => {
            held.family
                .validate_catalog_cancellable(c, &held.artifacts.allocation, cancel)
        }
        None => held.family.validate_catalog(c, &held.artifacts.allocation),
    };
    let catalog = match cancel {
        Some(cancel) => database_io::read_cancellable(&held.family.catalog, cancel, family_read),
        None => database_io::read(&held.family.catalog, family_read),
    }
    .map_err(|cause| report::Failure::new(cause, Some(ProviderDatabaseScope::FamilyCatalog)))?;
    if read_state_for(catalog, ProviderDatabaseScope::FamilyCatalog)? != Registration::Initializing
    {
        return Err(report::Failure::new(
            invalid("prepared provider requires exact initializing family registry"),
            Some(ProviderDatabaseScope::FamilyCatalog),
        ));
    }
    crate::projection::provider::cancellation::check(cancel).map_err(report::Failure::from)?;
    let private_read = |c: &Connection| {
        let prepared = held
            .artifacts
            .prepared
            .as_ref()
            .ok_or_else(|| invalid("preparation P absent"))?;
        match cancel {
            Some(cancel) => validation::private_preparation_with_source_cancellable(
                c,
                &held.artifacts.allocation,
                prepared,
                &held.binding,
                root_id,
                cancel,
            ),
            None => validation::private_preparation_with_source(
                c,
                &held.artifacts.allocation,
                prepared,
                &held.binding,
                root_id,
            ),
        }
    };
    let inspected = match cancel {
        Some(cancel) => {
            database_io::read_cancellable(&held.artifacts.database, cancel, private_read)
        }
        None => database_io::read(&held.artifacts.database, private_read),
    }
    .map_err(report::Failure::private)?;
    let readback = read_state(inspected)?;
    if readback.mount != *expected || readback.source.commitment() != &expected.source {
        return Err(report::Failure::private(invalid(
            "prepared metadata differs after observable close",
        )));
    }
    boundary(held, Some(native)).map_err(report::Failure::private)?;
    crate::projection::provider::cancellation::check(cancel).map_err(report::Failure::from)?;
    if held.artifacts.binding().map_err(report::Failure::private)? != held.binding {
        return Err(report::Failure::private(invalid(
            "preparation bootstrap binding changed",
        )));
    }
    crate::projection::provider::cancellation::check(cancel).map_err(report::Failure::from)?;
    Ok(())
}

pub(super) fn read_state<T>(
    state: database_io::ReadState<T>,
) -> std::result::Result<T, report::Failure> {
    read_state_for(state, ProviderDatabaseScope::PrivateMetadata)
}

fn read_state_for<T>(
    state: database_io::ReadState<T>,
    scope: ProviderDatabaseScope,
) -> std::result::Result<T, report::Failure> {
    let fail = |cause| report::Failure::new(cause, Some(scope));
    match state {
        database_io::ReadState::Valid(value) => Ok(value),
        database_io::ReadState::Uninitialized => {
            Err(fail(invalid("preparation requires initialized metadata")))
        }
        database_io::ReadState::Hot(hot) => Err(fail(Cause::ReadonlyRollback(hot.trigger))),
        database_io::ReadState::Rejected { cause, .. }
        | database_io::ReadState::ReadButUnavailable(cause)
        | database_io::ReadState::Unavailable(cause) => Err(fail(cause)),
        database_io::ReadState::UnboundRollback { trigger, cause } => {
            let mut error = fail(cause);
            error.rollback = Some(trigger);
            Err(error)
        }
    }
}
