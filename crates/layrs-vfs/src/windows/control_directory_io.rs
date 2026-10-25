//! Narrow control-only native helpers. No changes to ordinary namespace walkers.
use super::{
    api::{Api, win32_error},
    control_directory_privacy::{Privacy, READ_CONTROL},
    fs,
};
use crate::{
    Error, Result,
    control_directory::{MAX_FORMAT_BYTES, MAX_PATH_BYTES, validate_path},
};
use std::{
    fs::File,
    io::Read,
    mem::{offset_of, size_of, size_of_val},
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::{CloudFilters::*, FileSystem::*},
};

pub(super) const ALL_SHARING: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
pub(super) type Id = (u32, u64);
pub(super) fn id(file: &File) -> Result<Id> {
    let value = fs::identity(file)?;
    Ok((value.dwVolumeSerialNumber, fs::index(&value)))
}
pub(super) fn canonical(file: &File) -> Result<String> {
    let mut out = vec![0u16; MAX_PATH_BYTES + 5];
    let count = unsafe {
        GetFinalPathNameByHandleW(
            fs::handle(file),
            out.as_mut_ptr(),
            out.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if count == 0 {
        return Err(win32_error("GetFinalPathNameByHandleW(control)"));
    }
    if count as usize >= out.len() {
        return Err(Error::InvalidRoot("control path exceeds bounds".into()));
    }
    let name = String::from_utf16(&out[..count as usize])
        .map_err(|_| Error::InvalidRoot("non-Unicode control path".into()))?;
    let name = name
        .strip_prefix("\\\\?\\")
        .ok_or_else(|| Error::InvalidRoot("non-drive control path".into()))?;
    validate_path(name)?;
    Ok(name.to_owned())
}
pub(super) fn normalized(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| Error::InvalidRoot("non-Unicode control path".into()))?
        .replace('/', "\\");
    validate_path(&value)?;
    Ok(value)
}
pub(super) fn plain_directory(file: &File) -> Result<()> {
    let attributes = fs::attributes(file)?;
    if attributes.ReparseTag != 0
        || attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        || attributes.FileAttributes
            & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_SPARSE_FILE)
            != 0
    {
        return Err(Error::Unsupported("ordinary control directory required"));
    }
    Ok(())
}
fn root_directory(file: &File, allow_cloud: bool) -> Result<()> {
    let attributes = fs::attributes(file)?;
    if allow_cloud
        && attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
        && attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        && (attributes.ReparseTag & 0xffff0fff) == 0x9000001a
    {
        // A recognized non-name-surrogate cloud leaf grants only layout access.
        return Ok(());
    }
    plain_directory(file)
}

/// Closed policy for the owned stage/.layrs only. Ancestor/root policy remains
/// separate; these benign flags alter visibility/indexing or tighten writes.
pub(super) fn control_directory_attributes(file: &File) -> Result<()> {
    let value = fs::attributes(file)?;
    control_directory_attribute_policy(value.FileAttributes, value.ReparseTag)
}
fn control_directory_attribute_policy(attributes: u32, tag: u32) -> Result<()> {
    const ALLOWED: u32 = FILE_ATTRIBUTE_DIRECTORY
        | FILE_ATTRIBUTE_READONLY
        | FILE_ATTRIBUTE_HIDDEN
        | FILE_ATTRIBUTE_SYSTEM
        | FILE_ATTRIBUTE_ARCHIVE
        | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;
    if tag != 0 || attributes & FILE_ATTRIBUTE_DIRECTORY == 0 || attributes & !ALLOWED != 0 {
        return Err(Error::Unsupported(
            "unsupported control directory attributes",
        ));
    }
    Ok(())
}

