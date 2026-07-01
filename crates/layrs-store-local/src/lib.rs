use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use layrs_sync::{
    ChunkObject, Chunker, FileObject, LocalStepRef, ObjectDigest, TreeEntry, TreeObject,
};

pub const DEFAULT_STORE_DIR: &str = ".layrs";
pub const DEFAULT_CAS_DIR: &str = "objects";
pub const DEFAULT_METADATA_DIR: &str = "metadata";
pub const DEFAULT_TMP_DIR: &str = "tmp";
pub const DEFAULT_SQLITE_FILE: &str = "layrs.sqlite3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStoreConfig {
    pub root: PathBuf,
    pub cas_dir_name: String,
    pub metadata_dir_name: String,
    pub tmp_dir_name: String,
    pub sqlite_file_name: String,
    pub sqlite_wal: bool,
}

impl LocalStoreConfig {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cas_dir_name: DEFAULT_CAS_DIR.to_string(),
            metadata_dir_name: DEFAULT_METADATA_DIR.to_string(),
            tmp_dir_name: DEFAULT_TMP_DIR.to_string(),
            sqlite_file_name: DEFAULT_SQLITE_FILE.to_string(),
            sqlite_wal: true,
        }
    }

    pub fn layout(&self) -> StoreLayout {
        StoreLayout {
            root: self.root.clone(),
            cas_dir: self.root.join(&self.cas_dir_name),
            metadata_dir: self.root.join(&self.metadata_dir_name),
            tmp_dir: self.root.join(&self.tmp_dir_name),
            sqlite_path: self
                .root
                .join(&self.metadata_dir_name)
                .join(&self.sqlite_file_name),
            sqlite_wal_path: self
                .root
                .join(&self.metadata_dir_name)
                .join(format!("{}-wal", self.sqlite_file_name)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreLayout {
    pub root: PathBuf,
    pub cas_dir: PathBuf,
    pub metadata_dir: PathBuf,
    pub tmp_dir: PathBuf,
    pub sqlite_path: PathBuf,
    pub sqlite_wal_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct LocalStore {
    config: LocalStoreConfig,
    layout: StoreLayout,
}

impl LocalStore {
    pub fn init(config: LocalStoreConfig) -> io::Result<Self> {
        let layout = config.layout();
        ensure_layout(&layout)?;
        write_sqlite_skeleton(&layout, config.sqlite_wal)?;
        Ok(Self { config, layout })
    }

    pub fn open(config: LocalStoreConfig) -> io::Result<Self> {
        if !config.root.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("store root does not exist: {}", config.root.display()),
            ));
        }

        let layout = config.layout();
        ensure_layout(&layout)?;
        write_sqlite_skeleton(&layout, config.sqlite_wal)?;
        Ok(Self { config, layout })
    }

    pub fn init_or_open(config: LocalStoreConfig) -> io::Result<Self> {
        if config.root.exists() {
            Self::open(config)
        } else {
            Self::init(config)
        }
    }

    pub fn config(&self) -> &LocalStoreConfig {
        &self.config
    }

    pub fn layout(&self) -> &StoreLayout {
        &self.layout
    }

    pub fn cas_path(&self, hash: &ContentHash) -> PathBuf {
        cas_path(&self.layout.cas_dir, hash)
    }

    pub fn cas_path_v2(&self, digest: &ObjectDigest) -> PathBuf {
        cas_path_for_digest(&self.layout.cas_dir, digest)
    }

    pub fn write_temp_object(&self, bytes: &[u8]) -> io::Result<PendingObject> {
        fs::create_dir_all(&self.layout.tmp_dir)?;

        let mut attempts = 0_u8;
        loop {
            let path = self.layout.tmp_dir.join(temp_object_file_name(attempts));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(bytes)?;
                    file.sync_all()?;
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

    pub fn write_object_prehashed(
        &self,
        bytes: &[u8],
        hash: &ContentHash,
    ) -> io::Result<ObjectWriteOutcome> {
        match verify_content_hash(bytes, hash) {
            HashVerification::Mismatch { expected, actual } => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("content hash mismatch: expected {expected}, got {actual}"),
                ));
            }
            HashVerification::Verified | HashVerification::Unavailable { .. } => {}
        }

        let pending = self.write_temp_object(bytes)?;
        let target = self.cas_path(hash);

        if target.exists() {
            fs::remove_file(&pending.path)?;
            return Ok(ObjectWriteOutcome {
                path: target,
                existed: true,
                verification: verify_content_hash(bytes, hash),
            });
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::rename(&pending.path, &target)?;
        Ok(ObjectWriteOutcome {
            path: target,
            existed: false,
            verification: verify_content_hash(bytes, hash),
        })
    }

    pub fn write_object_v2(
        &self,
        bytes: &[u8],
        digest: &ObjectDigest,
    ) -> io::Result<ObjectWriteOutcomeV2> {
        let actual = ObjectDigest::blake3_for(bytes);
        if actual != *digest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("object digest mismatch: expected {digest}, got {actual}"),
            ));
        }

        let pending = self.write_temp_object(bytes)?;
        let target = self.cas_path_v2(digest);

        if target.exists() {
            fs::remove_file(&pending.path)?;
            return Ok(ObjectWriteOutcomeV2 {
                path: target,
                existed: true,
                digest: digest.clone(),
            });
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::rename(&pending.path, &target)?;
        Ok(ObjectWriteOutcomeV2 {
            path: target,
            existed: false,
            digest: digest.clone(),
        })
    }

    pub fn write_chunk_v2(&self, bytes: &[u8]) -> io::Result<StoredChunkObject> {
        let chunk = ChunkObject::from_bytes(bytes);
        let write = self.write_object_v2(bytes, &chunk.chunk_id)?;
        Ok(StoredChunkObject { chunk, write })
    }

    pub fn build_file_v2(
        &self,
        bytes: &[u8],
        chunker: &dyn Chunker,
    ) -> io::Result<StoredFileObject> {
        let (file, chunks) = FileObject::from_bytes(bytes, chunker)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let mut stored_chunks = Vec::with_capacity(chunks.len());

        for (file_chunk, chunk) in file.chunks.iter().zip(chunks) {
            let start = file_chunk.offset as usize;
            let end = start + file_chunk.byte_len as usize;
            let write = self.write_object_v2(&bytes[start..end], &chunk.chunk_id)?;
            stored_chunks.push(StoredChunkObject { chunk, write });
        }

        let file_write = self.write_object_v2(&file.canonical_bytes(), &file.file_object_id)?;
        Ok(StoredFileObject {
            file,
            chunks: stored_chunks,
            file_write,
        })
    }

    pub fn write_tree_v2(&self, entries: Vec<TreeEntry>) -> io::Result<StoredTreeObject> {
        let tree = TreeObject::from_entries(entries)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let write = self.write_object_v2(&tree.canonical_bytes(), &tree.tree_id)?;
        Ok(StoredTreeObject { tree, write })
    }

    pub fn write_local_step_ref_v2(&self, step: LocalStepRef) -> io::Result<StoredLocalStepRef> {
        step.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        let write = self.write_object_v2(&step.canonical_bytes(), &step.step_id)?;
        Ok(StoredLocalStepRef { step, write })
    }

    pub fn scrub(&self) -> io::Result<ScrubReport> {
        let mut report = ScrubReport::default();

        if !self.layout.cas_dir.exists() {
            return Ok(report);
        }

        scrub_dir(&self.layout.cas_dir, &mut report)?;
        Ok(report)
    }
}

