//! Durable content-addressed object installation and verification.

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_sync::{
    ChunkObject, Chunker, FileObject, LocalStepRef, ObjectDigest, TreeEntry, TreeObject,
};

use crate::fault::{self, FaultPoint};
use crate::layout::{StoreLayout, open_read_no_follow, validate_store_boundaries};
use crate::secure_fs::PinnedDirectory;

mod publication;

use publication::install_pending;

pub fn cas_path(cas_dir: impl AsRef<Path>, hash: &ContentHash) -> PathBuf {
    let hex = hash.hex();
    cas_dir
        .as_ref()
        .join(hash.algorithm().as_str())
        .join(&hex[0..2])
        .join(&hex[2..4])
        .join(hex)
}

pub fn cas_path_for_digest(cas_dir: impl AsRef<Path>, digest: &ObjectDigest) -> PathBuf {
    let hex = digest.hex();
    cas_dir
        .as_ref()
        .join(digest.algorithm())
        .join(&hex[0..2])
        .join(&hex[2..4])
        .join(hex)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Blake3,
    Placeholder,
}

impl HashAlgorithm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blake3 => "blake3",
            Self::Placeholder => "placeholder",
        }
    }

    pub fn can_verify(self) -> bool {
        matches!(self, Self::Blake3 | Self::Placeholder)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHash {
    algorithm: HashAlgorithm,
    hex: String,
}

impl ContentHash {
    pub fn new(algorithm: HashAlgorithm, hex: impl Into<String>) -> Result<Self, HashError> {
        let hex = hex.into();
        if hex.len() < 4 {
            return Err(HashError::TooShort);
        }
        if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(HashError::InvalidHex);
        }
        Ok(Self {
            algorithm,
            hex: hex.to_ascii_lowercase(),
        })
    }

    pub fn blake3(hex: impl Into<String>) -> Result<Self, HashError> {
        Self::new(HashAlgorithm::Blake3, hex)
    }

    pub fn placeholder_for(bytes: &[u8]) -> Self {
        Self {
            algorithm: HashAlgorithm::Placeholder,
            hex: placeholder_hash_hex(bytes),
        }
    }

    pub fn blake3_for(bytes: &[u8]) -> Self {
        Self {
            algorithm: HashAlgorithm::Blake3,
            hex: blake3::hash(bytes).to_hex().to_string(),
        }
    }

    pub fn from_object_digest(digest: &ObjectDigest) -> Result<Self, HashError> {
        match digest.algorithm() {
            "blake3" => Self::blake3(digest.hex()),
            _ => Err(HashError::UnsupportedAlgorithm),
        }
    }

    pub fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    pub fn hex(&self) -> &str {
        &self.hex
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.algorithm.as_str(), self.hex)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashError {
    TooShort,
    InvalidHex,
    UnsupportedAlgorithm,
}

impl fmt::Display for HashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(formatter, "content hash must have at least four hex chars"),
            Self::InvalidHex => write!(formatter, "content hash must be hexadecimal"),
            Self::UnsupportedAlgorithm => {
                write!(formatter, "content hash algorithm is unsupported")
            }
        }
    }
}

impl std::error::Error for HashError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashVerification {
    Verified,
    Unavailable {
        algorithm: HashAlgorithm,
        reason: &'static str,
    },
    Mismatch {
        expected: String,
        actual: String,
    },
}

pub fn verify_content_hash(bytes: &[u8], expected: &ContentHash) -> HashVerification {
    let actual = match expected.algorithm {
        HashAlgorithm::Placeholder => placeholder_hash_hex(bytes),
        HashAlgorithm::Blake3 => blake3::hash(bytes).to_hex().to_string(),
    };
    if actual == expected.hex {
        HashVerification::Verified
    } else {
        HashVerification::Mismatch {
            expected: expected.hex.clone(),
            actual,
        }
    }
}

