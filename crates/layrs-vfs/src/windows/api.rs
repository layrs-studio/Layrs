//! CFAPI is optional at runtime. Never create a CldApi import-table dependency.
use crate::{Error, Result};
use std::{ffi::c_void, mem::transmute};
use windows_sys::{
    Win32::{
        Foundation::{FreeLibrary, HANDLE, HMODULE},
        Storage::CloudFilters::*,
        System::{
            IO::OVERLAPPED,
            LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
        },
    },
    core::{HRESULT, PCWSTR},
};

macro_rules! api {
    ($($field:ident : $name:literal ($($arg:ty),*) -> $result:ty),+ $(,)?) => {
        pub(super) struct Api {
            module: HMODULE,
            $(pub $field: unsafe extern "system" fn($($arg),*) -> $result,)+
        }
        impl Api {
            pub fn load() -> Result<Self> {
                let name: Vec<u16> = "cldapi.dll\0".encode_utf16().collect();
                // System32-only prevents provider roots or CWD from supplying code.
                let module = unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
                if module.is_null() { return Err(win32_error("LoadLibraryExW(cldapi.dll)")); }
                $(let $field = unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) };
                if $field.is_none() {
                    let error = win32_error(concat!("GetProcAddress(", $name, ")"));
                    unsafe { FreeLibrary(module); }
                    return Err(error);
                }
                let $field = unsafe { transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn($($arg),*) -> $result>($field.unwrap()) };)+
                Ok(Self { module, $($field,)+ })
            }
        }
    }
}

api! {
    platform: "CfGetPlatformInfo" (*mut CF_PLATFORM_INFO) -> HRESULT,
    register: "CfRegisterSyncRoot" (PCWSTR, *const CF_SYNC_REGISTRATION, *const CF_SYNC_POLICIES, CF_REGISTER_FLAGS) -> HRESULT,
    connect: "CfConnectSyncRoot" (PCWSTR, *const CF_CALLBACK_REGISTRATION, *const c_void, CF_CONNECT_FLAGS, *mut CF_CONNECTION_KEY) -> HRESULT,
    disconnect: "CfDisconnectSyncRoot" (CF_CONNECTION_KEY) -> HRESULT,
    create: "CfCreatePlaceholders" (PCWSTR, *mut CF_PLACEHOLDER_CREATE_INFO, u32, CF_CREATE_FLAGS, *mut u32) -> HRESULT,
    execute: "CfExecute" (*const CF_OPERATION_INFO, *mut CF_OPERATION_PARAMETERS) -> HRESULT,
    root_info: "CfGetSyncRootInfoByHandle" (HANDLE, CF_SYNC_ROOT_INFO_CLASS, *mut c_void, u32, *mut u32) -> HRESULT,
    placeholder_info: "CfGetPlaceholderInfo" (HANDLE, CF_PLACEHOLDER_INFO_CLASS, *mut c_void, u32, *mut u32) -> HRESULT,
    state: "CfGetPlaceholderStateFromAttributeTag" (u32, u32) -> CF_PLACEHOLDER_STATE,
    hydrate: "CfHydratePlaceholder" (HANDLE, i64, i64, CF_HYDRATE_FLAGS, *mut OVERLAPPED) -> HRESULT,
}

// The DLL and immutable function table live until all callbacks finish. Windows
// permits invoking these functions from any thread; no thread-local ownership.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}
impl Drop for Api {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.module);
        }
    }
}

pub(super) fn check(operation: &'static str, hresult: i32) -> Result<()> {
    if hresult < 0 {
        Err(Error::Windows { operation, hresult })
    } else {
        Ok(())
    }
}
pub(super) fn win32_error(operation: &'static str) -> Error {
    let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
    Error::Windows {
        operation,
        hresult: if code == 0 {
            0x80004005u32 as i32
        } else {
            (0x80070000 | (code & 0xffff)) as i32
        },
    }
}
