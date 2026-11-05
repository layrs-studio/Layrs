//! Admission recipe bodies of the bootstrap fault tests; the test declarations stay in the parent.
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AdmissionCase {
    Hot,
    Nonhot,
    Wal,
    Shm,
    Unreadable,
    Reparse,
    Unsupported,
    Corrupt,
    PrivateHot,
    FreshSidecar,
    RemovalHot,
}

pub(super) fn exact_rollback(
    e: &ProviderBootstrapFailure,
    cause: &Cause,
    scope: ProviderDatabaseScope,
) {
    let Cause::ReadonlyRollback(original) = cause else {
        panic!("actual typed RO rollback required: {cause:?}")
    };
    assert!(
        matches!(original.as_ref(),rusqlite::Error::SqliteFailure(code,_) if code.extended_code==rusqlite::ffi::SQLITE_READONLY_ROLLBACK)
    );
    assert_eq!(e.database, Some(scope));
    assert_eq!(e.rollback_triggers.len(), 1);
    assert_eq!(e.rollback_triggers[0].database, scope);
    assert!(std::ptr::eq(
        e.rollback_triggers[0].error(),
        original.as_ref()
    ));
    assert!(std::ptr::eq(
        e.readonly_rollback.as_ref().unwrap().as_ref(),
        original.as_ref()
    ));
}

pub(super) fn junction_witness(path: &Path) -> (String, u32, u64) {
    use std::os::windows::{fs::MetadataExt, fs::OpenOptionsExt};
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x00200000 | 0x02000000)
        .open(path)
        .expect("UNQUALIFIED readonly reparse-object witness");
    let meta = file.metadata().unwrap();
    (
        crate::projection::file::identity(&file).unwrap(),
        meta.file_attributes(),
        meta.len(),
    )
}

