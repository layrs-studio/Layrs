use std::ffi::{OsStr, c_void};
use std::fs::File;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::ptr;

use super::{FileIdentity, PinnedFile};

const STATUS_OBJECT_NAME_COLLISION: i32 = 0xC000_0035_u32 as i32;
const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const FILE_OPEN_IF: u32 = 3;
const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
const FILE_SHARE_ALL: u32 = 0x0000_0007;
const FILE_ADD_FILE: u32 = 0x0000_0002;
const FILE_ADD_SUBDIRECTORY: u32 = 0x0000_0004;
const FILE_TRAVERSE: u32 = 0x0000_0020;
const FILE_READ_DATA: u32 = 0x0000_0001;
const FILE_WRITE_DATA: u32 = 0x0000_0002;
const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
const DELETE: u32 = 0x0001_0000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;
const FILE_RENAME_INFORMATION: u32 = 10;
const FILE_DISPOSITION_INFORMATION: u32 = 13;

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

#[repr(C)]
struct RenameInformation {
    replace_if_exists: u8,
    root_directory: RawHandle,
    file_name_length: u32,
    file_name: [u16; 1],
}

#[repr(C)]
struct DispositionInformation {
    delete_file: u8,
}

#[repr(C)]
struct ByHandleFileInformation {
    file_attributes: u32,
    creation_time_low: u32,
    creation_time_high: u32,
    last_access_time_low: u32,
    last_access_time_high: u32,
    last_write_time_low: u32,
    last_write_time_high: u32,
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
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
    fn NtSetInformationFile(
        file_handle: RawHandle,
        io_status_block: *mut IoStatusBlock,
        file_information: *mut c_void,
        length: u32,
        file_information_class: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[link(name = "Kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandle(
        file_handle: RawHandle,
        information: *mut ByHandleFileInformation,
    ) -> i32;
}

pub(super) fn file_identity(file: &File) -> io::Result<FileIdentity> {
    let mut information = ByHandleFileInformation {
        file_attributes: 0,
        creation_time_low: 0,
        creation_time_high: 0,
        last_access_time_low: 0,
        last_access_time_high: 0,
        last_write_time_low: 0,
        last_write_time_high: 0,
        volume_serial_number: 0,
        file_size_high: 0,
        file_size_low: 0,
        number_of_links: 0,
        file_index_high: 0,
        file_index_low: 0,
    };
    // SAFETY: `file` is live and the output structure is writable.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileIdentity {
        volume: u64::from(information.volume_serial_number),
        file: (u64::from(information.file_index_high) << 32)
            | u64::from(information.file_index_low),
    })
}

pub(super) fn open_directory_at(
    parent: &File,
    name: &OsStr,
    create: bool,
    writable: bool,
) -> io::Result<File> {
    let write_access = if writable {
        FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY
    } else {
        0
    };
    open_at(
        parent,
        name,
        FILE_TRAVERSE | write_access | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        if create { FILE_OPEN_IF } else { FILE_OPEN },
        FILE_DIRECTORY_FILE,
    )
}

pub(super) fn create_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    open_at(
        parent,
        name,
        FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE,
    )
}

pub(super) fn open_file_at(parent: &File, name: &OsStr) -> io::Result<File> {
    open_at(
        parent,
        name,
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE,
    )
}

pub(super) fn rename_at(source: &PinnedFile, destination: &File, name: &OsStr) -> io::Result<bool> {
    let name: Vec<u16> = name.encode_wide().collect();
    let name_bytes = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u32::try_from(length).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name is too long"))?;
    let name_offset = offset_of!(RenameInformation, file_name);
    let total = name_offset
        .checked_add(name_bytes as usize)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename buffer overflow"))?;
    let mut buffer = vec![0_u8; total];
    let information = buffer.as_mut_ptr().cast::<RenameInformation>();
    // SAFETY: the allocation is sized through the flexible file-name field,
    // and all pointers remain valid for the native call.
    unsafe {
        ptr::addr_of_mut!((*information).replace_if_exists).write(0);
        ptr::addr_of_mut!((*information).root_directory).write(destination.as_raw_handle());
        ptr::addr_of_mut!((*information).file_name_length).write(name_bytes);
        ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer.as_mut_ptr().add(name_offset).cast::<u16>(),
            name.len(),
        );
    }
    let status = set_information(
        &source.file,
        buffer.as_mut_ptr().cast(),
        u32::try_from(total).map_err(|_| io::Error::other("rename buffer too large"))?,
        FILE_RENAME_INFORMATION,
    );
    if status == STATUS_OBJECT_NAME_COLLISION {
        Ok(true)
    } else {
        nt_result(status).map(|()| false)
    }
}

pub(super) fn unlink_file(source: &PinnedFile) -> io::Result<()> {
    let mut information = DispositionInformation { delete_file: 1 };
    nt_result(set_information(
        &source.file,
        ptr::addr_of_mut!(information).cast(),
        size_of::<DispositionInformation>() as u32,
        FILE_DISPOSITION_INFORMATION,
    ))
}

pub(super) fn unlink_name_at(parent: &File, name: &OsStr) -> io::Result<()> {
    let file = open_at(
        parent,
        name,
        FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
        FILE_OPEN,
        0,
    )?;
    let mut information = DispositionInformation { delete_file: 1 };
    nt_result(set_information(
        &file,
        ptr::addr_of_mut!(information).cast(),
        size_of::<DispositionInformation>() as u32,
        FILE_DISPOSITION_INFORMATION,
    ))
}

fn open_at(
    parent: &File,
    name: &OsStr,
    access: u32,
    disposition: u32,
    kind: u32,
) -> io::Result<File> {
    let mut name: Vec<u16> = name.encode_wide().collect();
    let byte_length = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name is too long"))?;
    let mut unicode = UnicodeString {
        length: byte_length,
        maximum_length: byte_length,
        buffer: name.as_mut_ptr(),
    };
    let mut attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: parent.as_raw_handle(),
        object_name: ptr::addr_of_mut!(unicode),
        attributes: OBJ_CASE_INSENSITIVE,
        security_descriptor: ptr::null_mut(),
        security_quality_of_service: ptr::null_mut(),
    };
    let mut status_block = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut handle = ptr::null_mut();
    // SAFETY: all native structures and the UTF-16 name remain live for the call.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &mut attributes,
            &mut status_block,
            ptr::null_mut(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_ALL,
            disposition,
            kind | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            ptr::null_mut(),
            0,
        )
    };
    nt_result(status)?;
    // SAFETY: a successful NtCreateFile call returns one new owned handle.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn set_information(file: &File, information: *mut c_void, length: u32, class: u32) -> i32 {
    let mut status_block = IoStatusBlock {
        status: 0,
        information: 0,
    };
    // SAFETY: `file` is live and the caller provides a class-matching buffer.
    unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status_block,
            information,
            length,
            class,
        )
    }
}

fn nt_result(status: i32) -> io::Result<()> {
    if status >= 0 {
        return Ok(());
    }
    // SAFETY: status is an NTSTATUS returned by an ntdll call above.
    let code = unsafe { RtlNtStatusToDosError(status) };
    Err(io::Error::from_raw_os_error(code as i32))
}
