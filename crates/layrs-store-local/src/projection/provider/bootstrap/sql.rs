//! Clean, explicitly admitted writer transactions. No recovery or ordinary opener.
use super::*;
use crate::projection::{database, retention, selection};
use rusqlite::{Connection, params};

pub(super) fn phase_name(phase: MountPhase) -> &'static str {
    match phase {
        MountPhase::Allocating => "allocating",
        MountPhase::ControlPrepared => "control_prepared",
        MountPhase::ControlPublished => "control_published",
    }
}

struct CloseLedger<'a> {
    ctx: &'a mut WriteContext,
    purpose: ProviderBootstrapSqlWrite,
    target: &'a std::path::Path,
    index: Option<usize>,
}
impl<'a> CloseLedger<'a> {
    fn new(
        ctx: &'a mut WriteContext,
        purpose: ProviderBootstrapSqlWrite,
        target: &'a std::path::Path,
    ) -> Self {
        Self {
            ctx,
            purpose,
            target,
            index: None,
        }
    }
}
impl database_io::WriterCloseObserver for CloseLedger<'_> {
    fn before_close(&mut self, _raw: *mut rusqlite::ffi::sqlite3) -> Option<Cause> {
        let next = self.ctx.report.as_ref().map(|r| r.effects.len());
        let armed = self.ctx.arm(
            ProviderBootstrapWriteStep::SqlClose(self.purpose),
            self.target,
        );
        // arm pushes the entry BEFORE its fallible notification. Retain that
        // index even when notification fails; cleanup must still happen once.
        self.index = next.filter(|&index| {
            self.ctx.report.as_ref().is_some_and(|r| {
                r.effects
                    .get(index)
                    .is_some_and(|e| e.step == ProviderBootstrapWriteStep::SqlClose(self.purpose))
            })
        });
        let fault = armed.err();
        #[cfg(test)]
        {
            let setup = tests::hooks::writer_before_close(_raw, self.purpose).err();
            return match (fault, setup) {
                (None, fault) | (fault, None) => fault,
                (Some(primary), Some(close)) => Some(Cause::Cleanup {
                    primary: Box::new(primary),
                    close: Box::new(close),
                }),
            };
        }
        #[cfg(not(test))]
        fault
    }
    fn after_close(&mut self, succeeded: bool) -> Option<Cause> {
        #[cfg(test)]
        tests::hooks::writer_close_observed(self.purpose, succeeded);
        if succeeded {
            self.index.and_then(|index| self.ctx.confirm(index).err())
        } else {
            None
        }
    }
    #[cfg(test)]
    fn open_point(
        &mut self,
        point: database_io::WriterOpenPoint,
        raw: *mut rusqlite::ffi::sqlite3,
    ) -> CauseResult<()> {
        tests::hooks::writer_open_point(self.purpose, point, raw)
    }
}

fn transaction<T>(
    database: &database_io::DatabaseFile,
    admission: database_io::CleanWriteAdmission,
    purpose: ProviderBootstrapSqlWrite,
    ctx: &mut WriteContext,
    body: impl FnOnce(&Connection, &mut WriteContext) -> CauseResult<T>,
    boundary: impl Fn() -> CauseResult<()>,
) -> CauseResult<T> {
    boundary()?;
    let open = ctx.arm(
        ProviderBootstrapWriteStep::SqlOpen(purpose),
        database.path(),
    )?;
    let bare = {
        let mut observed = CloseLedger::new(ctx, purpose, database.path());
        admission.open(database, &mut observed)?
    };
    let result = (|| {
        admission.require_geometry(&bare)?;
        ctx.confirm(open)?;
        let tx = ctx.arm(
            ProviderBootstrapWriteStep::SqlTransaction(purpose),
            database.path(),
        )?;
        let c = bare.connection();
        if purpose == ProviderBootstrapSqlWrite::InitializePrivate {
            let mode: String = c.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
            let check: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
            if mode != "delete" || check != "delete" {
                return Err(invalid(
                    "fresh provider database did not establish DELETE mode",
                ));
            }
        }
        c.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")?;
        let result = body(c, ctx)?;
        ctx.confirm(tx)?;
        boundary()?;
        let commit = ctx.arm(
            ProviderBootstrapWriteStep::SqlCommit(purpose),
            database.path(),
        )?;
        #[cfg(test)]
        tests::hooks::sql_commit_checkpoint(false, purpose, c, database.path())?;
        c.execute_batch("COMMIT")?;
        // Mark success BEFORE any close, fault hook or concluding identity check.
        ctx.confirm(commit)?;
        #[cfg(test)]
        tests::hooks::sql_commit_checkpoint(true, purpose, c, database.path())?;
        Ok(result)
    })();
    // One observed close for this actual connection, including geometry refusal.
    let mut observed = CloseLedger::new(ctx, purpose, database.path());
    bare.finish_writer(result, &mut observed)
}

