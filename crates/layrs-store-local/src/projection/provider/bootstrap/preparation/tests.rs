//! Scoped writer fault checkpoints for owned child-process recipes.
pub(in crate::projection::provider::bootstrap) mod hooks {
    use super::super::*;
    use std::{cell::RefCell, path::PathBuf};

    #[derive(Default)]
    struct State {
        stop: Option<(ProviderPreparationWriteStep, bool)>,
        busy_close: Option<ProviderPreparationSqlPhase>,
        tamper_pending_after_prepared_open: Option<PathBuf>,
    }
    thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

    pub(in crate::projection::provider::bootstrap) fn reset() {
        STATE.with(|state| *state.borrow_mut() = State::default());
    }
    pub(in crate::projection::provider::bootstrap) fn stop(
        step: ProviderPreparationWriteStep,
        after: bool,
    ) {
        STATE.with(|state| state.borrow_mut().stop = Some((step, after)));
    }
    pub(in crate::projection::provider::bootstrap) fn busy_close(
        phase: ProviderPreparationSqlPhase,
    ) {
        STATE.with(|state| state.borrow_mut().busy_close = Some(phase));
    }
    pub(in crate::projection::provider::bootstrap) fn tamper_pending_after_prepared_open(
        path: PathBuf,
    ) {
        STATE.with(|state| state.borrow_mut().tamper_pending_after_prepared_open = Some(path));
    }
    pub(in crate::projection::provider::bootstrap) fn point(
        step: ProviderPreparationWriteStep,
        after: bool,
    ) -> CauseResult<()> {
        let tamper = STATE.with(|state| {
            if step
                == ProviderPreparationWriteStep::SqlOpen(
                    ProviderPreparationSqlPhase::ProviderPrepared,
                )
                && after
            {
                state.borrow_mut().tamper_pending_after_prepared_open.take()
            } else {
                None
            }
        });
        if let Some(path) = tamper {
            let c = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )?;
            let changed = c.execute(
                "UPDATE projection_provider_mounts SET digest=?1 WHERE phase='control_published'",
                [format!("blake3:{}", "0".repeat(64))],
            )?;
            let result = if changed == 1 {
                Ok(())
            } else {
                Err(invalid("pending tamper fixture changed no row"))
            };
            c.close().map_err(|(_, error)| Cause::Sqlite(error))?;
            result?;
        }
        if STATE.with(|state| state.borrow().stop == Some((step, after))) {
            return Err(Cause::Io(std::io::Error::other(
                "synthetic preparation checkpoint",
            )));
        }
        Ok(())
    }
    pub(in crate::projection::provider::bootstrap) fn before_close(
        raw: *mut rusqlite::ffi::sqlite3,
        phase: ProviderPreparationSqlPhase,
    ) -> CauseResult<()> {
        if !STATE.with(|state| state.borrow().busy_close == Some(phase)) {
            return Ok(());
        }
        let mut statement = std::ptr::null_mut();
        let rc = unsafe {
            rusqlite::ffi::sqlite3_prepare_v2(
                raw,
                c"SELECT 1".as_ptr(),
                -1,
                &mut statement,
                std::ptr::null_mut(),
            )
        };
        if rc != rusqlite::ffi::SQLITE_OK || statement.is_null() {
            return Err(invalid("preparation BUSY close fixture failed"));
        }
        Ok(())
    }
}

#[test]
fn nested_bootstrap_refusal_retains_scope_and_original_rollback_arc() {
    use super::*;
    let trigger = std::sync::Arc::new(rusqlite::Error::QueryReturnedNoRows);
    let bootstrap = ProviderBootstrapFailure {
        kind: recovery::ProviderBootstrapFailureKind::NeedsRepair,
        operation: "inspect published bootstrap",
        database: Some(ProviderDatabaseScope::FamilyCatalog),
        effects: Vec::new(),
        cause: Cause::ReadonlyRollback(std::sync::Arc::clone(&trigger)),
        rollback_triggers: Vec::new(),
        readonly_rollback: Some(std::sync::Arc::clone(&trigger)),
        write_report: None,
    };
    let cause = Cause::Validation(Box::new(ProjectionStoreError::ProviderBootstrap(Box::new(
        bootstrap,
    ))));
    let failure = report::Failure::new(cause, None);
    assert_eq!(failure.database, Some(ProviderDatabaseScope::FamilyCatalog));
    assert!(std::sync::Arc::ptr_eq(
        failure.rollback.as_ref().unwrap(),
        &trigger
    ));
    let worktree_id = validation::tests::fixture::Fixture::new("G0", false)
        .allocation
        .format
        .identity
        .worktree_id;
    let context = report::Context::new(&ProviderBootstrapWriteReport {
        provenance: ProviderBootstrapWriteReportProvenance::CurrentInvocation {
            operation: ProviderBootstrapWriterOperation::Begin,
        },
        attempt: ProviderBootstrapAttempt {
            family_root: "family".into(),
            checkout_root: "checkout".into(),
            stage_parent: "stage".into(),
            worktree_id,
            transaction_id: "tx".into(),
        },
        effects: Vec::new(),
    });
    let outer = context.failure(failure);
    assert_eq!(outer.kind, ProviderPreparationFailureKind::ReadonlyRollback);
    assert_eq!(outer.database, Some(ProviderDatabaseScope::FamilyCatalog));
    assert!(std::sync::Arc::ptr_eq(
        outer.readonly_rollback.as_ref().unwrap(),
        &trigger
    ));
    assert!(matches!(outer.cause, Cause::Validation(_)));
}
