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
    /// A sealed and verified staging pack is about to be renamed into
    /// `objects/packs`.
    BeforePackPublication,
    /// The pack is published and identified; its directory is not yet
    /// durable.
    AfterPackPublication,
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
    /// A registered pack v2 (format 6) is damaged: its bytes or metadata do
    /// not match what SQLite registered. Corruption, never absence.
    PackCorruption {
        path: PathBuf,
        reason: String,
    },
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
            Self::PackCorruption { .. } => "cas_pack_corruption",
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
            Self::PackCorruption { path, reason } => {
                write!(formatter, "corrupt CAS pack {}: {reason}", path.display())
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
    root_identity: String,
    objects_dir: PathBuf,
    staging_dir: PathBuf,
    staging_lease_path: PathBuf,
    faults: Arc<dyn CasFaultInjector>,
    /// Shard directories already fsynced by this view's existing-object fast
    /// path. One sync per distinct shard keeps the durability `install` gave
    /// through `destination_for` (ensure_child syncs) without one per object.
    synced_shards: Arc<std::sync::Mutex<std::collections::BTreeSet<PathBuf>>>,
    /// Root and objects directory pinned by the last full path check. Reads
    /// stay bound to these handles (as the Store's SQLite handle stays bound
    /// to its database) and the path binding is rechecked from the volume
    /// root at most every `PIN_RECHECK`; staging and publication always
    /// re-pin.
    pins: Arc<std::sync::Mutex<Option<CasPins>>>,
    /// Shard directories this view already ensured (created if needed, then
    /// it and its parent synced). Later publications into them reopen
    /// without repeating those syncs; each publication still syncs its shard.
    ensured_shards: Arc<std::sync::Mutex<std::collections::BTreeSet<PathBuf>>>,
    /// Set for the duration of a `put_ordered` batch: how the whole batch is
    /// made durable at its end instead of object by object.
    batch: Arc<std::sync::Mutex<Option<BatchState>>>,
    /// Format 5 only: the packs this process knows for this Store (ADR 0026).
    packs: Option<Arc<std::sync::RwLock<PackCatalog>>>,
    /// During a packing batch: its new small objects.
    packer: Arc<std::sync::Mutex<Option<Packer>>>,
    /// Format 6 (ADR 0028): this view reads only what SQLite registered.
    compact_format: bool,
    /// Format 6: the Store id this view was opened for; a scrub reopening
    /// the database authenticates it (council s8 r2).
    store_id: Option<layrs_core::scm::StoreId>,
    /// Format 6: the registered packs and loose objects of this view's
    /// SQLite snapshot (the Store's read connection, or the snapshot a
    /// writer took under its exclusive lock).
    compact: Option<Arc<compact::RegisteredCatalog>>,
    /// Format 6: the pack and loose objects of the running batch.
    compact_batch: Arc<std::sync::Mutex<Option<compact::CompactBatch>>>,
}

/// How a `put_ordered` batch becomes durable before it returns (and so
/// before any metadata can reference it). Decision of 4 November 2026: the
/// guarantee at commit is unchanged, only the per-object flushes are grouped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BatchDurability {
    /// Every object's data is synced; every directory the batch created or
    /// published into is synced once, after all objects, in parallel.
    Coalesced,
    /// NTFS: object data is written through to the device without a cache
    /// flush, directory updates are ordered by the NTFS journal, and one
    /// final flush of a marker created after every publication persists the
    /// journal up to that point and the device cache (as git's batch mode).
    #[cfg_attr(not(windows), allow(dead_code))]
    JournalOrdered,
}

#[derive(Debug)]
struct BatchState {
    mode: BatchDurability,
    /// Staging and objects directories pinned once for the batch; their path
    /// binding is rechecked before the batch is declared durable.
    staging: PinnedDirectory,
    objects: PinnedDirectory,
    directories: std::collections::BTreeMap<PathBuf, PinnedDirectory>,
    /// Coalesced: packs whose members the batch reuses; they are synced
    /// with `objects/packs` before the batch is declared durable.
    reused_packs: BTreeSet<std::ffi::OsString>,
    /// Whether the batch published or reused a file (`defer_directory`):
    /// without one, nothing outside SQLite needs ordering before the commit
    /// of a format 6 batch (its pack is inline, council s7 Q1). Format 6
    /// reuses only registered copies, durable before their registration.
    files: bool,
}

#[derive(Debug)]
#[allow(dead_code)]
struct CasPins {
    root: PinnedDirectory,
    objects: PinnedDirectory,
    verified_at: std::time::Instant,
}

const PIN_RECHECK: std::time::Duration = std::time::Duration::from_millis(250);

/// Mutating CAS view whose lifetime is fenced by an exclusive Store
/// capability. It cannot survive the lock that authorizes publication or
/// staging maintenance.
pub struct NativeCasWriter<'store> {
    cas: NativeCas,
    _exclusive: &'store NativeStoreExclusive,
}

/// One object for `NativeCasWriter::put_ordered`.
pub enum PutItem<'o> {
    Borrowed(&'o CanonicalObject),
    Owned(CanonicalObject),
}

impl PutItem<'_> {
    /// Bytes this item holds in memory beyond the caller's own data.
    fn owned_bytes(&self) -> u64 {
        match self {
            Self::Owned(CanonicalObject::ChunkRaw(chunk)) => chunk.bytes().len() as u64,
            Self::Owned(_) | Self::Borrowed(_) => 0,
        }
    }

    fn object(&self) -> &CanonicalObject {
        match self {
            Self::Borrowed(object) => object,
            Self::Owned(object) => object,
        }
    }
}

/// Read-only CAS view created exclusively from a native audit capability.
#[derive(Debug, Clone)]
pub struct NativeCasAudit {
    cas: NativeCas,
}

impl NativeCasAudit {
    pub(crate) fn into_inner(self) -> NativeCas {
        self.cas
    }

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

