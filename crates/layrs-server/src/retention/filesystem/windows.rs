//! Directory-relative, read-only NT handles and bounded handle enumeration.
use super::{AuditError, Result};
use std::{
    ffi::{OsStr, OsString, c_void},
    fs::File,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, RawHandle},
    },
    ptr,
};
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: RawHandle,
    object_name: *mut UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_quality_of_service: *mut c_void,
}
#[repr(C)]
struct IoStatusBlock {
    status: isize,
    information: usize,
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtCreateFile(
        file_handle: *mut RawHandle,
        desired_access: u32,
        object_attributes: *mut ObjectAttributes,
        io_status_block: *mut IoStatusBlock,
        allocation_size: *mut i64,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        ea_buffer: *mut c_void,
        ea_length: u32,
    ) -> i32;
}
#[link(name = "Kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandleEx(
        handle: RawHandle,
        class: u32,
        information: *mut c_void,
        bytes: u32,
    ) -> i32;
}
pub(super) fn child(parent: &File, name: &OsStr) -> Result<File> {
    open_at(parent, name, 0x00100081)
}
fn open_at(parent: &File, name: &OsStr, access: u32) -> Result<File> {
    let mut wide: Vec<u16> = name.encode_wide().collect();
    if wide.is_empty() || wide.len() > 255 || wide.iter().any(|v| matches!(*v, 0 | 47 | 92)) {
        return Err(AuditError("invalid retention path component"));
    }
    let mut text = UnicodeString {
        length: (wide.len() * 2) as u16,
        maximum_length: (wide.len() * 2) as u16,
        buffer: wide.as_mut_ptr(),
    };
    let mut attributes = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root_directory: parent.as_raw_handle(),
        object_name: &mut text,
        attributes: 0x1000,
        security_descriptor: ptr::null_mut(),
        security_quality_of_service: ptr::null_mut(),
    }; // OBJ_DONT_REPARSE
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut handle = ptr::null_mut();
    // SAFETY: all buffers remain valid, one lexical component under a live
    // directory handle. FILE_OPEN only, no write/delete rights, all sharing.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &mut attributes,
            &mut status,
            ptr::null_mut(),
            0,
            7,
            1,
            0x00200020,
            ptr::null_mut(),
            0,
        )
    };
    if result < 0 {
        return Err(AuditError(
            "retention filesystem entry unavailable or reparse refused",
        ));
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}
pub(super) fn names(directory: &File, limit: usize) -> Result<Vec<OsString>> {
    let mut names = Vec::new();
    let mut first = true;
    // FILE_ID_BOTH_DIR_INFO requires 8-byte alignment; 64KiB is bounded.
    let mut storage = vec![0u64; 8192];
    loop {
        // SAFETY: live readable directory handle and aligned output buffer.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle(),
                if first { 11 } else { 10 },
                storage.as_mut_ptr().cast(),
                65536,
            )
        };
        first = false;
        if ok == 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(18) {
                break;
            }
            return Err(AuditError("retention directory enumeration unavailable"));
        }
        // SAFETY: view the initialized allocation as bytes, never beyond it.
        let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), 65536) };
        let mut offset = 0;
        loop {
            if offset + 104 > bytes.len() {
                return Err(AuditError("invalid retention directory record"));
            }
            let next = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            let length =
                u32::from_le_bytes(bytes[offset + 60..offset + 64].try_into().unwrap()) as usize;
            if length > 510 || length % 2 != 0 || offset + 104 + length > bytes.len() {
                return Err(AuditError("invalid retention filename record"));
            }
            let units: Vec<u16> = bytes[offset + 104..offset + 104 + length]
                .chunks_exact(2)
                .map(|v| u16::from_le_bytes([v[0], v[1]]))
                .collect();
            let name = OsString::from_wide(&units);
            if name != "." && name != ".." {
                if names.len() >= limit {
                    return Err(AuditError("retention filesystem entry limit exceeded"));
                }
                names.push(name);
            }
            if next == 0 {
                break;
            }
            if next < 104 + length || next % 8 != 0 {
                return Err(AuditError("invalid retention directory offset"));
            }
            offset = offset
                .checked_add(next)
                .ok_or(AuditError("retention directory offset overflow"))?;
        }
    }
    Ok(names)
}
