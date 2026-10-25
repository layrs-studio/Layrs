use super::super::{
    process_profile::{self, tests::child},
    recovery::ProviderBootstrapCause,
};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
mod forward;
mod interruption;
pub(in crate::projection::provider) mod support;
static OPEN_CALLBACKS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn count_open(
    _: *mut ffi::sqlite3,
    _: *mut *mut std::ffi::c_char,
    _: *const ffi::sqlite3_api_routines,
) -> i32 {
    OPEN_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    ffi::SQLITE_OK
}
fn install_owned_counter() {
    // Qualified test-only callback: atomic counting, no SQL/filesystem effects.
    // It is confined to this owned child, never installed in a product host.
    assert_eq!(
        unsafe { ffi::sqlite3_auto_extension(Some(count_open)) },
        ffi::SQLITE_OK
    );
}
fn database(path: std::path::PathBuf) -> DatabaseFile {
    let file = crate::layout::open_read_no_follow(&path).unwrap();
    DatabaseFile::open(path, &crate::projection::file::identity(&file).unwrap()).unwrap()
}

#[test]
#[ignore = "native owned audited test host and absolute LAYRS_VFS_TEST_PARENT; fixtures retained"]
fn zero_family_private_sidecars_never_open_sqlite_or_change_witnesses() {
    child::run(
        concat!(
            module_path!(),
            "::zero_family_private_sidecars_never_open_sqlite_or_change_witnesses"
        ),
        120,
        || {
            install_owned_counter();
            child::declare();
            let root = support::owned_root("zero-sidecars");
            for role in ["catalog", "projection"] {
                for suffix in ["", "-journal", "-wal", "-shm"] {
                    for nonempty in [false, true] {
                        let dir = root.join(format!(
                            "{role}-{}-{nonempty}",
                            suffix.trim_start_matches('-')
                        ));
                        std::fs::create_dir(&dir).unwrap();
                        let path = dir.join(format!("{role}.sqlite3"));
                        support::create(&path, &[]);
                        let db = database(path.clone());
                        let side = (!suffix.is_empty()).then(|| db.sidecar_path(suffix));
                        if let Some(side) = &side {
                            support::create(side, if nonempty { b"journal-evidence" } else { b"" });
                        }
                        let before = support::witness(&path);
                        let side_before = side.as_ref().map(|p| support::witness(p));
                        let opens = OPEN_CALLBACKS.load(Ordering::SeqCst);
                        let result = read(&db, |_| Ok(())).unwrap();
                        if suffix.is_empty() {
                            assert!(matches!(result, ReadState::Uninitialized));
                        } else {
                            let ReadState::Unavailable(cause) = result else {
                                panic!("sidecar must prevent zero DB connection")
                            };
                            let message = cause.to_string();
                            assert!(
                                message.contains(if suffix == "-journal" {
                                    "zero database has a sidecar"
                                } else {
                                    "WAL/SHM sidecar present"
                                }),
                                "{message}"
                            );
                        }
                        assert_eq!(
                            OPEN_CALLBACKS.load(Ordering::SeqCst),
                            opens,
                            "zero SQLite opens"
                        );
                        assert_eq!(support::witness(&path), before);
                        assert_eq!(side.as_ref().map(|p| support::witness(p)), side_before);
                    }
                }
            }
            let positive = BareConnection::reference().unwrap();
            positive.finish(Ok(())).unwrap();
            assert_eq!(
                OPEN_CALLBACKS.load(Ordering::SeqCst),
                1,
                "discriminating real callback positive control"
            );
        },
    );
}