    pub fn locate(&self, object_id: &StoreObjectId) -> CasResult<Option<CasLocation>> {
        self.cas.locate(object_id)
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
        let packs = store.packs_enabled().then(|| store.store_id());
        let mut cas = Self::open_layout(store.layout(), Arc::new(NoFaults), packs)?;
        if store.compact() {
            cas.compact_format = true;
            cas.store_id = Some(store.store_id().clone());
            let connection = store.try_connection().map_err(|error| {
                io_error(
                    "open SQLite for the CAS catalog",
                    store.layout().sqlite_path(),
                    io::Error::other(error.to_string()),
                )
            })?;
            // Audits read the metadata on disk now, never the process cache.
            cas.compact = Some(Arc::new(compact::RegisteredCatalog::load_fresh(
                &connection,
                &cas,
            )?));
        }
        Ok(NativeCasAudit { cas })
    }

    /// Format 6: the registered catalog of this view's snapshot.
    pub(crate) fn compact_catalog(&self) -> Option<&Arc<compact::RegisteredCatalog>> {
        self.compact.as_ref()
    }

    /// For a bulk read through this view (a Room's population): its
    /// catalog keeps the pack files it reads open until the hold is dropped,
    /// instead of opening one per block (ADR 0026). `None` without a format 6
    /// catalog.
    pub fn hold_pack_files(&self) -> Option<PackFileHold> {
        self.compact
            .as_ref()
            .map(|catalog| PackFileHold(catalog.hold_pack_files()))
    }

    /// Whether this view belongs to a format 6 Store.
    pub(crate) fn is_compact(&self) -> bool {
        self.compact_format
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
        let mut cas = Self::open_store(store, faults)?;
        if store.compact() {
            cas.compact = Some(cas.store_catalog(store)?);
        }
        Ok(cas)
    }

    /// A view of a Ready Store without its format 6 catalog: the SCM reader
    /// loads that catalog only when it first needs content (`store_catalog`),
    /// so a command that reads none (a clean `status`) never pays for it.
    /// Such a view refuses every content operation of its own.
    pub(crate) fn open_lazy(store: &NativeStoreOpen) -> CasResult<Self> {
        if let Some(view) = store.cas_view().get() {
            store
                .verify_filesystem_identity()
                .map_err(|error| io_error("verify Store binding", store.layout().root(), error))?;
            return Ok(view.session_copy());
        }
        let view = Self::open_store(store, Arc::new(NoFaults))?;
        Ok(store.cas_view().get_or_init(|| view).session_copy())
    }

    /// Another view of the same validated session: shared read pins, packs
    /// and catalog; its own (empty) write and sync state.
    fn session_copy(&self) -> Self {
        Self {
            synced_shards: Arc::default(),
            ensured_shards: Arc::default(),
            batch: Arc::default(),
            packer: Arc::default(),
            compact_batch: Arc::default(),
            ..self.clone()
        }
    }

    fn open_store(store: &NativeStoreOpen, faults: Arc<dyn CasFaultInjector>) -> CasResult<Self> {
        store
            .verify_filesystem_identity()
            .map_err(|error| io_error("verify Store binding", store.layout().root(), error))?;
        let packs = store.packs_enabled().then(|| store.store_id());
        let mut cas =
            Self::open_layout_bound(store.layout(), faults, packs, store.bound_root_identity())?;
        cas.compact_format = store.compact();
        cas.store_id = store.compact().then(|| store.store_id().clone());
        store
            .verify_filesystem_identity()
            .map_err(|error| io_error("verify Store binding", store.layout().root(), error))?;
        Ok(cas)
    }

    /// A hold on `store`'s catalog for a bulk read (`hold_pack_files`).
    pub(crate) fn hold_store_pack_files(
        &self,
        store: &NativeStoreOpen,
    ) -> CasResult<Option<PackFileHold>> {
        if !store.compact() {
            return Ok(None);
        }
        Ok(Some(PackFileHold(
            self.store_catalog(store)?.hold_pack_files(),
        )))
    }

    /// The registered catalog of `store`'s immutable snapshot, loaded once
    /// per open Store.
    pub(crate) fn store_catalog(
        &self,
        store: &NativeStoreOpen,
    ) -> CasResult<Arc<compact::RegisteredCatalog>> {
        if let Some(catalog) = store.registered_catalog().get() {
            return Ok(Arc::clone(catalog));
        }
        let connection = store.connection().map_err(|error| {
            io_error(
                "open SQLite for the CAS catalog",
                store.layout().sqlite_path(),
                io::Error::other(error.to_string()),
            )
        })?;
        let loaded = Arc::new(compact::RegisteredCatalog::load(&connection, self)?);
        drop(connection);
        Ok(Arc::clone(
            store.registered_catalog().get_or_init(|| loaded),
        ))
    }

    /// This view's own catalog: `None` outside format 6; an error for a
    /// format 6 view opened without one, which must never fall back to the
    /// format 5 paths.
    fn compact_view(&self) -> CasResult<Option<&Arc<compact::RegisteredCatalog>>> {
        match (&self.compact, self.compact_format) {
            (Some(catalog), _) => Ok(Some(catalog)),
            (None, false) => Ok(None),
            (None, true) => Err(io_error(
                "read format 6 CAS",
                &self.root,
                io::Error::other("this CAS view has no registered catalog"),
            )),
        }
    }

    /// `packs`: the Store id when its format holds packs (ADR 0026).
    fn open_layout(
        layout: &NativeStoreLayout,
        faults: Arc<dyn CasFaultInjector>,
        packs: Option<&layrs_core::scm::StoreId>,
    ) -> CasResult<Self> {
        Self::open_layout_bound(layout, faults, packs, None)
    }

