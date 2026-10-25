//! Read advertised prefixes and individual initialized ABI fields. Never form
//! a full Rust reference to an externally sized CFAPI structure or its padding.
use std::{
    ffi::c_void,
    mem::{offset_of, size_of},
};
use windows_sys::Win32::Storage::CloudFilters::*;

#[derive(Clone, Copy)]
struct Fields {
    pointer: *const u8,
    length: usize,
}
impl Fields {
    /// CFAPI guarantees a readable size prefix and that its declared region
    /// stays live for the callback. No later field is assumed to be present.
    unsafe fn native(pointer: *const c_void) -> Option<Self> {
        if pointer.is_null() {
            return None;
        }
        let length = unsafe { pointer.cast::<u32>().read_unaligned() } as usize;
        if !(size_of::<u32>()..=65_536).contains(&length) {
            return None;
        }
        Some(Self {
            pointer: pointer.cast(),
            length,
        })
    }
    #[cfg(test)]
    fn bytes(bytes: &[u8]) -> Option<Self> {
        let length = u32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        if length < 4 || length > bytes.len() {
            return None;
        }
        Some(Self {
            pointer: bytes.as_ptr(),
            length,
        })
    }
    /// Used only for integers/raw pointers, never a type with invalid bit values.
    unsafe fn field<T: Copy>(self, offset: usize) -> Option<T> {
        if offset.checked_add(size_of::<T>())? > self.length {
            return None;
        }
        Some(unsafe { self.pointer.add(offset).cast::<T>().read_unaligned() })
    }
}

