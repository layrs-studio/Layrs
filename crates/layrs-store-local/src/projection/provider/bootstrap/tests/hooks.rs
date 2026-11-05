//! Test-only controlled faults/counters. Never active in a product build.
use super::*;
use std::{
    cell::RefCell,
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone)]
pub(super) struct Event {
    pub after: bool,
    pub step: ProviderBootstrapWriteStep,
    pub target: PathBuf,
}
type Observer = Box<dyn FnMut(&Event) -> std::io::Result<()>>;
#[derive(Default)]
struct State {
    callback_cause: Option<std::sync::Arc<Cause>>,
    writer_fault: Option<(ProviderBootstrapSqlWrite, WriterFault)>,
    cleanup_attempts: Vec<ProviderBootstrapSqlWrite>,
    cleanup_results: Vec<(ProviderBootstrapSqlWrite, bool)>,
    cleanup_boundary: Option<(usize, usize)>,
    null_failure_boundary: Option<(usize, usize)>,
    commit_checkpoint: Option<
        Box<
            dyn FnMut(
                bool,
                ProviderBootstrapSqlWrite,
                &rusqlite::Connection,
                &Path,
            ) -> CauseResult<()>,
        >,
    >,
    events: Vec<Event>,
    receipt_paths: Vec<(ProviderBootstrapReceipt, PathBuf)>,
    observer: Option<Observer>,
    owner_fail: bool,
    before_owner: Option<Box<dyn FnMut(&Path) -> std::io::Result<()>>>,
    prefix: Option<Option<ProviderBootstrapReceipt>>,
    prefix_ack: Option<PathBuf>,
    close_busy: Option<ProviderBootstrapSqlWrite>,
    commit_reader: Option<ProviderBootstrapSqlWrite>,
    reader: Option<rusqlite::Connection>,
    publish: Option<layrs_vfs::ControlDirectoryPublishPoint>,
    publish_observer:
        Option<Box<dyn FnMut(layrs_vfs::ControlDirectoryPublishPoint) -> std::io::Result<()>>>,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
pub(super) fn reset() {
    STATE.with(|s| *s.borrow_mut() = State::default());
}
pub(super) fn events() -> Vec<Event> {
    STATE.with(|s| s.borrow().events.clone())
}
pub(super) fn observe(action: impl FnMut(&Event) -> std::io::Result<()> + 'static) {
    STATE.with(|s| s.borrow_mut().observer = Some(Box::new(action)));
}
pub(super) fn fail_owner() {
    STATE.with(|s| s.borrow_mut().owner_fail = true);
}
pub(super) fn close_busy(purpose: ProviderBootstrapSqlWrite) {
    STATE.with(|s| s.borrow_mut().close_busy = Some(purpose));
}
pub(super) fn commit_busy(purpose: ProviderBootstrapSqlWrite) {
    STATE.with(|s| s.borrow_mut().commit_reader = Some(purpose));
}
pub(super) fn release_reader() {
    STATE.with(|s| s.borrow_mut().reader = None);
}
pub(super) fn stop_publish(point: layrs_vfs::ControlDirectoryPublishPoint) {
    STATE.with(|s| s.borrow_mut().publish = Some(point));
}
pub(super) fn observe_publish(
    action: impl FnMut(layrs_vfs::ControlDirectoryPublishPoint) -> std::io::Result<()> + 'static,
) {
    STATE.with(|s| s.borrow_mut().publish_observer = Some(Box::new(action)));
}
pub(super) fn prefix(kind: Option<ProviderBootstrapReceipt>, ack: Option<PathBuf>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.prefix = Some(kind);
        s.prefix_ack = ack;
    });
}
pub(in crate::projection::provider::bootstrap) fn point(
    after: bool,
    step: ProviderBootstrapWriteStep,
    target: &Path,
) -> std::io::Result<()> {
    let event = Event {
        after,
        step,
        target: target.to_owned(),
    };
    let mut observer = STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.events.push(event.clone());
        if after {
            if let ProviderBootstrapWriteStep::ExclusiveCreate(
                ProviderBootstrapCreate::ReceiptTemp(kind),
            ) = step
            {
                s.receipt_paths.push((kind, target.to_owned()));
            }
            if let ProviderBootstrapWriteStep::SqlOpen(purpose) = step {
                if s.commit_reader == Some(purpose) {
                    let c = rusqlite::Connection::open_with_flags(
                        target,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .map_err(std::io::Error::other)?;
                    c.execute_batch("BEGIN DEFERRED; SELECT count(*) FROM sqlite_schema;")
                        .map_err(std::io::Error::other)?;
                    s.reader = Some(c);
                }
            }
        }
        Ok::<_, std::io::Error>(s.observer.take())
    })?;
    // Release only the test RefCell borrow while the real checkpoint observer
    // reads its captured witnesses or starts an exact owned competing child.
    let result = match observer.as_mut() {
        Some(f) => f(&event),
        None => Ok(()),
    };
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.observer.is_none() {
            s.observer = observer;
        }
    });
    result
}
pub(in crate::projection::provider::bootstrap) fn owner_acquired() -> std::io::Result<()> {
    STATE.with(|s| {
        if s.borrow().owner_fail {
            Err(std::io::Error::other(
                "synthetic writer stop after owner acquisition",
            ))
        } else {
            Ok(())
        }
    })
}
pub(in crate::projection::provider::bootstrap) fn before_close(
    c: &rusqlite::Connection,
    purpose: ProviderBootstrapSqlWrite,
) {
    STATE.with(|s| {
        if s.borrow().close_busy == Some(purpose) {
            let mut statement = std::ptr::null_mut();
            let rc = unsafe {
                rusqlite::ffi::sqlite3_prepare_v2(
                    c.handle(),
                    c"SELECT 1".as_ptr(),
                    -1,
                    &mut statement,
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(rc, rusqlite::ffi::SQLITE_OK);
            assert!(!statement.is_null());
            // Deliberately retained statement makes the actual sqlite3_close BUSY.
            // Process-local quarantine owns the connection; no finalize/retry here.
        }
    });
}
fn write_prefix(
    file: &mut crate::secure_fs::PinnedFile,
    kind: Option<ProviderBootstrapReceipt>,
    bytes: &[u8],
) -> std::io::Result<()> {
    let (selected, ack) = STATE.with(|s| {
        let s = s.borrow();
        (s.prefix == Some(kind), s.prefix_ack.clone())
    });
    if !selected {
        return Ok(());
    }
    file.file_mut().write_all(&bytes[..bytes.len() / 2])?;
    file.file().sync_all()?;
    if let Some(ack) = ack {
        if let Some(kind) = kind {
            super::process::acknowledge_receipt_and_wait(&ack, kind, false, true, Some(bytes));
        } else {
            super::process::acknowledge_and_wait(&ack);
        }
    }
    Err(std::io::Error::other(
        "synthetic short writer payload after real prefix write",
    ))
}
pub(in crate::projection::provider::bootstrap) fn receipt_prefix(
    file: &mut crate::secure_fs::PinnedFile,
    kind: ProviderBootstrapReceipt,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_prefix(file, Some(kind), bytes)
}
pub(in crate::projection::provider::bootstrap) fn format_prefix(
    file: &mut crate::secure_fs::PinnedFile,
    bytes: &[u8],
) -> std::io::Result<()> {
    write_prefix(file, None, bytes)
}
pub(in crate::projection::provider::bootstrap) struct Publisher;
pub(in crate::projection::provider::bootstrap) static PUBLISHER: Publisher = Publisher;
impl layrs_vfs::ControlDirectoryPublishFaults for Publisher {
    fn at(&self, point: layrs_vfs::ControlDirectoryPublishPoint) -> std::io::Result<()> {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(observer) = s.publish_observer.as_mut() {
                observer(point)?;
            }
            if s.publish == Some(point) {
                Err(std::io::Error::other("synthetic writer publisher cut"))
            } else {
                Ok(())
            }
        })
    }
}