    /// `open_layout` for a session whose root is already pinned (`bound`,
    /// its identity, rechecked by the caller): the root is not pinned again,
    /// and a Ready Store whose boundaries this process validated in full is
    /// not walked for reparse points again.
    fn open_layout_bound(
        layout: &NativeStoreLayout,
        faults: Arc<dyn CasFaultInjector>,
        packs: Option<&layrs_core::scm::StoreId>,
        bound: Option<String>,
    ) -> CasResult<Self> {
        let root = layout.root().to_path_buf();
        let root_identity = match bound {
            Some(identity) => identity,
            None => PinnedDirectory::open(&root)
                .map_err(|error| io_error("pin CAS root", &root, error))?
                .identity_token(),
        };
        let objects_dir = layout.objects_dir().to_path_buf();
        let staging_dir = layout.staging_dir().to_path_buf();
        let staging_lease_path = layout.staging_lease_path().to_path_buf();
        let validated = super::layout::ready_boundaries_validated(&root);
        for directory in [&root, &objects_dir, &staging_dir] {
            // Each must still be a regular directory; only the walk of every
            // ancestor is skipped once validated.
            if !validated {
                reject_reparse_components(directory)
                    .map_err(|error| io_error("validate CAS directory", directory, error))?;
            }
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
        let packs = packs.map(|store_id| {
            shared_catalog(&format!(
                "{}|{root_identity}|{}",
                store_id.as_str(),
                root.display()
            ))
        });
        Ok(Self {
            root,
            objects_dir,
            staging_dir,
            staging_lease_path,
            root_identity,
            faults,
            synced_shards: Arc::default(),
            pins: Arc::default(),
            ensured_shards: Arc::default(),
            batch: Arc::default(),
            packs,
            packer: Arc::default(),
            compact_format: false,
            store_id: None,
            compact: None,
            compact_batch: Arc::default(),
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
        self.stage_encoded(object.object_id(), encode_object(object)?)
    }

    /// `stage` for the canonical bytes encoded from `object_id`'s object.
    fn stage_encoded(&self, object_id: StoreObjectId, bytes: Vec<u8>) -> CasResult<PreparedObject> {
        let _span = crate::perf::span("cas.stage");
        let lease = StagingLease::shared(&self.staging_lease_path)?;
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
                    let batch = self.batch_mode()?;
                    {
                        let _span = crate::perf::span("cas.file_sync");
                        match batch {
                            Some(BatchDurability::JournalOrdered) => write_through(file.file()),
                            _ => file.file().sync_all(),
                        }
                        .map_err(|error| io_error("sync staged object", &path, error))?;
                    }
                    self.hit(CasFaultPoint::AfterStagingSync)?;
                    // A staged name only matters until it is published; a
                    // batch never needs it durable.
                    if batch.is_none() {
                        sync_directory_compatible(staging.sync())
                            .map_err(|error| io_error("sync staging directory", &path, error))?;
                    }
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
        let _span = crate::perf::span("cas.install");
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
        let shard = target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.objects_dir.clone());
        if !self.defer_directory(&shard, &destination)? {
            sync_directory_compatible(destination.sync())
                .map_err(|error| io_error("sync object shard", &target, error))?;
        }
        self.hit(CasFaultPoint::AfterObjectDirectorySync)?;
        self.cleanup_published_staging(&source, &prepared.path, existed)?;
        Ok(outcome(&prepared, target, existed))
    }

    fn put(&self, object: &CanonicalObject) -> CasResult<InstallOutcome> {
        let object_id = object.object_id();
        let bytes = encode_object(object)?;
        if let Some(outcome) = self.verified_existing(object_id.clone(), &bytes)? {
            return Ok(outcome);
        }
        let prepared = self.stage_encoded(object_id, bytes)?;
        self.install(prepared)
    }

    /// One `put_ordered` item. Without a packer, exactly `put`. With one, a
    /// verified pack copy or separate file is reused; a new small object goes
    /// to the packer (published by `Packer::finish`); a large one is
    /// installed as by `put`.
    fn put_in_batch(&self, object: &CanonicalObject) -> CasResult<Slot> {
        if let Some(catalog) = self.compact_view()? {
            return self.put_compact(catalog, object);
        }
        let packing = self
            .packer
            .lock()
            .map_err(|_| io_error("lock CAS packer", &self.root, io::Error::other("poisoned")))?
            .is_some();
        if !packing {
            return self.put(object).map(Slot::Done);
        }
        let object_id = object.object_id();
        let bytes = encode_object(object)?;
        let bytes_len = bytes.len() as u64;
        let small = bytes.len() <= PACK_MAX_OBJECT_BYTES;
        if small && self.reuse_packed(&object_id, &bytes)? {
            return Ok(Slot::Done(InstallOutcome {
                path: self.object_path(&object_id)?,
                object_id,
                existed: true,
                bytes_len,
            }));
        }
        if let Some(outcome) = self.verified_existing(object_id.clone(), &bytes)? {
            return Ok(Slot::Done(outcome));
        }
        if !small {
            let prepared = self.stage_encoded(object_id, bytes)?;
            return self.install(prepared).map(Slot::Done);
        }
        validate_stored_bytes(&object_id, &bytes)?;
        let mut packer = self
            .packer
            .lock()
            .map_err(|_| io_error("lock CAS packer", &self.root, io::Error::other("poisoned")))?;
        let Some(packer) = packer.as_mut() else {
            return Err(io_error(
                "add to CAS pack",
                &self.root,
                io::Error::other("the batch's packer ended early"),
            ));
        };
        let new = packer.add(self, object_id.clone(), bytes)?;
        Ok(Slot::Packed {
            object_id,
            bytes_len,
            new,
        })
    }

    /// An immutable object already published at its content address is
    /// verified byte-for-byte and reused without staging or file sync. Its
    /// shard directory is fsynced once per view (as `install`'s
    /// `destination_for` did), so an object published by a process that
    /// crashed before its directory sync is durable before metadata commits.
    /// A mismatching or invalid existing file is refused as `install` would;
    /// absence falls back to the full path.
    fn verified_existing(
        &self,
        object_id: StoreObjectId,
        bytes: &[u8],
    ) -> CasResult<Option<InstallOutcome>> {
        let relative = object_relative_path(&object_id)?;
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
        let target = self.objects_dir.join(&relative);
        let mut directory = self.pinned_objects()?;
        for component in components {
            directory = match directory.open_child(&component) {
                Ok(child) => child,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(io_error("inspect object shard", &target, error)),
            };
        }
        let mut existing = match directory.open_file(&target_name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error("inspect publication target", &target, error)),
        };
        let stored = read_pinned(&mut existing, &target)?;
        if stored != bytes {
            return Err(CasError::ExistingObjectMismatch {
                kind: object_id.kind(),
                id: object_id.to_string(),
            });
        }
        // Equal bytes: one validation covers both the encoding and the file.
        validate_stored_bytes(&object_id, &stored)?;
        let shard = target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.objects_dir.clone());
        if !self.defer_directory(&shard, &directory)? {
            let mut synced = self.synced_shards.lock().map_err(|_| {
                io_error(
                    "lock synced shard set",
                    &target,
                    io::Error::other("poisoned"),
                )
            })?;
            if !synced.contains(&shard) {
                sync_directory_compatible(directory.sync())
                    .map_err(|error| io_error("sync object shard", &target, error))?;
                synced.insert(shard);
            }
        }
        crate::perf::count("cas.put_existing", 1);
        Ok(Some(InstallOutcome {
            object_id,
            path: target,
            existed: true,
            bytes_len: bytes.len() as u64,
        }))
    }