pub(super) fn admission_case(f: Rc<Fixture>, kind: AdmissionCase) {
    use std::io::{Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    let private = matches!(
        kind,
        AdmissionCase::PrivateHot | AdmissionCase::FreshSidecar
    );
    let database = if private {
        f.metadata.join("projection.sqlite3")
    } else {
        f.family.root().join("metadata/catalog.sqlite3")
    };
    let boundary = if kind == AdmissionCase::PrivateHot {
        Step::ReceiptFinalize(Receipt::Prepared)
    } else if kind == AdmissionCase::FreshSidecar {
        Step::SqlClose(Sql::RegisterInitializing)
    } else if kind == AdmissionCase::RemovalHot {
        Step::ReceiptFinalize(Receipt::Abandoned)
    } else {
        Step::ReceiptFinalize(Receipt::Allocation)
    };
    if kind == AdmissionCase::RemovalHot {
        drop(f.published());
        hooks::reset();
    }
    let saved = Rc::new(RefCell::new(None));
    let held = Rc::new(RefCell::new(None::<std::fs::File>));
    let junction = Rc::new(RefCell::new(None));
    let seen = saved.clone();
    let hold = held.clone();
    let link_seen = junction.clone();
    let ff = f.clone();
    let db = database.clone();
    hooks::observe(move |event| {
        if event.after && event.step == boundary {
            assert!(seen.borrow().is_none(), "single admission mutation");
            let mut side = db.as_os_str().to_owned();
            side.push(match kind {
                AdmissionCase::Wal => "-wal",
                AdmissionCase::Shm => "-shm",
                _ => "-journal",
            });
            let side = PathBuf::from(side);
            match kind {
                AdmissionCase::Hot | AdmissionCase::PrivateHot | AdmissionCase::RemovalHot => {
                    writer_observer::hot(&ff.outer, &db, !private);
                }
                AdmissionCase::Nonhot | AdmissionCase::Unreadable | AdmissionCase::FreshSidecar => {
                    database_io::tests::support::create(&side, &[])
                }
                AdmissionCase::Wal | AdmissionCase::Shm => {
                    database_io::tests::support::create(&side, b"retained sidecar witness")
                }
                AdmissionCase::Reparse => {
                    let target = ff.outer.join("journal-target");
                    fs::create_dir(&target).unwrap();
                    database_io::tests::support::create(
                        &target.join("witness"),
                        b"unchanged target",
                    );
                    writer_observer::junction(&ff.outer, &side, &target);
                    *link_seen.borrow_mut() = Some((
                        side.clone(),
                        junction_witness(&side),
                        database_io::tests::support::witness(&target.join("witness")),
                    ));
                }
                AdmissionCase::Unsupported | AdmissionCase::Corrupt => {
                    let mut file = fs::OpenOptions::new().write(true).open(&db).unwrap();
                    file.seek(SeekFrom::Start(if kind == AdmissionCase::Unsupported {
                        18
                    } else {
                        100
                    }))
                    .unwrap();
                    file.write_all(if kind == AdmissionCase::Unsupported {
                        &[2, 2]
                    } else {
                        &[255]
                    })
                    .unwrap();
                    file.sync_all().unwrap();
                }
            }
            *seen.borrow_mut() = Some((
                regular_boundary(&ff, kind == AdmissionCase::Reparse),
                writer_observer::events().len(),
            ));
            if kind == AdmissionCase::Unreadable {
                *hold.borrow_mut() = Some(
                    fs::OpenOptions::new()
                        .read(true)
                        .share_mode(0)
                        .open(side)
                        .unwrap(),
                );
            }
        }
        Ok(())
    });
    let error = if kind == AdmissionCase::RemovalHot {
        let mut r = f.recovery();
        failure(r.abandon())
    } else {
        failure(f.begin())
    };
    drop(held.borrow_mut().take());
    let saved = saved.borrow();
    let (witness, events) = saved
        .as_ref()
        .expect("actual post-allocation admission boundary reached");
    assert_eq!(
        *witness,
        regular_boundary(&f, kind == AdmissionCase::Reparse)
    );
    assert_eq!(disk_rw_since(*events), 0);
    assert!(error.effects.is_empty());
    let callback = if kind == AdmissionCase::PrivateHot {
        Some(hooks::take_callback_cause())
    } else {
        None
    };
    let cause = callback.as_deref().unwrap_or(&error.cause);
    if !matches!(
        kind,
        AdmissionCase::Hot | AdmissionCase::PrivateHot | AdmissionCase::RemovalHot
    ) {
        assert!(error.readonly_rollback.is_none());
        assert!(error.rollback_triggers.is_empty());
    }
    match kind {
        AdmissionCase::Hot | AdmissionCase::PrivateHot | AdmissionCase::RemovalHot => {
            exact_rollback(
                &error,
                cause,
                if private {
                    ProviderDatabaseScope::PrivateMetadata
                } else {
                    ProviderDatabaseScope::FamilyCatalog
                },
            );
            assert!(main_opens_since(*events) > 0);
        }
        AdmissionCase::Nonhot => {
            invalid(cause, "clean provider writer refuses any rollback journal");
            assert!(main_opens_since(*events) > 0);
        }
        AdmissionCase::Wal | AdmissionCase::Shm => {
            invalid(
                cause,
                "provider database WAL/SHM sidecar present before SQLite open",
            );
            assert_eq!(main_opens_since(*events), 0);
        }
        AdmissionCase::FreshSidecar => {
            invalid(cause, "zero database has a sidecar; SQLite open forbidden");
            assert_eq!(main_opens_since(*events), 0);
        }
        AdmissionCase::Unreadable => {
            assert!(matches!(cause,Cause::Io(e) if e.raw_os_error()==Some(32)));
            assert_eq!(main_opens_since(*events), 0);
        }
        AdmissionCase::Reparse => {
            assert!(
                matches!(cause,Cause::Io(e) if e.to_string().contains("symlink or reparse point"))
            );
            assert_eq!(main_opens_since(*events), 0);
            let j = junction.borrow();
            let (path, old, target) = j.as_ref().unwrap();
            assert_eq!(*old, junction_witness(path));
            assert_eq!(
                *target,
                database_io::tests::support::witness(&f.outer.join("journal-target/witness"))
            );
        }
        AdmissionCase::Unsupported => {
            assert!(matches!(
                cause,
                Cause::Journal(
                    super::super::super::super::journal::Refusal::DatabaseVersions {
                        write: 2,
                        read: 2
                    }
                )
            ));
            assert_eq!(main_opens_since(*events), 0);
        }
        AdmissionCase::Corrupt => {
            assert!(
                matches!(cause,Cause::Sqlite(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::DatabaseCorrupt)
            );
            assert!(main_opens_since(*events) > 0);
        }
    }
    assert!(
        has(
            &error,
            Step::ReceiptFinalize(Receipt::Allocation),
            State::Confirmed
        ) || kind == AdmissionCase::RemovalHot
    );
    if kind == AdmissionCase::RemovalHot {
        assert!(has(
            &error,
            Step::ReceiptFinalize(Receipt::Abandoned),
            State::Confirmed
        ));
        no_later_writes(&error, |s| {
            matches!(s, Step::SqlOpen(Sql::RemoveInitializing))
        });
    } else if kind == AdmissionCase::PrivateHot {
        no_later_writes(&error, |s| matches!(s, Step::SqlOpen(Sql::PrepareControl)));
    } else if kind == AdmissionCase::FreshSidecar {
        no_later_writes(&error, |s| {
            matches!(s, Step::SqlOpen(Sql::InitializePrivate))
        });
    } else {
        no_later_writes(&error, |s| matches!(s, Step::SqlOpen(_)));
    }
}
