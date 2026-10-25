//! Real API SQLITE_BUSY close fixture; no production callback or order change.
use std::cell::Cell;
thread_local! { static BUSY_CLOSE:Cell<bool>=const{Cell::new(false)}; }
pub(in crate::projection::provider) fn busy_next_writable_close() {
    BUSY_CLOSE.with(|v| v.set(true));
}
pub(super) fn before_close(connection: &rusqlite::Connection, writable: bool) {
    if !writable || !BUSY_CLOSE.with(|v| v.replace(false)) {
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