    /// Whether some copy exists (a pack member or a separate file). Presence
    /// is not content proof.
    pub fn contains(&self, object_id: &StoreObjectId) -> CasResult<bool> {
        if let Some(catalog) = self.compact_view()? {
            // Content is present only when registered; another kind's loose
            // file is authorized by its `objects` row, checked by the reader.
            return Ok(catalog.contains(object_id)?
                || (!is_content(object_id.kind()) && self.contains_loose(object_id)?));
        }
        self.contains_any(object_id, || self.contains_loose(object_id))
    }

    fn contains_loose(&self, object_id: &StoreObjectId) -> CasResult<bool> {
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

    /// The first copy of `object_id` that verifies (see `with_copy`).
    pub fn read(&self, object_id: &StoreObjectId) -> CasResult<CanonicalObject> {
        if let Some(catalog) = self.compact_view()? {
            return self.read_registered(catalog, object_id);
        }
        match self.with_copy(object_id, false, || self.read_loose(object_id))? {
            Found::Packed(_, object) | Found::Loose(object) => Ok(object),
        }
    }

    fn read_loose(&self, object_id: &StoreObjectId) -> CasResult<CanonicalObject> {
        let relative = object_relative_path(object_id)?;
        let path = self.objects_dir.join(&relative);
        let objects = self.pinned_objects_for_read()?;
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
        if let Some(catalog) = self.compact_view()? {
            if catalog.placements(object_id)?.is_empty() && !is_content(object_id.kind()) {
                return self.verify_loose(object_id);
            }
            return self.read_registered(catalog, object_id).map(|_| ());
        }
        self.with_copy(object_id, false, || self.verify_loose(object_id))
            .map(|_| ())
    }

    /// Format 6 read through `catalog` (a view's own, or a transaction's):
    /// a registered copy, or for kinds other than content the loose file an
    /// `objects` row authorizes.
    pub(crate) fn read_registered(
        &self,
        catalog: &compact::RegisteredCatalog,
        object_id: &StoreObjectId,
    ) -> CasResult<CanonicalObject> {
        match catalog.read(self, object_id)? {
            Some(object) => Ok(object),
            None if is_content(object_id.kind()) => Err(CasError::NotFound {
                kind: object_id.kind(),
                id: object_id.to_string(),
            }),
            None => self.read_loose(object_id),
        }
    }

    fn verify_loose(&self, object_id: &StoreObjectId) -> CasResult<()> {
        let relative = object_relative_path(object_id)?;
        let path = self.objects_dir.join(&relative);
        let objects = self.pinned_objects_for_read()?;
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

    /// Streams one verified Chunk. A pack member is verified before the
    /// callback sees it; a separate file is streamed and verified as it is
    /// read, so its failure is final.
    pub fn stream_verified_chunk(
        &self,
        chunk_id: &ChunkId,
        mut consume: impl FnMut(&[u8]),
    ) -> CasResult<u64> {
        if let Some(catalog) = self.compact_view()? {
            return catalog.stream_chunk(self, chunk_id, &mut consume);
        }
        let object_id = StoreObjectId::ChunkRaw(chunk_id.clone());
        let found = self.with_copy(&object_id, true, || {
            self.stream_loose_chunk(chunk_id, &object_id, &mut consume)
        })?;
        match found {
            Found::Packed(bytes, _) => {
                consume(&bytes);
                Ok(bytes.len() as u64)
            }
            Found::Loose(length) => Ok(length),
        }
    }

    fn stream_loose_chunk(
        &self,
        chunk_id: &ChunkId,
        object_id: &StoreObjectId,
        consume: &mut impl FnMut(&[u8]),
    ) -> CasResult<u64> {
        let relative = object_relative_path(object_id)?;
        let path = self.objects_dir.join(&relative);
        let objects = self.pinned_objects_for_read()?;
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
        stream_and_verify_chunk(file.file_mut(), object_id, &path, consume)
    }

    /// Iteratively checks every physical SCM V1 object without consulting any
    /// Space admission table.
    ///
    /// Format 6: the registered catalog is reread from the database now, so
    /// the Store capability this view was opened from must still hold its
    /// lock, outside any write in progress (an uncheckpointed WAL refuses).
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
                    // Packs are verified member by member below.
                    if (self.packs.is_some() || self.compact_format)
                        && path == self.objects_dir.join(PACKS_DIR)
                    {
                        continue;
                    }
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
                // This file itself: a sound copy elsewhere must not hide it.
                if let Err(error) = self.verify_loose(&object_id) {
                    report.issues.push(CasScrubIssue {
                        path,
                        code: error.code(),
                        message: error.to_string(),
                    });
                }
            }
        }
        if self.packs.is_some() {
            self.scrub_packs(&mut report)?;
        }
        if let Some(view) = self.compact_view()? {
            if view.is_fresh() {
                // An audit view: its catalog was read from the database now.
                self.scrub_registered(view, &mut report)?;
            } else {
                // From the database now (council s8 r1): an inline pack's
                // image is reread, as a pack file is, never the one this view
                // loaded. The root must still be this view's and the database
                // this Store's (council s8 r2); the root and its metadata
                // directory stay pinned and are checked at their paths again
                // once the catalog is read (council s8 r3).
                let root = self.pinned_root()?;
                let metadata = root
                    .open_child_for_traversal(std::ffi::OsStr::new(
                        super::layout::METADATA_DIR_NAME,
                    ))
                    .map_err(|error| io_error("pin Store metadata", &self.root, error))?;
                let path = self
                    .root
                    .join(super::layout::METADATA_DIR_NAME)
                    .join(super::layout::SQLITE_FILE_NAME);
                let database = |error: super::sqlite::ScmSchemaError| {
                    io_error(
                        "open Store database",
                        &path,
                        io::Error::other(error.to_string()),
                    )
                };
                let connection =
                    super::sqlite::open_read_only(&path, std::time::Duration::from_secs(5))
                        .map_err(database)?;
                let store_id = self.store_id.as_ref().ok_or_else(|| {
                    io_error(
                        "scrub format 6 CAS",
                        &self.root,
                        io::Error::other("this CAS view has no Store id"),
                    )
                })?;
                super::sqlite::validate_store_identity(&connection, store_id).map_err(database)?;
                let catalog = compact::RegisteredCatalog::load_fresh(&connection, self)?;
                for pinned in [&root, &metadata] {
                    pinned
                        .verify_path_identity()
                        .map_err(|error| io_error("verify Store binding", &self.root, error))?;
                }
                self.scrub_registered(&catalog, &mut report)?;
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
        let directory = PinnedDirectory::open(&self.root)
            .map_err(|error| io_error("pin CAS root", &self.root, error))?;
        if directory.identity_token() != self.root_identity {
            return Err(io_error(
                "verify CAS root",
                &self.root,
                io::Error::other("Store filesystem identity changed"),
            ));
        }
        Ok(directory)
    }

    fn pinned_objects(&self) -> CasResult<PinnedDirectory> {
        if let Some(objects) = self.batch_pin(|state| &state.objects)? {
            return Ok(objects);
        }
        self.pinned_root()?
            .open_child(OsStr::new(OBJECTS_DIR))
            .map_err(|error| io_error("pin objects directory", &self.objects_dir, error))
    }

    /// A directory pinned for the current batch, if one runs.
    fn batch_pin(
        &self,
        select: impl Fn(&BatchState) -> &PinnedDirectory,
    ) -> CasResult<Option<PinnedDirectory>> {
        let batch = self
            .batch
            .lock()
            .map_err(|_| io_error("lock CAS batch", &self.root, io::Error::other("poisoned")))?;
        batch
            .as_ref()
            .map(|state| {
                select(state)
                    .try_clone()
                    .map_err(|error| io_error("reuse batch pin", &self.root, error))
            })
            .transpose()
    }

    fn batch_mode(&self) -> CasResult<Option<BatchDurability>> {
        Ok(self
            .batch
            .lock()
            .map_err(|_| io_error("lock CAS batch", &self.root, io::Error::other("poisoned")))?
            .as_ref()
            .map(|state| state.mode))
    }

    /// Within a batch, records `directory` for the batch's final durability
    /// step instead of syncing it now. `false` outside a batch.
    fn defer_directory(&self, path: &Path, directory: &PinnedDirectory) -> CasResult<bool> {
        let mut batch = self
            .batch
            .lock()
            .map_err(|_| io_error("lock CAS batch", &self.root, io::Error::other("poisoned")))?;
        let Some(state) = batch.as_mut() else {
            return Ok(false);
        };
        state.files = true;
        if state.mode == BatchDurability::Coalesced && !state.directories.contains_key(path) {
            let pinned = directory
                .try_clone()
                .map_err(|error| io_error("record batch directory", path, error))?;
            state.directories.insert(path.to_path_buf(), pinned);
        }
        Ok(true)
    }

    fn begin_batch(&self) -> CasResult<()> {
        let mode = match std::env::var("LAYRS_CAS_BATCH_DURABILITY").as_deref() {
            Ok("per-object") => return Ok(()),
            Ok("coalesced") => BatchDurability::Coalesced,
            _ => batch_durability_for(&self.root),
        };
        let root = self.pinned_root()?;
        let staging = root
            .open_child(OsStr::new(STAGING_DIR))
            .map_err(|error| io_error("pin staging directory", &self.staging_dir, error))?;
        let objects = root
            .open_child(OsStr::new(OBJECTS_DIR))
            .map_err(|error| io_error("pin objects directory", &self.objects_dir, error))?;
        *self
            .batch
            .lock()
            .map_err(|_| io_error("lock CAS batch", &self.root, io::Error::other("poisoned")))? =
            Some(BatchState {
                mode,
                staging,
                objects,
                directories: Default::default(),
                reused_packs: BTreeSet::new(),
                files: false,
            });
        // Format 5: the batch packs its new small objects. Its inventory of
        // packs is taken here once; its own packs join it as published.
        if self.packs.is_some() && self.refresh_packs().is_ok() {
            *self.packer.lock().map_err(|_| {
                io_error("lock CAS packer", &self.root, io::Error::other("poisoned"))
            })? = Some(Packer::default());
        }
        Ok(())
    }

    /// Ends the batch. With `durable`, makes everything it published durable
    /// first: the pinned paths are rechecked, then the deferred directories
    /// are synced (Coalesced) or the NTFS journal and device cache are
    /// flushed past every publication (JournalOrdered).
    fn end_batch(&self, durable: bool) -> CasResult<()> {
        // A packer still here was abandoned: its staging packs are removed.
        drop(self.packer.lock().ok().and_then(|mut packer| packer.take()));
        if !durable {
            self.abandon_compact_batch();
        }
        let Some(state) = self
            .batch
            .lock()
            .map_err(|_| io_error("lock CAS batch", &self.root, io::Error::other("poisoned")))?
            .take()
        else {
            return Ok(());
        };
        if !durable {
            return Ok(());
        }
        // Format 6 only (council s8 r1, P1): a format 5 batch may reuse
        // members of a pack published by an earlier batch that never reached
        // its barrier, which JournalOrdered does not record; its barrier
        // always runs, as before.
        if self.compact_format && !state.files && state.reused_packs.is_empty() {
            crate::perf::count("cas.batch_durability_skipped", 1);
            return Ok(());
        }
        let _span = crate::perf::span("cas.batch_durability");
        for (directory, path) in [
            (&state.staging, &self.staging_dir),
            (&state.objects, &self.objects_dir),
        ] {
            directory
                .verify_path_identity()
                .map_err(|error| io_error("verify batch pin", path, error))?;
        }
        match state.mode {
            BatchDurability::Coalesced => {
                let directories: Vec<_> = state.directories.into_iter().collect();
                let threads = std::thread::available_parallelism()
                    .map_or(4, |n| n.get())
                    .clamp(1, 8);
                let next = std::sync::atomic::AtomicUsize::new(0);
                let failure = std::sync::Mutex::new(None);
                std::thread::scope(|scope| {
                    for _ in 0..threads {
                        scope.spawn(|| {
                            loop {
                                let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let Some((path, directory)) = directories.get(index) else {
                                    break;
                                };
                                if let Err(error) = sync_directory_compatible(directory.sync()) {
                                    if let Ok(mut failure) = failure.lock() {
                                        failure.get_or_insert_with(|| {
                                            io_error("sync batch directory", path, error)
                                        });
                                    }
                                    break;
                                }
                            }
                        });
                    }
                });
                if let Some(error) = failure.into_inner().ok().flatten() {
                    return Err(error);
                }
                if !state.reused_packs.is_empty() {
                    let path = self.objects_dir.join(PACKS_DIR);
                    let packs = state
                        .objects
                        .open_child(std::ffi::OsStr::new(PACKS_DIR))
                        .map_err(|error| io_error("pin CAS packs", &path, error))?;
                    for name in &state.reused_packs {
                        sync_pack_file(&packs, name)
                            .map_err(|error| io_error("sync reused CAS pack", &path, error))?;
                    }
                    sync_directory_compatible(packs.sync())
                        .map_err(|error| io_error("sync CAS packs directory", &path, error))?;
                }
            }
            BatchDurability::JournalOrdered => {
                let name = staging_name();
                let path = self.staging_dir.join(&name);
                let mut marker = state
                    .staging
                    .create_file(OsStr::new(&name))
                    .map_err(|error| io_error("create durability marker", &path, error))?;
                let flushed = marker
                    .file_mut()
                    .write_all(b"layrs batch durability marker\n")
                    .and_then(|()| marker.file().sync_all());
                let removed = marker.unlink();
                flushed.map_err(|error| io_error("flush CAS batch", &path, error))?;
                if let Err(error) = removed {
                    if error.kind() != io::ErrorKind::NotFound {
                        return Err(io_error("remove durability marker", &path, error));
                    }
                }
            }
        }
        Ok(())
    }

    fn pinned_staging_for_batch(&self) -> CasResult<Option<PinnedDirectory>> {
        self.batch_pin(|state| &state.staging)
    }

    /// Objects directory for content-verified reads only (`read`, `verify`,
    /// chunk streaming; `contains` and publication re-pin): every byte read is checked against its
    /// content address, so the throttled path recheck cannot admit wrong
    /// content. Publication and staging use `pinned_objects`/`pinned_staging`.
    fn pinned_objects_for_read(&self) -> CasResult<PinnedDirectory> {
        let poisoned = || io_error("lock CAS pins", &self.root, io::Error::other("poisoned"));
        let mut pins = self.pins.lock().map_err(|_| poisoned())?;
        if let Some(pins) = pins
            .as_ref()
            .filter(|pins| pins.verified_at.elapsed() < PIN_RECHECK)
        {
            return pins
                .objects
                .try_clone()
                .map_err(|error| io_error("pin objects directory", &self.objects_dir, error));
        }
        let root = self.pinned_root()?;
        let objects = root
            .open_child(OsStr::new(OBJECTS_DIR))
            .map_err(|error| io_error("pin objects directory", &self.objects_dir, error))?;
        let result = objects
            .try_clone()
            .map_err(|error| io_error("pin objects directory", &self.objects_dir, error))?;
        *pins = Some(CasPins {
            root,
            objects,
            verified_at: std::time::Instant::now(),
        });
        Ok(result)
    }

    fn pinned_staging(&self) -> CasResult<PinnedDirectory> {
        if let Some(staging) = self.pinned_staging_for_batch()? {
            return Ok(staging);
        }
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
            let ensured = self
                .ensured_shards
                .lock()
                .map_err(|_| io_error("lock ensured shards", &path, io::Error::other("poisoned")))?
                .contains(&path);
            let reopened = if ensured {
                match directory.open_child(&component) {
                    Ok(child) => Some(child),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(io_error("open pinned object shard", &path, error)),
                }
            } else {
                None
            };
            directory = match reopened {
                Some(child) => child,
                None => {
                    let child = if self.batch_mode()?.is_some() {
                        let child =
                            directory
                                .ensure_child_unsynced(&component)
                                .map_err(|error| {
                                    io_error("create pinned object shard", &path, error)
                                })?;
                        let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
                        self.defer_directory(&parent, &directory)?;
                        self.defer_directory(&path, &child)?;
                        child
                    } else {
                        directory
                            .ensure_child(&component)
                            .map_err(|error| io_error("create pinned object shard", &path, error))?
                    };
                    self.ensured_shards
                        .lock()
                        .map_err(|_| {
                            io_error("lock ensured shards", &path, io::Error::other("poisoned"))
                        })?
                        .insert(path.clone());
                    child
                }
            };
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
        if self.batch_mode()?.is_none() {
            sync_directory_compatible(source.sync_parent())
                .map_err(|error| io_error("sync staging cleanup", path, error))?;
        }
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
        if self.batch_mode()?.is_none() {
            sync_directory_compatible(source.sync_parent())
                .map_err(|error| io_error("sync moved staging source", path, error))?;
        }
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
        store
            .verify_filesystem_identity()
            .map_err(|error| io_error("verify Store binding", store.layout().root(), error))?;
        let packs = store.packs_enabled().then(|| store.store_id());
        let mut cas = NativeCas::open_layout(store.layout(), faults, packs)?;
        if store.compact() {
            // The writer's snapshot of what is registered, taken under the
            // exclusive lock: no other writer can register anything until
            // this capability ends.
            cas.compact_format = true;
            cas.store_id = Some(store.store_id().clone());
            let connection = super::sqlite::open_read_only(
                store.layout().sqlite_path(),
                std::time::Duration::from_secs(5),
            )
            .map_err(|error| {
                io_error(
                    "open SQLite for the CAS catalog",
                    store.layout().sqlite_path(),
                    io::Error::other(error.to_string()),
                )
            })?;
            let catalog = Arc::new(compact::RegisteredCatalog::load(&connection, &cas)?);
            cas.collect_orphan_packs(&catalog);
            cas.compact = Some(catalog);
        }
        store
            .verify_filesystem_identity()
            .map_err(|error| io_error("verify Store binding", store.layout().root(), error))?;
        Ok(Self {
            cas,
            _exclusive: store,
        })
    }

    /// Format 5 only. A format 6 object is present only once SQLite
    /// registers it, which only `put_ordered` within a batch prepares: a
    /// lone loose file would be reported installed yet never be present.
    pub fn stage(&self, object: &CanonicalObject) -> CasResult<PreparedObject> {
        self.refuse_unregistered()?;
        self.cas.stage(object)
    }

    /// Format 5 only (see `stage`).
    pub fn install(&self, prepared: PreparedObject) -> CasResult<InstallOutcome> {
        self.refuse_unregistered()?;
        self.cas.install(prepared)
    }

    /// Format 5 only (see `stage`).
    pub fn put(&self, object: &CanonicalObject) -> CasResult<InstallOutcome> {
        self.refuse_unregistered()?;
        self.cas.put(object)
    }

    fn refuse_unregistered(&self) -> CasResult<()> {
        if self.cas.compact_format {
            return Err(CasError::UnsupportedPublication);
        }
        Ok(())
    }

    /// Installs a sequence of objects, each exactly as `put` (verified reuse,
    /// or staged + synced, published without replacement, reread, shard
    /// synced), on a few worker threads. `source` is drained on the caller's
    /// thread in order and may read external input; its first error stops
    /// submission and is returned after every submitted install finished.
    /// Results are in submission order. After the first failing install,
    /// later queued objects are not installed and report a synthetic
    /// `Corruption` ("not installed after an earlier install failed"); claims
    /// are FIFO, so the first error in order is always a real one. Fault
    /// injectors are consulted from worker threads. Objects are independent
    /// content-addressed files; only their device waits overlap.
    pub fn put_ordered<'o, E: Send>(
        &self,
        mut source: impl FnMut() -> Option<std::result::Result<PutItem<'o>, E>>,
    ) -> std::result::Result<Vec<CasResult<InstallOutcome>>, E> {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let _span = crate::perf::span("cas.put_ordered");
        // The whole batch becomes durable at its end (see BatchDurability);
        // a batch that fails is never referenced, so it needs no flush.
        struct EndBatch<'c>(&'c NativeCas, bool);
        impl Drop for EndBatch<'_> {
            fn drop(&mut self) {
                if !self.1 {
                    let _ = self.0.end_batch(false);
                }
            }
        }
        // If the batch cannot start, every object is made durable on its own.
        let batch_started = self.cas.begin_batch().is_ok();
        let mut end_batch = EndBatch(&self.cas, false);
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, 8);
        let failed = AtomicBool::new(false);
        // Owned (externally read) bytes queued or being installed are bounded,
        // so a streamed source never accumulates in memory; at least one item
        // is always admitted.
        const MAX_IN_FLIGHT_BYTES: u64 = 8 * 1024 * 1024;
        let in_flight = (std::sync::Mutex::new(0u64), std::sync::Condvar::new());
        let alive = AtomicUsize::new(threads);
        let cas = &self.cas;
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<(usize, PutItem<'o>, u64)>(threads);
        // Owned by the workers only: if every worker exits (even by panic),
        // the receiver drops and `send` fails instead of blocking.
        let receiver = Arc::new(std::sync::Mutex::new(receiver));
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    let (receiver, failed, in_flight, alive) =
                        (Arc::clone(&receiver), &failed, &in_flight, &alive);
                    scope.spawn(move || {
                        // Wakes a waiting submitter when this worker exits,
                        // even by panic, so it never waits on a dead pool.
                        struct Exit<'a>(&'a AtomicUsize, &'a std::sync::Condvar);
                        impl Drop for Exit<'_> {
                            fn drop(&mut self) {
                                self.0.fetch_sub(1, Ordering::AcqRel);
                                self.1.notify_all();
                            }
                        }
                        let _exit = Exit(alive, &in_flight.1);
                        let mut done = Vec::new();
                        loop {
                            let next = match receiver.lock() {
                                Ok(receiver) => receiver.recv(),
                                Err(_) => break,
                            };
                            let Ok((index, item, size)) = next else { break };
                            let result = if failed.load(Ordering::Acquire) {
                                Err(CasError::Corruption {
                                    kind: item.object().object_id().kind(),
                                    id: item.object().object_id().to_string(),
                                    reason: "not installed after an earlier install failed".into(),
                                })
                            } else {
                                let result = cas.put_in_batch(item.object());
                                if result.is_err() {
                                    failed.store(true, Ordering::Release);
                                }
                                result
                            };
                            drop(item);
                            if size > 0 {
                                if let Ok(mut bytes) = in_flight.0.lock() {
                                    *bytes = bytes.saturating_sub(size);
                                }
                                in_flight.1.notify_all();
                            }
                            done.push((index, result));
                        }
                        done
                    })
                })
                .collect();
            drop(receiver);
            let mut submitted = 0usize;
            let mut source_error = None;
            while !failed.load(Ordering::Acquire) {
                match source() {
                    None => break,
                    Some(Ok(item)) => {
                        let size = item.owned_bytes();
                        if size > 0 {
                            let Ok(mut bytes) = in_flight.0.lock() else {
                                break;
                            };
                            while *bytes > 0
                                && bytes.saturating_add(size) > MAX_IN_FLIGHT_BYTES
                                && alive.load(Ordering::Acquire) > 0
                                && !failed.load(Ordering::Acquire)
                            {
                                bytes = match in_flight
                                    .1
                                    .wait_timeout(bytes, std::time::Duration::from_millis(50))
                                {
                                    Ok((bytes, _)) => bytes,
                                    Err(poisoned) => poisoned.into_inner().0,
                                };
                            }
                            *bytes = bytes.saturating_add(size);
                        }
                        if sender.send((submitted, item, size)).is_err() {
                            break;
                        }
                        submitted += 1;
                    }
                    Some(Err(error)) => {
                        source_error = Some(error);
                        break;
                    }
                }
            }
            drop(sender);
            let mut results: Vec<(usize, CasResult<Slot>)> = Vec::with_capacity(submitted);
            let mut panicked = false;
            for worker in workers {
                match worker.join() {
                    Ok(done) => results.extend(done),
                    Err(_) => panicked = true,
                }
            }
            if let Some(error) = source_error {
                return Err(error);
            }
            results.sort_by_key(|(index, _)| *index);
            let incomplete = panicked || results.len() != submitted;
            let mut complete = !incomplete && results.iter().all(|(_, slot)| slot.is_ok());
            // Packed objects are published once every worker finished
            // cleanly, before the batch's durability step.
            let packer = cas.packer.lock().ok().and_then(|mut packer| packer.take());
            let mut finish_error = None;
            if let Some(packer) = packer {
                if complete {
                    if let Err(error) = packer.finish(cas) {
                        finish_error = Some(error);
                        complete = false;
                    }
                }
            }
            if cas.compact.is_some() {
                if complete {
                    if let Err(error) = cas.finish_compact_batch() {
                        finish_error = Some(error);
                        complete = false;
                    }
                } else {
                    cas.abandon_compact_batch();
                }
            }
            let mut ordered: Vec<CasResult<InstallOutcome>> = results
                .into_iter()
                .map(|(_, slot)| match slot {
                    Ok(Slot::Done(outcome)) => Ok(outcome),
                    Ok(Slot::Packed {
                        object_id,
                        bytes_len,
                        new,
                    }) if complete => cas.object_path(&object_id).map(|path| InstallOutcome {
                        object_id,
                        path,
                        existed: !new,
                        bytes_len,
                    }),
                    Ok(Slot::Packed { object_id, .. }) => Err(CasError::Corruption {
                        kind: object_id.kind(),
                        id: object_id.to_string(),
                        reason: "not installed after an earlier install failed".into(),
                    }),
                    Err(error) => Err(error),
                })
                .collect();
            if incomplete {
                ordered.truncate(0);
                ordered.push(Err(io_error(
                    "install CAS objects",
                    &cas.objects_dir,
                    io::Error::other("CAS install worker panicked"),
                )));
            } else if let Some(error) = finish_error {
                ordered.truncate(0);
                ordered.push(Err(error));
            }
            end_batch.1 = true;
            let complete = complete && ordered.iter().all(|outcome| outcome.is_ok());
            let durable = if batch_started {
                cas.end_batch(complete)
            } else {
                Ok(())
            };
            if let Err(error) = durable {
                ordered.truncate(0);
                ordered.push(Err(error));
            }
            Ok(ordered)
        })
    }

    pub fn recover_staging(&self) -> CasResult<u64> {
        self.cas.recover_staging()
    }
}

