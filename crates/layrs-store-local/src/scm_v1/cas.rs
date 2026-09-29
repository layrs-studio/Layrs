//! Native SCM V1 content-addressed storage.
//!
//! This module installs immutable bytes only. Space admission and mutable
//! database references are deliberately handled by higher layers after every
//! required object has been installed successfully.

use super::layout::{
    NativeStoreAudit, NativeStoreExclusive, NativeStoreLayout, NativeStoreOpen,
    STAGING_LEASE_FILE_NAME,
};
use crate::layout::{open_read_write_no_follow, reject_reparse_components};
use crate::secure_fs::{PinnedDirectory, PinnedFile};
use fs2::FileExt;
use layrs_core::scm::{
    Chunk, ChunkId, ConflictId, FileManifest, FileManifestId, RepositoryView, RepositoryViewId,
    Step, StepId, Symlink, SymlinkId, Tombstone, TombstoneId, Tree, TreeId, canonical_json,
};
use layrs_store::scm::{CanonicalConflict, CanonicalObject, StoreObjectId, StoreObjectKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const OBJECTS_DIR: &str = "objects";
const STAGING_DIR: &str = "staging";
const ENVELOPE_VERSION: &str = "layrs.cas-envelope.v1";
const STAGING_PREFIX: &str = "scm-v1-";
const IO_BUFFER_SIZE: usize = 64 * 1024;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CasFaultPoint {
    BeforeStagingSync,
    AfterStagingSync,
    AfterStagingValidation,
    BeforePublication,
    AfterPublication,
    AfterObjectDirectorySync,
    BeforeStagingCleanup,
    AfterStagingCleanup,
    AfterStagingDirectorySync,
}

/// Injectable durable-boundary hook. Production callers normally use the
/// default no-op implementation; tests may return a descriptive failure.
pub trait CasFaultInjector: Send + Sync {
    fn check(&self, point: CasFaultPoint) -> std::result::Result<(), String>;
}

#[derive(Debug, Default)]
struct NoFaults;

impl CasFaultInjector for NoFaults {
    fn check(&self, _point: CasFaultPoint) -> std::result::Result<(), String> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasError {
    Io {
        operation: &'static str,
        path: PathBuf,
        message: String,
    },
    InvalidObjectId {
        kind: StoreObjectKind,
        id: String,
    },
    NotFound {
        kind: StoreObjectKind,
        id: String,
    },
    KindMismatch {
        expected: StoreObjectKind,
        actual: String,
    },
    IdMismatch {
        expected: StoreObjectId,
        actual: String,
    },
    Corruption {
        kind: StoreObjectKind,
        id: String,
        reason: String,
    },
    ExistingObjectMismatch {
        kind: StoreObjectKind,
        id: String,
    },
    InvalidStagingPath(PathBuf),
    InjectedFault {
        point: CasFaultPoint,
        message: String,
    },
    UnsupportedPublication,
}

impl CasError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "cas_io_error",
            Self::InvalidObjectId { .. } => "cas_invalid_object_id",
            Self::NotFound { .. } => "cas_object_not_found",
            Self::KindMismatch { .. } => "cas_kind_mismatch",
            Self::IdMismatch { .. } => "cas_id_mismatch",
            Self::Corruption { .. } => "cas_corruption",
            Self::ExistingObjectMismatch { .. } => "cas_existing_object_mismatch",
            Self::InvalidStagingPath(_) => "cas_invalid_staging_path",
            Self::InjectedFault { .. } => "cas_injected_fault",
            Self::UnsupportedPublication => "cas_publication_unsupported",
        }
    }
}

impl fmt::Display for CasError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                message,
            } => write!(formatter, "{operation} {}: {message}", path.display()),
            Self::InvalidObjectId { kind, id } => {
                write!(formatter, "invalid {kind} object id {id}")
            }
            Self::NotFound { kind, id } => write!(formatter, "{kind} object {id} was not found"),
            Self::KindMismatch { expected, actual } => {
                write!(formatter, "expected {expected} envelope, found {actual}")
            }
            Self::IdMismatch { expected, actual } => {
                write!(formatter, "expected object id {expected}, found {actual}")
            }
            Self::Corruption { kind, id, reason } => {
                write!(formatter, "corrupt {kind} object {id}: {reason}")
            }
            Self::ExistingObjectMismatch { kind, id } => write!(
                formatter,
                "existing {kind} object {id} differs byte-for-byte from the candidate"
            ),
            Self::InvalidStagingPath(path) => {
                write!(
                    formatter,
                    "staging path escapes its directory: {}",
                    path.display()
                )
            }
            Self::InjectedFault { point, message } => {
                write!(formatter, "injected fault at {point:?}: {message}")
            }
            Self::UnsupportedPublication => {
                formatter.write_str("atomic no-clobber CAS publication is unsupported")
            }
        }
    }
}

