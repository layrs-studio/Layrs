use super::api::win32_error;
use crate::{Error, Result};
use std::{
    ffi::OsStr,
    fs::File,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::*,
};

pub(super) fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut chars: Vec<_> = value.encode_wide().collect();
    if chars.contains(&0) || chars.len() > 30_000 {
        return Err(Error::InvalidRoot("NUL or overlong path".into()));
    }
    chars.push(0);
    Ok(chars)
}
pub(super) fn handle(file: &File) -> HANDLE {
    file.as_raw_handle() as HANDLE
}

/// Convert only a normal absolute drive path for native calls. Public roots
/// still reject caller-supplied verbatim/device/UNC paths in RootGuard::open.
/// The extended prefix avoids MAX_PATH without relying on a process manifest;
/// it also disables Win32 normalization, so reject aliases before adding it.
pub(super) fn native_path(path: &Path) -> Result<Vec<u16>> {
    let value = path
        .to_str()
        .ok_or_else(|| Error::InvalidRoot("non-Unicode native path".into()))?;
    let mut parts = path.components();
    if !matches!(parts.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
        || !matches!(parts.next(), Some(Component::RootDir))
        || parts.any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::InvalidRoot(
            "native path requires a normal absolute drive path".into(),
        ));
    }
    let normalized = value.replace('/', "\\");
    if normalized[3..].split('\\').any(|part| {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let dos_device = matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || matches!(
            stem.as_bytes(),
            [b'C', b'O', b'M', b'1'..=b'9'] | [b'L', b'P', b'T', b'1'..=b'9']
        );
        matches!(part, "." | "..") || part.ends_with(['.', ' ']) || part.contains(':') || dos_device
    }) {
        return Err(Error::InvalidRoot("native path aliases unsupported".into()));
    }
    wide(OsStr::new(&format!("\\\\?\\{normalized}")))
}

pub(super) fn open(path: &Path, sharing: u32, access: u32) -> Result<File> {
    let name = native_path(path)?;
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            sharing,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    // The File owns exactly one successful CreateFileW handle.
    Ok(unsafe { File::from_raw_handle(h) })
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn native_long_paths_preserve_drive_scope_and_reject_external_prefixes() {
        let path = format!("C:/owned/{}/stage", "a".repeat(270));
        let converted = native_path(Path::new(&path)).unwrap();
        assert_eq!(
            String::from_utf16(&converted[..converted.len() - 1]).unwrap(),
            format!("\\\\?\\{}", path.replace('/', "\\")),
        );
        for path in [
            r"\\?\C:\owned\stage",
            r"\\.\C:\owned\stage",
            r"\\server\share\stage",
            r"C:relative",
            r"relative",
            r"C:\owned\..\stage",
            r"C:\owned\.\stage",
            r"C:\owned\stage.",
            r"C:\owned\stage ",
            r"C:\owned\stage:stream",
            r"C:\owned\CON.txt",
            r"C:\LPT1\stage",
        ] {
            assert!(native_path(Path::new(path)).is_err(), "accepted {path}");
        }
    }
}
pub(super) fn attributes(file: &File) -> Result<FILE_ATTRIBUTE_TAG_INFO> {
    // Windows may disguise cloud reparses as ordinary files for this process,
    // especially after disconnect. Expose only this metadata query, restoring
    // the caller's exact thread mode before any evidence leaves this function.
    super::visibility::with_exposed(|| {
        let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
        if unsafe {
            GetFileInformationByHandleEx(
                handle(file),
                FileAttributeTagInfo,
                (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            // Capture GetLastError before the restoration call can change it.
            return Err(win32_error("GetFileInformationByHandleEx(attributes)"));
        }
        Ok(info)
    })
}
pub(super) fn identity(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle(file), &mut info) } == 0 {
        return Err(win32_error("GetFileInformationByHandle"));
    }
    Ok(info)
}
pub(super) fn index(info: &BY_HANDLE_FILE_INFORMATION) -> u64 {
    ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64
}
pub(super) fn size(info: &BY_HANDLE_FILE_INFORMATION) -> u64 {
    ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64
}

pub(super) struct RootGuard {
    pub root: PathBuf,
    // READ_DATA/LIST_DIRECTORY participates in sharing checks; attribute-only
    // opens do not. Deny DELETE while retaining each ancestor and the root.
    pub locks: Vec<File>,
}
impl RootGuard {
    /// Resolve only the exact enumerated spelling of every component. Keep
    /// each directory pinned before descending, then recheck the name after
    /// acquiring its effective guard to catch replacement during the open.
    pub fn inspection_file(&self, relative: &str) -> Result<Option<(File, Vec<File>)>> {
        let mut path = self.root.clone();
        let mut guards = Vec::new();
        let parts: Vec<_> = relative.split('/').collect();
        for (position, part) in parts.iter().enumerate() {
            if !contains_exact_name(&path, part)? {
                return Ok(None);
            }
            let parent = path.clone();
            path.push(part);
            let directory = position + 1 < parts.len();
            let file = open(
                &path,
                if directory {
                    FILE_SHARE_READ | FILE_SHARE_WRITE
                } else {
                    FILE_SHARE_READ
                },
                FILE_READ_ATTRIBUTES
                    | if directory {
                        FILE_LIST_DIRECTORY
                    } else {
                        FILE_READ_DATA
                    },
            )?;
            if directory {
                let attrs = attributes(&file)?;
                if attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
                    || attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                {
                    return Err(Error::Unsupported(
                        "intermediate directory replaced or reparse point",
                    ));
                }
            }
            if !contains_exact_name(&parent, part)? {
                return Err(Error::Busy);
            }
            if !directory {
                return Ok(Some((file, guards)));
            }
            guards.push(file);
        }
        Err(Error::IdentityMismatch)
    }