fn placeholder_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}{:016x}", bytes.len() as u64)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingObject {
    pub path: PathBuf,
    pub bytes_len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectWriteOutcome {
    pub path: PathBuf,
    pub existed: bool,
    pub verification: HashVerification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectWriteOutcomeV2 {
    pub path: PathBuf,
    pub existed: bool,
    pub digest: ObjectDigest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChunkObject {
    pub chunk: ChunkObject,
    pub write: ObjectWriteOutcomeV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFileObject {
    pub file: FileObject,
    pub chunks: Vec<StoredChunkObject>,
    pub file_write: ObjectWriteOutcomeV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTreeObject {
    pub tree: TreeObject,
    pub write: ObjectWriteOutcomeV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLocalStepRef {
    pub step: LocalStepRef,
    pub write: ObjectWriteOutcomeV2,
}

pub(crate) fn write_temp_object(layout: &StoreLayout, bytes: &[u8]) -> io::Result<PendingObject> {
    validate_store_boundaries(layout)?;
    let root =
        PinnedDirectory::open(&layout.root).map_err(|error| io_context(error, "pin store root"))?;
    let temporary = root
        .open_child(layout.tmp_dir.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "temporary directory has no file name",
            )
        })?)
        .map_err(|error| io_context(error, "pin temporary directory"))?;
    let mut attempts = 0_u8;
    loop {
        let name = temp_object_file_name(attempts);
        let path = layout.tmp_dir.join(&name);
        match temporary.create_file(name.as_ref()) {
            Ok(mut pending) => {
                pending
                    .file_mut()
                    .write_all(bytes)
                    .map_err(|error| io_context(error, "write pending object"))?;
                fault::hit(FaultPoint::BeforeTempSync)?;
                pending
                    .file()
                    .sync_all()
                    .map_err(|error| io_context(error, "sync pending object"))?;
                fault::hit(FaultPoint::AfterTempSync)?;
                temporary
                    .sync()
                    .map_err(|error| io_context(error, "sync temporary directory"))?;
                return Ok(PendingObject {
                    path,
                    bytes_len: bytes.len() as u64,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempts < 16 => {
                attempts += 1;
            }
            Err(error) => return Err(io_context(error, "create pending object")),
        }
    }
}

fn io_context(error: io::Error, operation: &str) -> io::Error {
    io::Error::new(error.kind(), format!("{operation}: {error}"))
}

pub(crate) fn write_object_prehashed(
    layout: &StoreLayout,
    bytes: &[u8],
    hash: &ContentHash,
) -> io::Result<ObjectWriteOutcome> {
    require_hash(bytes, hash)?;
    let pending = write_temp_object(layout, bytes)?;
    let target = cas_path(&layout.cas_dir, hash);
    let existed = install_pending(layout, &pending, &target, |stored| {
        require_hash(stored, hash)
    })?;
    Ok(ObjectWriteOutcome {
        path: target,
        existed,
        verification: HashVerification::Verified,
    })
}

pub(crate) fn write_object_v2(
    layout: &StoreLayout,
    bytes: &[u8],
    digest: &ObjectDigest,
) -> io::Result<ObjectWriteOutcomeV2> {
    require_digest(bytes, digest)?;
    let pending = write_temp_object(layout, bytes)?;
    let target = cas_path_for_digest(&layout.cas_dir, digest);
    let existed = install_pending(layout, &pending, &target, |stored| {
        require_digest(stored, digest)
    })?;
    Ok(ObjectWriteOutcomeV2 {
        path: target,
        existed,
        digest: digest.clone(),
    })
}

fn read_file_no_follow(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_read_no_follow(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn require_hash(bytes: &[u8], expected: &ContentHash) -> io::Result<()> {
    match verify_content_hash(bytes, expected) {
        HashVerification::Verified => Ok(()),
        HashVerification::Unavailable { reason, .. } => {
            Err(io::Error::new(io::ErrorKind::Unsupported, reason))
        }
        HashVerification::Mismatch { expected, actual } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("content hash mismatch: expected {expected}, got {actual}"),
        )),
    }
}

fn require_digest(bytes: &[u8], expected: &ObjectDigest) -> io::Result<()> {
    let actual = ObjectDigest::blake3_for(bytes);
    if actual == *expected {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("object digest mismatch: expected {expected}, got {actual}"),
        ))
    }
}

pub(crate) fn write_chunk_v2(layout: &StoreLayout, bytes: &[u8]) -> io::Result<StoredChunkObject> {
    let chunk = ChunkObject::from_bytes(bytes);
    let write = write_object_v2(layout, bytes, &chunk.chunk_id)?;
    Ok(StoredChunkObject { chunk, write })
}

pub(crate) fn build_file_v2(
    layout: &StoreLayout,
    bytes: &[u8],
    chunker: &dyn Chunker,
) -> io::Result<StoredFileObject> {
    let (file, chunks) = FileObject::from_bytes(bytes, chunker)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let mut stored_chunks = Vec::with_capacity(chunks.len());
    for (file_chunk, chunk) in file.chunks.iter().zip(chunks) {
        let start = file_chunk.offset as usize;
        let end = start + file_chunk.byte_len as usize;
        let write = write_object_v2(layout, &bytes[start..end], &chunk.chunk_id)?;
        stored_chunks.push(StoredChunkObject { chunk, write });
    }
    let file_write = write_object_v2(layout, &file.canonical_bytes(), &file.file_object_id)?;
    Ok(StoredFileObject {
        file,
        chunks: stored_chunks,
        file_write,
    })
}

pub(crate) fn write_tree_v2(
    layout: &StoreLayout,
    entries: Vec<TreeEntry>,
) -> io::Result<StoredTreeObject> {
    let tree = TreeObject::from_entries(entries)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let write = write_object_v2(layout, &tree.canonical_bytes(), &tree.tree_id)?;
    Ok(StoredTreeObject { tree, write })
}

pub(crate) fn write_local_step_ref_v2(
    layout: &StoreLayout,
    step: LocalStepRef,
) -> io::Result<StoredLocalStepRef> {
    step.validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    let write = write_object_v2(layout, &step.canonical_bytes(), &step.step_id)?;
    Ok(StoredLocalStepRef { step, write })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScrubReport {
    pub checked_objects: u64,
    pub missing_objects: u64,
    pub unverified_objects: u64,
    pub errors: Vec<String>,
}

impl ScrubReport {
    pub fn is_clean(&self) -> bool {
        self.missing_objects == 0 && self.errors.is_empty()
    }
}

pub(crate) fn scrub(layout: &StoreLayout) -> io::Result<ScrubReport> {
    validate_store_boundaries(layout)?;
    let mut report = ScrubReport::default();
    if layout.cas_dir.exists() {
        scrub_dir(&layout.cas_dir, &layout.cas_dir, &mut report)?;
    }
    Ok(report)
}

fn scrub_dir(root: &Path, path: &Path, report: &mut ScrubReport) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            scrub_dir(root, &entry.path(), report)?;
        } else if file_type.is_file() {
            scrub_file(root, &entry.path(), report)?;
        } else {
            report
                .errors
                .push(format!("non-regular CAS entry: {}", entry.path().display()));
        }
    }
    Ok(())
}

fn scrub_file(root: &Path, path: &Path, report: &mut ScrubReport) -> io::Result<()> {
    report.checked_objects += 1;
    let relative = path.strip_prefix(root).map_err(io::Error::other)?;
    let components: Vec<_> = relative.components().collect();
    if components.len() != 4 {
        report.unverified_objects += 1;
        report
            .errors
            .push(format!("invalid CAS path shape: {}", path.display()));
        return Ok(());
    }
    let algorithm = components[0].as_os_str().to_string_lossy();
    let expected = components[3].as_os_str().to_string_lossy();
    let bytes = read_file_no_follow(path)?;
    let actual = match algorithm.as_ref() {
        "blake3" => blake3::hash(&bytes).to_hex().to_string(),
        "placeholder" => placeholder_hash_hex(&bytes),
        _ => {
            report.unverified_objects += 1;
            report
                .errors
                .push(format!("unknown CAS algorithm at {}", path.display()));
            return Ok(());
        }
    };
    if expected != actual {
        report.errors.push(format!(
            "CAS corruption at {}: expected {}, got {}",
            path.display(),
            expected,
            actual
        ));
    }
    Ok(())
}

fn temp_object_file_name(attempt: u8) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("cas-{}-{timestamp}-{attempt}.tmp", std::process::id())
}

#[cfg(test)]
mod tests;