impl Error for CasError {}

pub type CasResult<T> = std::result::Result<T, CasError>;

#[derive(Debug)]
struct StagingLease {
    file: fs::File,
}

impl StagingLease {
    fn shared(path: &Path) -> CasResult<Self> {
        let file = open_read_write_no_follow(path)
            .map_err(|error| io_error("open shared staging lease", path, error))?;
        FileExt::lock_shared(&file)
            .map_err(|error| io_error("lock shared staging lease", path, error))?;
        Ok(Self { file })
    }

    fn exclusive(path: &Path) -> CasResult<Self> {
        let file = open_read_write_no_follow(path)
            .map_err(|error| io_error("open exclusive staging lease", path, error))?;
        FileExt::lock_exclusive(&file)
            .map_err(|error| io_error("lock exclusive staging lease", path, error))?;
        Ok(Self { file })
    }
}

impl Drop for StagingLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[derive(Debug)]
pub struct PreparedObject {
    object_id: StoreObjectId,
    path: PathBuf,
    bytes: Vec<u8>,
    _lease: StagingLease,
}

impl PreparedObject {
    pub fn object_id(&self) -> &StoreObjectId {
        &self.object_id
    }

    pub fn kind(&self) -> StoreObjectKind {
        self.object_id.kind()
    }

    pub fn staging_path(&self) -> &Path {
        &self.path
    }

    pub fn bytes_len(&self) -> u64 {
        self.bytes.len() as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub object_id: StoreObjectId,
    pub path: PathBuf,
    pub existed: bool,
    pub bytes_len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CasScrubIssue {
    pub path: PathBuf,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CasScrubReport {
    pub checked_objects: u64,
    pub issues: Vec<CasScrubIssue>,
}

impl CasScrubReport {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Clone)]
pub struct NativeCas {
    root: PathBuf,
    objects_dir: PathBuf,
    staging_dir: PathBuf,
    staging_lease_path: PathBuf,
    faults: Arc<dyn CasFaultInjector>,
}

/// Mutating CAS view whose lifetime is fenced by an exclusive Store
/// capability. It cannot survive the lock that authorizes publication or
/// staging maintenance.
pub struct NativeCasWriter<'store> {
    cas: NativeCas,
    _exclusive: &'store NativeStoreExclusive,
}

/// Read-only CAS view created exclusively from a native audit capability.
#[derive(Debug, Clone)]
pub struct NativeCasAudit {
    cas: NativeCas,
}

impl NativeCasAudit {
    pub fn objects_dir(&self) -> &Path {
        self.cas.objects_dir()
    }

    pub fn object_path(&self, object_id: &StoreObjectId) -> CasResult<PathBuf> {
        self.cas.object_path(object_id)
    }

    pub fn contains(&self, object_id: &StoreObjectId) -> CasResult<bool> {
        self.cas.contains(object_id)
    }

    pub fn read(&self, object_id: &StoreObjectId) -> CasResult<CanonicalObject> {
        self.cas.read(object_id)
    }

    pub fn verify(&self, object_id: &StoreObjectId) -> CasResult<()> {
        self.cas.verify(object_id)
    }

    /// Streams and verifies one Chunk through the same pinned, no-follow file
    /// handle. The callback never observes bytes from a path reopened after
    /// validation.
    pub fn stream_verified_chunk(
        &self,
        chunk_id: &ChunkId,
        consume: impl FnMut(&[u8]),
    ) -> CasResult<u64> {
        self.cas.stream_verified_chunk(chunk_id, consume)
    }
}

impl fmt::Debug for NativeCasWriter<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeCasWriter")
            .field("cas", &self.cas)
            .finish_non_exhaustive()
    }
}