#[test]
#[ignore = "native owned audited test host and disposable parent; DELETE header WAL/SHM witnesses retained"]
fn nonzero_delete_database_rejects_wal_shm_before_sqlite_open() {
    child::run(
        concat!(
            module_path!(),
            "::nonzero_delete_database_rejects_wal_shm_before_sqlite_open"
        ),
        120,
        || {
            install_owned_counter();
            child::declare();
            let root = support::owned_root("delete-sidecars");
            for suffix in ["-wal", "-shm"] {
                for nonempty in [false, true] {
                    let path = root.join(format!(
                        "{}-{nonempty}.sqlite3",
                        suffix.trim_start_matches('-')
                    ));
                    let c = Connection::open(&path).unwrap();
                    let mode: String = c
                        .query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))
                        .unwrap();
                    assert_eq!(mode, "delete");
                    c.execute_batch("CREATE TABLE fixture(value TEXT); INSERT INTO fixture VALUES('committed');").unwrap();
                    c.close().unwrap();
                    let db = database(path.clone());
                    assert!(
                        matches!(read(&db,|c|Ok(c.query_row("SELECT value FROM fixture",[],|r|r.get::<_,String>(0))?)).unwrap(),ReadState::Valid(value) if value=="committed")
                    );
                    let side = db.sidecar_path(suffix);
                    support::create(&side, if nonempty { b"not-adopted" } else { b"" });
                    let before = [support::witness(&path), support::witness(&side)];
                    let opens = OPEN_CALLBACKS.load(Ordering::SeqCst);
                    let ReadState::Unavailable(cause) = read(&db, |_| Ok(())).unwrap() else {
                        panic!("sidecar refusal required")
                    };
                    assert!(cause.to_string().contains("WAL/SHM sidecar present"));
                    assert_eq!(OPEN_CALLBACKS.load(Ordering::SeqCst), opens);
                    assert_eq!([support::witness(&path), support::witness(&side)], before);
                }
            }
        },
    );
}

#[test]
#[ignore = "isolated owned audited SQLite process; deliberately retains a BUSY close handle until exit"]
fn observable_close_busy_is_preserved_and_stops_later_opens() {
    child::run(
        concat!(
            module_path!(),
            "::observable_close_busy_is_preserved_and_stops_later_opens"
        ),
        60,
        || {
            install_owned_counter();
            child::declare();
            let bare = BareConnection::reference().unwrap();
            let raw = unsafe { bare.connection().handle() };
            let mut statement = std::ptr::null_mut();
            assert_eq!(
                unsafe {
                    ffi::sqlite3_prepare_v2(
                        raw,
                        c"SELECT 1".as_ptr(),
                        -1,
                        &mut statement,
                        std::ptr::null_mut(),
                    )
                },
                ffi::SQLITE_OK
            );
            assert!(!statement.is_null());
            let error = bare
                .finish::<()>(Err(invalid("original owned operation cause")))
                .unwrap_err();
            let Cause::Cleanup { primary, close } = error else {
                panic!("separate observable close error required")
            };
            assert!(
                primary
                    .to_string()
                    .contains("original owned operation cause")
            );
            assert!(
                matches!(*close,Cause::Sqlite(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::DatabaseBusy)
            );
            let opens = OPEN_CALLBACKS.load(Ordering::SeqCst);
            assert!(matches!(
                BareConnection::reference(),
                Err(ProviderBootstrapCause::Profile(
                    process_profile::ProviderSqliteProfileError::Invalidated
                ))
            ));
            assert_eq!(OPEN_CALLBACKS.load(Ordering::SeqCst), opens);
            // Deliberately no Drop retry, finalize or secondary close. The child
            // exits with quarantined SQLite ownership; this is not confirmed closure.
        },
    );
}

#[test]
fn exact_rollback_trigger_distinguishes_busy_and_generic_readonly() {
    for code in [
        ffi::SQLITE_BUSY,
        ffi::SQLITE_LOCKED,
        ffi::SQLITE_READONLY,
        ffi::SQLITE_CORRUPT,
    ] {
        assert!(!is_rollback(&rusqlite::Error::SqliteFailure(
            ffi::Error::new(code),
            None
        )));
    }
    assert!(is_rollback(&rusqlite::Error::SqliteFailure(
        ffi::Error::new(ffi::SQLITE_READONLY_ROLLBACK),
        None
    )));
}

#[test]
#[ignore = "owned audited Windows test host; real nonhot journal, sharing and SQLite BUSY controls"]
fn nonhot_journal_and_busy_or_unreadable_sidecar_are_not_hot_or_absent() {
    child::run(
        concat!(
            module_path!(),
            "::nonhot_journal_and_busy_or_unreadable_sidecar_are_not_hot_or_absent"
        ),
        120,
        || {
            use std::os::windows::fs::OpenOptionsExt;
            install_owned_counter();
            child::declare();
            let root = support::owned_root("nonhot-busy");
            let path = root.join("database.sqlite3");
            let c = Connection::open(&path).unwrap();
            c.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE fixture(value INTEGER); INSERT INTO fixture VALUES(1)").unwrap();
            let db = database(path.clone());
            let journal = db.sidecar_path("-journal");
            support::create(&journal, &[]);
            let journal_before = support::witness(&journal);
            assert!(matches!(
                read(&db, |_| Ok(())).unwrap(),
                ReadState::Valid(())
            ));
            assert_eq!(
                support::witness(&journal),
                journal_before,
                "nonhot empty journal remains"
            );
            let exclusive = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&journal)
                .unwrap();
            let opens = OPEN_CALLBACKS.load(Ordering::SeqCst);
            let ReadState::Unavailable(Cause::Io(error)) = read(&db, |_| Ok(())).unwrap() else {
                panic!("sharing cause must remain original IO")
            };
            assert_eq!(error.raw_os_error(), Some(32));
            assert_eq!(OPEN_CALLBACKS.load(Ordering::SeqCst), opens);
            drop(exclusive);
            assert_eq!(support::witness(&journal), journal_before);
            c.execute_batch("BEGIN EXCLUSIVE").unwrap();
            let result = read(&db, |_| Ok(())).unwrap();
            let ReadState::Unavailable(Cause::Sqlite(rusqlite::Error::SqliteFailure(code, _))) =
                result
            else {
                panic!("SQLite BUSY must remain unavailable, never hot")
            };
            assert_eq!(code.code, rusqlite::ErrorCode::DatabaseBusy);
            c.execute_batch("ROLLBACK").unwrap();
            c.close().unwrap();
            assert!(matches!(
                read(&db, |_| Ok(())).unwrap(),
                ReadState::Valid(())
            ));
        },
    );
}

