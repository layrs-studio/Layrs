use super::{api::win32_error, fs, Session};
use crate::{Error, Result};
use std::{
    fs::File,
    path::{Path, PathBuf},
};
use windows_sys::Win32::Storage::FileSystem::*;

/// Retained ordinary export destination and ancestor identities. This is not a
/// source, capture or namespace capability. The caller must compare its ordinary
/// checkout pin with filesystem_identity() and retain both through publication.
/// Directory pins prevent replacement, not atomic child-name/type changes;
/// revalidate before each output/publication boundary.
pub struct ExportParentGuard<'a> {
    session: &'a Session,
    root: PathBuf,
    directories: Vec<DirectoryPin>,
}

struct DirectoryPin {
    path: PathBuf,
    file: File,
    volume: u32,
    index: u64,
}

impl ExportParentGuard<'_> {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn filesystem_identity(&self) -> String {
        let leaf = self
            .directories
            .last()
            .expect("validated absolute drive path");
        format!("fs:v1:{:016x}:{:016x}", leaf.volume, leaf.index)
    }

    pub fn revalidate(&self) -> Result<()> {
        self.session.verify_registration()?;
        for pin in &self.directories {
            let retained = ordinary_identity(self.session, &pin.file)?;
            if retained != (pin.volume, pin.index) {
                return Err(Error::IdentityMismatch);
            }
            // All original ancestors, including this directory, remain pinned.
            // Reopening checks the requested path/alias against the actual held
            // native identity; string canonicalization is not identity proof.
            let reopened = open_directory(&pin.path)?;
            if ordinary_identity(self.session, &reopened)? != retained {
                return Err(Error::IdentityMismatch);
            }
        }
        self.session.verify_registration()?;
        Ok(())
    }
}

impl Session {
    /// Acquire effective no-delete guards for an ordinary export parent and its
    /// whole ancestor chain. Reject own/foreign reparses even under compatibility
    /// masking. No content reads, connection, file creation or path adoption.
    pub fn guard_export_parent(&self, candidate: &Path) -> Result<ExportParentGuard<'_>> {
        self.verify_registration()?;
        validate_candidate(candidate)?;
        let paths: Vec<_> = candidate.ancestors().collect();
        let mut directories = Vec::with_capacity(paths.len());
        for path in paths.into_iter().rev() {
            // Earlier ancestors are already held before opening the next leaf.
            let file = open_directory(path)?;
            let (volume, index) = ordinary_identity(self, &file)?;
            directories.push(DirectoryPin {
                path: path.to_owned(),
                file,
                volume,
                index,
            });
        }
        let guard = ExportParentGuard {
            session: self,
            root: candidate.to_owned(),
            directories,
        };
        guard.revalidate()?;
        Ok(guard)
    }

    /// Point-in-time identity probe for an export destination's ancestor walk.
    /// This grants no namespace/content authority and retains no caller pins.
    /// Probe ancestors from the volume root toward the destination; on false,
    /// the caller must still acquire and retain its ordinary directory guards.
    /// Foreign reparses are errors even if Windows would disguise them later.
    pub fn is_owned_root_directory(&self, candidate: &Path) -> Result<bool> {
        self.verify_registration()?;
        validate_candidate(candidate)?;
        let paths: Vec<_> = candidate.ancestors().collect();
        let mut guards: Vec<File> = Vec::with_capacity(paths.len());
        for (position, path) in paths.iter().rev().enumerate() {
            // Every preceding component is already pinned and positively plain.
            // OPEN_REPARSE_POINT then observes this component without traversal.
            let file = open_directory(path)?;
            let attrs = fs::attributes(&file)?;
            let native = fs::identity(&file)?;
            if attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                return Err(Error::Unsupported("owned-root probe requires directories"));
            }
            let owned = native.dwVolumeSerialNumber == self.registration.volume
                && fs::index(&native) == self.registration.file_index;
            let reparse = attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
            if owned {
                // Identity alone cannot excuse a replaced/invalid registration,
                // a removed cloud tag, or traversal through the provider root.
                if !reparse || (attrs.ReparseTag & 0xffff0fff) != 0x9000001a {
                    return Err(Error::ForeignRoot);
                }
                if position + 1 != paths.len() {
                    return Err(Error::Unsupported(
                        "owned-root probe cannot traverse the provider root",
                    ));
                }
                verify_spelling(path, &file)?;
                guards.push(file);
                self.verify_registration()?;
                return Ok(true);
            }
            if reparse || attrs.ReparseTag != 0 {
                return Err(Error::Unsupported(
                    "owned-root probe refuses a foreign or unsupported reparse point",
                ));
            }
            guards.push(file);
        }
        // Only positively ordinary, nonmatching directories reach false.
        Ok(false)
    }
}

fn validate_candidate(candidate: &Path) -> Result<()> {
    if candidate.as_os_str().len() > 4096 {
        return Err(Error::InvalidRoot(
            "export parent path exceeds bounds".into(),
        ));
    }
    // Includes a normal drive volume root; rejects UNC/device/verbatim,
    // drive-relative, dot, stream, trailing-dot/space and DOS-device aliases.
    // Ordinary 8.3 and Unicode case aliases are resolved by native identity.
    fs::native_path(candidate)?;
    Ok(())
}

fn open_directory(path: &Path) -> Result<File> {
    fs::open(
        path,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
    )
}

fn ordinary_identity(session: &Session, file: &File) -> Result<(u32, u64)> {
    let attrs = fs::attributes(file)?;
    let native = fs::identity(file)?;
    if attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(Error::Unsupported(
            "export parent requires ordinary directories",
        ));
    }
    let identity = (native.dwVolumeSerialNumber, fs::index(&native));
    if identity == (session.registration.volume, session.registration.file_index) {
        return Err(Error::Unsupported(
            "export parent must be outside the provider checkout",
        ));
    }
    if attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || attrs.ReparseTag != 0 {
        return Err(Error::Unsupported("export parent contains a reparse point"));
    }
    Ok(identity)
}

fn verify_spelling(path: &Path, file: &File) -> Result<()> {
    let mut name = vec![0u16; 32_768];
    let count = unsafe {
        GetFinalPathNameByHandleW(
            fs::handle(file),
            name.as_mut_ptr(),
            name.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if count == 0 || count as usize >= name.len() {
        return Err(win32_error("GetFinalPathNameByHandleW(owned-root probe)"));
    }
    let canonical = String::from_utf16(&name[..count as usize])
        .map_err(|_| Error::InvalidRoot("non-Unicode owned-root probe path".into()))?;
    let canonical = canonical
        .strip_prefix("\\\\?\\")
        .ok_or_else(|| Error::InvalidRoot("non-drive owned-root probe path".into()))?;
    let requested = path
        .to_str()
        .ok_or(Error::IdentityMismatch)?
        .replace('/', "\\");
    if !canonical
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(requested.trim_end_matches('\\'))
    {
        return Err(Error::InvalidRoot(
            "owned-root probe path alias refused".into(),
        ));
    }
    Ok(())
}