impl Deref for NativeCasWriter<'_> {
    type Target = NativeCas;

    fn deref(&self) -> &Self::Target {
        &self.cas
    }
}

impl NativeCasWriter<'_> {
    pub(crate) fn store_id(&self) -> &layrs_core::scm::StoreId {
        self._exclusive.store_id()
    }
    /// Borrows the verified CAS read surface while retaining the writer's
    /// exclusive Store capability as the lifetime fence.
    pub(crate) fn reader(&self) -> &NativeCas {
        &self.cas
    }
}

impl fmt::Debug for NativeCas {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeCas")
            .field("root", &self.root)
            .field("objects_dir", &self.objects_dir)
            .field("staging_dir", &self.staging_dir)
            .field("staging_lease_path", &self.staging_lease_path)
            .finish_non_exhaustive()
    }
}

impl NativeCas {
    pub fn open(store: &NativeStoreOpen) -> CasResult<Self> {
        Self::with_fault_injector(store, Arc::new(NoFaults))
    }

    /// Opens a read-only CAS view under an authenticated audit capability.
    pub fn open_audit(store: &NativeStoreAudit) -> CasResult<NativeCasAudit> {
        Self::open_layout(store.layout(), Arc::new(NoFaults)).map(|cas| NativeCasAudit { cas })
    }