mod codec;
pub(crate) mod compact;

/// A bulk read's hold on a format 6 catalog's pack files
/// (`NativeCas::hold_pack_files`): they stay open until it is dropped.
pub struct PackFileHold(compact::CatalogHold);

impl PackFileHold {
    /// Whether this hold is on `cas`'s catalog.
    pub fn holds(&self, cas: &NativeCas) -> bool {
        cas.compact
            .as_ref()
            .is_some_and(|catalog| self.0.holds(catalog))
    }
}

mod pack2;
mod packs;
mod paths;
use codec::*;
pub(crate) use compact::{
    AuditEvent, CompactPublication, FileManifestEntry, FileRecordPlan, Placement,
    RegisteredCatalog, SpaceAdmissions, check_bitmap, set_bit,
};

/// Content kinds: in format 6 they have no `objects` row.
pub(crate) fn is_content(kind: StoreObjectKind) -> bool {
    matches!(
        kind,
        StoreObjectKind::ChunkRaw | StoreObjectKind::FileManifest
    )
}

/// The raw 32-byte digest of an object id (format 6 SQLite keys).
pub(crate) fn raw_object_digest(object_id: &StoreObjectId) -> CasResult<[u8; 32]> {
    pack2::raw_digest(object_id)
}

/// The raw 32-byte digest of a FileId.
pub(crate) fn raw_file_digest(file_id: &layrs_core::scm::FileId) -> Option<[u8; 32]> {
    pack2::hex_digest(file_id.as_str().strip_prefix("blake3:")?)
}

