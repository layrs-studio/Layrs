//! No-schema SQLite opener. No PRAGMA, prepare, or busy-timeout hook precedes geometry.
use super::super::recovery::error::{ProviderBootstrapCause as Cause, Result, invalid};
use super::super::{journal::ReplayGeometry, process_profile};
use rusqlite::{Connection, ffi};
use std::{
    ffi::{CStr, CString},
    path::{Component, Path, Prefix},
    ptr,
    sync::Mutex,
};

// A failed observable sqlite3_close leaves ownership here for the process
// lifetime. We never retry in Drop or report SQLITE_OK as native xClose proof.
static QUARANTINED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

pub(in crate::projection::provider) struct BareConnection {
    connection: Option<Connection>,
    pub(in crate::projection::provider) geometry: Option<ReplayGeometry>,
    #[cfg(test)]
    writable_file: bool,
}

impl BareConnection {
    pub(in crate::projection::provider) fn open(path: &Path, write: bool) -> Result<Self> {
        let filename = native_filename(path)?;
        Self::open_name(&filename, write, false)
    }

    pub(in crate::projection::provider) fn reference() -> Result<Self> {
        // Internal empty memory DB only. Never accept this name from a locator.
        Self::open_name(c":memory:", true, true)
    }

    fn open_name(name: &CStr, write: bool, reference: bool) -> Result<Self> {
        let baseline = process_profile::gate()?;
        let mut raw = ptr::null_mut();
        let flags = (if write {
            ffi::SQLITE_OPEN_READWRITE
        } else {
            ffi::SQLITE_OPEN_READONLY
        }) | ffi::SQLITE_OPEN_PRIVATECACHE
            | ffi::SQLITE_OPEN_FULLMUTEX
            | ffi::SQLITE_OPEN_EXRESCODE;
        // SAFETY: terminated native filename/literal VFS, live out pointer. The
        // audited host excludes callbacks/configuration that can perform SQL in
        // open. SQLite itself already reads the header during a successful open.
        let rc = unsafe { ffi::sqlite3_open_v2(name.as_ptr(), &mut raw, flags, c"win32".as_ptr()) };
        if rc != ffi::SQLITE_OK {
            let cause = sqlite_error(raw, rc); // Capture BEFORE cleanup changes diagnostics.
            return Err(close_raw::<()>(raw, Err(cause)).unwrap_err());
        }
        if raw.is_null() {
            return Err(invalid("successful SQLite open returned no handle"));
        }
        // No SQLite statements or setup pragmas in from_handle_owned (rusqlite0.32.1).
        let connection = match unsafe { Connection::from_handle_owned(raw) } {
            Ok(c) => c,
            Err(e) => return Err(close_raw::<()>(raw, Err(Cause::Sqlite(e))).unwrap_err()),
        };
        let mut bare = Self {
            connection: Some(connection),
            geometry: None,
            #[cfg(test)]
            writable_file: write && !reference,
        };
        let checked = bare.capture_geometry(baseline.vfs_address, reference);
        match checked {
            Ok(geometry) => {
                bare.geometry = geometry;
                Ok(bare)
            }
            Err(e) => bare.finish(Err(e)),
        }
    }