    /// Opens CAS maintenance under an already-acquired exclusive Store
    /// capability. This does not create or repair any path.
    pub fn open_exclusive(store: &NativeStoreExclusive) -> CasResult<NativeCasWriter<'_>> {
        NativeCasWriter::open(store)
    }

    pub fn with_fault_injector(
        store: &NativeStoreOpen,
        faults: Arc<dyn CasFaultInjector>,
    ) -> CasResult<Self> {
        Self::open_layout(store.layout(), faults)
    }

    fn open_layout(
        layout: &NativeStoreLayout,
        faults: Arc<dyn CasFaultInjector>,
    ) -> CasResult<Self> {
        let root = layout.root().to_path_buf();
        let objects_dir = layout.objects_dir().to_path_buf();
        let staging_dir = layout.staging_dir().to_path_buf();
        let staging_lease_path = layout.staging_lease_path().to_path_buf();
        for directory in [&root, &objects_dir, &staging_dir] {
            reject_reparse_components(directory)
                .map_err(|error| io_error("validate CAS directory", directory, error))?;
            let metadata = fs::symlink_metadata(directory)
                .map_err(|error| io_error("inspect CAS directory", directory, error))?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(io_error(
                    "validate CAS directory",
                    directory,
                    io::Error::new(io::ErrorKind::InvalidData, "expected a regular directory"),
                ));
            }
        }
        open_read_write_no_follow(&staging_lease_path)
            .map_err(|error| io_error("validate staging lease", &staging_lease_path, error))?;
        Ok(Self {
            root,
            objects_dir,
            staging_dir,
            staging_lease_path,
            faults,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }

    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    pub fn object_path(&self, object_id: &StoreObjectId) -> CasResult<PathBuf> {
        Ok(self.objects_dir.join(object_relative_path(object_id)?))
    }

    /// Encodes, writes, syncs, rereads, and validates one exclusive staging
    /// file. No database reference should be created from a PreparedObject.
    fn stage(&self, object: &CanonicalObject) -> CasResult<PreparedObject> {
        let lease = StagingLease::shared(&self.staging_lease_path)?;
        let object_id = object.object_id();
        let bytes = encode_object(object)?;
        validate_stored_bytes(&object_id, &bytes)?;
        let staging = self.pinned_staging()?;
        let mut last_error = None;
        for _ in 0..64 {
            let name = staging_name();
            match staging.create_file(OsStr::new(&name)) {
                Ok(mut file) => {
                    let path = self.staging_dir.join(&name);
                    file.file_mut()
                        .write_all(&bytes)
                        .map_err(|error| io_error("write staged object", &path, error))?;
                    self.hit(CasFaultPoint::BeforeStagingSync)?;
                    file.file()
                        .sync_all()
                        .map_err(|error| io_error("sync staged object", &path, error))?;
                    self.hit(CasFaultPoint::AfterStagingSync)?;
                    sync_directory_compatible(staging.sync())
                        .map_err(|error| io_error("sync staging directory", &path, error))?;
                    let reread = read_pinned(&mut file, &path)?;
                    if reread != bytes {
                        return Err(CasError::Corruption {
                            kind: object_id.kind(),
                            id: object_id.to_string(),
                            reason: "staging reread differs from encoded bytes".into(),
                        });
                    }
                    validate_stored_bytes(&object_id, &reread)?;
                    self.hit(CasFaultPoint::AfterStagingValidation)?;
                    return Ok(PreparedObject {
                        object_id,
                        path,
                        bytes,
                        _lease: lease,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    last_error = Some(error);
                }
                Err(error) => {
                    return Err(io_error(
                        "create exclusive staging object",
                        &self.staging_dir,
                        error,
                    ));
                }
            }
        }
        Err(io_error(
            "allocate exclusive staging object",
            &self.staging_dir,
            last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "collision")),
        ))
    }

    /// Publishes a validated staging object without replacing an existing
    /// path. Success means the immutable CAS object is durable and may be
    /// referenced by a later database transaction.
    fn install(&self, prepared: PreparedObject) -> CasResult<InstallOutcome> {
        let name = contained_name(&self.staging_dir, &prepared.path)?;
        let staging = self.pinned_staging()?;
        let mut source = staging
            .open_file(name)
            .map_err(|error| io_error("open staged object", &prepared.path, error))?;
        let staged = read_pinned(&mut source, &prepared.path)?;
        if staged != prepared.bytes {
            return Err(CasError::Corruption {
                kind: prepared.kind(),
                id: prepared.object_id.to_string(),
                reason: "staging object changed before publication".into(),
            });
        }
        validate_stored_bytes(&prepared.object_id, &staged)?;

        let relative = object_relative_path(&prepared.object_id)?;
        let objects = self.pinned_objects()?;
        let (destination, target_name) = self.destination_for(objects, &relative)?;
        let target = self.objects_dir.join(&relative);

        match destination.open_file(&target_name) {
            Ok(mut existing) => {
                self.verify_existing(&mut existing, &prepared, &target)?;
                self.cleanup_staging(&source, &prepared.path)?;
                return Ok(outcome(&prepared, target, true));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("inspect publication target", &target, error)),
        }

        self.hit(CasFaultPoint::BeforePublication)?;
        let existed = publish_no_clobber(&source, &destination, &target_name)?;
        let mut installed = destination
            .open_file(&target_name)
            .map_err(|error| io_error("reread published object", &target, error))?;
        self.verify_existing(&mut installed, &prepared, &target)?;
        if !existed && !source.same_file(&installed) {
            return Err(CasError::ExistingObjectMismatch {
                kind: prepared.kind(),
                id: prepared.object_id.to_string(),
            });
        }
        self.hit(CasFaultPoint::AfterPublication)?;
        sync_directory_compatible(destination.sync())
            .map_err(|error| io_error("sync object shard", &target, error))?;
        self.hit(CasFaultPoint::AfterObjectDirectorySync)?;
        self.cleanup_published_staging(&source, &prepared.path, existed)?;
        Ok(outcome(&prepared, target, existed))
    }

    fn put(&self, object: &CanonicalObject) -> CasResult<InstallOutcome> {
        let prepared = self.stage(object)?;
        self.install(prepared)
    }

    pub fn contains(&self, object_id: &StoreObjectId) -> CasResult<bool> {
        let relative = object_relative_path(object_id)?;
        let objects = self.pinned_objects()?;
        match objects.open_relative_file(&relative) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(io_error(
                "check CAS object",
                &self.objects_dir.join(relative),
                error,
            )),
        }
    }

    pub fn read(&self, object_id: &StoreObjectId) -> CasResult<CanonicalObject> {
        let relative = object_relative_path(object_id)?;
        let path = self.objects_dir.join(&relative);
        let objects = self.pinned_objects()?;
        let mut file = match objects.open_relative_file(&relative) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(CasError::NotFound {
                    kind: object_id.kind(),
                    id: object_id.to_string(),
                });
            }
            Err(error) => return Err(io_error("open CAS object", &path, error)),
        };
        let bytes = read_pinned(&mut file, &path)?;
        validate_stored_bytes(object_id, &bytes)
    }

    /// Verifies one object. Chunk hashing is streamed; JSON envelopes are
    /// reread and decoded through Core constructors, which recompute claimed
    /// identities and reject non-canonical bytes.
    pub fn verify(&self, object_id: &StoreObjectId) -> CasResult<()> {
        let relative = object_relative_path(object_id)?;
        let path = self.objects_dir.join(&relative);
        let objects = self.pinned_objects()?;
        let mut file = match objects.open_relative_file(&relative) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(CasError::NotFound {
                    kind: object_id.kind(),
                    id: object_id.to_string(),
                });
            }
            Err(error) => return Err(io_error("open CAS object", &path, error)),
        };
        if object_id.kind() == StoreObjectKind::ChunkRaw {
            verify_chunk_stream(file.file_mut(), object_id, &path)
        } else {
            let bytes = read_pinned(&mut file, &path)?;
            validate_stored_bytes(object_id, &bytes).map(|_| ())
        }
    }

    pub fn stream_verified_chunk(
        &self,
        chunk_id: &ChunkId,
        mut consume: impl FnMut(&[u8]),
    ) -> CasResult<u64> {
        let object_id = StoreObjectId::ChunkRaw(chunk_id.clone());
        let relative = object_relative_path(&object_id)?;
        let path = self.objects_dir.join(&relative);
        let objects = self.pinned_objects()?;
        let mut file = match objects.open_relative_file(&relative) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(CasError::NotFound {
                    kind: StoreObjectKind::ChunkRaw,
                    id: chunk_id.to_string(),
                });
            }
            Err(error) => return Err(io_error("open CAS Chunk", &path, error)),
        };
        stream_and_verify_chunk(file.file_mut(), &object_id, &path, &mut consume)
    }

    /// Iteratively checks every physical SCM V1 object without consulting any
    /// Space admission table.
    pub fn scrub(&self) -> CasResult<CasScrubReport> {
        reject_reparse_components(&self.objects_dir)
            .map_err(|error| io_error("validate objects directory", &self.objects_dir, error))?;
        let mut report = CasScrubReport::default();
        let mut pending = vec![self.objects_dir.clone()];
        while let Some(directory) = pending.pop() {
            let entries = fs::read_dir(&directory)
                .map_err(|error| io_error("read CAS directory", &directory, error))?;
            for entry in entries {
                let entry = entry.map_err(|error| io_error("read CAS entry", &directory, error))?;
                let path = entry.path();
                let file_type = entry
                    .file_type()
                    .map_err(|error| io_error("inspect CAS entry", &path, error))?;
                if file_type.is_dir() {
                    pending.push(path);
                    continue;
                }
                report.checked_objects += 1;
                if !file_type.is_file() {
                    report.issues.push(CasScrubIssue {
                        path,
                        code: "cas_non_regular_entry",
                        message: "CAS entry is not a regular file".into(),
                    });
                    continue;
                }
                let object_id = match object_id_from_path(&self.objects_dir, &path) {
                    Ok(id) => id,
                    Err(error) => {
                        report.issues.push(CasScrubIssue {
                            path,
                            code: error.code(),
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                if let Err(error) = self.verify(&object_id) {
                    report.issues.push(CasScrubIssue {
                        path,
                        code: error.code(),
                        message: error.to_string(),
                    });
                }
            }
        }
        report
            .issues
            .sort_by(|left, right| left.path.cmp(&right.path));
        Ok(report)
    }

    /// Removes orphaned exclusive staging files and durably syncs the staging
    /// directory. Non-regular or foreign entries are rejected, never followed.
    fn recover_staging(&self) -> CasResult<u64> {
        let _exclusive_lease = StagingLease::exclusive(&self.staging_lease_path)?;
        let staging = self.pinned_staging()?;
        let mut names = BTreeSet::new();
        for entry in fs::read_dir(&self.staging_dir)
            .map_err(|error| io_error("read staging directory", &self.staging_dir, error))?
        {
            let entry =
                entry.map_err(|error| io_error("read staging entry", &self.staging_dir, error))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| io_error("inspect staging entry", &path, error))?;
            let name = entry.file_name();
            if name == OsStr::new(STAGING_LEASE_FILE_NAME) {
                continue;
            }
            if !file_type.is_file() || !name.to_string_lossy().starts_with(STAGING_PREFIX) {
                return Err(CasError::InvalidStagingPath(path));
            }
            names.insert(name);
        }
        let mut removed = 0;
        for name in names {
            let file = staging.open_file(&name).map_err(|error| {
                io_error("open orphan staging object", &self.staging_dir, error)
            })?;
            file.unlink().map_err(|error| {
                io_error("remove orphan staging object", &self.staging_dir, error)
            })?;
            removed += 1;
        }
        sync_directory_compatible(staging.sync()).map_err(|error| {
            io_error("sync recovered staging directory", &self.staging_dir, error)
        })?;
        Ok(removed)
    }

    fn pinned_root(&self) -> CasResult<PinnedDirectory> {
        PinnedDirectory::open(&self.root)
            .map_err(|error| io_error("pin CAS root", &self.root, error))
    }

    fn pinned_objects(&self) -> CasResult<PinnedDirectory> {
        self.pinned_root()?
            .open_child(OsStr::new(OBJECTS_DIR))
            .map_err(|error| io_error("pin objects directory", &self.objects_dir, error))
    }

    fn pinned_staging(&self) -> CasResult<PinnedDirectory> {
        self.pinned_root()?
            .open_child(OsStr::new(STAGING_DIR))
            .map_err(|error| io_error("pin staging directory", &self.staging_dir, error))
    }

    fn destination_for(
        &self,
        mut directory: PinnedDirectory,
        relative: &Path,
    ) -> CasResult<(PinnedDirectory, std::ffi::OsString)> {
        let mut components = Vec::new();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(CasError::InvalidStagingPath(relative.to_path_buf()));
            };
            components.push(name.to_os_string());
        }
        let target_name = components
            .pop()
            .ok_or_else(|| CasError::InvalidStagingPath(relative.to_path_buf()))?;
        let mut path = self.objects_dir.clone();
        for component in components {
            path.push(&component);
            directory = directory
                .ensure_child(&component)
                .map_err(|error| io_error("create pinned object shard", &path, error))?;
        }
        Ok((directory, target_name))
    }

    fn verify_existing(
        &self,
        existing: &mut PinnedFile,
        prepared: &PreparedObject,
        target: &Path,
    ) -> CasResult<()> {
        let bytes = read_pinned(existing, target)?;
        if bytes != prepared.bytes {
            return Err(CasError::ExistingObjectMismatch {
                kind: prepared.kind(),
                id: prepared.object_id.to_string(),
            });
        }
        validate_stored_bytes(&prepared.object_id, &bytes).map(|_| ())
    }

    fn cleanup_staging(&self, source: &PinnedFile, path: &Path) -> CasResult<()> {
        self.hit(CasFaultPoint::BeforeStagingCleanup)?;
        source
            .verify_parent()
            .map_err(|error| io_error("verify staging parent", path, error))?;
        match source.unlink() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("remove staging object", path, error)),
        }
        self.hit(CasFaultPoint::AfterStagingCleanup)?;
        sync_directory_compatible(source.sync_parent())
            .map_err(|error| io_error("sync staging cleanup", path, error))?;
        self.hit(CasFaultPoint::AfterStagingDirectorySync)
    }

    #[cfg(unix)]
    fn cleanup_published_staging(
        &self,
        source: &PinnedFile,
        path: &Path,
        _existed: bool,
    ) -> CasResult<()> {
        self.cleanup_staging(source, path)
    }

    #[cfg(windows)]
    fn cleanup_published_staging(
        &self,
        source: &PinnedFile,
        path: &Path,
        existed: bool,
    ) -> CasResult<()> {
        if existed {
            return self.cleanup_staging(source, path);
        }
        // A successful Windows no-clobber rename moved the source handle to
        // the CAS target. Unlinking that handle would delete the installed
        // object, so only the now-empty staging parent needs durability.
        self.hit(CasFaultPoint::BeforeStagingCleanup)?;
        self.hit(CasFaultPoint::AfterStagingCleanup)?;
        sync_directory_compatible(source.sync_parent())
            .map_err(|error| io_error("sync moved staging source", path, error))?;
        self.hit(CasFaultPoint::AfterStagingDirectorySync)
    }

    #[cfg(not(any(unix, windows)))]
    fn cleanup_published_staging(
        &self,
        source: &PinnedFile,
        path: &Path,
        _existed: bool,
    ) -> CasResult<()> {
        self.cleanup_staging(source, path)
    }

    fn hit(&self, point: CasFaultPoint) -> CasResult<()> {
        self.faults
            .check(point)
            .map_err(|message| CasError::InjectedFault { point, message })
    }
}

