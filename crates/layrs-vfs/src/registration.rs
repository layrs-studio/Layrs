use crate::{Error, Result};
use layrs_core::scm::FileId;
use std::path::{Path, PathBuf};

pub(crate) const DIRECTORY_REVISION: u32 = 1;
const DIRECTORY_DOMAIN: &[u8] = b"layrs-vfs-control-directory-registration-v2\0";
const DIRECTORY_TICKET_PREFIX: usize = 186;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegistrationFlavor {
    Legacy,
    ControlDirectory { digest: FileId },
}

/// Persist this ticket outside the sync root before forgetting a session.
/// Reconnect compares it with CFAPI registration AND native directory identity.
/// It is an ownership guard, not an authorization/security boundary.
#[derive(Debug, Clone)]
pub struct Registration {
    pub(crate) root: PathBuf,
    pub(crate) root_id: [u8; 16],
    pub(crate) catalog_id: FileId,
    pub(crate) volume: u32,
    pub(crate) file_index: u64,
    pub(crate) flavor: RegistrationFlavor,
}
impl Registration {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn root_id(&self) -> [u8; 16] {
        self.root_id
    }
    pub fn catalog_id(&self) -> &FileId {
        &self.catalog_id
    }
    pub fn filesystem_identity(&self) -> String {
        format!("fs:v1:{:016x}:{:016x}", self.volume, self.file_index)
    }
    /// Checks the owned native registration without opening a connection.
    #[cfg(windows)]
    pub fn verify(&self, catalog: &crate::Catalog) -> Result<()> {
        self.require_legacy()?;
        crate::windows::verify_registration(self, catalog)
    }
    /// Point-in-time verification of this ticket against the native root and
    /// ready registration. Does not validate a serving source or control file,
    /// retain guards, connect a provider, or read placeholder content.
    #[cfg(windows)]
    pub fn verify_root_identity(&self) -> Result<()> {
        self.require_legacy()?;
        crate::windows::verify_root_identity(self)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = match self.flavor {
            RegistrationFlavor::Legacy => b"LYRTICK1".to_vec(),
            RegistrationFlavor::ControlDirectory { .. } => b"LYRTICK2".to_vec(),
        };
        bytes.extend_from_slice(&self.root_id);
        bytes.extend_from_slice(self.catalog_id.as_str().as_bytes());
        bytes.extend_from_slice(&self.volume.to_le_bytes());
        bytes.extend_from_slice(&self.file_index.to_le_bytes());
        if let RegistrationFlavor::ControlDirectory { digest } = &self.flavor {
            bytes.extend_from_slice(digest.as_str().as_bytes());
            bytes.extend_from_slice(&DIRECTORY_REVISION.to_le_bytes());
            bytes.extend_from_slice(
                &(self.root.to_str().expect("validated root").len() as u32).to_le_bytes(),
            );
        }
        bytes.extend_from_slice(self.root.to_str().expect("validated root").as_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 110 || bytes.len() > 32_768 || &bytes[..8] != b"LYRTICK1" {
            return Err(Error::IdentityMismatch);
        }
        let catalog_id =
            std::str::from_utf8(&bytes[24..95]).map_err(|_| Error::IdentityMismatch)?;
        let root = std::str::from_utf8(&bytes[107..]).map_err(|_| Error::IdentityMismatch)?;
        Ok(Self {
            root: PathBuf::from(root),
            root_id: bytes[8..24].try_into().unwrap(),
            catalog_id: FileId::parse(catalog_id).map_err(|_| Error::IdentityMismatch)?,
            volume: u32::from_le_bytes(bytes[95..99].try_into().unwrap()),
            file_index: u64::from_le_bytes(bytes[99..107].try_into().unwrap()),
            flavor: RegistrationFlavor::Legacy,
        })
    }
    /// Strict V2 comparison data only. The caller must authenticate its receipt;
    /// decoding neither opens a root nor grants source or serving authority.
    pub fn decode_control_directory(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < DIRECTORY_TICKET_PREFIX
            || bytes.len() > DIRECTORY_TICKET_PREFIX + crate::control_directory::MAX_PATH_BYTES
            || &bytes[..8] != b"LYRTICK2"
        {
            return Err(Error::IdentityMismatch);
        }
        let revision = u32::from_le_bytes(bytes[178..182].try_into().unwrap());
        if revision != DIRECTORY_REVISION {
            return Err(Error::Unsupported(
                "control-directory registration revision",
            ));
        }
        let length = u32::from_le_bytes(bytes[182..186].try_into().unwrap()) as usize;
        if length != bytes.len() - DIRECTORY_TICKET_PREFIX {
            return Err(Error::IdentityMismatch);
        }
        let root = std::str::from_utf8(&bytes[186..]).map_err(|_| Error::IdentityMismatch)?;
        crate::control_directory::validate_control_paths(root)?;
        let value = Self {
            root: root.into(),
            root_id: bytes[8..24].try_into().unwrap(),
            catalog_id: canonical_file_id(&bytes[24..95])?,
            volume: u32::from_le_bytes(bytes[95..99].try_into().unwrap()),
            file_index: u64::from_le_bytes(bytes[99..107].try_into().unwrap()),
            flavor: RegistrationFlavor::ControlDirectory {
                digest: canonical_file_id(&bytes[107..178])?,
            },
        };
        if value.root_id == [0; 16] || value.file_index == 0 {
            return Err(Error::IdentityMismatch);
        }
        Ok(value)
    }

    pub(crate) fn for_control_directory(
        control: &crate::ControlDirectoryIdentity,
        root_id: [u8; 16],
        catalog_id: FileId,
    ) -> Result<Self> {
        // Revalidate comparison data without treating it as receipt authority.
        crate::ControlDirectoryIdentity::decode(&control.encode())?;
        if root_id == [0; 16] {
            return Err(Error::InvalidRoot("nonzero root identity required".into()));
        }
        Ok(Self {
            root: control.stage.root.clone().into(),
            root_id,
            catalog_id,
            volume: control.stage.volume,
            file_index: control.stage.root_index,
            flavor: RegistrationFlavor::ControlDirectory {
                digest: control_digest(control),
            },
        })
    }

    pub(crate) fn require_legacy(&self) -> Result<()> {
        match self.flavor {
            RegistrationFlavor::Legacy => Ok(()),
            RegistrationFlavor::ControlDirectory { .. } => Err(Error::Unsupported(
                "directory registration requires a retained complete provider root guard",
            )),
        }
    }

    pub(crate) fn matches_control(&self, control: &crate::ControlDirectoryIdentity) -> Result<()> {
        let RegistrationFlavor::ControlDirectory { digest } = &self.flavor else {
            return Err(Error::Unsupported(
                "legacy registration cannot mint a provider root guard",
            ));
        };
        if self.root.to_str() != Some(control.stage.root.as_str())
            || self.volume != control.stage.volume
            || self.file_index != control.stage.root_index
            || *digest != control_digest(control)
        {
            return Err(Error::IdentityMismatch);
        }
        Ok(())
    }

    pub(crate) fn root_blob(&self, ready: bool) -> Vec<u8> {
        let mut bytes = match self.flavor {
            RegistrationFlavor::Legacy => b"LYRROOT1".to_vec(),
            RegistrationFlavor::ControlDirectory { .. } => b"LYRROOT2".to_vec(),
        };
        bytes.push(u8::from(ready));
        bytes.extend_from_slice(&self.root_id);
        bytes.extend_from_slice(self.catalog_id.as_str().as_bytes());
        bytes.extend_from_slice(&self.volume.to_le_bytes());
        bytes.extend_from_slice(&self.file_index.to_le_bytes());
        if let RegistrationFlavor::ControlDirectory { digest } = &self.flavor {
            bytes.extend_from_slice(digest.as_str().as_bytes());
            bytes.extend_from_slice(&DIRECTORY_REVISION.to_le_bytes());
        }
        bytes
    }
}

fn canonical_file_id(bytes: &[u8]) -> Result<FileId> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::IdentityMismatch)?;
    let id = FileId::parse(text).map_err(|_| Error::IdentityMismatch)?;
    if id.as_str() != text {
        return Err(Error::IdentityMismatch);
    }
    Ok(id)
}

fn control_digest(control: &crate::ControlDirectoryIdentity) -> FileId {
    let mut bytes = DIRECTORY_DOMAIN.to_vec();
    bytes.extend_from_slice(&control.encode());
    FileId::from_bytes(&bytes)
}

#[cfg(test)]
#[path = "registration_tests.rs"]
mod tests;
