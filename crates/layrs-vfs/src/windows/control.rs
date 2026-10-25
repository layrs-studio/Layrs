use super::{Session, fs};
use crate::{ControlFileIdentity, Error, Result, control::MAX_CONTROL_BYTES};
use layrs_core::scm::FileId;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::*;

pub(super) struct ControlGuard {
    pub identity: ControlFileIdentity,
    pub file: File,
}

fn read_guarded(root: &fs::RootGuard) -> Result<(Vec<u8>, ControlGuard)> {
    // Require the exact spelling, not a case-insensitive alias accepted by NTFS.
    let mut exact = false;
    let mut budget = crate::catalog::NamespaceBudget::default();
    for entry in std::fs::read_dir(&root.root)? {
        if entry?.file_name() == ".layrs" {
            exact = true;
            break;
        }
        budget.user_entry()?;
    }
    if !exact {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound).into());
    }
    // A directory guard owns DELETE access, so an initial shareREAD open would
    // fail with a sharing error before reporting the legacy FILE/DIRECTORY type
    // mismatch. This compatible metadata observation grants no file capability.
    {
        let observed = fs::open(
            &root.root.join(".layrs"),
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_READ_ATTRIBUTES,
        )?;
        let attrs = fs::attributes(&observed)?;
        if attrs.ReparseTag != 0
            || attrs.FileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        {
            return Err(Error::Unsupported(
                ".layrs must be a plain single-link file on the owned volume",
            ));
        }
    }
    // The original strong plain-file open and independent validation follow.
    let mut file = fs::open(
        &root.root.join(".layrs"),
        FILE_SHARE_READ,
        FILE_READ_DATA | FILE_READ_ATTRIBUTES,
    )?;
    let (bytes, identity) = read_file(root, &mut file)?;
    // Discovery has one fixed candidate, validated as a plain single-link file
    // above. It returns hints only; namespace exclusion still requires attach.
    budget.verified_control_entry()?;
    Ok((bytes, ControlGuard { identity, file }))
}

pub(super) fn read_file(
    root: &fs::RootGuard,
    file: &mut File,
) -> Result<(Vec<u8>, ControlFileIdentity)> {
    let attrs = fs::attributes(file)?;
    let native = fs::identity(file)?;
    let root_native = fs::identity(root.file())?;
    if attrs.FileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || native.nNumberOfLinks != 1
        || native.dwVolumeSerialNumber != root_native.dwVolumeSerialNumber
    {
        return Err(Error::Unsupported(
            ".layrs must be a plain single-link file on the owned volume",
        ));
    }
    let size = fs::size(&native);
    if size == 0 || size > MAX_CONTROL_BYTES as u64 {
        return Err(Error::Unsupported(".layrs control bytes exceed bounds"));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.seek(SeekFrom::Start(0))?;
    (&mut *file)
        .take(MAX_CONTROL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size {
        return Err(Error::IdentityMismatch);
    }
    let identity = ControlFileIdentity {
        volume: native.dwVolumeSerialNumber,
        root_index: fs::index(&root_native),
        file_index: fs::index(&native),
        size: size as u32,
        content: FileId::from_bytes(&bytes),
    };
    Ok((bytes, identity))
}

/// Discovery only: returned bytes are locator hints until the private journal,
/// native registration and exact identity have all been validated by the caller.
pub fn read_control_file(root: &Path) -> Result<(Vec<u8>, ControlFileIdentity)> {
    let root = fs::RootGuard::open(root, true)?;
    let (bytes, guard) = read_guarded(&root)?;
    Ok((bytes, guard.identity))
}

impl Session {
    /// The old one-step API cannot durably bind a partially published file.
    /// Use publish_control_file_recoverable with a private staging directory.
    pub fn publish_control_file(&mut self, _bytes: &[u8]) -> Result<ControlFileIdentity> {
        self.registration.require_legacy()?;
        Err(Error::Unsupported(
            "control publication requires private staging and a durable identity hook",
        ))
    }

    pub fn attach_control_file(&mut self, expected: &ControlFileIdentity) -> Result<()> {
        self.registration.require_legacy()?;
        self.verify_registration()?;
        self.reject_control_catalog_collision()?;
        let (_, guard) = read_guarded(&self.root)?;
        if &guard.identity != expected {
            return Err(Error::IdentityMismatch);
        }
        self.control = super::AttachedControl::File(guard);
        Ok(())
    }

    pub(super) fn reject_control_catalog_collision(&self) -> Result<()> {
        if self.catalog.entries().iter().any(|entry| {
            entry
                .path
                .as_str()
                .split('/')
                .next()
                .unwrap()
                .eq_ignore_ascii_case(".layrs")
        }) {
            return Err(Error::InvalidCatalog(
                ".layrs control file collides with catalog".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn is_control(&self, path: &str, file: &File) -> Result<bool> {
        self.control.matches_entry(path, file)
    }
}