unsafe fn info_fields(fields: Fields) -> Option<CF_CALLBACK_INFO> {
    macro_rules! field {
        ($name:ident) => {
            unsafe { fields.field(offset_of!(CF_CALLBACK_INFO, $name))? }
        };
    }
    Some(CF_CALLBACK_INFO {
        StructSize: field!(StructSize),
        ConnectionKey: field!(ConnectionKey),
        CallbackContext: field!(CallbackContext),
        SyncRootIdentity: field!(SyncRootIdentity),
        SyncRootIdentityLength: field!(SyncRootIdentityLength),
        FileId: field!(FileId),
        FileSize: field!(FileSize),
        FileIdentity: field!(FileIdentity),
        FileIdentityLength: field!(FileIdentityLength),
        TransferKey: field!(TransferKey),
        CorrelationVector: field!(CorrelationVector),
        ProcessInfo: field!(ProcessInfo),
        RequestKey: field!(RequestKey),
        ..Default::default()
    })
}
pub(super) unsafe fn callback_info(pointer: *const CF_CALLBACK_INFO) -> Option<CF_CALLBACK_INFO> {
    unsafe { info_fields(Fields::native(pointer.cast())?) }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Fetch {
    pub required_offset: i64,
    pub required_length: i64,
    pub optional_offset: i64,
    pub optional_length: i64,
}
unsafe fn fetch_fields(fields: Fields) -> Option<Fetch> {
    macro_rules! field {
        ($name:ident) => {
            unsafe {
                fields.field(
                    offset_of!(CF_CALLBACK_PARAMETERS, Anonymous)
                        + offset_of!(CF_CALLBACK_PARAMETERS_0_1, $name),
                )?
            }
        };
    }
    Some(Fetch {
        required_offset: field!(RequiredFileOffset),
        required_length: field!(RequiredLength),
        optional_offset: field!(OptionalFileOffset),
        optional_length: field!(OptionalLength),
    })
}
pub(super) unsafe fn fetch(pointer: *const CF_CALLBACK_PARAMETERS) -> Option<Fetch> {
    unsafe { fetch_fields(Fields::native(pointer.cast())?) }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Cancel {
    pub offset: i64,
    pub length: i64,
}
unsafe fn cancel_fields(fields: Fields) -> Option<Cancel> {
    let start = offset_of!(CF_CALLBACK_PARAMETERS, Anonymous)
        + offset_of!(CF_CALLBACK_PARAMETERS_0_0, Anonymous);
    Some(Cancel {
        offset: unsafe {
            fields.field(start + offset_of!(CF_CALLBACK_PARAMETERS_0_0_0_0, FileOffset))?
        },
        length: unsafe {
            fields.field(start + offset_of!(CF_CALLBACK_PARAMETERS_0_0_0_0, Length))?
        },
    })
}
pub(super) unsafe fn cancel(pointer: *const CF_CALLBACK_PARAMETERS) -> Option<Cancel> {
    unsafe { cancel_fields(Fields::native(pointer.cast())?) }
}
pub(super) unsafe fn process_id(pointer: *const CF_PROCESS_INFO) -> Option<u32> {
    unsafe { Fields::native(pointer.cast())?.field(offset_of!(CF_PROCESS_INFO, ProcessId)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sized(length: usize) -> Vec<u8> {
        let mut bytes = vec![0; length];
        if length >= 4 {
            bytes[..4].copy_from_slice(&(length as u32).to_ne_bytes());
        }
        bytes
    }
    #[test]
    fn short_info_prefixes_are_rejected_without_forming_full_references() {
        let required = offset_of!(CF_CALLBACK_INFO, RequestKey) + size_of::<i64>();
        for length in 0..required {
            let bytes = sized(length);
            let parsed = Fields::bytes(&bytes).and_then(|fields| unsafe { info_fields(fields) });
            assert!(parsed.is_none(), "accepted length {length}");
        }
        let bytes = sized(required);
        assert!(unsafe { info_fields(Fields::bytes(&bytes).unwrap()) }.is_some());
        let mut bytes = sized(4);
        bytes[..4].copy_from_slice(&(required as u32).to_ne_bytes());
        assert!(Fields::bytes(&bytes).is_none());
    }
    #[test]
    fn fetch_requires_the_optional_fields_it_reads_but_not_unused_tail() {
        let required = offset_of!(CF_CALLBACK_PARAMETERS, Anonymous)
            + offset_of!(CF_CALLBACK_PARAMETERS_0_1, OptionalLength)
            + 8;
        for length in 0..required {
            assert!(
                Fields::bytes(&sized(length))
                    .and_then(|fields| unsafe { fetch_fields(fields) })
                    .is_none(),
                "accepted length {length}"
            );
        }
        let mut bytes = sized(required);
        bytes[required - 8..].copy_from_slice(&8192i64.to_ne_bytes());
        let fields = Fields::bytes(&bytes).unwrap();
        assert_eq!(
            unsafe { fetch_fields(fields) }.unwrap().optional_length,
            8192
        );
        // The complete advertised region may be unaligned.
        let mut unaligned = vec![0xff];
        unaligned.extend_from_slice(&bytes);
        assert_eq!(
            unsafe { fetch_fields(Fields::bytes(&unaligned[1..]).unwrap()) }
                .unwrap()
                .optional_length,
            8192
        );
    }
    #[test]
    fn cancel_validates_both_range_fields_and_process_info_validates_its_prefix() {
        let required = offset_of!(CF_CALLBACK_PARAMETERS, Anonymous)
            + offset_of!(CF_CALLBACK_PARAMETERS_0_0, Anonymous)
            + size_of::<CF_CALLBACK_PARAMETERS_0_0_0_0>();
        for length in 0..required {
            assert!(
                Fields::bytes(&sized(length))
                    .and_then(|fields| unsafe { cancel_fields(fields) })
                    .is_none()
            );
        }
        assert_eq!(
            unsafe { cancel_fields(Fields::bytes(&sized(required)).unwrap()) },
            Some(Cancel {
                offset: 0,
                length: 0
            })
        );
        let prefix = sized(4);
        assert!(unsafe { process_id(prefix.as_ptr().cast()) }.is_none());
        let mut process = sized(offset_of!(CF_PROCESS_INFO, ProcessId) + 4);
        let offset = offset_of!(CF_PROCESS_INFO, ProcessId);
        process[offset..offset + 4].copy_from_slice(&123u32.to_ne_bytes());
        assert_eq!(unsafe { process_id(process.as_ptr().cast()) }, Some(123));
    }
}