fn ensure_layout(layout: &StoreLayout) -> io::Result<()> {
    fs::create_dir_all(&layout.root)?;
    fs::create_dir_all(&layout.cas_dir)?;
    fs::create_dir_all(&layout.metadata_dir)?;
    fs::create_dir_all(&layout.tmp_dir)?;
    Ok(())
}

fn write_sqlite_skeleton(layout: &StoreLayout, sqlite_wal: bool) -> io::Result<()> {
    let marker = layout.metadata_dir.join("store-layout.txt");
    let body = format!(
        "layrs local store layout v1\nsqlite_path={}\nsqlite_wal_target={}\ncas_dir={}\n",
        layout.sqlite_path.display(),
        sqlite_wal,
        layout.cas_dir.display()
    );
    fs::write(marker, body)?;

    if !layout.sqlite_path.exists() {
        File::create(&layout.sqlite_path)?.sync_all()?;
    }

    Ok(())
}

pub fn cas_path(cas_dir: impl AsRef<Path>, hash: &ContentHash) -> PathBuf {
    let hex = hash.hex();
    let first = &hex[0..2];
    let second = &hex[2..4];
    cas_dir
        .as_ref()
        .join(hash.algorithm().as_str())
        .join(first)
        .join(second)
        .join(hex)
}