impl<'store> NativeCasWriter<'store> {
    pub(crate) fn open(store: &'store NativeStoreExclusive) -> CasResult<Self> {
        Self::with_fault_injector(store, Arc::new(NoFaults))
    }

    pub fn with_fault_injector(
        store: &'store NativeStoreExclusive,
        faults: Arc<dyn CasFaultInjector>,
    ) -> CasResult<Self> {
        Ok(Self {
            cas: NativeCas::open_layout(store.layout(), faults)?,
            _exclusive: store,
        })
    }

    pub fn stage(&self, object: &CanonicalObject) -> CasResult<PreparedObject> {
        self.cas.stage(object)
    }

    pub fn install(&self, prepared: PreparedObject) -> CasResult<InstallOutcome> {
        self.cas.install(prepared)
    }

    pub fn put(&self, object: &CanonicalObject) -> CasResult<InstallOutcome> {
        self.cas.put(object)
    }

    pub fn recover_staging(&self) -> CasResult<u64> {
        self.cas.recover_staging()
    }
}

mod codec;
mod paths;
use codec::*;
use paths::*;

fn outcome(prepared: &PreparedObject, path: PathBuf, existed: bool) -> InstallOutcome {
    InstallOutcome {
        object_id: prepared.object_id.clone(),
        path,
        existed,
        bytes_len: prepared.bytes.len() as u64,
    }
}

