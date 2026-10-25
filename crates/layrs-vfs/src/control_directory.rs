//! Versioned comparison data for an externally owned control allocation.
//! Decoding never authenticates a Store receipt or grants filesystem authority.
use crate::{Error, Result};
use layrs_core::scm::FileId;
use std::path::Path;

/// Each canonical path is bounded in UTF-8 bytes, not characters/UTF-16 units.
pub const CONTROL_DIRECTORY_MAX_PATH_UTF8_BYTES: usize = 4096;
pub const CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES: usize =
    44 + 3 * (4 + CONTROL_DIRECTORY_MAX_PATH_UTF8_BYTES);
pub const CONTROL_DIRECTORY_MAX_ENCODED_BYTES: usize =
    8 + 4 + CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES + 4 + 71;
pub const CONTROL_DIRECTORY_MAX_FORMAT_BYTES: usize = 65536;
pub(crate) const MAX_PATH_BYTES: usize = CONTROL_DIRECTORY_MAX_PATH_UTF8_BYTES;
pub(crate) const MAX_STAGE_BYTES: usize = CONTROL_DIRECTORY_STAGE_MAX_ENCODED_BYTES;
pub(crate) const MAX_IDENTITY_BYTES: usize = CONTROL_DIRECTORY_MAX_ENCODED_BYTES;
pub(crate) const MAX_FORMAT_BYTES: usize = CONTROL_DIRECTORY_MAX_FORMAT_BYTES;