pub fn cas_path_for_digest(cas_dir: impl AsRef<Path>, digest: &ObjectDigest) -> PathBuf {
    let hex = digest.hex();
    let first = &hex[0..2];
    let second = &hex[2..4];
    cas_dir
        .as_ref()
        .join(digest.algorithm())
        .join(first)
        .join(second)
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
    match expected.algorithm {
        HashAlgorithm::Placeholder => {
            let actual = placeholder_hash_hex(bytes);
            if actual == expected.hex {
                HashVerification::Verified
            } else {
                HashVerification::Mismatch {
                    expected: expected.hex.clone(),
                    actual,
                }
            }
        }
        HashAlgorithm::Blake3 => {
            let actual = blake3::hash(bytes).to_hex().to_string();
            if actual == expected.hex {
                HashVerification::Verified
            } else {
                HashVerification::Mismatch {
                    expected: expected.hex.clone(),
                    actual,
                }
            }
        }
    }
}

fn placeholder_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    let len = bytes.len() as u64;
    format!("{hash:016x}{len:016x}")
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

fn scrub_dir(path: &Path, report: &mut ScrubReport) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            scrub_dir(&entry.path(), report)?;
        } else if file_type.is_file() {
            report.checked_objects += 1;
            report.unverified_objects += 1;
        }
    }

    Ok(())
}

fn temp_object_file_name(attempt: u8) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let process_id = std::process::id();
    format!("cas-{process_id}-{timestamp}-{attempt}.tmp")
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn init_creates_expected_layout() {
        let root = unique_test_dir("layout");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");

        assert!(store.layout().cas_dir.is_dir());
        assert!(store.layout().metadata_dir.is_dir());
        assert!(store.layout().tmp_dir.is_dir());
        assert!(store.layout().sqlite_path.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_object_places_bytes_at_content_address() {
        let root = unique_test_dir("cas-write");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");
        let bytes = b"hello layrs";
        let hash = ContentHash::placeholder_for(bytes);

        let outcome = store
            .write_object_prehashed(bytes, &hash)
            .expect("write object");

        assert!(!outcome.existed);
        assert_eq!(outcome.verification, HashVerification::Verified);
        assert_eq!(fs::read(outcome.path).expect("read object"), bytes);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn blake3_hashes_are_verified() {
        let bytes = b"verified by blake3";
        let hash = ContentHash::blake3_for(bytes);

        assert_eq!(
            verify_content_hash(bytes, &hash),
            HashVerification::Verified
        );
        assert!(matches!(
            verify_content_hash(b"different", &hash),
            HashVerification::Mismatch { .. }
        ));
    }

    #[test]
    fn write_chunk_v2_uses_blake3_digest_path() {
        let root = unique_test_dir("chunk-v2");
        let store = LocalStore::init(LocalStoreConfig::new(&root)).expect("init store");

        let stored = store.write_chunk_v2(b"hello v2").expect("write chunk");

        assert_eq!(stored.chunk.chunk_id, stored.write.digest);
        assert!(stored.write.path.is_file());
        assert!(stored.write.path.ends_with(stored.chunk.chunk_id.hex()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn build_file_tree_and_step_v2_persist_merkle_objects() {
        let root = unique_test_dir("objects-v2");
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

        let _ = fs::remove_dir_all(root);
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "layrs-store-local-{label}-{}-{timestamp}",
            std::process::id()
        ))
    }
}
