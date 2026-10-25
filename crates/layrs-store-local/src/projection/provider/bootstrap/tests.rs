//! SOURCE RECIPES ONLY until ROOT compiles/reviews/releases a serial queue.
use super::super::process_profile::tests::child;
use super::*;
mod fixture;
pub(super) mod hooks;
pub(super) mod process;
mod recipes;
use fixture::{Fixture, failure, has};
use std::path::PathBuf;

fn attempt() -> ProviderBootstrapAttempt {
    ProviderBootstrapAttempt {
        family_root: PathBuf::from(r"C:\writer-test\family"),
        checkout_root: PathBuf::from(r"C:\writer-test\checkout"),
        stage_parent: PathBuf::from(r"C:\writer-test\stages"),
        worktree_id: super::super::validation::tests::fixture::Fixture::new("G0", false)
            .allocation
            .format
            .identity
            .worktree_id,
        transaction_id: fixture::TX.into(),
    }
}
#[test]
fn report_order_and_retained_issuance_are_data_only() {
    let started = std::time::Instant::now();
    hooks::reset();
    for operation in [
        ProviderBootstrapWriterOperation::Begin,
        ProviderBootstrapWriterOperation::ResumeControlPublication,
        ProviderBootstrapWriterOperation::Abandon,
    ] {
        let mut context = WriteContext::current(operation, attempt());
        let target = Path::new(r"C:\writer-test\object");
        let a = context
            .arm(
                ProviderBootstrapWriteStep::ExclusiveCreate(ProviderBootstrapCreate::Member),
                target,
            )
            .unwrap();
        context.confirm(a).unwrap();
        let b = context
            .arm(
                ProviderBootstrapWriteStep::ReceiptRename(ProviderBootstrapReceipt::Allocation),
                target,
            )
            .unwrap();
        context.no_effect(b).unwrap();
        context
            .arm(
                ProviderBootstrapWriteStep::SqlCommit(
                    ProviderBootstrapSqlWrite::RegisterInitializing,
                ),
                target,
            )
            .unwrap();
        let issuance = context.report.as_ref().unwrap();
        let retained = issuance.retained();
        assert_eq!(
            issuance.provenance,
            ProviderBootstrapWriteReportProvenance::CurrentInvocation { operation }
        );
        assert_eq!(
            retained.provenance,
            ProviderBootstrapWriteReportProvenance::RetainedIssuance { operation }
        );
        assert_eq!(issuance.attempt.family_root, retained.attempt.family_root);
        assert_eq!(
            issuance.attempt.checkout_root,
            retained.attempt.checkout_root
        );
        assert_eq!(issuance.attempt.stage_parent, retained.attempt.stage_parent);
        assert_eq!(issuance.attempt.worktree_id, retained.attempt.worktree_id);
        assert_eq!(
            issuance.attempt.transaction_id,
            retained.attempt.transaction_id
        );
        assert_eq!(issuance.effects.len(), 3);
        for (old, new) in issuance.effects.iter().zip(&retained.effects) {
            assert_eq!(old.step, new.step);
            assert_eq!(old.state, new.state);
            assert_eq!(old.target, new.target);
        }
        assert_eq!(
            retained.effects.iter().map(|e| e.state).collect::<Vec<_>>(),
            vec![
                ProviderBootstrapWriteState::Confirmed,
                ProviderBootstrapWriteState::NoNewEffect,
                ProviderBootstrapWriteState::Uncertain
            ]
        );
        let empty = WriteContext::current(operation, attempt()).report.unwrap();
        assert!(empty.retained().effects.is_empty());
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}
#[test]
fn public_source_and_writer_reporting_types_are_nameable() {
    use crate::projection::{
        ProjectionStoreError as PublicError, ProviderBootstrapSource as PublicSource,
        ProviderBootstrapWriteReport as PublicReport, ProviderSourceDescriptor as PublicDescriptor,
    };
    let _: fn(
        layrs_core::projection::ProjectionSnapshot,
        layrs_core::projection::ProjectionSelection,
        Vec<PublicDescriptor>,
    ) -> std::result::Result<PublicSource, PublicError> = PublicSource::validate;
    fn report(_: Option<&PublicReport>) {}
    report(None);
    fn send_sync<T: Send + Sync>() {}
    send_sync::<ProviderBootstrapCause>();
}
#[test]
fn callback_bridge_preserves_original_and_both_cleanup_causes() {
    use std::{error::Error, sync::Arc};
    let cause = Arc::new(Cause::Cleanup {
        primary: Box::new(Cause::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("synthetic original".into()),
        ))),
        close: Box::new(Cause::Io(std::io::Error::from_raw_os_error(32))),
    });
    let bridge = report::CallbackCause(Arc::clone(&cause));
    let exposed = bridge.source().unwrap().downcast_ref::<Cause>().unwrap();
    assert!(std::ptr::eq(exposed, cause.as_ref()));
    let outer = layrs_vfs::Error::ControlDirectory {
        kind: layrs_vfs::ControlDirectoryFailure::NeedsRepair,
        point: layrs_vfs::ControlDirectoryPublishPoint::PreRename,
        operation: "synthetic callback bridge",
        root: PathBuf::from(r"C:\writer-test"),
        stage: PathBuf::from(r"C:\writer-stage"),
        cause: Box::new(layrs_vfs::Error::from(std::io::Error::other(bridge))),
    };
    let mut next: Option<&(dyn Error + 'static)> = Some(&outer);
    let mut found = false;
    while let Some(error) = next {
        if let Some(actual) = error.downcast_ref::<Cause>() {
            if std::ptr::eq(actual, cause.as_ref()) {
                found = true;
                break;
            }
        }
        next = error.source();
    }
    assert!(found);
    match cause.as_ref() {
        Cause::Cleanup { primary, close } => {
            assert!(
                matches!(primary.as_ref(),Cause::Sqlite(rusqlite::Error::SqliteFailure(e,_)) if e.extended_code==rusqlite::ffi::SQLITE_BUSY)
            );
            assert!(matches!(close.as_ref(),Cause::Io(e) if e.raw_os_error()==Some(32)));
        }
        _ => panic!("typed cleanup lost"),
    }
}
#[test]
fn legacy_error_constructors_keep_none_without_losing_triggers() {
    for error in recovery::writer_legacy_none_controls() {
        let f = failure::<()>(Err(error));
        assert!(f.write_report.is_none());
        assert!(f.effects.is_empty());
        assert!(f.rollback_triggers.is_empty());
    }
}

