use crate::{Error, Result};
use layrs_core::scm::FileId;

pub(crate) const MAX_CONTROL_BYTES: usize = 64 * 1024;

/// Failure seams for durable control publication. A failure retains staging
/// evidence and never authorizes replacing or deleting a foreign file.
pub trait ControlPublishFaults {
    fn after_stage_created(&self) -> std::io::Result<()> {
        Ok(())
    }
    fn write_staged(&self, output: &mut dyn std::io::Write, bytes: &[u8]) -> std::io::Result<()> {
        output.write_all(bytes)
    }
    fn after_stage_flushed(&self) -> std::io::Result<()> {
        Ok(())
    }
    fn after_published(&self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct NoControlPublishFaults;
impl ControlPublishFaults for NoControlPublishFaults {}

/// Capability for the single `.layrs` control file, bound to root, native file
/// identity and exact bytes. It is not a general namespace exclusion pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFileIdentity {
    pub(crate) volume: u32,
    pub(crate) root_index: u64,
    pub(crate) file_index: u64,
    pub(crate) size: u32,
    pub(crate) content: FileId,
}
impl ControlFileIdentity {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = b"LYRCTRL1".to_vec();
        bytes.extend_from_slice(&self.volume.to_le_bytes());
        bytes.extend_from_slice(&self.root_index.to_le_bytes());
        bytes.extend_from_slice(&self.file_index.to_le_bytes());
        bytes.extend_from_slice(&self.size.to_le_bytes());
        bytes.extend_from_slice(self.content.as_str().as_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 103 || &bytes[..8] != b"LYRCTRL1" {
            return Err(Error::IdentityMismatch);
        }
        let size = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        if size == 0 || size as usize > MAX_CONTROL_BYTES {
            return Err(Error::IdentityMismatch);
        }
        let content = std::str::from_utf8(&bytes[32..]).map_err(|_| Error::IdentityMismatch)?;
        Ok(Self {
            volume: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            root_index: u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
            file_index: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
            size,
            content: FileId::parse(content).map_err(|_| Error::IdentityMismatch)?,
        })
    }
}