fn corruption(expected: &StoreObjectId, reason: impl Into<String>) -> CasError {
    CasError::Corruption {
        kind: expected.kind(),
        id: expected.to_string(),
        reason: reason.into(),
    }
}

fn io_error(operation: &'static str, path: &Path, error: io::Error) -> CasError {
    CasError::Io {
        operation,
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

fn staging_name() -> String {
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{STAGING_PREFIX}{}-{nanos:032x}-{sequence:016x}.pending",
        std::process::id()
    )
}

fn contained_name<'a>(parent: &Path, path: &'a Path) -> CasResult<&'a OsStr> {
    if path.parent() != Some(parent) {
        return Err(CasError::InvalidStagingPath(path.to_path_buf()));
    }
    path.file_name()
        .ok_or_else(|| CasError::InvalidStagingPath(path.to_path_buf()))
}

fn read_pinned(file: &mut PinnedFile, path: &Path) -> CasResult<Vec<u8>> {
    let file = file.file_mut();
    const MAX_CANONICAL_BYTES: u64 = 64 * 1024 * 1024;
    if file
        .metadata()
        .map_err(|e| io_error("stat object", path, e))?
        .len()
        > MAX_CANONICAL_BYTES
    {
        return Err(io_error(
            "read object",
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical object exceeds64MiB",
            ),
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| io_error("seek object", path, error))?;
    let mut bytes = Vec::new();
    file.take(MAX_CANONICAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read object", path, error))?;
    if bytes.len() as u64 > MAX_CANONICAL_BYTES {
        return Err(io_error(
            "read object",
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical object grew beyond64MiB",
            ),
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn publish_no_clobber(
    source: &PinnedFile,
    destination: &PinnedDirectory,
    target_name: &OsStr,
) -> CasResult<bool> {
    source
        .hard_link_to(destination, target_name)
        .map_err(|error| io_error("link CAS object", Path::new(target_name), error))
}

#[cfg(windows)]
fn publish_no_clobber(
    source: &PinnedFile,
    destination: &PinnedDirectory,
    target_name: &OsStr,
) -> CasResult<bool> {
    source
        .rename_to(destination, target_name)
        .map_err(|error| io_error("rename CAS object", Path::new(target_name), error))
}

#[cfg(not(any(unix, windows)))]
fn publish_no_clobber(
    _source: &PinnedFile,
    _destination: &PinnedDirectory,
    _target_name: &OsStr,
) -> CasResult<bool> {
    Err(CasError::UnsupportedPublication)
}

fn sync_directory_compatible(result: io::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(error) if error.raw_os_error() == Some(1) => Ok(()),
        Err(error) => Err(error),
    }
}