#[test]
fn preallocation_filename_admissibility_uses_opener_rules() {
    let started = std::time::Instant::now();
    for path in [
        r"C:\family\metadata\catalog.sqlite3",
        r"C:\family\worktrees\member\metadata\projection.sqlite3",
        r"\\?\C:\family\metadata\catalog.sqlite3",
    ] {
        database_io::validate_writer_filename(Path::new(path)).unwrap();
    }
    for path in [
        r"C:\family#one\metadata\catalog.sqlite3",
        r"C:\family#one\worktrees\member\metadata\projection.sqlite3",
        r"C:\family?one\metadata\catalog.sqlite3",
        r"file:C:\family\catalog.sqlite3",
        r"C:\family\catalog.sqlite3:stream",
    ] {
        let cause = database_io::validate_writer_filename(Path::new(path)).unwrap_err();
        assert!(
            matches!(cause,Cause::Validation(e) if matches!(e.as_ref(),ProjectionStoreError::Invalid(s) if s=="provider SQLite requires an exact absolute local native filename"))
        );
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}
#[test]
fn receipt_temporary_name_boundaries() {
    let started = std::time::Instant::now();
    let maximum = "340282366920938463463374607431768211455-4294967295-18446744073709551615";
    let name =
        receipt_io::temporary_name(ProviderBootstrapReceipt::Allocation, fixture::TX, maximum)
            .unwrap();
    assert_eq!(name.len(), 122);
    assert!(name.is_ascii());
    assert_eq!(name, format!("pb-allocation-{}-{maximum}.tmp", fixture::TX));
    for kind in [
        ProviderBootstrapReceipt::Allocation,
        ProviderBootstrapReceipt::Prepared,
        ProviderBootstrapReceipt::Abandoned,
    ] {
        assert!(receipt_io::temporary_name(kind, fixture::TX, "1-2-3").is_ok());
        for nonce in [
            "",
            "1-2",
            "1-2-3-4",
            "１-2-3",
            "1/2-2-3",
            r"1\2-2-3",
            "-2-3",
            "3402823669209384634633746074317682114550-2-3",
            "1-42949672950-3",
            "1-2-184467440737095516150",
        ] {
            let cause = receipt_io::temporary_name(kind, fixture::TX, nonce).unwrap_err();
            assert!(
                matches!(cause,Cause::Validation(e) if matches!(e.as_ref(),ProjectionStoreError::Invalid(s) if s=="provider receipt nonce shape differs"))
            );
        }
    }
    // With valid bounded components, a 123-byte allocation name is unreachable.
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}
#[test]
fn writer_close_observer_failure_order() {
    use std::{error::Error, fmt, sync::Arc};
    #[derive(Debug)]
    struct Original(Arc<()>);
    impl fmt::Display for Original {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("synthetic DATA identity")
        }
    }
    impl Error for Original {}
    fn cause(id: &Arc<()>) -> Cause {
        Cause::Io(std::io::Error::other(Original(id.clone())))
    }
    fn same(c: &Cause, id: &Arc<()>) {
        match c {
            Cause::Io(e) => assert!(Arc::ptr_eq(
                &e.get_ref().unwrap().downcast_ref::<Original>().unwrap().0,
                id
            )),
            _ => panic!("lost owned original {c:?}"),
        }
    }
    let started = std::time::Instant::now();
    let primary = Arc::new(());
    let before = Arc::new(());
    let after = Arc::new(());
    let close = Arc::new(());
    let e = database_io::writer_close_result::<()>(
        Err(cause(&primary)),
        Some(cause(&before)),
        Some(cause(&after)),
        Some(cause(&close)),
    )
    .unwrap_err();
    let Cause::Cleanup {
        primary: p,
        close: c,
    } = e
    else {
        panic!("actual close must remain outer cause")
    };
    same(&c, &close);
    let Cause::Cleanup {
        primary: p,
        close: c,
    } = *p
    else {
        panic!("after observation missing")
    };
    same(&c, &after);
    let Cause::Cleanup {
        primary: p,
        close: c,
    } = *p
    else {
        panic!("before observation missing")
    };
    same(&c, &before);
    same(&p, &primary);
    same(
        &database_io::writer_close_result::<()>(Err(cause(&primary)), None, None, None)
            .unwrap_err(),
        &primary,
    );
    assert_eq!(
        database_io::writer_close_result(Ok(7), None, None, None).unwrap(),
        7
    );
    same(
        &database_io::writer_close_result::<()>(Ok(()), Some(cause(&before)), None, None)
            .unwrap_err(),
        &before,
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}
#[test]
fn cut_ack_validation_rejects_foreign_or_oversized_fields() {
    let started = std::time::Instant::now();
    process::pure_ack_cases();
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn writer_adapters_preserve_nested_cleanup_history() {
    use recovery::{
        ProviderBootstrapFailureKind as Kind, ProviderRecoveryEffect,
        ProviderRecoveryEffectState as RecoveryState,
    };
    use std::{error::Error, sync::Arc};
    fn hot(label: &str) -> Arc<rusqlite::Error> {
        Arc::new(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY_ROLLBACK),
            Some(format!("synthetic DATA original {label}")),
        ))
    }
    fn nested(cause: &Cause) -> &ProviderBootstrapFailure {
        match cause {
            Cause::Cleanup { primary, .. } => nested(primary),
            Cause::Validation(e) => match e.as_ref() {
                ProjectionStoreError::ProviderBootstrap(f) => f,
                _ => panic!("nested typed failure missing"),
            },
            _ => panic!("wrong primary chain"),
        }
    }
    fn owned_nodes(c: &Cause, nodes: &mut Vec<*const Cause>) {
        if let Cause::Cleanup { primary, close } = c {
            nodes.push(primary.as_ref() as *const Cause);
            nodes.push(close.as_ref() as *const Cause);
            owned_nodes(primary, nodes);
            owned_nodes(close, nodes);
        }
    }
    let started = std::time::Instant::now();
    hooks::reset();
    let family = hot("family");
    let private = hot("private");
    let explicit = hot("explicit family shortcut");
    for callback in [false, true] {
        for wrapping in 0..4 {
            for case in 0..5 {
                let mut inner = failure::<()>(Err(recovery::writer_failure(
                    "nested historical operation",
                    Cause::ReadonlyRollback(family.clone()),
                    Some(ProviderDatabaseScope::FamilyCatalog),
                    None,
                    None,
                )));
                let second = failure::<()>(Err(recovery::writer_failure(
                    "other original",
                    Cause::ReadonlyRollback(private.clone()),
                    Some(ProviderDatabaseScope::PrivateMetadata),
                    None,
                    None,
                )));
                if case != 1 {
                    inner.rollback_triggers.extend(second.rollback_triggers);
                }
                inner.database = match case {
                    1 => Some(ProviderDatabaseScope::PrivateMetadata),
                    2 | 4 => None,
                    _ => Some(ProviderDatabaseScope::FamilyCatalog),
                };
                inner.readonly_rollback = if case == 0 || case == 4 {
                    Some(family.clone())
                } else {
                    None
                };
                inner.kind = Kind::BindingChanged;
                inner.effects = vec![
                    ProviderRecoveryEffect {
                        database: ProviderDatabaseScope::FamilyCatalog,
                        state: RecoveryState::Completed,
                    },
                    ProviderRecoveryEffect {
                        database: ProviderDatabaseScope::PrivateMetadata,
                        state: RecoveryState::Uncertain,
                    },
                ];
                inner.write_report =
                    WriteContext::current(ProviderBootstrapWriterOperation::Abandon, attempt())
                        .report;
                let expected_scope = inner.database;
                let primary = Cause::Validation(Box::new(ProjectionStoreError::ProviderBootstrap(
                    Box::new(inner),
                )));
                let cause = database_io::writer_close_result::<()>(
                    Err(primary),
                    (wrapping == 1 || wrapping == 3)
                        .then(|| Cause::Io(std::io::Error::other("synthetic arm fault"))),
                    (wrapping == 3)
                        .then(|| Cause::Io(std::io::Error::other("synthetic confirmation fault"))),
                    (wrapping == 2 || wrapping == 3).then(|| {
                        Cause::Sqlite(rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                            Some("synthetic DATA close".into()),
                        ))
                    }),
                )
                .unwrap_err();
                let inner_address = nested(&cause) as *const ProviderBootstrapFailure;
                let mut original_nodes = Vec::new();
                owned_nodes(&cause, &mut original_nodes);
                let mut current = WriteContext::current(
                    ProviderBootstrapWriterOperation::ResumeControlPublication,
                    attempt(),
                );
                let step =
                    ProviderBootstrapWriteStep::SqlClose(ProviderBootstrapSqlWrite::PrepareControl);
                let index = current
                    .arm(step, Path::new(r"C:\writer-test\current-close"))
                    .unwrap();
                current.confirm(index).unwrap();
                let report = if case == 1 {
                    None
                } else {
                    current.report.clone()
                };
                let supplied = (!callback && case == 0).then(|| explicit.clone());
                let mut direct = Some(cause);
                let original = if callback {
                    Some(Arc::new(direct.take().unwrap()))
                } else {
                    None
                };
                let result = if let Some(original) = &original {
                    let outer = layrs_vfs::Error::ControlDirectory {
                        kind: layrs_vfs::ControlDirectoryFailure::NeedsRepair,
                        point: layrs_vfs::ControlDirectoryPublishPoint::PreRename,
                        operation: "synthetic publisher wrapper",
                        root: PathBuf::from(r"C:\writer-test"),
                        stage: PathBuf::from(r"C:\writer-stage"),
                        cause: Box::new(layrs_vfs::Error::from(std::io::Error::other(
                            report::CallbackCause(original.clone()),
                        ))),
                    };
                    recovery::writer_callback_failure(
                        "current adapter operation",
                        outer,
                        original,
                        Some(ProviderDatabaseScope::PrivateMetadata),
                        report,
                    )
                } else {
                    recovery::writer_failure(
                        "current adapter operation",
                        direct.take().unwrap(),
                        Some(ProviderDatabaseScope::PrivateMetadata),
                        supplied.clone(),
                        report,
                    )
                };
                let f = failure::<()>(Err(result));
                assert_eq!(f.operation, "current adapter operation");
                assert_eq!(f.database, expected_scope);
                assert_eq!(
                    f.kind,
                    if wrapping == 0 {
                        Kind::BindingChanged
                    } else {
                        Kind::ObservableCloseFailure
                    }
                );
                assert_eq!(
                    f.effects
                        .iter()
                        .map(|e| (e.database, e.state))
                        .collect::<Vec<_>>(),
                    vec![
                        (
                            ProviderDatabaseScope::FamilyCatalog,
                            RecoveryState::Completed
                        ),
                        (
                            ProviderDatabaseScope::PrivateMetadata,
                            RecoveryState::Uncertain
                        )
                    ]
                );
                assert_eq!(f.rollback_triggers.len(), if case == 1 { 1 } else { 2 });
                assert_eq!(
                    f.rollback_triggers[0].database,
                    ProviderDatabaseScope::FamilyCatalog
                );
                assert!(std::ptr::eq(
                    f.rollback_triggers[0].error(),
                    family.as_ref()
                ));
                if case != 1 {
                    assert_eq!(
                        f.rollback_triggers[1].database,
                        ProviderDatabaseScope::PrivateMetadata
                    );
                    assert!(std::ptr::eq(
                        f.rollback_triggers[1].error(),
                        private.as_ref()
                    ));
                }
                let expected = match case {
                    0 if !callback => Some(&explicit),
                    0 | 3 | 4 => Some(&family),
                    2 => Some(&private),
                    1 => None,
                    _ => unreachable!(),
                };
                match (f.readonly_rollback.as_ref(), expected) {
                    (Some(a), Some(b)) => assert!(Arc::ptr_eq(a, b)),
                    (None, None) => (),
                    _ => panic!("shortcut precedence differs"),
                }
                let retained = if let Some(original) = &original {
                    assert!(matches!(f.cause, Cause::Vfs(_)));
                    let mut next: Option<&(dyn Error + 'static)> = Some(&f.cause);
                    let mut found = false;
                    while let Some(error) = next {
                        if let Some(c) = error.downcast_ref::<Cause>() {
                            found |= std::ptr::eq(c, original.as_ref());
                        }
                        next = error.source();
                    }
                    assert!(found, "publisher must retain the original cause Arc");
                    original.as_ref()
                } else {
                    &f.cause
                };
                assert_eq!(
                    nested(retained) as *const ProviderBootstrapFailure,
                    inner_address
                );
                let mut retained_nodes = Vec::new();
                owned_nodes(retained, &mut retained_nodes);
                assert_eq!(
                    retained_nodes, original_nodes,
                    "no primary/close/reporting cause box may be replaced"
                );
                if wrapping >= 2 {
                    let Cause::Cleanup { close, .. } = retained else {
                        panic!("actual close wrapper lost")
                    };
                    assert!(
                        matches!(close.as_ref(),Cause::Sqlite(rusqlite::Error::SqliteFailure(e,_)) if e.extended_code==rusqlite::ffi::SQLITE_BUSY)
                    );
                }
                if wrapping == 1 || wrapping == 3 {
                    fn messages(c: &Cause, out: &mut Vec<String>) {
                        match c {
                            Cause::Cleanup { primary, close } => {
                                messages(primary, out);
                                messages(close, out);
                            }
                            Cause::Io(e) => out.push(e.to_string()),
                            _ => (),
                        }
                    }
                    let mut faults = Vec::new();
                    messages(retained, &mut faults);
                    assert_eq!(
                        faults,
                        if wrapping == 3 {
                            vec!["synthetic arm fault", "synthetic confirmation fault"]
                        } else {
                            vec!["synthetic arm fault"]
                        }
                    );
                }
                if case == 1 {
                    assert!(f.write_report.is_none());
                } else {
                    let actual = f.write_report.as_ref().unwrap();
                    let expected = current.report.as_ref().unwrap();
                    assert_eq!(actual.provenance, expected.provenance);
                    assert_eq!(
                        actual.attempt.transaction_id,
                        expected.attempt.transaction_id
                    );
                    assert_eq!(actual.attempt.family_root, expected.attempt.family_root);
                    assert_eq!(actual.attempt.checkout_root, expected.attempt.checkout_root);
                    assert_eq!(actual.attempt.stage_parent, expected.attempt.stage_parent);
                    assert_eq!(actual.attempt.worktree_id, expected.attempt.worktree_id);
                    assert_eq!(actual.effects.len(), 1);
                    assert_eq!(actual.effects[0].step, step);
                    assert_eq!(
                        actual.effects[0].state,
                        ProviderBootstrapWriteState::Confirmed
                    );
                    assert_eq!(actual.effects[0].target, expected.effects[0].target);
                }
            }
        }
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}
