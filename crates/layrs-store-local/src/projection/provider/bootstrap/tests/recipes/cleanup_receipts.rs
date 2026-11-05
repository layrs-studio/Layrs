//! Cleanup, receipt-mutation and nested-cleanup recipe bodies of the bootstrap fault tests; the test declarations stay in the parent.
use super::*;

pub(super) fn cleanup_case(f: Rc<Fixture>, fault: hooks::WriterFault, notification: Option<bool>) {
    hooks::writer_fault(Sql::RegisterInitializing, fault);
    if let Some(after) = notification {
        hooks::observe(move |e| {
            if e.step == Step::SqlClose(Sql::RegisterInitializing) && e.after == after {
                Err(std::io::Error::other(if after {
                    "synthetic close confirmation notification"
                } else {
                    "synthetic close arm notification"
                }))
            } else {
                Ok(())
            }
        });
    }
    let baseline = writer_observer::events().len();
    let e = failure(f.begin());
    let null = fault == hooks::WriterFault::NullOpen;
    let busy = matches!(
        fault,
        hooks::WriterFault::RawBusy | hooks::WriterFault::OwnedBusy
    );
    assert_eq!(disk_rw_since(baseline), if null { 0 } else { 1 });
    let (attempts, results) = hooks::cleanup_observations();
    let terminal = hooks::cleanup_final_boundary(null);
    assert_eq!(
        writer_observer::events().len(),
        terminal.0,
        "no subsequent SQLite VFS probe after terminal open/close result"
    );
    assert_eq!(
        writer_observer::writable_sql(),
        terminal.1,
        "no subsequent watched SQL"
    );
    if null {
        assert!(attempts.is_empty() && results.is_empty());
        no_later_writes(&e, |s| matches!(s, Step::SqlClose(_)));
    } else {
        assert_eq!(attempts, vec![Sql::RegisterInitializing]);
        assert_eq!(results, vec![(Sql::RegisterInitializing, !busy)]);
        assert!(has(
            &e,
            Step::SqlClose(Sql::RegisterInitializing),
            if busy {
                State::Uncertain
            } else {
                State::Confirmed
            }
        ));
    }
    assert!(has(
        &e,
        Step::SqlOpen(Sql::RegisterInitializing),
        State::Uncertain
    ));
    assert!(has(
        &e,
        Step::ReceiptFinalize(Receipt::Allocation),
        State::Confirmed
    ));
    no_later_writes(&e, |s| {
        matches!(
            s,
            Step::SqlTransaction(_) | Step::SqlCommit(_) | Step::FormatWrite | Step::ControlPublish
        )
    });
    let original = cause_original(&e.cause);
    match fault {
        hooks::WriterFault::NullOpen => assert!(
            matches!(original,Cause::Io(err) if err.to_string()=="synthetic writer null-open routing")
        ),
        hooks::WriterFault::NonnullOpen | hooks::WriterFault::RawBusy => assert!(
            matches!(original,Cause::Io(err) if err.to_string()=="synthetic writer nonnull-open routing")
        ),
        hooks::WriterFault::Adoption => assert!(
            matches!(original,Cause::Io(err) if err.to_string()=="synthetic writer adoption routing")
        ),
        hooks::WriterFault::Profile | hooks::WriterFault::OwnedBusy => assert!(matches!(
            original,
            Cause::Profile(
                super::super::super::super::process_profile::ProviderSqliteProfileError::Invalidated
            )
        )),
        hooks::WriterFault::Geometry => invalid(
            original,
            "clean writer read-only/writable geometry differs before SQL",
        ),
    }
    if busy {
        let Cause::Cleanup { close, .. } = &e.cause else {
            panic!("actual close cause must remain separate")
        };
        assert!(
            matches!(close.as_ref(),Cause::Sqlite(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::DatabaseBusy)
        );
        assert!(matches!(
            super::super::super::super::process_profile::gate(),
            Err(super::super::super::super::process_profile::ProviderSqliteProfileError::Invalidated)
        ));
    }
    if let Some(after) = notification {
        assert!(io_fault_present(
            &e.cause,
            if after {
                "synthetic close confirmation notification"
            } else {
                "synthetic close arm notification"
            }
        ));
    }
    let close_event = hooks::events()
        .iter()
        .position(|event| event.step == Step::SqlClose(Sql::RegisterInitializing) && !event.after);
    if !null {
        assert!(close_event.is_some());
    }
    assert!(!f.checkout.join(".layrs").try_exists().unwrap());
    assert_eq!(
        fs::metadata(stages(&f).stage_identity().unwrap().stage().join("FORMAT"))
            .unwrap()
            .len(),
        0
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReceiptMutation {
    TempCollision,
    FinalCollision,
    Identity,
    Length,
    Bytes,
}

pub(super) fn receipt_mutation(f: Rc<Fixture>, kind: Receipt, mode: ReceiptMutation) {
    use std::io::{Seek, SeekFrom};
    if kind == Receipt::Abandoned {
        drop(f.published());
        hooks::reset();
    }
    let captured = Rc::new(RefCell::new(None));
    let seen = captured.clone();
    let stage = Rc::new(RefCell::new(None));
    let stage_seen = stage.clone();
    let ff = f.clone();
    hooks::observe(move |event| {
        if event.after && event.step == Step::ExclusiveCreate(Create::Format) {
            *stage_seen.borrow_mut() = Some(event.target.clone());
        }
        let selected = match mode {
            ReceiptMutation::TempCollision => {
                !event.after && event.step == Step::ExclusiveCreate(Create::ReceiptTemp(kind))
            }
            ReceiptMutation::FinalCollision => {
                !event.after && event.step == Step::ReceiptRename(kind)
            }
            _ => event.after && event.step == Step::ReceiptRename(kind),
        };
        if !selected {
            return Ok(());
        }
        assert!(seen.borrow().is_none(), "one actual mutation, no retry");
        let final_path = ff.receipt(kind);
        let parent_path = final_path.parent().unwrap();
        let mut retained = Vec::new();
        match mode {
            ReceiptMutation::TempCollision => {
                database_io::tests::support::create(
                    &event.target,
                    b"retained exact TEMP collision",
                );
                retained.push((
                    event.target.clone(),
                    database_io::tests::support::witness(&event.target),
                ));
                assert!(!final_path.try_exists().unwrap());
            }
            ReceiptMutation::FinalCollision => {
                let temporary = hooks::receipt_path(kind);
                retained.push((
                    temporary.clone(),
                    database_io::tests::support::witness(&temporary),
                ));
                database_io::tests::support::create(&final_path, b"retained exact final collision");
                retained.push((
                    final_path.clone(),
                    database_io::tests::support::witness(&final_path),
                ));
                assert_ne!(retained[0].1.0, retained[1].1.0);
            }
            ReceiptMutation::Identity | ReceiptMutation::Length | ReceiptMutation::Bytes => {
                let before = database_io::tests::support::witness(&final_path);
                let bytes = fs::read(&final_path).unwrap();
                assert!(!bytes.is_empty() && bytes.len() <= AllocationRecord::MAX_BYTES);
                let saved = parent_path.join(format!("retained-{}-original.bin", kind.names().0));
                assert!(!saved.try_exists().unwrap());
                if mode == ReceiptMutation::Identity {
                    let parent = crate::secure_fs::PinnedDirectory::open(parent_path).unwrap();
                    let mut original = parent
                        .open_file(final_path.file_name().unwrap())
                        .expect("UNQUALIFIED receipt interference handle");
                    assert!(
                        !original
                            .rename_to(&parent, saved.file_name().unwrap())
                            .expect("UNQUALIFIED no-replace receipt move; no sharing fallback")
                    );
                    assert_eq!(database_io::tests::support::witness(&saved), before);
                    let mut replacement = parent
                        .create_new_private_file(final_path.file_name().unwrap())
                        .unwrap();
                    replacement.file_mut().write_all(&bytes).unwrap();
                    replacement.file().sync_all().unwrap();
                    drop(replacement);
                    let current = database_io::tests::support::witness(&final_path);
                    assert_ne!(before.0, current.0);
                    assert_eq!((before.1, before.2.clone()), (current.1, current.2.clone()));
                } else {
                    database_io::tests::support::create(&saved, &bytes);
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .open(&final_path)
                        .unwrap();
                    if mode == ReceiptMutation::Length {
                        file.set_len(bytes.len() as u64 + 1).unwrap();
                    } else {
                        let at = bytes.len() / 2;
                        file.seek(SeekFrom::Start(at as u64)).unwrap();
                        file.write_all(&[bytes[at] ^ 1]).unwrap();
                    }
                    file.sync_all().unwrap();
                    let current = database_io::tests::support::witness(&final_path);
                    assert_eq!(current.0, before.0);
                    if mode == ReceiptMutation::Length {
                        assert_eq!(current.1, before.1 + 1);
                    } else {
                        assert_eq!(current.1, before.1);
                        assert_ne!(current.2, before.2);
                    }
                }
                retained.push((saved.clone(), database_io::tests::support::witness(&saved)));
                retained.push((
                    final_path.clone(),
                    database_io::tests::support::witness(&final_path),
                ));
                assert!(!hooks::receipt_path(kind).try_exists().unwrap());
            }
        }
        *seen.borrow_mut() = Some((retained, writer_observer::events().len()));
        Ok(()) // The production final check must actually run and reject.
    });
    let error = if kind == Receipt::Abandoned {
        let mut recovery = f.recovery();
        failure(recovery.abandon())
    } else {
        failure(f.begin())
    };
    let snapshot = captured.borrow();
    let (retained, events) = snapshot
        .as_ref()
        .expect("actual receipt collision/finalization point reached");
    for (path, witness) in retained {
        assert_eq!(*witness, database_io::tests::support::witness(path));
    }
    assert_eq!(disk_rw_since(*events), 0);
    let original = if kind == Receipt::Prepared {
        Some(hooks::take_callback_cause())
    } else {
        None
    };
    let cause = original.as_deref().unwrap_or(&error.cause);
    if let Some(original) = &original {
        use std::error::Error;
        let mut next: Option<&(dyn Error + 'static)> = Some(&error.cause);
        let mut found = false;
        while let Some(e) = next {
            if let Some(c) = e.downcast_ref::<Cause>() {
                found |= std::ptr::eq(c, original.as_ref());
            }
            next = e.source();
        }
        assert!(found, "actual callback cause identity retained");
    }
    let report = error.write_report.as_ref().unwrap();
    assert_eq!(
        report
            .effects
            .iter()
            .filter(|e| e.step == Step::ExclusiveCreate(Create::ReceiptTemp(kind)))
            .count(),
        1
    );
    match mode {
        ReceiptMutation::TempCollision => {
            assert!(matches!(cause,Cause::Io(e) if e.raw_os_error()==Some(183)));
            assert!(has(
                &error,
                Step::ExclusiveCreate(Create::ReceiptTemp(kind)),
                State::Uncertain
            ));
            no_later_writes(
                &error,
                |s| matches!(s,Step::ReceiptWrite(k)|Step::ReceiptRename(k)|Step::ReceiptFinalize(k) if k==kind),
            );
            assert!(!f.receipt(kind).try_exists().unwrap());
        }
        ReceiptMutation::FinalCollision => {
            assert!(
                matches!(cause,Cause::Io(e) if e.kind()==std::io::ErrorKind::AlreadyExists && e.to_string()=="provider authoritative receipt collision; temporary retained")
            );
            assert!(has(&error, Step::ReceiptWrite(kind), State::Confirmed));
            assert!(has(&error, Step::ReceiptRename(kind), State::NoNewEffect));
            no_later_writes(&error, |s| s == Step::ReceiptFinalize(kind));
        }
        ReceiptMutation::Identity | ReceiptMutation::Length | ReceiptMutation::Bytes => {
            invalid(
                cause,
                if mode == ReceiptMutation::Bytes {
                    "provider receipt final bytes differ"
                } else {
                    "provider receipt final identity/length differs"
                },
            );
            assert!(has(&error, Step::ReceiptWrite(kind), State::Confirmed));
            assert!(has(&error, Step::ReceiptRename(kind), State::Confirmed));
            assert!(has(&error, Step::ReceiptFinalize(kind), State::Uncertain));
        }
    }
    match kind {
        Receipt::Allocation => {
            assert_eq!(
                fs::metadata(f.metadata.join("projection.sqlite3"))
                    .unwrap()
                    .len(),
                0
            );
            assert_eq!(
                fs::metadata(stage.borrow().as_ref().expect("owned FORMAT allocation"))
                    .unwrap()
                    .len(),
                0
            );
            no_later_writes(&error, |s| {
                matches!(
                    s,
                    Step::SqlOpen(_) | Step::FormatWrite | Step::ControlPublish
                )
            });
        }
        Receipt::Prepared => {
            assert_eq!(phase(&f.metadata.join("projection.sqlite3")), "allocating");
            assert!(!f.checkout.join(".layrs").try_exists().unwrap());
            no_later_writes(&error, |s| s == Step::SqlOpen(Sql::PrepareControl));
        }
        Receipt::Abandoned => {
            assert_eq!(
                phase(&f.metadata.join("projection.sqlite3")),
                "control_published"
            );
            no_later_writes(&error, |s| s == Step::SqlOpen(Sql::RemoveInitializing));
            raw_sql(&f.family.root().join("metadata/catalog.sqlite3"), |c| {
                let state: String = c
                    .query_row(
                        "SELECT state FROM worktrees WHERE id=?1",
                        [f.worktree.as_str()],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(state, "initializing");
            });
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NestedCleanup {
    DirectSuccess,
    CallbackBusyArm,
    CallbackConfirmedFault,
}

pub(super) fn nested_primary(cause: &Cause) -> &ProviderBootstrapFailure {
    match cause {
        Cause::Cleanup { primary, .. } => nested_primary(primary),
        Cause::Validation(e) => match e.as_ref() {
            ProjectionStoreError::ProviderBootstrap(f) => f,
            _ => panic!("nested provider failure missing"),
        },
        _ => panic!("wrong nested primary cause {cause:?}"),
    }
}

pub(super) fn nested_cleanup_case(f: Rc<Fixture>, mode: NestedCleanup) {
    use recovery::ProviderBootstrapFailureKind as Kind;
    let callback = mode != NestedCleanup::DirectSuccess;
    let purpose = if callback {
        Sql::PrepareControl
    } else {
        Sql::InitializePrivate
    };
    if mode == NestedCleanup::CallbackBusyArm {
        hooks::close_busy(purpose);
    }
    let observed = Rc::new(RefCell::new(None));
    let saved = observed.clone();
    let ff = f.clone();
    let family = f.family.root().join("metadata/catalog.sqlite3");
    let db = family.clone();
    hooks::observe(move |event| {
        if event.after && event.step == Step::SqlTransaction(purpose) {
            assert!(
                saved.borrow().is_none(),
                "one actual nested family interruption"
            );
            writer_observer::hot(&ff.outer, &db, true);
            let mut journal = db.as_os_str().to_owned();
            journal.push("-journal");
            let journal = PathBuf::from(journal);
            *saved.borrow_mut() = Some((
                vec![
                    (db.clone(), database_io::tests::support::witness(&db)),
                    (
                        journal.clone(),
                        database_io::tests::support::witness(&journal),
                    ),
                ],
                writer_observer::events().len(),
            ));
        }
        if event.step == Step::SqlClose(purpose)
            && ((mode == NestedCleanup::CallbackBusyArm && !event.after)
                || (mode == NestedCleanup::CallbackConfirmedFault && event.after))
        {
            return Err(std::io::Error::other(if event.after {
                "synthetic nested close confirmation fault"
            } else {
                "synthetic nested close arm fault"
            }));
        }
        Ok(())
    });
    let error = failure(f.begin());
    let callback_cause = if callback {
        Some(hooks::take_callback_cause())
    } else {
        None
    };
    let original = callback_cause.as_deref().unwrap_or(&error.cause);
    let nested = nested_primary(original);
    let trigger = nested
        .readonly_rollback
        .as_ref()
        .expect("original family rollback trigger");
    assert!(
        matches!(trigger.as_ref(),rusqlite::Error::SqliteFailure(e,_)if e.extended_code==rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
    );
    assert_eq!(nested.database, Some(ProviderDatabaseScope::FamilyCatalog));
    assert_eq!(error.operation, "begin provider bootstrap");
    assert_eq!(error.database, Some(ProviderDatabaseScope::FamilyCatalog));
    assert_eq!(
        error.kind,
        if mode == NestedCleanup::DirectSuccess {
            Kind::NeedsRepair
        } else {
            Kind::ObservableCloseFailure
        }
    );
    assert!(error.effects.is_empty());
    assert_eq!(error.effects, nested.effects);
    assert_eq!(error.rollback_triggers.len(), 1);
    assert_eq!(
        error.rollback_triggers[0].database,
        ProviderDatabaseScope::FamilyCatalog
    );
    assert!(std::ptr::eq(
        error.rollback_triggers[0].error(),
        trigger.as_ref()
    ));
    assert!(Arc::ptr_eq(
        error.readonly_rollback.as_ref().unwrap(),
        trigger
    ));
    assert_eq!(nested.rollback_triggers.len(), 1);
    assert!(std::ptr::eq(
        nested.rollback_triggers[0].error(),
        trigger.as_ref()
    ));
    if let Some(original) = &callback_cause {
        use std::error::Error;
        assert!(matches!(error.cause, Cause::Vfs(_)));
        let mut next: Option<&(dyn Error + 'static)> = Some(&error.cause);
        let mut found = false;
        while let Some(e) = next {
            if let Some(c) = e.downcast_ref::<Cause>() {
                found |= std::ptr::eq(c, original.as_ref());
            }
            next = e.source();
        }
        assert!(found, "outer publisher preserves exact original Arc cause");
    }
    let busy = mode == NestedCleanup::CallbackBusyArm;
    assert!(has(
        &error,
        Step::SqlClose(purpose),
        if busy {
            State::Uncertain
        } else {
            State::Confirmed
        }
    ));
    let (attempts, results) = hooks::cleanup_observations();
    assert_eq!(attempts.iter().filter(|&&p| p == purpose).count(), 1);
    assert_eq!(
        results
            .iter()
            .copied()
            .filter(|(p, _)| *p == purpose)
            .collect::<Vec<_>>(),
        vec![(purpose, !busy)]
    );
    let terminal = hooks::cleanup_final_boundary(false);
    assert_eq!(writer_observer::events().len(), terminal.0);
    assert_eq!(writer_observer::writable_sql(), terminal.1);
    if busy {
        let Cause::Cleanup { close, .. } = original else {
            panic!("actual close cause missing")
        };
        assert!(
            matches!(close.as_ref(),Cause::Sqlite(rusqlite::Error::SqliteFailure(e,_))if e.extended_code==rusqlite::ffi::SQLITE_BUSY)
        );
        assert!(io_fault_present(
            original,
            "synthetic nested close arm fault"
        ));
    } else if mode == NestedCleanup::CallbackConfirmedFault {
        assert!(io_fault_present(
            original,
            "synthetic nested close confirmation fault"
        ));
    }
    let report = error.write_report.as_ref().unwrap();
    assert_eq!(
        report.provenance,
        ProviderBootstrapWriteReportProvenance::CurrentInvocation {
            operation: ProviderBootstrapWriterOperation::Begin
        }
    );
    assert_eq!(report.attempt.worktree_id, f.worktree);
    assert_eq!(report.attempt.transaction_id, fixture::TX);
    assert!(has(&error, Step::SqlTransaction(purpose), State::Confirmed));
    assert!(has(
        &error,
        Step::ReceiptFinalize(Receipt::Allocation),
        State::Confirmed
    ));
    no_later_writes(&error, |s| {
        s == Step::SqlCommit(purpose)
            || s == Step::SqlOpen(Sql::PublishControl)
            || s == Step::SqlOpen(Sql::RemoveInitializing)
    });
    assert!(!f.checkout.join(".layrs").try_exists().unwrap());
    assert!(!f.receipt(Receipt::Abandoned).try_exists().unwrap());
    if !callback {
        no_later_writes(&error, |s| {
            s == Step::FormatWrite || s == Step::ControlPublish
        });
        assert!(!f.receipt(Receipt::Prepared).try_exists().unwrap());
    }
    let saved = observed.borrow();
    let (witnesses, events) = saved.as_ref().expect("actual hot family cut reached");
    assert!(main_opens_since(*events) > 0);
    assert_eq!(disk_rw_since(*events), 0);
    for (path, before) in witnesses {
        assert_eq!(*before, database_io::tests::support::witness(path));
    }
}
