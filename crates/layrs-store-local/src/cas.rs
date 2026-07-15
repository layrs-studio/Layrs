//! Durable content-addressed object installation and verification.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_sync::{
    ChunkObject, Chunker, FileObject, LocalStepRef, ObjectDigest, TreeEntry, TreeObject,
};

use crate::fault::{self, FaultPoint};
#[cfg(windows)]
use crate::layout::durable_rename;
use crate::layout::{
    StoreLayout, create_dir_all_durable, open_read_no_follow, reject_reparse_components,
    sync_directory, validate_store_boundaries,
};

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
    create_dir_all_durable(&layout.tmp_dir)?;
    let mut attempts = 0_u8;
    loop {
        let path = layout.tmp_dir.join(temp_object_file_name(attempts));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                fault::hit(FaultPoint::BeforeTempSync)?;
                file.sync_all()?;
                fault::hit(FaultPoint::AfterTempSync)?;
                sync_directory(&layout.tmp_dir)?;
                return Ok(PendingObject {
                    path,
                    bytes_len: bytes.len() as u64,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempts < 16 => {
                attempts += 1;
            }
            Err(error) => return Err(error),
        }
    }
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

fn install_pending(
    layout: &StoreLayout,
    pending: &PendingObject,
    target: &Path,
    verify: impl Fn(&[u8]) -> io::Result<()>,
) -> io::Result<bool> {
    fault::hit(FaultPoint::BeforeTempHash)?;
    validate_store_boundaries(layout)?;
    if !pending.path.starts_with(&layout.tmp_dir) || !target.starts_with(&layout.cas_dir) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "CAS publication path escapes the store root",
        ));
    }
    verify(&read_file_no_follow(&pending.path)?)?;

    match fs::symlink_metadata(target) {
        Ok(_) => {
            let target_bytes = read_file_no_follow(target)?;
            let result = verify(&target_bytes);
            cleanup_pending(layout, &pending.path)?;
            result?;
            return Ok(true);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "CAS target has no parent directory",
        )
    })?;
    create_dir_all_durable(parent)?;
    reject_reparse_components(parent)?;
    fault::hit(FaultPoint::BeforeRename)?;
    if publish_no_clobber(&pending.path, target)? {
        let target_bytes = read_file_no_follow(target)?;
        let result = verify(&target_bytes);
        cleanup_pending(layout, &pending.path)?;
        result?;
        return Ok(true);
    }
    verify(&read_file_no_follow(target)?)?;
    fault::hit(FaultPoint::AfterRename)?;
    fault::hit(FaultPoint::BeforeDirectorySync)?;
    sync_directory(parent)?;
    sync_directory(&layout.tmp_dir)?;
    Ok(false)
}

fn publish_no_clobber(source: &Path, destination: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        match fs::hard_link(source, destination) {
            Ok(()) => {
                fs::remove_file(source)?;
                Ok(false)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(true),
            Err(error) => Err(error),
        }
    }

    #[cfg(windows)]
    {
        match durable_rename(source, destination) {
            Ok(()) => Ok(false),
            Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists
                    || fs::symlink_metadata(destination).is_ok() =>
            {
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (source, destination);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-clobber publication is unsupported on this platform",
        ))
    }
}

