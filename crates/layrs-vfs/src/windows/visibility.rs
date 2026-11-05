//! Cloud placeholder visibility is thread-local ambient state, not file state.
//! Never let compatibility masking become proof that a reparse is ordinary.
use crate::{Error, Result};
use std::{marker::PhantomData, rc::Rc, sync::OnceLock};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32},
};

type SetMode = unsafe extern "system" fn(i8) -> i8;
const EXPOSE: i8 = 2;

struct Api {
    module: HMODULE,
    set: SetMode,
}
// Immutable function pointer and DLL reference; each invocation changes only
// its calling native thread. The scope below cannot move between threads.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

#[derive(Debug, Clone, Copy)]
struct LoadError {
    operation: &'static str,
    hresult: i32,
}
impl LoadError {
    fn last(operation: &'static str) -> Self {
        let Error::Windows { operation, hresult } = super::api::win32_error(operation) else {
            unreachable!()
        };
        Self { operation, hresult }
    }
}
impl Api {
    fn load() -> std::result::Result<Self, LoadError> {
        let name: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
        let module = unsafe {
            LoadLibraryExW(
                name.as_ptr(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if module.is_null() {
            return Err(LoadError::last("LoadLibraryExW(ntdll.dll)"));
        }
        let symbol = unsafe {
            GetProcAddress(
                module,
                c"RtlSetThreadPlaceholderCompatibilityMode".as_ptr().cast(),
            )
        };
        let Some(symbol) = symbol else {
            let error = LoadError::last("GetProcAddress(RtlSetThreadPlaceholderCompatibilityMode)");
            unsafe { FreeLibrary(module) };
            return Err(error);
        };
        Ok(Self {
            module,
            set: unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, SetMode>(symbol)
            },
        })
    }
}
impl Drop for Api {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.module) };
    }
}

struct Exposure {
    set: SetMode,
    previous: i8,
    active: bool,
    _same_thread: PhantomData<Rc<()>>,
}
impl Exposure {
    fn enter(set: SetMode) -> Result<Self> {
        let previous = unsafe { set(EXPOSE) };
        if previous < 0 {
            return Err(Error::PlaceholderCompatibility {
                operation: "RtlSetThreadPlaceholderCompatibilityMode(expose)",
                result: previous,
            });
        }
        Ok(Self {
            set,
            previous,
            active: true,
            _same_thread: PhantomData,
        })
    }
    fn restore(&mut self) -> Result<()> {
        let result = unsafe { (self.set)(self.previous) };
        if result < 0 {
            // Leave active: Drop makes one further attempt, but even a successful
            // retry must not turn the explicit restoration failure into success.
            return Err(Error::PlaceholderCompatibility {
                operation: "RtlSetThreadPlaceholderCompatibilityMode(restore)",
                result,
            });
        }
        self.active = false;
        Ok(())
    }
}
impl Drop for Exposure {
    fn drop(&mut self) {
        if self.active {
            // Unwind protection only. Normal returns use fallible restore().
            unsafe { (self.set)(self.previous) };
        }
    }
}

pub(super) fn with_exposed<T>(query: impl FnOnce() -> Result<T>) -> Result<T> {
    static API: OnceLock<std::result::Result<Api, LoadError>> = OnceLock::new();
    let api = API
        .get_or_init(Api::load)
        .as_ref()
        .map_err(|error| Error::Windows {
            operation: error.operation,
            hresult: error.hresult,
        })?;
    scoped(api.set, query)
}
fn scoped<T>(set: SetMode, query: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut exposure = Exposure::enter(set)?;
    let result = query();
    exposure.restore()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static MODE: Cell<i8> = const { Cell::new(1) };
        static FAIL: Cell<Option<i8>> = const { Cell::new(None) };
    }
    unsafe extern "system" fn setter(mode: i8) -> i8 {
        if FAIL.with(|fail| fail.get()) == Some(mode) {
            FAIL.with(|fail| fail.set(None));
            return -2;
        }
        MODE.with(|value| value.replace(mode))
    }

    #[test]
    fn scope_restores_exact_prior_modes_on_success_and_error() {
        for prior in [0, 1, 2] {
            MODE.with(|mode| mode.set(prior));
            let value = scoped(setter, || {
                assert_eq!(MODE.with(Cell::get), EXPOSE);
                Ok(17)
            })
            .unwrap();
            assert_eq!(value, 17);
            assert_eq!(MODE.with(Cell::get), prior);
            assert!(matches!(
                scoped::<()>(setter, || Err(Error::Busy)),
                Err(Error::Busy)
            ));
            assert_eq!(MODE.with(Cell::get), prior);
        }
    }

    #[test]
    fn scope_restores_during_unwind() {
        MODE.with(|mode| mode.set(1));
        let panic = std::panic::catch_unwind(|| {
            let _: Result<()> = scoped(setter, || panic!("query failure"));
        });
        assert!(panic.is_err());
        assert_eq!(MODE.with(Cell::get), 1);
    }

    #[test]
    fn mode_failures_are_precise_and_never_successful_evidence() {
        MODE.with(|mode| mode.set(1));
        FAIL.with(|fail| fail.set(Some(EXPOSE)));
        assert!(matches!(
            scoped::<()>(setter, || panic!("must not query")),
            Err(Error::PlaceholderCompatibility {
                operation: "RtlSetThreadPlaceholderCompatibilityMode(expose)",
                result: -2
            })
        ));
        assert_eq!(MODE.with(Cell::get), 1);
        let result = scoped(setter, || {
            FAIL.with(|fail| fail.set(Some(1)));
            Ok(())
        });
        assert!(matches!(
            result,
            Err(Error::PlaceholderCompatibility {
                operation: "RtlSetThreadPlaceholderCompatibilityMode(restore)",
                result: -2
            })
        ));
        // Drop retried successfully; the explicit error still reached the caller.
        assert_eq!(MODE.with(Cell::get), 1);
    }

    #[test]
    fn native_thread_mode_restored_after_metadata_error_and_unwind() {
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            Storage::FileSystem::{
                FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_TAG_INFO,
            },
        };
        let api = Api::load().unwrap();
        let query = unsafe {
            GetProcAddress(
                api.module,
                c"RtlQueryThreadPlaceholderCompatibilityMode"
                    .as_ptr()
                    .cast(),
            )
        }
        .unwrap();
        let query = unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn() -> i8,
            >(query)
        };
        let previous = unsafe { (api.set)(1) };
        assert!(previous >= 0);
        let mut outer = Exposure {
            set: api.set,
            previous,
            active: true,
            _same_thread: PhantomData,
        };
        let error = with_exposed(|| {
            assert_eq!(unsafe { query() }, EXPOSE);
            let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
            assert_eq!(
                unsafe {
                    GetFileInformationByHandleEx(
                        INVALID_HANDLE_VALUE,
                        FileAttributeTagInfo,
                        (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                        std::mem::size_of_val(&info) as u32,
                    )
                },
                0
            );
            Err::<(), _>(super::super::api::win32_error(
                "GetFileInformationByHandleEx(test invalid handle)",
            ))
        })
        .unwrap_err();
        assert!(matches!(error, Error::Windows { hresult, .. } if hresult == 0x80070006u32 as i32));
        assert_eq!(unsafe { query() }, 1);
        let panic = std::panic::catch_unwind(|| {
            let _: Result<()> = with_exposed(|| panic!("native scope unwind"));
        });
        assert!(panic.is_err());
        assert_eq!(unsafe { query() }, 1);
        outer.restore().unwrap();
        assert_eq!(unsafe { query() }, previous);
    }
}