/// The pack kind code of `kind` (`loose_objects.object_kind`).
pub(crate) fn content_kind_code(kind: StoreObjectKind) -> u8 {
    pack2::kind_code(kind)
}

impl NativeCas {
    /// Length of the loose file at `object_id`'s derived path, if any.
    pub(crate) fn loose_length(&self, object_id: &StoreObjectId) -> CasResult<Option<u64>> {
        let relative = object_relative_path(object_id)?;
        let path = self.objects_dir.join(&relative);
        match self
            .pinned_objects_for_read()?
            .open_relative_file(&relative)
        {
            Ok(file) => file
                .file()
                .metadata()
                .map(|metadata| Some(metadata.len()))
                .map_err(|error| io_error("inspect CAS object", &path, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error("inspect CAS object", &path, error)),
        }
    }
}
#[cfg(any(test, feature = "test-support"))]
pub use compact::set_inline_pack_limit;
pub use packs::CasLocation;
use packs::*;
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

/// The batch durability a Store on this volume supports: journal ordering
/// on NTFS (directory updates are journaled metadata), coalesced syncs
/// anywhere else.
fn batch_durability_for(root: &Path) -> BatchDurability {
    #[cfg(windows)]
    {
        if super::change_index::local_ntfs(root) {
            return BatchDurability::JournalOrdered;
        }
    }
    let _ = root;
    BatchDurability::Coalesced
}

/// Writes a file's dirty data to the device without asking the device to
/// flush its cache (NtFlushBuffersFileEx FLUSH_FLAGS_NO_SYNC); the batch's
/// final flush persists it. Elsewhere, a full sync.
fn write_through(file: &std::fs::File) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Wdk::Storage::FileSystem::NtFlushBuffersFileEx;
        use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
        const FLUSH_FLAGS_NO_SYNC: u32 = 2;
        let mut status = IO_STATUS_BLOCK::default();
        let result = unsafe {
            NtFlushBuffersFileEx(
                file.as_raw_handle() as _,
                FLUSH_FLAGS_NO_SYNC,
                std::ptr::null(),
                0,
                &mut status,
            )
        };
        if result < 0 {
            // Unsupported here: fall back to the full per-file sync.
            return file.sync_all();
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        file.sync_all()
    }
}

fn sync_directory_compatible(result: io::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(error) if error.raw_os_error() == Some(1) => Ok(()),
        Err(error) => Err(error),
    }
}