    fn capture_geometry(
        &self,
        expected_vfs: usize,
        reference: bool,
    ) -> Result<Option<ReplayGeometry>> {
        // Every raw pointer stays within this exclusive live connection's scope.
        let raw = unsafe { self.connection().handle() };
        let mut vfs: *mut ffi::sqlite3_vfs = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3_file_control(
                raw,
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_VFS_POINTER,
                (&mut vfs as *mut *mut ffi::sqlite3_vfs).cast(),
            )
        };
        if rc != ffi::SQLITE_OK {
            return Err(sqlite_error(raw, rc));
        }
        if vfs as usize != expected_vfs {
            return Err(invalid("opened SQLite VFS differs from process baseline"));
        }
        process_profile::gate()?;
        if reference {
            return Ok(None);
        }
        let mut file: *mut ffi::sqlite3_file = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3_file_control(
                raw,
                c"main".as_ptr(),
                ffi::SQLITE_FCNTL_FILE_POINTER,
                (&mut file as *mut *mut ffi::sqlite3_file).cast(),
            )
        };
        if rc != ffi::SQLITE_OK {
            return Err(sqlite_error(raw, rc));
        }
        if file.is_null() {
            return Err(invalid("SQLite database file pointer unavailable"));
        }
        let methods = unsafe { (*file).pMethods };
        if methods.is_null() {
            return Err(invalid("SQLite database file methods unavailable"));
        }
        let methods = unsafe { &*methods };
        if !(1..=3).contains(&methods.iVersion) {
            return Err(invalid("unsupported SQLite file methods version"));
        }
        let sector = methods
            .xSectorSize
            .ok_or_else(|| invalid("SQLite sector callback absent"))?;
        let characteristics = methods
            .xDeviceCharacteristics
            .ok_or_else(|| invalid("SQLite device callback absent"))?;
        let observed_sector = unsafe { sector(file) };
        let powersafe =
            unsafe { characteristics(file) } & ffi::SQLITE_IOCAP_POWERSAFE_OVERWRITE != 0;
        let geometry = ReplayGeometry::from_observed_device(observed_sector, powersafe);
        // setSectorSize clamps/falls back but never rounds intermediate values.
        let effective = if powersafe || observed_sector < 32 {
            512
        } else {
            observed_sector.min(65536) as u32
        };
        if !effective.is_power_of_two() {
            return Err(invalid(
                "unsupported non-power-of-two SQLite replay geometry",
            ));
        }
        Ok(Some(geometry))
    }

    pub(in crate::projection::provider) fn connection(&self) -> &Connection {
        // The sole caller consumes finish; there is no public/raw ownership escape.
        self.connection
            .as_ref()
            .expect("live provider connection ownership")
    }

    pub(in crate::projection::provider) fn finish<T>(mut self, result: Result<T>) -> Result<T> {
        let Some(c) = self.connection.take() else {
            return Err(invalid("provider connection already closed"));
        };
        #[cfg(test)]
        super::test_hooks::before_close(&c, self.writable_file);
        match c.close() {
            Ok(()) => result,
            Err((c, error)) => {
                let raw = unsafe { c.handle() };
                std::mem::forget(c);
                quarantine(raw);
                let close = Cause::Sqlite(error);
                Err(match result {
                    Ok(_) => Cause::Cleanup {
                        primary: Box::new(invalid("observable SQLite close failed")),
                        close: Box::new(close),
                    },
                    Err(primary) => Cause::Cleanup {
                        primary: Box::new(primary),
                        close: Box::new(close),
                    },
                })
            }
        }
    }
}
impl Drop for BareConnection {
    fn drop(&mut self) {
        // Unwinding cannot carry a close result. Keep ownership and disable later
        // provider opens rather than invoke rusqlite's implicit retry/drop path.
        if let Some(c) = self.connection.take() {
            let raw = unsafe { c.handle() };
            std::mem::forget(c);
            quarantine(raw);
        }
    }
}

fn quarantine(raw: *mut ffi::sqlite3) {
    process_profile::invalidate();
    if let Ok(mut held) = QUARANTINED.lock() {
        held.push(raw as usize);
    }
}

fn close_raw<T>(raw: *mut ffi::sqlite3, primary: Result<T>) -> Result<T> {
    if raw.is_null() {
        return primary;
    }
    let rc = unsafe { ffi::sqlite3_close(raw) };
    if rc == ffi::SQLITE_OK {
        return primary;
    }
    let close = sqlite_error(raw, rc);
    quarantine(raw);
    Err(Cause::Cleanup {
        primary: Box::new(
            primary
                .err()
                .unwrap_or_else(|| invalid("observable SQLite close failed")),
        ),
        close: Box::new(close),
    })
}

fn sqlite_error(raw: *mut ffi::sqlite3, rc: i32) -> Cause {
    let message = unsafe {
        CStr::from_ptr(if raw.is_null() {
            ffi::sqlite3_errstr(rc)
        } else {
            ffi::sqlite3_errmsg(raw)
        })
    }
    .to_string_lossy()
    .into_owned();
    Cause::Sqlite(rusqlite::Error::SqliteFailure(
        ffi::Error::new(rc),
        Some(message),
    ))
}

pub(super) fn native_filename(path: &Path) -> Result<CString> {
    let text = path
        .to_str()
        .ok_or_else(|| invalid("provider database path is not Unicode"))?;
    let mut components = path.components();
    let drive = matches!(components.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)));
    let native = text.strip_prefix("\\\\?\\").unwrap_or(text);
    if !drive
        || !matches!(components.next(), Some(Component::RootDir))
        || components.any(|c| !matches!(c, Component::Normal(_)))
        || text.len() > 4096
        || native.contains(['?', '#'])
        || native.get(2..).is_some_and(|s| s.contains(':'))
    {
        return Err(invalid(
            "provider SQLite requires an exact absolute local native filename",
        ));
    }
    CString::new(text).map_err(|_| invalid("provider database path contains NUL"))
}

/// Operation-local observer used only by the bootstrap writer. Returning faults
/// are accumulated; neither callback can veto the one mandatory actual close.
pub(in crate::projection::provider) trait WriterCloseObserver {
    fn before_close(&mut self, raw: *mut ffi::sqlite3) -> Option<Cause>;
    fn after_close(&mut self, succeeded: bool) -> Option<Cause>;
    #[cfg(test)]
    fn open_point(&mut self, _: WriterOpenPoint, _: *mut ffi::sqlite3) -> Result<()> {
        Ok(())
    }
}
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::projection::provider) enum WriterOpenPoint {
    BeforeRaw,
    AfterRaw,
    BeforeAdopt,
    BeforeGeometry,
}