pub(super) fn on_before_owner(action: impl FnMut(&Path) -> std::io::Result<()> + 'static) {
    STATE.with(|s| s.borrow_mut().before_owner = Some(Box::new(action)));
}
pub(in crate::projection::provider::bootstrap) fn before_owner(path: &Path) -> std::io::Result<()> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(action) = s.before_owner.as_mut() {
            action(path)?;
        }
        Ok(())
    })
}

pub(in crate::projection::provider::bootstrap) fn callback_cause(cause: &std::sync::Arc<Cause>) {
    STATE.with(|s| s.borrow_mut().callback_cause = Some(cause.clone()));
}
pub(super) fn take_callback_cause() -> std::sync::Arc<Cause> {
    STATE.with(|s| {
        s.borrow_mut()
            .callback_cause
            .take()
            .expect("real callback cause recorded")
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WriterFault {
    NullOpen,
    NonnullOpen,
    Adoption,
    Profile,
    Geometry,
    RawBusy,
    OwnedBusy,
}
pub(super) fn writer_fault(purpose: ProviderBootstrapSqlWrite, fault: WriterFault) {
    STATE.with(|s| s.borrow_mut().writer_fault = Some((purpose, fault)));
}
pub(super) fn cleanup_observations() -> (
    Vec<ProviderBootstrapSqlWrite>,
    Vec<(ProviderBootstrapSqlWrite, bool)>,
) {
    STATE.with(|s| {
        let s = s.borrow();
        (s.cleanup_attempts.clone(), s.cleanup_results.clone())
    })
}
pub(in crate::projection::provider::bootstrap) fn writer_open_point(
    purpose: ProviderBootstrapSqlWrite,
    point: database_io::WriterOpenPoint,
    _raw: *mut rusqlite::ffi::sqlite3,
) -> CauseResult<()> {
    use database_io::WriterOpenPoint as P;
    let fault = STATE.with(|s| {
        s.borrow()
            .writer_fault
            .filter(|(p, _)| *p == purpose)
            .map(|(_, f)| f)
    });
    let message = match (fault, point) {
        (Some(WriterFault::NullOpen), P::BeforeRaw) => Some("synthetic writer null-open routing"),
        (Some(WriterFault::NonnullOpen | WriterFault::RawBusy), P::AfterRaw) => {
            Some("synthetic writer nonnull-open routing")
        }
        (Some(WriterFault::Adoption), P::BeforeAdopt) => Some("synthetic writer adoption routing"),
        (Some(WriterFault::Profile | WriterFault::OwnedBusy), P::BeforeGeometry) => {
            super::super::super::process_profile::invalidate();
            None
        }
        (Some(WriterFault::Geometry), P::BeforeRaw) => {
            database_io::tests::writer_support::drift_geometry();
            None
        }
        _ => None,
    };
    match message {
        Some(message) => {
            if point == P::BeforeRaw {
                STATE.with(|s| {
                    s.borrow_mut().null_failure_boundary = Some((
                        database_io::tests::writer_support::events().len(),
                        database_io::tests::writer_support::writable_sql(),
                    ))
                });
            }
            Err(std::io::Error::other(message).into())
        }
        None => Ok(()),
    }
}
pub(in crate::projection::provider::bootstrap) fn writer_before_close(
    raw: *mut rusqlite::ffi::sqlite3,
    purpose: ProviderBootstrapSqlWrite,
) -> CauseResult<()> {
    let busy = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.cleanup_attempts.push(purpose);
        s.close_busy == Some(purpose)
            || s.writer_fault.is_some_and(|(p, f)| {
                p == purpose && matches!(f, WriterFault::RawBusy | WriterFault::OwnedBusy)
            })
    });
    if busy {
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
        if rc != rusqlite::ffi::SQLITE_OK {
            return Err(Cause::Sqlite(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rc),
                None,
            )));
        }
        assert!(!statement.is_null());
        // Actual retained statement: the following real close must report BUSY.
        // This is deliberate test-only SQL, not a zero-SQL assertion.
    }
    Ok(())
}
pub(in crate::projection::provider::bootstrap) fn writer_close_observed(
    purpose: ProviderBootstrapSqlWrite,
    success: bool,
) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.cleanup_results.push((purpose, success));
        s.cleanup_boundary = Some((
            database_io::tests::writer_support::events().len(),
            database_io::tests::writer_support::writable_sql(),
        ));
    });
}
pub(super) fn on_sql_commit(
    action: impl FnMut(bool, ProviderBootstrapSqlWrite, &rusqlite::Connection, &Path) -> CauseResult<()>
    + 'static,
) {
    STATE.with(|s| s.borrow_mut().commit_checkpoint = Some(Box::new(action)));
}
pub(in crate::projection::provider::bootstrap) fn sql_commit_checkpoint(
    after: bool,
    purpose: ProviderBootstrapSqlWrite,
    c: &rusqlite::Connection,
    database: &Path,
) -> CauseResult<()> {
    STATE.with(|s| match s.borrow_mut().commit_checkpoint.as_mut() {
        Some(f) => f(after, purpose, c, database),
        None => Ok(()),
    })
}

pub(super) fn receipt_path(kind: ProviderBootstrapReceipt) -> PathBuf {
    STATE.with(|s| {
        s.borrow()
            .receipt_paths
            .iter()
            .rev()
            .find(|(k, _)| *k == kind)
            .expect("actual TEMP creation checkpoint")
            .1
            .clone()
    })
}

pub(super) fn cleanup_final_boundary(null: bool) -> (usize, usize) {
    STATE.with(|s| {
        let s = s.borrow();
        if null {
            s.null_failure_boundary
        } else {
            s.cleanup_boundary
        }
        .expect("actual terminal opener/close observation")
    })
}