#[test]
#[ignore = "owned audited Windows test host; actual junction and missing-parent refusal, fixtures retained"]
fn reparse_sidecar_and_missing_parent_are_not_absence() {
    child::run(
        concat!(
            module_path!(),
            "::reparse_sidecar_and_missing_parent_are_not_absence"
        ),
        120,
        || {
            install_owned_counter();
            child::declare();
            let root = support::owned_root("reparse-sidecar");
            let path = root.join("database.sqlite3");
            let c = Connection::open(&path).unwrap();
            c.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE fixture(value INTEGER); INSERT INTO fixture VALUES(1)").unwrap();
            c.close().unwrap();
            let db = database(path.clone());
            assert!(matches!(
                read(&db, |_| Ok(())).unwrap(),
                ReadState::Valid(())
            ));
            let target = root.join("junction-target");
            std::fs::create_dir(&target).unwrap();
            let payload = target.join("preserved.txt");
            support::create(&payload, b"unrelated owned witness");
            support::junction(&root, &db.sidecar_path("-journal"), &target);
            let before = [support::witness(&path), support::witness(&payload)];
            let opens = OPEN_CALLBACKS.load(Ordering::SeqCst);
            let ReadState::Unavailable(Cause::Io(error)) = read(&db, |_| Ok(())).unwrap() else {
                panic!("nofollow reparse refusal must retain IO cause")
            };
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(error.to_string().contains("symlink or reparse point"));
            assert_eq!(OPEN_CALLBACKS.load(Ordering::SeqCst), opens);
            assert_eq!(
                [support::witness(&path), support::witness(&payload)],
                before
            );
            let missing = DatabaseFile::open(root.join("never-created/database.sqlite3"), "1:0:1");
            let Err(Cause::Io(error)) = missing else {
                panic!("missing parent must fail construction, not become an absent sidecar")
            };
            // PinnedDirectory traversal preserves typed NotFound and path text,
            // but wraps away raw Win32 codes. This is construction failure,
            // never the separate exact raw2 sidecar-absence policy.
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
            assert_eq!(error.raw_os_error(), None);
            assert!(error.to_string().contains("never-created"), "{error:?}");
            assert_eq!(OPEN_CALLBACKS.load(Ordering::SeqCst), opens);
            assert_eq!(
                [support::witness(&path), support::witness(&payload)],
                before
            );
        },
    );
}