pub(super) fn register(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    ctx: &mut WriteContext,
) -> CauseResult<()> {
    let _catalog = family.writer_catalog()?;
    let admission = database_io::clean_admission(&family.catalog, false, |c| {
        if family.validate_catalog(c, &artifacts.allocation)? != Registration::Absent {
            return Err(invalid("provider initializing registration already exists"));
        }
        Ok(())
    })?;
    transaction(
        &family.catalog,
        admission,
        ProviderBootstrapSqlWrite::RegisterInitializing,
        ctx,
        |c, _| {
            let a = &artifacts.allocation;
            if family.validate_catalog(c, a)? != Registration::Absent {
                return Err(invalid("provider registry changed before INSERT"));
            }
            c.execute("INSERT INTO worktrees(id,metadata_path,database_identity,marker,state) VALUES(?1,?2,?3,?4,'initializing')",
            params![a.format.identity.worktree_id.as_str(),a.private.metadata_path,a.private.database_file_identity,a.format.bytes()?])?;
            if family.validate_catalog(c, a)? != Registration::Initializing {
                return Err(invalid(
                    "provider initializing registration readback differs",
                ));
            }
            Ok(())
        },
        || artifacts.verify(family),
    )
}

pub(super) fn initialize(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    source: &ProviderBootstrapSource,
    ctx: &mut WriteContext,
) -> CauseResult<()> {
    source.require_commitment(&artifacts.allocation.source)?;
    let admission = database_io::clean_admission(&artifacts.database, true, |_| {
        Err(invalid(
            "fresh initializer must not read an existing schema",
        ))
    })?;
    transaction(
        &artifacts.database,
        admission,
        ProviderBootstrapSqlWrite::InitializePrivate,
        ctx,
        |c, _| {
            c.execute_batch(
                &[
                    database::SQL,
                    selection::SQL,
                    retention::SQL,
                    super::super::schema::SQL,
                    "PRAGMA user_version=8;",
                ]
                .concat(),
            )?;
            c.execute(
                "INSERT INTO projection_meta VALUES(1,?1)",
                [artifacts.allocation.format.bytes()?],
            )?;
            c.execute(
                "INSERT INTO projection_snapshots VALUES(0,?1,NULL,?2)",
                params![
                    source.commitment().snapshot_digest,
                    source
                        .initial()
                        .canonical_bytes()
                        .map_err(crate::projection::invalid)?
                ],
            )?;
            c.execute("INSERT INTO projection_head VALUES(1,0)", [])?;
            selection::insert(c, 0, source.selection())?;
            let q = source.catalog();
            c.execute(
                "INSERT INTO projection_provider_catalogs VALUES(?1,0,?2,?3,?4)",
                params![
                    source.commitment().catalog_id,
                    q.source_digest,
                    q.selection_digest,
                    q.encode()?
                ],
            )?;
            for d in source.descriptors() {
                c.execute(
                    "INSERT INTO projection_provider_descriptors VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        source.commitment().catalog_id,
                        d.path.as_str(),
                        d.file_id.as_str(),
                        d.bytes,
                        d.executable,
                        d.manifest_id.as_str(),
                        d.manifest_bytes()?
                    ],
                )?;
            }
            let m = mount(artifacts, MountPhase::Allocating)?;
            c.execute(
                "INSERT INTO projection_provider_mounts VALUES(?1,?2,?3,?4,?5)",
                params![
                    m.transaction_id,
                    m.catalog_id,
                    phase_name(m.phase),
                    m.encode()?,
                    m.digest()?
                ],
            )?;
            c.execute(
                "INSERT INTO projection_backend VALUES(1,'provider',?1)",
                [&artifacts.allocation.transaction_id],
            )?;
            require_mount(
                validation::private(c, &artifacts.allocation, None)?,
                MountPhase::Allocating,
            )?;
            Ok(())
        },
        || {
            artifacts.verify(family)?;
            require_initializing(family, artifacts)
        },
    )
}