// Observation faults are secondary to the original operation. If the native
// close fails, that actual cause is the OUTER Cleanup.close; observation faults
// remain nested on its primary side. No error is reconstructed from its text.
pub(in crate::projection::provider) fn writer_close_result<T>(
    result: Result<T>,
    before: Option<Cause>,
    after: Option<Cause>,
    close: Option<Cause>,
) -> Result<T> {
    let mut result = result;
    for fault in [before, after].into_iter().flatten() {
        result = Err(match result {
            Ok(_) => fault,
            Err(primary) => Cause::Cleanup {
                primary: Box::new(primary),
                close: Box::new(fault),
            },
        });
    }
    match close {
        None => result,
        Some(close) => Err(Cause::Cleanup {
            primary: Box::new(
                result
                    .err()
                    .unwrap_or_else(|| invalid("observable SQLite close failed")),
            ),
            close: Box::new(close),
        }),
    }
}
fn close_raw_writer<T>(
    raw: *mut ffi::sqlite3,
    result: Result<T>,
    observed: &mut impl WriterCloseObserver,
) -> Result<T> {
    if raw.is_null() {
        return result;
    }
    let before = observed.before_close(raw);
    let rc = unsafe { ffi::sqlite3_close(raw) };
    let close = if rc == ffi::SQLITE_OK {
        None
    } else {
        let cause = sqlite_error(raw, rc);
        quarantine(raw);
        Some(cause)
    };
    let after = observed.after_close(close.is_none());
    writer_close_result(result, before, after, close)
}
impl BareConnection {
    /// Writer companion; the legacy open_name/reference/read/recovery paths above
    /// remain unchanged. No SQL precedes geometry and no CREATE/URI flag is added.
    pub(in crate::projection::provider) fn open_writer(
        path: &Path,
        observed: &mut impl WriterCloseObserver,
    ) -> Result<Self> {
        let name = native_filename(path)?;
        let baseline = process_profile::gate()?;
        #[cfg(test)]
        observed.open_point(WriterOpenPoint::BeforeRaw, ptr::null_mut())?;
        let mut raw = ptr::null_mut();
        let flags = ffi::SQLITE_OPEN_READWRITE
            | ffi::SQLITE_OPEN_PRIVATECACHE
            | ffi::SQLITE_OPEN_FULLMUTEX
            | ffi::SQLITE_OPEN_EXRESCODE;
        let rc = unsafe { ffi::sqlite3_open_v2(name.as_ptr(), &mut raw, flags, c"win32".as_ptr()) };
        if rc != ffi::SQLITE_OK {
            let cause = sqlite_error(raw, rc); // BEFORE cleanup or observation.
            return close_raw_writer(raw, Err(cause), observed);
        }
        if raw.is_null() {
            return Err(invalid("successful SQLite open returned no handle"));
        }
        #[cfg(test)]
        for point in [WriterOpenPoint::AfterRaw, WriterOpenPoint::BeforeAdopt] {
            if let Err(cause) = observed.open_point(point, raw) {
                return close_raw_writer(raw, Err(cause), observed);
            }
        }
        let connection = match unsafe { Connection::from_handle_owned(raw) } {
            Ok(c) => c,
            Err(cause) => return close_raw_writer(raw, Err(Cause::Sqlite(cause)), observed),
        };
        let mut bare = Self {
            connection: Some(connection),
            geometry: None,
            #[cfg(test)]
            writable_file: true,
        };
        #[cfg(test)]
        if let Err(cause) = observed.open_point(WriterOpenPoint::BeforeGeometry, raw) {
            return bare.finish_writer(Err(cause), observed);
        }
        match bare.capture_geometry(baseline.vfs_address, false) {
            Ok(geometry) => {
                bare.geometry = geometry;
                Ok(bare)
            }
            Err(cause) => bare.finish_writer(Err(cause), observed),
        }
    }
    /// Observes one actual owned close, even if an observer returns an error.
    pub(in crate::projection::provider) fn finish_writer<T>(
        mut self,
        result: Result<T>,
        observed: &mut impl WriterCloseObserver,
    ) -> Result<T> {
        let Some(c) = self.connection.take() else {
            return Err(invalid("provider connection already closed"));
        };
        let before = observed.before_close(unsafe { c.handle() });
        let close = match c.close() {
            Ok(()) => None,
            Err((c, error)) => {
                let raw = unsafe { c.handle() };
                std::mem::forget(c);
                quarantine(raw);
                Some(Cause::Sqlite(error))
            }
        };
        let after = observed.after_close(close.is_none());
        writer_close_result(result, before, after, close)
    }
}