#[test]
fn post_pager_validation_distinguishes_contradiction_from_unavailable() {
    assert!(matches!(
        classify_sql_failure::<()>(
            Cause::Sqlite(rusqlite::Error::SqliteFailure(
                ffi::Error::new(ffi::SQLITE_CORRUPT),
                None
            )),
            false
        ),
        ReadState::Rejected {
            pager_read: false,
            ..
        }
    ));
    assert!(matches!(
        validation_failure::<()>(invalid("known schema mismatch")),
        ReadState::Rejected {
            pager_read: true,
            ..
        }
    ));
    for code in [
        ffi::SQLITE_BUSY,
        ffi::SQLITE_LOCKED,
        ffi::SQLITE_IOERR,
        ffi::SQLITE_NOMEM,
    ] {
        let error = rusqlite::Error::SqliteFailure(ffi::Error::new(code), Some("original".into()));
        assert!(matches!(
            validation_failure::<()>(Cause::Sqlite(error)),
            ReadState::ReadButUnavailable(_)
        ));
    }
    assert!(matches!(
        validation_failure::<()>(Cause::Io(std::io::Error::from_raw_os_error(32))),
        ReadState::ReadButUnavailable(_)
    ));
    assert!(matches!(
        validation_failure::<()>(Cause::Profile(
            process_profile::ProviderSqliteProfileError::Drift
        )),
        ReadState::ReadButUnavailable(_)
    ));
    assert!(matches!(
        validation_failure::<()>(Cause::Sqlite(rusqlite::Error::QueryReturnedNoRows)),
        ReadState::Rejected {
            pager_read: true,
            ..
        }
    ));
}

// Writer qualification reuses the reviewed child-only VFS and interruption
// implementation. This bridge neither registers a VFS nor declares a host until
// an explicitly selected owned recipe calls the corresponding helper.
pub(in crate::projection::provider) mod writer_support {
    use super::*;
    use std::{
        cell::RefCell,
        path::{Path, PathBuf},
    };
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(in crate::projection::provider) enum AdmissionPoint {
        ReadonlyActive,
        ReadonlyClosed,
        BeforeWritableOpen,
    }
    type Observer = Box<dyn FnMut(&Path, AdmissionPoint) -> Result<()>>;
    thread_local! { static ADMISSION: RefCell<Option<Observer>> = RefCell::new(None); }
    pub(in crate::projection::provider) fn observe(
        observer: impl FnMut(&Path, AdmissionPoint) -> Result<()> + 'static,
    ) {
        ADMISSION.with(|s| *s.borrow_mut() = Some(Box::new(observer)));
    }
    pub(in crate::projection::provider) fn clear_observer() {
        ADMISSION.with(|s| *s.borrow_mut() = None);
    }
    pub(in crate::projection::provider) fn admission(
        path: &Path,
        point: AdmissionPoint,
    ) -> Result<()> {
        ADMISSION.with(|s| match s.borrow_mut().as_mut() {
            Some(f) => f(path, point),
            None => Ok(()),
        })
    }
    pub(in crate::projection::provider) fn install(path: &Path) {
        forward::install(path);
    }
    pub(in crate::projection::provider) fn install_open_counter() {
        super::install_owned_counter();
    }
    pub(in crate::projection::provider) fn open_callbacks() -> usize {
        super::OPEN_CALLBACKS.load(Ordering::SeqCst)
    }
    pub(in crate::projection::provider) fn events() -> Vec<(String, String, i32)> {
        forward::events()
    }
    pub(in crate::projection::provider) fn writable_opens() -> usize {
        forward::RW_OPENS.load(Ordering::SeqCst)
    }
    pub(in crate::projection::provider) fn writable_sql() -> usize {
        forward::WATCHED_RW_SQL.load(Ordering::SeqCst)
    }
    pub(in crate::projection::provider) fn drift_geometry() {
        forward::drift_geometry();
    }
    pub(in crate::projection::provider) fn hot(
        root: &Path,
        database: &Path,
        family: bool,
    ) -> Vec<PathBuf> {
        interruption::cut(root, database, family, "delete")
    }
    pub(in crate::projection::provider) fn junction(root: &Path, link: &Path, target: &Path) {
        support::junction(root, link, target);
    }
}