fn read_file_no_follow(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = open_read_no_follow(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn cleanup_pending(layout: &StoreLayout, path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => sync_directory(&layout.tmp_dir),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::fault;
    use crate::{LocalStore, LocalStoreConfig};
    use layrs_sync::{DeterministicChunker, LocalStepRef, TreeEntry};

    #[test]
    fn cas_path_shards_by_algorithm_and_first_four_hex_chars() {
        let hash = ContentHash::blake3("abcdef0123456789").expect("valid hash");
        let path = cas_path(Path::new("store").join("objects"), &hash);

        assert_eq!(
            path,
            Path::new("store")
                .join("objects")
                .join("blake3")
                .join("ab")
                .join("cd")
                .join("abcdef0123456789")
        );
    }

    #[test]
    fn placeholder_object_write_preserves_public_cas_behavior() {
        let root = unique_test_dir("placeholder");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
        let bytes = b"hello layrs";
        let hash = ContentHash::placeholder_for(bytes);

        let outcome = store
            .write_object_prehashed(bytes, &hash)
            .expect("write object");
        assert!(!outcome.existed);
        assert_eq!(outcome.verification, HashVerification::Verified);
        assert_eq!(fs::read(outcome.path).expect("read object"), bytes);

        drop(store);
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn build_file_tree_and_step_v2_persists_merkle_objects() {
        let root = unique_test_dir("merkle-v2");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
        let chunker = DeterministicChunker::default();

        let stored_file = store
            .build_file_v2(b"hello merkle store", &chunker)
            .expect("build file");
        let stored_tree = store
            .write_tree_v2(vec![TreeEntry::file("src/hello.txt", &stored_file.file)])
            .expect("write tree");
        let step = LocalStepRef::new(
            "layer-main",
            stored_tree.tree.tree_id.clone(),
            None,
            1,
            "2026-06-30T12:00:00Z",
            None,
        )
        .expect("step ref");
        let stored_step = store.write_local_step_ref_v2(step).expect("write step ref");

        assert_eq!(stored_file.chunks.len(), 1);
        assert!(stored_file.file_write.path.is_file());
        assert!(stored_tree.write.path.is_file());
        assert!(stored_step.write.path.is_file());

        drop(store);
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn detects_existing_corrupt_object_instead_of_accepting_it() {
        let root = unique_test_dir("corrupt");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
        let bytes = b"durable bytes";
        let hash = ContentHash::blake3_for(bytes);
        let first = store
            .write_object_prehashed(bytes, &hash)
            .expect("write object");
        fs::write(&first.path, b"corrupt").expect("corrupt object");

        let error = store
            .write_object_prehashed(bytes, &hash)
            .expect_err("corruption must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        drop(store);
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn fault_before_rename_never_publishes_target() {
        let root = unique_test_dir("fault");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
        let bytes = b"not published";
        let hash = ContentHash::blake3_for(bytes);
        fault::fail_once(FaultPoint::BeforeRename);

        store
            .write_object_prehashed(bytes, &hash)
            .expect_err("fault must abort write");
        assert!(!store.cas_path(&hash).exists());

        drop(store);
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn concurrent_publishers_never_clobber_the_atomic_winner() {
        let root = unique_test_dir("publish-race");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
        let first_bytes = b"first synchronized writer".to_vec();
        let second_bytes = b"second synchronized writer".to_vec();
        let first_pending =
            write_temp_object(store.layout(), &first_bytes).expect("first pending object");
        let second_pending =
            write_temp_object(store.layout(), &second_bytes).expect("second pending object");
        let target = store.layout().cas_dir.join("synchronized-race-winner");
        let barrier = Arc::new(Barrier::new(2));

        let first = spawn_racing_publish(
            store.layout().clone(),
            first_pending,
            target.clone(),
            first_bytes.clone(),
            Arc::clone(&barrier),
        );
        let second = spawn_racing_publish(
            store.layout().clone(),
            second_pending,
            target.clone(),
            second_bytes.clone(),
            barrier,
        );
        let outcomes = [
            first.join().expect("first writer thread"),
            second.join().expect("second writer thread"),
        ];

        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Ok(false)))
                .count(),
            1
        );
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_err()).count(),
            1
        );
        let winner = fs::read(&target).expect("read atomic winner");
        assert!(winner == first_bytes || winner == second_bytes);

        drop(store);
        fs::remove_dir_all(root).expect("remove root");
    }

    fn spawn_racing_publish(
        layout: StoreLayout,
        pending: PendingObject,
        target: PathBuf,
        expected: Vec<u8>,
        barrier: Arc<Barrier>,
    ) -> thread::JoinHandle<io::Result<bool>> {
        thread::spawn(move || {
            let first_verification = AtomicBool::new(true);
            install_pending(&layout, &pending, &target, |stored| {
                if first_verification.swap(false, Ordering::SeqCst) {
                    barrier.wait();
                }
                if stored == expected {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "another synchronized writer won publication",
                    ))
                }
            })
        })
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "layrs-store-local-cas-{label}-{}-{timestamp}",
            std::process::id()
        ))
    }
}