pub(super) fn mount(artifacts: &Artifacts, phase: MountPhase) -> CauseResult<MountRecord> {
    Ok(MountRecord {
        schema_version: "layrs.projection-provider-mount.v1".into(),
        transaction_id: artifacts.allocation.transaction_id.clone(),
        catalog_id: artifacts.allocation.source.catalog_id.clone(),
        phase,
        allocation: artifacts.allocation.data_reference()?,
        prepared: if phase == MountPhase::Allocating {
            None
        } else {
            Some(
                artifacts
                    .prepared
                    .as_ref()
                    .ok_or_else(|| invalid("provider prepared receipt absent"))?
                    .data_reference()?,
            )
        },
    })
}
pub(super) fn require_mount(
    value: validation::PrivateValidation,
    phase: MountPhase,
) -> CauseResult<MountRecord> {
    match value {
        validation::PrivateValidation::Initialized(m) if m.phase == phase => Ok(m),
        _ => Err(invalid(
            "provider private mount is not the required initialized phase",
        )),
    }
}
pub(super) fn require_initializing(
    family: &FamilyAccess,
    artifacts: &Artifacts,
) -> CauseResult<()> {
    let result = (|| -> CauseResult<()> {
        let _catalog = family.writer_catalog()?;
        let state = database_io::read(&family.catalog, |c| {
            family.validate_catalog(c, &artifacts.allocation)
        })?;
        if known(state, ProviderDatabaseScope::FamilyCatalog)? != Registration::Initializing {
            return Err(invalid(
                "provider publication requires exact initializing registry",
            ));
        }
        Ok(())
    })();
    result.map_err(|cause| {
        Cause::Validation(Box::new(recovery::writer_failure(
            "validate initializing writer registry",
            cause,
            Some(ProviderDatabaseScope::FamilyCatalog),
            None,
            None,
        )))
    })
}
pub(super) fn known<T>(
    state: database_io::ReadState<T>,
    scope: ProviderDatabaseScope,
) -> CauseResult<T> {
    match state {
        database_io::ReadState::Valid(v) => Ok(v),
        database_io::ReadState::Hot(h) => Err(Cause::ReadonlyRollback(h.trigger)),
        database_io::ReadState::Rejected { cause, .. }
        | database_io::ReadState::Unavailable(cause)
        | database_io::ReadState::ReadButUnavailable(cause) => Err(cause),
        database_io::ReadState::UnboundRollback { trigger, cause } => {
            Err(Cause::Validation(Box::new(recovery::writer_failure(
                "validate provider writer database",
                cause,
                Some(scope),
                Some(trigger),
                None,
            ))))
        }
        database_io::ReadState::Uninitialized => Err(invalid("provider database is uninitialized")),
    }
}
pub(super) fn set_phase(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    from: MountPhase,
    to: MountPhase,
    ctx: &mut WriteContext,
    control_boundary: impl Fn() -> CauseResult<()>,
) -> CauseResult<()> {
    let purpose = if to == MountPhase::ControlPrepared {
        ProviderBootstrapSqlWrite::PrepareControl
    } else {
        ProviderBootstrapSqlWrite::PublishControl
    };
    let admission = database_io::clean_admission(&artifacts.database, false, |c| {
        require_mount(
            validation::private(c, &artifacts.allocation, artifacts.prepared.as_ref())?,
            from,
        )
        .map(|_| ())
    })?;
    transaction(
        &artifacts.database,
        admission,
        purpose,
        ctx,
        |c, _| {
            let old = require_mount(
                validation::private(c, &artifacts.allocation, artifacts.prepared.as_ref())?,
                from,
            )?;
            let next = mount(artifacts, to)?;
            let changed=c.execute("UPDATE projection_provider_mounts SET phase=?1,payload=?2,digest=?3 WHERE id=?4 AND phase=?5 AND digest=?6 AND payload=?7",
            params![phase_name(to),next.encode()?,next.digest()?,next.transaction_id,phase_name(from),old.digest()?,old.encode()?])?;
            if changed != 1 {
                return Err(invalid("provider mount CAS changed no exact row"));
            }
            require_mount(
                validation::private(c, &artifacts.allocation, artifacts.prepared.as_ref())?,
                to,
            )?;
            Ok(())
        },
        || {
            artifacts.verify(family)?;
            if artifacts.abandoned.is_some() {
                return Err(invalid("provider bootstrap is terminal"));
            }
            control_boundary()?;
            require_initializing(family, artifacts)
        },
    )
}

pub(super) fn remove_initializing_locked(
    family: &FamilyAccess,
    artifacts: &Artifacts,
    ctx: &mut WriteContext,
    boundary: impl Fn() -> CauseResult<()>,
) -> CauseResult<()> {
    let terminal = artifacts
        .abandoned
        .as_ref()
        .ok_or_else(|| invalid("registry removal requires authenticated terminal receipt"))?;
    let admission = database_io::clean_admission(&family.catalog, false, |c| {
        let registration = family.validate_catalog(c, &artifacts.allocation)?;
        if terminal.eligibility.registry_at_decision == RegistryAtDecision::Absent
            || registration != Registration::Initializing
        {
            return Err(invalid(
                "terminal does not permit removing this registration",
            ));
        }
        Ok(())
    })?;
    transaction(
        &family.catalog,
        admission,
        ProviderBootstrapSqlWrite::RemoveInitializing,
        ctx,
        |c, _| {
            let a = &artifacts.allocation;
            if family.validate_catalog(c, a)? != Registration::Initializing {
                return Err(invalid("initializing registry changed before removal"));
            }
            let count=c.execute("DELETE FROM worktrees WHERE id=?1 AND metadata_path=?2 AND database_identity=?3 AND marker=?4 AND state='initializing'",
            params![a.format.identity.worktree_id.as_str(),a.private.metadata_path,a.private.database_file_identity,a.format.bytes()?])?;
            if count != 1 || family.validate_catalog(c, a)? != Registration::Absent {
                return Err(invalid("terminal registry removal readback differs"));
            }
            Ok(())
        },
        || {
            artifacts.verify(family)?;
            boundary()
        },
    )
}
