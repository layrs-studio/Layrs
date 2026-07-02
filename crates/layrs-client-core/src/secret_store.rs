use std::{error::Error, fmt};

const SERVICE_NAME: &str = "dev.layrs.studio.desktop";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretStoreStatus {
    pub available: bool,
    pub provider: &'static str,
    pub message: String,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum SecretStoreError {
    Unavailable(String),
    OperationFailed(String),
    InvalidSecret(String),
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) => write!(f, "{message}"),
            Self::OperationFailed(message) => write!(f, "{message}"),
            Self::InvalidSecret(message) => write!(f, "{message}"),
        }
    }
}

impl Error for SecretStoreError {}

pub trait SecretStore {
    fn status(&self) -> SecretStoreStatus;
    fn set_token(&self, device_id: &str, token: &str) -> Result<(), SecretStoreError>;
    fn get_token(&self, device_id: &str) -> Result<Option<String>, SecretStoreError>;
    #[allow(dead_code)]
    fn delete_token(&self, device_id: &str) -> Result<(), SecretStoreError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OsSecretStore;

impl OsSecretStore {
    pub fn new() -> Self {
        Self
    }
}

impl SecretStore for OsSecretStore {
    fn status(&self) -> SecretStoreStatus {
        platform::status()
    }

    fn set_token(&self, device_id: &str, token: &str) -> Result<(), SecretStoreError> {
        if token.trim().is_empty() {
            return Err(SecretStoreError::InvalidSecret(
                "Layrs refused to store an empty desktop token.".to_string(),
            ));
        }

        platform::set_secret(&target_name(device_id), token)
    }

    fn get_token(&self, device_id: &str) -> Result<Option<String>, SecretStoreError> {
        platform::get_secret(&target_name(device_id))
    }

    fn delete_token(&self, device_id: &str) -> Result<(), SecretStoreError> {
        platform::delete_secret(&target_name(device_id))
    }
}

fn target_name(device_id: &str) -> String {
    format!("{SERVICE_NAME}:device:{device_id}")
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{SecretStoreError, SecretStoreStatus};
    use std::{
        ffi::{c_void, OsStr},
        os::windows::ffi::OsStrExt,
        ptr, slice,
    };

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;

    #[repr(C)]
    struct FileTime {
        low_date_time: u32,
        high_date_time: u32,
    }

    #[repr(C)]
    struct CredentialW {
        flags: u32,
        type_: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: FileTime,
        credential_blob_size: u32,
        credential_blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }

    #[link(name = "Advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredReadW(
            target_name: *const u16,
            type_: u32,
            flags: u32,
            credential: *mut *mut CredentialW,
        ) -> i32;
        fn CredDeleteW(target_name: *const u16, type_: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut c_void);
        fn GetLastError() -> u32;
    }

    pub fn status() -> SecretStoreStatus {
        SecretStoreStatus {
            available: true,
            provider: "windows-credential-manager",
            message: "Windows Credential Manager is available for Layrs desktop tokens."
                .to_string(),
        }
    }

    pub fn set_secret(target_name: &str, secret: &str) -> Result<(), SecretStoreError> {
        let mut target_name = wide_null(target_name);
        let mut user_name = wide_null("Layrs Studio Desktop");
        let mut blob = secret.as_bytes().to_vec();

        if blob.len() > 2560 {
            return Err(SecretStoreError::InvalidSecret(
                "Layrs desktop token is too large for Windows Credential Manager.".to_string(),
            ));
        }

        let credential = CredentialW {
            flags: 0,
            type_: CRED_TYPE_GENERIC,
            target_name: target_name.as_mut_ptr(),
            comment: ptr::null_mut(),
            last_written: FileTime {
                low_date_time: 0,
                high_date_time: 0,
            },
            credential_blob_size: blob.len() as u32,
            credential_blob: blob.as_mut_ptr(),
            persist: CRED_PERSIST_LOCAL_MACHINE,
            attribute_count: 0,
            attributes: ptr::null_mut(),
            target_alias: ptr::null_mut(),
            user_name: user_name.as_mut_ptr(),
        };

        let ok = unsafe { CredWriteW(&credential, 0) };
        if ok == 0 {
            return Err(last_error(
                "Windows Credential Manager refused the Layrs token",
            ));
        }

        Ok(())
    }

    pub fn get_secret(target_name: &str) -> Result<Option<String>, SecretStoreError> {
        let target_name = wide_null(target_name);
        let mut credential = ptr::null_mut();

        let ok = unsafe { CredReadW(target_name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };

        if ok == 0 {
            let code = unsafe { GetLastError() };
            if code == 1168 {
                return Ok(None);
            }

            return Err(SecretStoreError::OperationFailed(format!(
                "Windows Credential Manager could not read the Layrs token (error {code})."
            )));
        }

        let result = unsafe {
            let credential_ref = &*credential;
            let bytes = slice::from_raw_parts(
                credential_ref.credential_blob,
                credential_ref.credential_blob_size as usize,
            );
            let secret = String::from_utf8(bytes.to_vec()).map_err(|_| {
                SecretStoreError::InvalidSecret(
                    "Windows Credential Manager returned a non UTF-8 Layrs token.".to_string(),
                )
            });
            CredFree(credential as *mut c_void);
            secret
        }?;

        Ok(Some(result))
    }

    pub fn delete_secret(target_name: &str) -> Result<(), SecretStoreError> {
        let target_name = wide_null(target_name);
        let ok = unsafe { CredDeleteW(target_name.as_ptr(), CRED_TYPE_GENERIC, 0) };

        if ok == 0 {
            let code = unsafe { GetLastError() };
            if code == 1168 {
                return Ok(());
            }

            return Err(SecretStoreError::OperationFailed(format!(
                "Windows Credential Manager could not delete the Layrs token (error {code})."
            )));
        }

        Ok(())
    }

    fn wide_null(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }

    fn last_error(prefix: &str) -> SecretStoreError {
        let code = unsafe { GetLastError() };
        SecretStoreError::OperationFailed(format!("{prefix} (error {code})."))
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::{SecretStoreError, SecretStoreStatus};

    pub fn status() -> SecretStoreStatus {
        SecretStoreStatus {
            available: false,
            provider: "unavailable",
            message: "No OS secret store is wired for this platform yet. Layrs Desktop will not connect until an OS-backed keyring provider is configured.".to_string(),
        }
    }

    pub fn set_secret(_target_name: &str, _secret: &str) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Unavailable(status().message))
    }

    pub fn get_secret(_target_name: &str) -> Result<Option<String>, SecretStoreError> {
        Err(SecretStoreError::Unavailable(status().message))
    }

    pub fn delete_secret(_target_name: &str) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::Unavailable(status().message))
    }
}