    pub fn file(&self) -> &File {
        self.locks.last().unwrap()
    }
    pub fn open(root: &Path, owned: bool) -> Result<Self> {
        if !root.is_absolute() || root.to_str().is_none() || root.as_os_str().len() > 4096 {
            return Err(Error::InvalidRoot(
                "explicit absolute Unicode drive path required".into(),
            ));
        }
        let mut components = root.components();
        if !matches!(components.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
            || !matches!(components.next(), Some(Component::RootDir))
        {
            return Err(Error::InvalidRoot(
                "UNC, device, verbatim and relative roots unsupported".into(),
            ));
        }
        // Path::components normalizes interior dots; reject them lexically too.
        if root
            .to_str()
            .unwrap()
            .split(['/', '\\'])
            .any(|p| matches!(p, "." | ".."))
        {
            return Err(Error::InvalidRoot("dot aliases unsupported".into()));
        }
        let parts: Vec<_> = components.collect();
        if parts.is_empty() || parts.iter().any(|p| !matches!(p, Component::Normal(_))) {
            return Err(Error::InvalidRoot(
                "volume root or parent traversal refused".into(),
            ));
        }
        let mut path = PathBuf::from(&root.as_os_str().to_string_lossy()[..3]);
        let mut locks = Vec::new();
        for (position, part) in parts.iter().enumerate() {
            path.push(part.as_os_str());
            let file = open(
                &path,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            )?;
            let attrs = attributes(&file)?;
            if attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
                || (attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                    && !(owned
                        && position + 1 == parts.len()
                        && (attrs.ReparseTag & 0xffff0fff) == 0x9000001a))
            {
                return Err(Error::InvalidRoot(
                    "root/ancestor is a reparse point or not a directory".into(),
                ));
            }
            locks.push(file);
        }
        let file = locks.last().unwrap();
        let mut name = vec![0u16; 32_768];
        let count = unsafe {
            GetFinalPathNameByHandleW(
                handle(file),
                name.as_mut_ptr(),
                name.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        };
        if count == 0 || count as usize >= name.len() {
            return Err(win32_error("GetFinalPathNameByHandleW"));
        }
        let canonical = String::from_utf16(&name[..count as usize])
            .map_err(|_| Error::InvalidRoot("non-Unicode root".into()))?;
        let canonical = canonical
            .strip_prefix("\\\\?\\")
            .ok_or_else(|| Error::InvalidRoot("non-drive root".into()))?;
        if !canonical.eq_ignore_ascii_case(
            root.to_str()
                .unwrap()
                .trim_end_matches(['/', '\\'])
                .replace('/', "\\")
                .as_str(),
        ) {
            return Err(Error::InvalidRoot(
                "short-name or normalized alias root refused".into(),
            ));
        }
        let mut filesystem = [0u16; 32];
        if unsafe {
            GetVolumeInformationByHandleW(
                handle(file),
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
            return Err(win32_error("GetVolumeInformationByHandleW"));
        }
        if String::from_utf16_lossy(
            &filesystem[..filesystem.iter().position(|c| *c == 0).unwrap_or(32)],
        ) != "NTFS"
        {
            return Err(Error::Unsupported("prototype requires local NTFS"));
        }
        Ok(Self {
            root: PathBuf::from(canonical),
            locks,
        })
    }

    /// Validate and retain every intermediate directory before any leaf access.
    pub fn parents(&self, relative: &str) -> Result<Vec<File>> {
        let mut path = self.root.clone();
        let mut guards = Vec::new();
        let parts: Vec<_> = relative.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            path.push(part);
            let file = open(
                &path,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            )?;
            let attr = attributes(&file)?;
            if attr.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
                || attr.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(Error::Unsupported(
                    "intermediate directory replaced or reparse point",
                ));
            }
            guards.push(file);
        }
        Ok(guards)
    }
}

fn contains_exact_name(parent: &Path, leaf: &str) -> Result<bool> {
    for (count, item) in std::fs::read_dir(parent)?.enumerate() {
        // At most the user namespace limit plus the fixed root control entry.
        // This does not exclude or adopt any control file by name.
        if count > crate::catalog::MAX_ENTRIES {
            return Err(Error::Unsupported(
                "inspection directory entry budget exceeded",
            ));
        }
        if item?.file_name() == OsStr::new(leaf) {
            return Ok(true);
        }
    }
    Ok(false)
}
