//! Control bytes become visible only after complete staging and durable intent.
use super::{Session, api::win32_error, control, fs};
use crate::{ControlFileIdentity, ControlPublishFaults, Error, Result, control::MAX_CONTROL_BYTES};
use layrs_core::scm::FileId;
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    mem::{offset_of, size_of},
    os::windows::fs::OpenOptionsExt,
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::*;

const MAX_STAGE_ATTEMPTS: usize = 256;

impl Session {
    /// The caller supplies a pinned private journal directory on the same NTFS
    /// volume, outside this checkout. `persist` must durably record the identity
    /// before returning. If that record already exists, pass it as `expected`;
    /// the hook is then not called. Unbound staging leftovers are never adopted
    /// or deleted. A final `.layrs` without a saved identity is always foreign.
    /// At most 256 deterministic staging slots are probed per root/byte intent;
    /// exhaustion is NeedsRepair. File flushes and process-exit tests do not
    /// establish power-loss durability of NTFS directory updates.
    pub fn publish_control_file_recoverable(
        &mut self,
        bytes: &[u8],
        staging_root: &Path,
        expected: Option<&ControlFileIdentity>,
        persist: impl FnOnce(&ControlFileIdentity) -> Result<()>,
        faults: &dyn ControlPublishFaults,
    ) -> Result<ControlFileIdentity> {
        self.registration.require_legacy()?;
        self.verify_registration()?;
        self.reject_control_catalog_collision()?;
        if self.control.is_some() {
            return Err(Error::Busy);
        }
        if bytes.is_empty() || bytes.len() > MAX_CONTROL_BYTES {
            return Err(Error::Unsupported(".layrs control bytes exceed bounds"));
        }
        let staging = fs::RootGuard::open(staging_root, false)?;
        let root_identity = fs::identity(self.root.file())?;
        if fs::identity(staging.file())?.dwVolumeSerialNumber != root_identity.dwVolumeSerialNumber
        {
            return Err(Error::Unsupported(
                "control staging requires the same NTFS volume",
            ));
        }
        for ancestor in &staging.locks {
            let id = fs::identity(ancestor)?;
            if id.dwVolumeSerialNumber == root_identity.dwVolumeSerialNumber
                && fs::index(&id) == fs::index(&root_identity)
            {
                return Err(Error::Unsupported(
                    "control staging must be outside the checkout",
                ));
            }
        }
        if let Some(identity) = expected {
            if identity.volume != root_identity.dwVolumeSerialNumber
                || identity.root_index != fs::index(&root_identity)
                || identity.size as usize != bytes.len()
                || identity.content != FileId::from_bytes(bytes)
            {
                return Err(Error::IdentityMismatch);
            }
        }
        match std::fs::symlink_metadata(self.root.root.join(".layrs")) {
            Ok(_) => {
                let identity = expected.ok_or(Error::ForeignRoot)?;
                self.attach_control_file(identity)?;
                return Ok(identity.clone());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let prefix = stage_prefix(&self.registration.root_id(), bytes);
        let (file, identity) = if let Some(identity) = expected {
            let mut found = None;
            for attempt in 0..MAX_STAGE_ATTEMPTS {
                let path = staging.root.join(format!("{prefix}-{attempt:03}"));
                // Observe identity without reading or modifying foreign stages.
                let probe = match fs::open(
                    &path,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    FILE_READ_ATTRIBUTES,
                ) {
                    Ok(file) => file,
                    Err(Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                let native = fs::identity(&probe)?;
                if native.dwVolumeSerialNumber != identity.volume
                    || fs::index(&native) != identity.file_index
                {
                    continue;
                }
                drop(probe);
                let mut file = fs::open(
                    &path,
                    FILE_SHARE_READ,
                    FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE,
                )?;
                let (observed, current) = control::read_file(&self.root, &mut file)?;
                if &current != identity || observed != bytes {
                    return Err(Error::IdentityMismatch);
                }
                found = Some(file);
                break;
            }
            (
                found.ok_or(Error::NeedsRepair("recorded control stage is missing"))?,
                identity.clone(),
            )
        } else {
            let mut created = None;
            for attempt in 0..MAX_STAGE_ATTEMPTS {
                let path = staging.root.join(format!("{prefix}-{attempt:03}"));
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .access_mode(FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE)
                    .create_new(true)
                    .share_mode(FILE_SHARE_READ)
                    .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH)
                    .open(path)
                {
                    Ok(file) => {
                        created = Some(file);
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            let mut file = created.ok_or(Error::NeedsRepair(
                "control staging attempt budget exhausted",
            ))?;
            faults.after_stage_created()?;
            faults.write_staged(
                &mut BoundedWrite {
                    file: &mut file,
                    remaining: bytes.len(),
                },
                bytes,
            )?;
            // A short writer returning success must not produce a published file.
            let (observed, identity) = control::read_file(&self.root, &mut file)?;
            if observed != bytes {
                return Err(Error::IdentityMismatch);
            }
            file.sync_all()?;
            faults.after_stage_flushed()?;
            persist(&identity)?;
            (file, identity)
        };
        // Rename this very handle; never reopen a source by path for mutation.
        // No replacement flag means even a collision after preflight survives.
        rename_control(&file, &self.root)?;
        file.sync_all()?;
        faults.after_published()?;
        // Closing the writer is necessary to acquire the final deny-write guard.
        // The durable intent requires this exact native identity across that gap.
        drop(file);
        self.attach_control_file(&identity)?;
        Ok(identity)
    }
}

fn stage_prefix(root_id: &[u8; 16], bytes: &[u8]) -> String {
    let mut intent = b"layrs.control-stage.v1".to_vec();
    intent.extend_from_slice(root_id);
    intent.extend_from_slice(bytes);
    let digest = FileId::from_bytes(&intent);
    format!("staged-layrs-control-{}", &digest.as_str()[7..])
}

struct BoundedWrite<'a> {
    file: &'a mut File,
    remaining: usize,
}
impl Write for BoundedWrite<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "control write exceeds intent",
            ));
        }
        let count = self.file.write(bytes)?;
        self.remaining -= count;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn rename_control(file: &File, destination: &fs::RootGuard) -> Result<()> {
    let name = fs::native_path(&destination.root.join(".layrs"))?;
    let byte_length = (name.len() - 1) * size_of::<u16>();
    let length = (offset_of!(FILE_RENAME_INFO, FileName) + name.len() * size_of::<u16>())
        .max(size_of::<FILE_RENAME_INFO>());
    let mut buffer = vec![0u64; length.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = byte_length as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            buffer
                .as_mut_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
        if SetFileInformationByHandle(fs::handle(file), FileRenameInfo, info.cast(), length as u32)
            == 0
        {
            return Err(win32_error("SetFileInformationByHandle(control publish)"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_names_bind_root_and_exact_control_intent() {
        let name = stage_prefix(&[1; 16], b"locator");
        assert_eq!(name, stage_prefix(&[1; 16], b"locator"));
        assert_ne!(name, stage_prefix(&[2; 16], b"locator"));
        assert_ne!(name, stage_prefix(&[1; 16], b"locator changed"));
        assert!(name.starts_with("staged-"));
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        );
    }
}