/// Observed allocation DATA. The caller must independently own the allocation
/// and authenticate its durable receipt before passing this data to publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlDirectoryStageIdentity {
    pub(crate) root: String,
    pub(crate) parent: String,
    pub(crate) stage: String,
    pub(crate) volume: u32,
    pub(crate) root_index: u64,
    pub(crate) parent_index: u64,
    pub(crate) directory_index: u64,
    pub(crate) format_index: u64,
}
impl ControlDirectoryStageIdentity {
    pub fn root(&self) -> &Path {
        Path::new(&self.root)
    }
    pub fn stage_parent(&self) -> &Path {
        Path::new(&self.parent)
    }
    pub fn stage(&self) -> &Path {
        Path::new(&self.stage)
    }
    pub fn root_filesystem_identity(&self) -> String {
        filesystem_identity(self.volume, self.root_index)
    }
    pub fn stage_parent_filesystem_identity(&self) -> String {
        filesystem_identity(self.volume, self.parent_index)
    }
    pub fn directory_filesystem_identity(&self) -> String {
        filesystem_identity(self.volume, self.directory_index)
    }
    pub fn format_binding_identity(&self) -> String {
        format_identity(self.volume, self.format_index)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = b"LYRCSTG1".to_vec();
        out.extend_from_slice(&self.volume.to_le_bytes());
        for id in [
            self.root_index,
            self.parent_index,
            self.directory_index,
            self.format_index,
        ] {
            out.extend_from_slice(&id.to_le_bytes());
        }
        for path in [&self.root, &self.parent, &self.stage] {
            out.extend_from_slice(&(path.len() as u32).to_le_bytes());
            out.extend_from_slice(path.as_bytes());
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_STAGE_BYTES {
            return Err(Error::IdentityMismatch);
        }
        let mut input = Input(bytes);
        if input.take(8)? != b"LYRCSTG1" {
            return Err(Error::IdentityMismatch);
        }
        let value = Self {
            volume: input.u32()?,
            root_index: input.u64()?,
            parent_index: input.u64()?,
            directory_index: input.u64()?,
            format_index: input.u64()?,
            root: input.path()?,
            parent: input.path()?,
            stage: input.path()?,
        };
        if !input.0.is_empty() {
            return Err(Error::IdentityMismatch);
        }
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn validate(&self) -> Result<()> {
        for path in [&self.root, &self.parent, &self.stage] {
            validate_path(path)?;
        }
        validate_control_paths(&self.root)?;
        validate_stage_paths(&self.stage)?;
        let prefix = format!("{}\\", self.parent.trim_end_matches('\\'));
        let leaf = self
            .stage
            .strip_prefix(&prefix)
            .ok_or(Error::IdentityMismatch)?;
        if leaf.is_empty() || leaf.contains('\\') {
            return Err(Error::IdentityMismatch);
        }
        let ids = [
            self.root_index,
            self.parent_index,
            self.directory_index,
            self.format_index,
        ];
        for (at, id) in ids.iter().enumerate() {
            if *id == 0 || ids[..at].contains(id) {
                return Err(Error::IdentityMismatch);
            }
        }
        // Paths are comparison data. Native ancestry, same-volume, privacy and
        // registration checks belong to the actual guards, not this decoder.
        Ok(())
    }
}

/// Expected final layout DATA, including the exact historical stage relation.
/// Public decode is required for cross-crate receipts; it is not authentication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlDirectoryIdentity {
    pub(crate) stage: ControlDirectoryStageIdentity,
    pub(crate) size: u32,
    pub(crate) content: FileId,
}
impl ControlDirectoryIdentity {
    pub fn stage_identity(&self) -> &ControlDirectoryStageIdentity {
        &self.stage
    }
    pub fn encode(&self) -> Vec<u8> {
        let stage = self.stage.encode();
        let mut out = b"LYRCDIR1".to_vec();
        out.extend_from_slice(&(stage.len() as u32).to_le_bytes());
        out.extend_from_slice(&stage);
        out.extend_from_slice(&self.size.to_le_bytes());
        out.extend_from_slice(self.content.as_str().as_bytes());
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_IDENTITY_BYTES {
            return Err(Error::IdentityMismatch);
        }
        let mut input = Input(bytes);
        if input.take(8)? != b"LYRCDIR1" {
            return Err(Error::IdentityMismatch);
        }
        let length = input.u32()? as usize;
        if length > MAX_STAGE_BYTES {
            return Err(Error::IdentityMismatch);
        }
        let stage = ControlDirectoryStageIdentity::decode(input.take(length)?)?;
        let size = input.u32()?;
        if size == 0 || size as usize > MAX_FORMAT_BYTES {
            return Err(Error::IdentityMismatch);
        }
        let text = std::str::from_utf8(input.take(71)?).map_err(|_| Error::IdentityMismatch)?;
        let content = FileId::parse(text).map_err(|_| Error::IdentityMismatch)?;
        if content.as_str() != text || !input.0.is_empty() {
            return Err(Error::IdentityMismatch);
        }
        Ok(Self {
            stage,
            size,
            content,
        })
    }
}

/// Only an inspection snapshot. It cannot be converted into a retained guard or
/// an expected identity. Its bytes remain locator hints until Store binding.
pub struct ObservedControlDirectory {
    pub(crate) root: String,
    pub(crate) volume: u32,
    pub(crate) root_index: u64,
    pub(crate) directory_index: u64,
    pub(crate) format_index: u64,
    pub(crate) bytes: Vec<u8>,
    pub(crate) content: FileId,
}
impl ObservedControlDirectory {
    pub fn root(&self) -> &Path {
        Path::new(&self.root)
    }
    pub fn format_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn identity(&self) -> ObservedControlDirectoryIdentity<'_> {
        ObservedControlDirectoryIdentity(self)
    }
}
/// Borrowed, non-convertible observed identity. The comparison covers physical
/// final fields only: an observation cannot attest historical stage ownership.
pub struct ObservedControlDirectoryIdentity<'a>(&'a ObservedControlDirectory);
impl ObservedControlDirectoryIdentity<'_> {
    pub fn matches_expected(&self, expected: &ControlDirectoryIdentity) -> bool {
        let observed = self.0;
        observed.root == expected.stage.root
            && observed.volume == expected.stage.volume
            && observed.root_index == expected.stage.root_index
            && observed.directory_index == expected.stage.directory_index
            && observed.format_index == expected.stage.format_index
            && observed.bytes.len() == expected.size as usize
            && observed.content == expected.content
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlDirectoryPublishPoint {
    StageValidated,
    AfterPersist,
    PreClose,
    PostClose,
    PreRename,
    PostRename,
    PreStrongPin,
    BeforeProof,
}
/// Faults retain all files. They never authorize cleanup, repair or adoption.
pub trait ControlDirectoryPublishFaults {
    fn at(&self, _point: ControlDirectoryPublishPoint) -> std::io::Result<()> {
        Ok(())
    }
}
pub struct NoControlDirectoryPublishFaults;
impl ControlDirectoryPublishFaults for NoControlDirectoryPublishFaults {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlDirectoryFailure {
    /// Refuses this action only. It grants no cleanup, repair or adoption
    /// authority; transient sharing errors retain their original cause.
    NeedsRepair,
    /// The raw cause is retained. Access denied does not identify the blocker.
    RenameRefused,
    /// A persist callback error does not establish rollback of its durable write.
    ReceiptDurabilityUnknown,
    Interrupted,
}

pub(crate) fn filesystem_identity(volume: u32, index: u64) -> String {
    format!("fs:v1:{volume:016x}:{index:016x}")
}
pub(crate) fn format_identity(volume: u32, index: u64) -> String {
    format!("{volume}:{}:{}", index >> 32, index as u32)
}
pub(crate) fn validate_path(path: &str) -> Result<()> {
    let bytes = path.as_bytes();
    if bytes.len() < 4
        || bytes.len() > MAX_PATH_BYTES
        || !bytes[0].is_ascii_alphabetic()
        || &bytes[1..3] != b":\\"
        || path.contains(['\0', '/'])
    {
        return Err(Error::InvalidRoot(
            "bounded canonical absolute drive path required".into(),
        ));
    }
    for part in path[3..].split('\\') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || part.ends_with(['.', ' '])
            || part.contains([':', '*', '?', '"', '<', '>', '|'])
            || part.chars().any(char::is_control)
            || matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            )
            || matches!(
                stem.as_bytes(),
                [b'C', b'O', b'M', b'1'..=b'9'] | [b'L', b'P', b'T', b'1'..=b'9']
            )
        {
            return Err(Error::InvalidRoot(
                "control path alias or unsupported component".into(),
            ));
        }
    }
    Ok(())
}
/// Fixed control suffixes consume the same UTF-8 path budget as every native
/// canonical path. Check feasibility before allocation/persistence/rename.
pub(crate) fn validate_control_paths(root: &str) -> Result<()> {
    validate_path(root)?;
    let directory = format!("{root}\\.layrs");
    validate_path(&directory)?;
    validate_stage_paths(&directory)
}
pub(crate) fn validate_stage_paths(stage: &str) -> Result<()> {
    validate_path(stage)?;
    validate_path(&format!("{stage}\\FORMAT"))
}
struct Input<'a>(&'a [u8]);
impl<'a> Input<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.0.len() {
            return Err(Error::IdentityMismatch);
        }
        let (value, rest) = self.0.split_at(length);
        self.0 = rest;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn path(&mut self) -> Result<String> {
        let length = self.u32()? as usize;
        if length > MAX_PATH_BYTES {
            return Err(Error::IdentityMismatch);
        }
        Ok(std::str::from_utf8(self.take(length)?)
            .map_err(|_| Error::IdentityMismatch)?
            .to_owned())
    }
}

#[cfg(test)]
#[path = "control_directory_tests.rs"]
mod tests;