struct Pin {
    path: PathBuf,
    file: File,
    identity: Id,
}
pub(super) struct RootPins {
    pub root: PathBuf,
    pins: Vec<Pin>,
}
impl RootPins {
    pub fn open(path: &Path, allow_cloud_leaf: bool) -> Result<Self> {
        let value = normalized(path)?;
        let path = Path::new(&value);
        let mut pins = Vec::new();
        let paths: Vec<_> = path.ancestors().collect();
        for (at, path) in paths.iter().rev().enumerate() {
            let file = fs::open(
                path,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            )?;
            root_directory(&file, allow_cloud_leaf && at + 1 == paths.len())?;
            pins.push(Pin {
                path: path.to_path_buf(),
                identity: id(&file)?,
                file,
            });
        }
        let file = &pins.last().ok_or(Error::IdentityMismatch)?.file;
        let canonical = canonical(file)?;
        if !canonical.eq_ignore_ascii_case(&value) {
            return Err(Error::InvalidRoot(
                "normalized/short control root alias refused".into(),
            ));
        }
        let mut filesystem = [0u16; 32];
        if unsafe {
            GetVolumeInformationByHandleW(
                fs::handle(file),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                filesystem.as_mut_ptr(),
                filesystem.len() as u32,
            )
        } == 0
        {
            return Err(win32_error("GetVolumeInformationByHandleW(control)"));
        }
        let end = filesystem
            .iter()
            .position(|value| *value == 0)
            .ok_or(Error::IdentityMismatch)?;
        if &filesystem[..end] != [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16] {
            return Err(Error::Unsupported("control requires local NTFS"));
        }
        let value = Self {
            root: PathBuf::from(canonical),
            pins,
        };
        value.revalidate(allow_cloud_leaf)?;
        Ok(value)
    }
    pub fn file(&self) -> &File {
        &self.pins.last().expect("validated root pins").file
    }
    pub fn identity(&self) -> Id {
        self.pins.last().expect("validated root pins").identity
    }
    pub fn contains(&self, identity: Id) -> bool {
        self.pins.iter().any(|pin| pin.identity == identity)
    }
    // Borrowed native pins only. Stage2 must pair held and fresh registration
    // queries; no public handles or change to Step1's layout semantics.
    pub(super) fn held_paths(&self) -> impl Iterator<Item = (&Path, &File, Id)> {
        self.pins
            .iter()
            .map(|pin| (pin.path.as_path(), &pin.file, pin.identity))
    }
    pub fn duplicate(&self, privacy: &Privacy) -> Result<Self> {
        let pins = self
            .pins
            .iter()
            .map(|pin| {
                Ok(Pin {
                    path: pin.path.clone(),
                    identity: pin.identity,
                    file: privacy.duplicate(&pin.file)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            root: self.root.clone(),
            pins,
        })
    }
    pub fn revalidate(&self, allow_cloud_leaf: bool) -> Result<()> {
        for (at, pin) in self.pins.iter().enumerate() {
            let allowed = allow_cloud_leaf && at + 1 == self.pins.len();
            root_directory(&pin.file, allowed)?;
            let current = fs::open(
                &pin.path,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            )?;
            root_directory(&current, allowed)?;
            if id(&current)? != pin.identity || id(&pin.file)? != pin.identity {
                return Err(Error::IdentityMismatch);
            }
        }
        if canonical(self.file())? != self.root.to_str().ok_or(Error::IdentityMismatch)? {
            return Err(Error::IdentityMismatch);
        }
        Ok(())
    }
    pub fn outside_sync_roots(&self, api: &Api) -> Result<()> {
        self.revalidate(true)?;
        for pin in &self.pins {
            not_under(api, &pin.file)?;
            plain_directory(&pin.file)?;
            let current = fs::open(
                &pin.path,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            )?;
            plain_directory(&current)?;
            if id(&current)? != pin.identity {
                return Err(Error::IdentityMismatch);
            }
            not_under(api, &current)?;
        }
        Ok(())
    }
}
pub(super) fn not_under(api: &Api, file: &File) -> Result<()> {
    // Do not treat an arbitrary native failure as evidence of no registration.
    let mut value = CF_SYNC_ROOT_BASIC_INFO::default();
    let hr = unsafe {
        (api.root_info)(
            fs::handle(file),
            CF_SYNC_ROOT_INFO_BASIC,
            (&mut value as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
            size_of_val(&value) as u32,
            std::ptr::null_mut(),
        )
    };
    if hr != (0x80070000 | ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT) as i32 {
        return Err(Error::Windows {
            operation: "CfGetSyncRootInfoByHandle (expected NOT_UNDER_SYNC_ROOT)",
            hresult: hr,
        });
    }
    Ok(())
}
pub(super) fn directory_owner(path: &Path) -> Result<File> {
    fs::open(
        path,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        DELETE | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
    )
}
pub(super) fn directory_observer(path: &Path) -> Result<File> {
    fs::open(
        path,
        ALL_SHARING,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL,
    )
}
pub(super) fn format_pin(path: &Path) -> Result<File> {
    fs::open(
        path,
        FILE_SHARE_READ,
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL,
    )
}
pub(super) fn format_attributes(file: &File) -> Result<()> {
    let value = fs::attributes(file)?;
    format_attribute_policy(value.FileAttributes, value.ReparseTag)?;
    let identity = fs::identity(file)?;
    if identity.nNumberOfLinks != 1 {
        return Err(Error::Unsupported("FORMAT must have exactly one link"));
    }
    Ok(())
}
fn format_attribute_policy(attributes: u32, tag: u32) -> Result<()> {
    // These flags affect visibility, indexing/backup or further write access,
    // not byte identity. TEMPORARY, compression/encryption, recall, offline,
    // sparse, reparse and every unknown flag are outside this initial contract.
    const BENIGN: u32 = FILE_ATTRIBUTE_NORMAL
        | FILE_ATTRIBUTE_READONLY
        | FILE_ATTRIBUTE_HIDDEN
        | FILE_ATTRIBUTE_SYSTEM
        | FILE_ATTRIBUTE_ARCHIVE
        | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;
    if tag != 0 || attributes & !BENIGN != 0 {
        return Err(Error::Unsupported("unsupported FORMAT attributes"));
    }
    Ok(())
}
pub(super) fn read_format(mut file: &File, privacy: &Privacy) -> Result<Vec<u8>> {
    format_attributes(file)?;
    privacy.verify(file)?;
    stream_inventory(file, false)?;
    let size = fs::size(&fs::identity(file)?);
    if size == 0 || size > MAX_FORMAT_BYTES as u64 {
        return Err(Error::Unsupported("FORMAT byte bounds"));
    }
    // The caller always gives a freshly opened, private file cursor, never a
    // DuplicateHandle of a data reader. No shared seek position exists.
    let mut out = vec![0; size as usize];
    file.read_exact(&mut out)?;
    if file.read(&mut [0; 1])? != 0 || fs::size(&fs::identity(file)?) != size {
        return Err(Error::IdentityMismatch);
    }
    format_attributes(file)?;
    privacy.verify(file)?;
    stream_inventory(file, false)?;
    Ok(out)
}
pub(super) fn empty(path: &Path) -> Result<()> {
    if std::fs::read_dir(path)?.next().transpose()?.is_some() {
        return Err(Error::NeedsRepair("bootstrap root is not empty"));
    }
    Ok(())
}
pub(super) fn only_child(path: &Path, name: &str) -> Result<()> {
    let mut entries = std::fs::read_dir(path)?;
    let first = entries
        .next()
        .transpose()?
        .ok_or(Error::NeedsRepair("control child missing"))?;
    if first.file_name() != name || entries.next().transpose()?.is_some() {
        return Err(Error::NeedsRepair("control/bootstrap layout differs"));
    }
    Ok(())
}
pub(super) fn stream_inventory(file: &File, directory: bool) -> Result<()> {
    let mut buffer = vec![u64::MAX; 8192]; // fixed 64KiB, maximum 32 entries; no retry/growth
    unsafe { SetLastError(0) };
    let ok = unsafe {
        GetFileInformationByHandleEx(
            fs::handle(file),
            FileStreamInfo,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
        )
    };
    let error = unsafe { GetLastError() };
    // Documented no-stream signal, including calls that report success.
    if error == ERROR_HANDLE_EOF {
        return if directory {
            Ok(())
        } else {
            Err(Error::Unsupported("FORMAT unnamed stream missing"))
        };
    }
    if ok == 0 {
        return Err(std::io::Error::from_raw_os_error(error as i32).into());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 8) };
    let mut offset = 0usize;
    for entry in 0..32 {
        let fixed = offset_of!(FILE_STREAM_INFO, StreamName);
        if offset
            .checked_add(fixed)
            .is_none_or(|end| end > bytes.len())
        {
            return Err(Error::Unsupported("stream inventory truncated"));
        }
        let next = u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let length = u32::from_ne_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if length == 0
            || length > 4096
            || length % 2 != 0
            || offset
                .checked_add(fixed + length)
                .is_none_or(|end| end > bytes.len())
        {
            return Err(Error::Unsupported("stream inventory malformed"));
        }
        let name: Vec<u16> = bytes[offset + fixed..offset + fixed + length]
            .chunks_exact(2)
            .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
            .collect();
        if name != "::$DATA".encode_utf16().collect::<Vec<_>>() || (!directory && entry != 0) {
            return Err(Error::Unsupported("named control streams unsupported"));
        }
        if next == 0 {
            return Ok(());
        }
        if next % 8 != 0 || next < fixed + length {
            return Err(Error::Unsupported("stream inventory offset malformed"));
        }
        offset = offset.checked_add(next).ok_or(Error::IdentityMismatch)?;
    }
    Err(Error::Unsupported("stream inventory count exceeds bounds"))
}
pub(super) fn rename_no_replace(directory: &File, destination: &Path) -> Result<()> {
    // Absolute destination resolution is retained as a disclosed limitation:
    // ancestors remain pinned and final native/path identities are rechecked.
    let name = fs::native_path(destination)?;
    let length = (offset_of!(FILE_RENAME_INFO, FileName) + name.len() * 2)
        .max(size_of::<FILE_RENAME_INFO>());
    let mut out = vec![0u64; length.div_ceil(8)];
    let info = out.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = ((name.len() - 1) * 2) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            out.as_mut_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(
            fs::handle(directory),
            FileRenameInfo,
            info.cast(),
            length as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_control_directory_attribute_policy_is_closed() {
        assert!(
            control_directory_attribute_policy(
                FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_READONLY,
                0
            )
            .is_ok()
        );
        for attribute in [
            FILE_ATTRIBUTE_OFFLINE,
            FILE_ATTRIBUTE_SPARSE_FILE,
            FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_ATTRIBUTE_COMPRESSED,
            FILE_ATTRIBUTE_ENCRYPTED,
            FILE_ATTRIBUTE_TEMPORARY,
            FILE_ATTRIBUTE_NORMAL,
            0x80000000,
        ] {
            assert!(
                control_directory_attribute_policy(FILE_ATTRIBUTE_DIRECTORY | attribute, 0)
                    .is_err()
            );
        }
        assert!(control_directory_attribute_policy(FILE_ATTRIBUTE_ARCHIVE, 0).is_err());
        assert!(control_directory_attribute_policy(FILE_ATTRIBUTE_DIRECTORY, 0x42).is_err());
    }
    #[test]
    fn format_attribute_policy_is_closed_and_not_an_immutability_claim() {
        assert!(
            format_attribute_policy(
                FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN,
                0
            )
            .is_ok()
        );
        for attribute in [
            FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_ATTRIBUTE_OFFLINE,
            FILE_ATTRIBUTE_SPARSE_FILE,
            FILE_ATTRIBUTE_TEMPORARY,
            FILE_ATTRIBUTE_COMPRESSED,
            FILE_ATTRIBUTE_ENCRYPTED,
            0x80000000,
        ] {
            assert!(format_attribute_policy(FILE_ATTRIBUTE_ARCHIVE | attribute, 0).is_err());
        }
        assert!(format_attribute_policy(FILE_ATTRIBUTE_NORMAL, 0x42).is_err());
    }
}
