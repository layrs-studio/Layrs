//! Actual nested RO cleanup regression. Fresh owned databases only; no product
//! fixture is reopened. The BUSY variant deliberately leaves one SQLite handle
//! quarantined until its owned child exits, as the existing close fixture does.
use super::{child, database, fresh_ordinary_support::*, support};
use crate::projection::provider::{
    database_io::{self, ReadState},
    recovery::error::{ProviderBootstrapCause as Cause, Result, invalid},
};
use layrs_vfs::Cancellation;
use rusqlite::Connection;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

const PRIMARY: &str = "strict nested callback failed after its operation";

fn run(busy_close: bool) {
    child::declare();
    let root = support::owned_root("strict-nested-read");
    let paths = [root.join("source.sqlite3"), root.join("family.sqlite3")];
    for path in &paths {
        let c = Connection::open(path).unwrap();
        c.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE evidence(value INTEGER); INSERT INTO evidence VALUES(7)").unwrap();
        c.close().unwrap();
    }
    let before = paths.each_ref().map(|p| support::witness(p));
    let source = database(paths[0].clone());
    let family = database(paths[1].clone());
    let cancel = Cancellation::default();

    // Success must retain the concluding snapshot comparison, not merely skip
    // all post-close verification to make failure cases pass.
    let events = Rc::new(RefCell::new(Vec::<(PathBuf, ReadBoundary)>::new()));
    let observed = events.clone();
    let success = with_observer(
        move |p, point| {
            observed.borrow_mut().push((p.to_owned(), point));
            Ok(())
        },
        || {
            database_io::read_cancellable_strict(&source, &cancel, |b| {
                assert!(!b.is_autocommit());
                match database_io::read_cancellable_strict(&family, &cancel, |f| {
                    assert!(!f.is_autocommit());
                    Ok(f.query_row("SELECT value FROM evidence", [], |r| r.get::<_, u64>(0))?)
                })? {
                    ReadState::Valid(value) => Ok(value),
                    _ => panic!("fresh clean inner fixture must be valid"),
                }
            })
        },
    )
    .unwrap();
    assert!(matches!(success, ReadState::Valid(7)));
    for path in &paths {
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|(p, point)| p == path && *point == ReadBoundary::BeforeSnapshot)
                .count(),
            1
        );
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|(p, point)| p == path && *point == ReadBoundary::DatabaseSnapshot)
                .count(),
            2
        );
        assert!(
            events
                .borrow()
                .iter()
                .any(|(p, point)| p == path && *point == ReadBoundary::DatabaseIdentity)
        );
    }

    let failed = Rc::new(Cell::new(false));
    let watched_failed = failed.clone();
    let post_failure = Rc::new(Cell::new(0));
    let watched_calls = post_failure.clone();
    let error = with_observer(
        move |_, _| {
            if watched_failed.get() {
                watched_calls.set(watched_calls.get() + 1);
                return Err(invalid("concluding probe would mask the original failure"));
            }
            Ok(())
        },
        || -> Result<ReadState<()>> {
            database_io::read_cancellable_strict(&source, &cancel, |_| {
                database_io::read_cancellable_strict(&family, &cancel, |_| -> Result<()> {
                    if busy_close {
                        // The next real close is precisely this family scope. Its
                        // hook prepares an outstanding statement solely to induce
                        // actual SQLITE_BUSY, not an OS-close or crash simulation.
                        database_io::test_hooks::busy_next_read_close();
                    }
                    failed.set(true);
                    Err(invalid(PRIMARY))
                })?;
                panic!("a failed inner scope must never continue its parent callback")
            })
        },
    )
    .err()
    .expect("strict scopes must preserve and propagate the callback error");
    assert_eq!(
        post_failure.get(),
        0,
        "no concluding database identity/snapshot or new read entry"
    );
    if busy_close {
        match error {
            Cause::Cleanup { primary, close } => {
                assert_eq!(primary.to_string(), PRIMARY);
                assert!(
                    matches!(*close, Cause::Sqlite(rusqlite::Error::SqliteFailure(code,_)) if code.code==rusqlite::ErrorCode::DatabaseBusy)
                );
            }
            other => panic!("callback and actual close causes must both survive: {other}"),
        }
        // No further SQL/close retries: owned child exits quarantined.
    } else {
        assert_eq!(error.to_string(), PRIMARY);
        assert_eq!(paths.each_ref().map(|p| support::witness(p)), before);
        // RAII observation ended; the old interpreting API still classifies a
        // semantic contradiction after its unchanged successful cleanup.
        let old = database_io::read_cancellable::<()>(&source, &cancel, |_| Err(invalid(PRIMARY)))
            .unwrap();
        assert!(
            matches!(old,ReadState::Rejected{cause:Cause::Validation(message),pager_read:true} if message.to_string()==PRIMARY)
        );
    }
}

#[test]
#[ignore = "ROOT source release and owned audited SQL child120/outer180 required"]
fn strict_nested_read_preserves_callback_failure_without_concluding_filesystem_probes() {
    child::run(
        concat!(
            module_path!(),
            "::strict_nested_read_preserves_callback_failure_without_concluding_filesystem_probes"
        ),
        120,
        || run(false),
    );
}

#[test]
#[ignore = "ROOT source release and owned audited SQL child120/outer180; retains BUSY handle until exit"]
fn strict_nested_read_preserves_primary_and_actual_busy_close_without_concluding_probes() {
    child::run(
        concat!(
            module_path!(),
            "::strict_nested_read_preserves_primary_and_actual_busy_close_without_concluding_probes"
        ),
        120,
        || run(true),
    );
}
