//! Exactly one control flavor. No path-only exclusion or layout-to-proof promotion.
use super::{
    control::ControlGuard, control_directory::ControlDirectoryGuard, control_directory_io as io,
    fs, provider_root::ProviderRootGuard,
};
use crate::{Error, Result};
use std::fs::File;
use windows_sys::Win32::Storage::FileSystem::*;

pub(super) enum AttachedControl {
    None,
    File(ControlGuard),
    Directory(ProviderRootGuard),
}
impl AttachedControl {
    pub fn is_some(&self) -> bool {
        !matches!(self, Self::None)
    }
    pub fn observer_access(&self) -> u32 {
        FILE_READ_ATTRIBUTES
            | if matches!(self, Self::Directory(_)) {
                FILE_LIST_DIRECTORY
            } else {
                0
            }
    }
    pub fn proof(&self) -> Option<&ProviderRootGuard> {
        match self {
            Self::Directory(proof) => Some(proof),
            _ => None,
        }
    }
    pub fn revalidate(&self) -> Result<()> {
        match self {
            Self::Directory(proof) => proof.revalidate(),
            _ => Ok(()),
        }
    }
    pub fn matches_entry(&self, path: &str, file: &File) -> Result<bool> {
        if path != ".layrs" {
            return Ok(false);
        }
        match self {
            Self::None => Ok(false),
            Self::Directory(proof) => {
                verify_directory_entry(proof.control_directory(), file)?;
                Ok(true)
            }
            Self::File(control) => {
                let current = fs::identity(file)?;
                if fs::attributes(file)?.FileAttributes
                    & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                    != 0
                {
                    return Err(Error::IdentityMismatch);
                }
                let guarded = fs::identity(&control.file)?;
                if current.dwVolumeSerialNumber != control.identity.volume
                    || fs::index(&current) != control.identity.file_index
                    || fs::index(&guarded) != control.identity.file_index
                    || current.nNumberOfLinks != 1
                    || fs::size(&current) != control.identity.size as u64
                {
                    return Err(Error::IdentityMismatch);
                }
                Ok(true)
            }
        }
    }
}
pub(super) fn verify_directory_entry(control: &ControlDirectoryGuard, file: &File) -> Result<()> {
    control.revalidate()?;
    io::control_directory_attributes(file)?;
    if io::id(file)? != io::id(&control.directory)?
        || io::canonical(file)?
            != control
                .root()
                .join(".layrs")
                .to_str()
                .ok_or(Error::IdentityMismatch)?
    {
        return Err(Error::IdentityMismatch);
    }
    Ok(())
}
