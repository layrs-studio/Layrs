//! Real API SQLITE_BUSY close fixture; no production callback or order change.
use std::cell::Cell;
thread_local! { static BUSY_CLOSE:Cell<bool>=const{Cell::new(false)}; }
thread_local! { static BUSY_READ_CLOSE:Cell<bool>=const{Cell::new(false)}; }
pub(in crate::projection::provider) fn busy_next_writable_close() {
    BUSY_CLOSE.with(|v| v.set(true));
}
/// Arm only after the fixture's successful family writer close. The next
/// non-writable close must be the retained source reader; reference readers
/// also use this category, so the caller must establish that precise order.
pub(in crate::projection::provider) fn busy_next_read_close() {
    BUSY_READ_CLOSE.with(|v| v.set(true));
}
pub(super) fn before_close(connection: &rusqlite::Connection, writable: bool) {
    let armed = if writable {
        BUSY_CLOSE.with(|v| v.replace(false))
    } else {
        BUSY_READ_CLOSE.with(|v| v.replace(false))
    };
    if !armed {
        return;
    }
    let mut statement = std::ptr::null_mut();
    let code = unsafe {
        rusqlite::ffi::sqlite3_prepare_v2(
            connection.handle(),
            c"SELECT 1".as_ptr(),
            -1,
            &mut statement,
            std::ptr::null_mut(),
        )
    };
    assert_eq!(code, rusqlite::ffi::SQLITE_OK);
    assert!(!statement.is_null());
    // Deliberately retained until this owned child exits. The real explicit
    // sqlite3_close reports BUSY; production ownership quarantines that handle.
}
