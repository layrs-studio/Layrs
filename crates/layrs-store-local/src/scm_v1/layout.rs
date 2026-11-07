use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use layrs_core::scm::StoreId;
use layrs_store::scm::StoreError as ScmStoreError;
use rusqlite::Connection;

use super::sqlite::{self, ScmSchemaError};
use crate::layout::{
    create_dir_all_durable, create_new_file_no_follow, open_read_no_follow,
    reject_reparse_components, sync_directory,
};
use crate::lock::StoreLock;

pub const FORMAT_FILE_NAME: &str = "FORMAT";
pub const OBJECTS_DIR_NAME: &str = "objects";
pub const STAGING_DIR_NAME: &str = "staging";
pub const STAGING_LEASE_FILE_NAME: &str = ".lease";
pub const METADATA_DIR_NAME: &str = "metadata";
pub const SQLITE_FILE_NAME: &str = "layrs.sqlite3";
pub const LOCK_FILE_NAME: &str = "layrs.lock";
pub const REPAIR_BACKUPS_DIR_NAME: &str = "repair-backups";

/// Paths owned by the fresh native SCM store baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeStoreLayout {
    root: PathBuf,
    format_path: PathBuf,
    objects_dir: PathBuf,
    staging_dir: PathBuf,
    staging_lease_path: PathBuf,
    metadata_dir: PathBuf,
    sqlite_path: PathBuf,
    lock_path: PathBuf,
    repair_backups_dir: PathBuf,
}

fn ready_validated_roots() -> &'static std::sync::Mutex<std::collections::HashSet<PathBuf>> {
    static VALIDATED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    VALIDATED.get_or_init(Default::default)
}

/// Whether this process validated every boundary of the Ready Store at
/// `root` (`validate_ready_boundaries`, once per process and root).
pub(crate) fn ready_boundaries_validated(root: &Path) -> bool {
    ready_validated_roots()
        .lock()
        .is_ok_and(|roots| roots.contains(root))
}

/// Records a full `validate_ready_boundaries` of the Store at `root`.
pub(crate) fn remember_ready_boundaries_validated(root: &Path) {
    if let Ok(mut roots) = ready_validated_roots().lock() {
        roots.insert(root.to_path_buf());
    }
}

impl NativeStoreLayout {
    pub fn new(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = absolute_lexical(root.as_ref())?;
        if root.file_name().is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native store root must not be a filesystem root",
            ));
        }
        // A Ready Store this process already validated in full (every
        // boundary and every ancestor, `ready_boundaries_validated`) skips the
        // repeated walk; its opens still re-read the marker and pin the root.
        let validated = ready_boundaries_validated(&root);
        if !validated {
            reject_reparse_components(&root)?;
        }
        if let Ok(metadata) = fs::symlink_metadata(&root) {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "native store root is not a regular directory: {}",
                        root.display()
                    ),
                ));
            }
        }

        let metadata_dir = root.join(METADATA_DIR_NAME);
        let staging_dir = root.join(STAGING_DIR_NAME);
        let layout = Self {
            format_path: root.join(FORMAT_FILE_NAME),
            objects_dir: root.join(OBJECTS_DIR_NAME),
            staging_lease_path: staging_dir.join(STAGING_LEASE_FILE_NAME),
            staging_dir,
            sqlite_path: metadata_dir.join(SQLITE_FILE_NAME),
            lock_path: metadata_dir.join(LOCK_FILE_NAME),
            repair_backups_dir: metadata_dir.join(REPAIR_BACKUPS_DIR_NAME),
            metadata_dir,
            root,
        };
        layout.validate_containment()?;
        if !validated {
            layout.validate_existing_boundaries()?;
        }
        Ok(layout)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn format_path(&self) -> &Path {
        &self.format_path
    }

    pub fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }

    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    pub fn staging_lease_path(&self) -> &Path {
        &self.staging_lease_path
    }

    pub fn metadata_dir(&self) -> &Path {
        &self.metadata_dir
    }

    pub fn sqlite_path(&self) -> &Path {
        &self.sqlite_path
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    pub fn repair_backups_dir(&self) -> &Path {
        &self.repair_backups_dir
    }

    pub(crate) fn ensure_bootstrap_directories(&self) -> io::Result<()> {
        self.validate_containment()?;
        for directory in [
            &self.root,
            &self.objects_dir,
            &self.staging_dir,
            &self.metadata_dir,
            &self.repair_backups_dir,
        ] {
            create_dir_all_durable(directory)?;
        }
        ensure_regular_file_durable(&self.staging_lease_path, &self.staging_dir)?;
        ensure_regular_file_durable(&self.lock_path, &self.metadata_dir)?;
        self.validate_existing_boundaries()
    }

    /// Establishes the minimal lock boundary needed before bootstrap may mutate
    /// any subordinate Store state. No SQLite or CAS path is created here.
    pub(crate) fn ensure_lock_boundary(&self) -> io::Result<()> {
        self.validate_containment()?;
        create_dir_all_durable(&self.root)?;
        create_dir_all_durable(&self.metadata_dir)?;
        ensure_regular_file_durable(&self.lock_path, &self.metadata_dir)?;
        self.validate_existing_boundaries()
    }

    pub(crate) fn validate_ready_boundaries(&self) -> io::Result<()> {
        self.validate_containment()?;
        for directory in [
            &self.root,
            &self.objects_dir,
            &self.staging_dir,
            &self.metadata_dir,
            &self.repair_backups_dir,
        ] {
            let metadata = fs::symlink_metadata(directory)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "native store boundary is not a directory: {}",
                        directory.display()
                    ),
                ));
            }
        }
        for file in [&self.sqlite_path, &self.lock_path, &self.staging_lease_path] {
            let metadata = fs::symlink_metadata(file)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "native Store boundary is not a regular file: {}",
                        file.display()
                    ),
                ));
            }
            drop(open_read_no_follow(file)?);
        }
        self.validate_existing_boundaries()
    }

    fn validate_existing_boundaries(&self) -> io::Result<()> {
        crate::layout::reject_reparse_components_all(&[
            &self.root,
            &self.format_path,
            &self.objects_dir,
            &self.staging_dir,
            &self.staging_lease_path,
            &self.metadata_dir,
            &self.sqlite_path,
            &self.lock_path,
            &self.repair_backups_dir,
        ])
    }

    fn validate_containment(&self) -> io::Result<()> {
        for path in [
            &self.format_path,
            &self.objects_dir,
            &self.staging_dir,
            &self.metadata_dir,
        ] {
            if path.parent() != Some(self.root.as_path()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("native store path escapes root: {}", path.display()),
                ));
            }
        }
        for path in [&self.sqlite_path, &self.lock_path, &self.repair_backups_dir] {
            if path.parent() != Some(self.metadata_dir.as_path()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "native metadata path escapes metadata root: {}",
                        path.display()
                    ),
                ));
            }
        }
        if self.staging_lease_path.parent() != Some(self.staging_dir.as_path()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "native staging lease escapes staging root: {}",
                    self.staging_lease_path.display()
                ),
            ));
        }
        Ok(())
    }
}

/// Successfully opened native format identity and paths.
#[derive(Debug)]
pub struct NativeStoreOpen {
    connection: Mutex<Connection>,
    _store_lock: StoreLock,
    layout: NativeStoreLayout,
    store_id: StoreId,
    bound_directory: Option<crate::secure_fs::PinnedDirectory>,
    bound_metadata: Option<crate::secure_fs::PinnedDirectory>,
    /// Last successful implicit identity check made by `connection()`.
    implicit_verified: Mutex<Option<std::time::Instant>>,
    format_version: u32,
    /// Format 6: the registered CAS catalog of this immutable snapshot,
    /// loaded once by the first CAS view opened on it.
    registered: std::sync::OnceLock<std::sync::Arc<super::cas::RegisteredCatalog>>,
    /// Trees read and verified through this session (CAS bytes against
    /// their id, `objects` row, edges). The shared Store lock excludes
    /// writers for the session's life, so its rows cannot change and a Tree
    /// verified once in it stays verified; nothing outlives the session.
    /// With the total number of their entries (bounded).
    verified_trees: Mutex<(
        std::collections::HashMap<layrs_core::scm::TreeId, std::sync::Arc<layrs_core::scm::Tree>>,
        usize,
    )>,
    /// The CAS view validated by this session's first reader (its root,
    /// objects and staging directories checked). Later readers of the same
    /// session reuse it: the session's shared lock excludes writers, and every
    /// CAS read is still checked against its content address.
    cas_view: std::sync::OnceLock<super::cas::NativeCas>,
    /// The local identity registry, read and validated once per session
    /// (changed only under the exclusive lock).
    identities: std::sync::OnceLock<super::identity::IdentityRegistry>,
}

/// Implicit per-query rebinding checks are throttled to this interval. SQLite
/// reads stay bound to the already opened database handle and CAS reads stay
/// bound to their pinned objects directory, rechecked on the same interval
/// (`NativeCas::pinned_objects_for_read`); every CAS byte is verified against
/// its content address, so a per-query recheck adds no integrity. CAS
/// publication re-pins on every access. Bind time, every explicit
/// `verify_filesystem_identity()` and each write commit stay unconditional;
/// a replaced path is still detected within one interval.
const IMPLICIT_IDENTITY_RECHECK: std::time::Duration = std::time::Duration::from_millis(250);

impl NativeStoreOpen {
    pub(crate) fn new(
        layout: NativeStoreLayout,
        store_id: StoreId,
        connection: Connection,
        store_lock: StoreLock,
    ) -> Self {
        Self {
            connection: Mutex::new(connection),
            _store_lock: store_lock,
            layout,
            store_id,
            bound_directory: None,
            bound_metadata: None,
            implicit_verified: Mutex::new(None),
            format_version: super::FORMAT_VERSION,
            registered: std::sync::OnceLock::new(),
            verified_trees: Mutex::new((std::collections::HashMap::new(), 0)),
            cas_view: std::sync::OnceLock::new(),
            identities: std::sync::OnceLock::new(),
        }
    }

    /// The CAS view validated by this session's first reader.
    pub(crate) fn cas_view(&self) -> &std::sync::OnceLock<super::cas::NativeCas> {
        &self.cas_view
    }

    /// The identity registry as this session first read it.
    pub(crate) fn identities(
        &self,
    ) -> Result<super::identity::IdentityRegistry, layrs_store::scm::StoreError> {
        if let Some(registry) = self.identities.get() {
            return Ok(registry.clone());
        }
        let registry = super::identity::read_registry(self.layout.root(), &self.store_id)?;
        Ok(self.identities.get_or_init(|| registry).clone())
    }

    /// A Tree this session already read and verified.
    pub(crate) fn verified_tree(
        &self,
        id: &layrs_core::scm::TreeId,
    ) -> Option<std::sync::Arc<layrs_core::scm::Tree>> {
        self.verified_trees.lock().ok()?.0.get(id).cloned()
    }

    /// Records a Tree this session read and verified, while all recorded
    /// Trees hold at most 400,000 entries (about 60 MB); the memory goes
    /// with the session.
    pub(crate) fn remember_verified_tree(&self, tree: &layrs_core::scm::Tree) {
        const MAX_ENTRIES: usize = 400_000;
        if let Ok(mut guard) = self.verified_trees.lock() {
            let (trees, entries) = &mut *guard;
            let size = tree.entries().len().max(1);
            if *entries + size <= MAX_ENTRIES && !trees.contains_key(tree.id()) {
                *entries += size;
                trees.insert(tree.id().clone(), std::sync::Arc::new(tree.clone()));
            }
        }
    }

    pub(crate) fn registered_catalog(
        &self,
    ) -> &std::sync::OnceLock<std::sync::Arc<super::cas::RegisteredCatalog>> {
        &self.registered
    }

    pub fn layout(&self) -> &NativeStoreLayout {
        &self.layout
    }

    pub fn store_id(&self) -> &StoreId {
        &self.store_id
    }

    /// The validated format version of this Store (2, 5 or 6).
    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    /// Whether this Store's CAS may hold packs v1 of small objects (ADR
    /// 0026): format 5 only. Format 6 has packs v2 registered in SQLite.
    pub fn packs_enabled(&self) -> bool {
        self.format_version == super::PACKED_FORMAT_VERSION
    }

    /// Whether this Store has the compact format 6 (ADR 0028).
    pub fn compact(&self) -> bool {
        self.format_version == super::COMPACT_FORMAT_VERSION
    }

    pub(crate) fn with_format_version(mut self, format_version: u32) -> Self {
        self.format_version = format_version;
        self
    }

    pub(crate) fn connection(&self) -> Result<MutexGuard<'_, Connection>, ScmStoreError> {
        crate::perf::count("store.connection", 1);
        let backend = |message: String| ScmStoreError::Backend { message };
        let mut verified = self
            .implicit_verified
            .lock()
            .map_err(|_| backend("native Store identity timer mutex is poisoned".into()))?;
        if verified.is_none_or(|at| at.elapsed() >= IMPLICIT_IDENTITY_RECHECK) {
            self.verify_filesystem_identity()
                .map_err(|error| backend(error.to_string()))?;
            *verified = Some(std::time::Instant::now());
        }
        drop(verified);
        self.connection.lock().map_err(|_| ScmStoreError::Backend {
            message: "native Store read connection mutex is poisoned".into(),
        })
    }

    /// Bind this shared capability to an identity retained by its caller.
    pub fn with_filesystem_identity(mut self, expected: &str) -> io::Result<Self> {
        self.bound_directory = Some(bind_directory(self.layout.root(), expected)?);
        self.bound_metadata = Some(crate::secure_fs::PinnedDirectory::open(
            self.layout.metadata_dir(),
        )?);
        Ok(self)
    }

    pub fn with_metadata_identity(mut self, expected: &str) -> io::Result<Self> {
        self.bound_metadata = Some(bind_directory(self.layout.metadata_dir(), expected)?);
        self.verify_filesystem_identity()?;
        Ok(self)
    }

    /// `with_filesystem_identity` then `with_metadata_identity` in one pass:
    /// the root is pinned from the volume root, then `metadata` is opened
    /// through that pinned handle (no second walk of the path), and each must
    /// have its expected identity.
    pub fn with_identities(mut self, store: &str, metadata: &str) -> io::Result<Self> {
        let root = bind_directory(self.layout.root(), store)?;
        let pinned = root.open_child_for_traversal(std::ffi::OsStr::new(METADATA_DIR_NAME))?;
        if pinned.identity_token() != metadata {
            return Err(io::Error::other("Store filesystem identity changed"));
        }
        self.bound_directory = Some(root);
        self.bound_metadata = Some(pinned);
        self.verify_filesystem_identity()?;
        Ok(self)
    }

    /// The identity of the Store root this session is bound to, if bound.
    pub(crate) fn bound_root_identity(&self) -> Option<String> {
        self.bound_directory
            .as_ref()
            .map(crate::secure_fs::PinnedDirectory::identity_token)
    }

    pub fn verify_filesystem_identity(&self) -> io::Result<()> {
        if let Some(directory) = &self.bound_directory {
            directory.verify_path_identity()?;
        }
        if let Some(directory) = &self.bound_metadata {
            directory.verify_path_identity()?;
        }
        Ok(())
    }
}

/// Ready format capability for audit paths that must report schema damage.
#[derive(Debug)]
pub struct NativeStoreAudit {
    _store_lock: StoreLock,
    layout: NativeStoreLayout,
    store_id: StoreId,
    format_version: u32,
}

impl NativeStoreAudit {
    pub(crate) fn new(layout: NativeStoreLayout, store_id: StoreId, store_lock: StoreLock) -> Self {
        Self {
            _store_lock: store_lock,
            layout,
            store_id,
            format_version: super::FORMAT_VERSION,
        }
    }

    /// The validated format version of this Store (2, 5 or 6).
    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    /// Whether this Store's CAS may hold packs v1 of small objects (ADR
    /// 0026): format 5 only. Format 6 has packs v2 registered in SQLite.
    pub fn packs_enabled(&self) -> bool {
        self.format_version == super::PACKED_FORMAT_VERSION
    }

    /// Whether this Store has the compact format 6 (ADR 0028).
    pub fn compact(&self) -> bool {
        self.format_version == super::COMPACT_FORMAT_VERSION
    }

    pub(crate) fn with_format_version(mut self, format_version: u32) -> Self {
        self.format_version = format_version;
        self
    }

    pub fn layout(&self) -> &NativeStoreLayout {
        &self.layout
    }

    pub fn store_id(&self) -> &StoreId {
        &self.store_id
    }

    /// Opens SQLite lazily in immutable, schema-tolerant mode for fsck.
    pub(crate) fn try_connection(&self) -> Result<Connection, ScmSchemaError> {
        sqlite::open_audit_read_only(self.layout.sqlite_path(), Duration::from_secs(5))
    }
}

/// Exclusive writer capability.
///
/// Acquiring this handle is deliberately side-effect free: SQLite is only opened
/// when a mutating operation explicitly begins.
#[derive(Debug)]
pub struct NativeStoreExclusive {
    _store_lock: StoreLock,
    layout: NativeStoreLayout,
    store_id: StoreId,
    bound_directory: Option<crate::secure_fs::PinnedDirectory>,
    bound_metadata: Option<crate::secure_fs::PinnedDirectory>,
    format_version: u32,
}

impl NativeStoreExclusive {
    pub(crate) fn new(layout: NativeStoreLayout, store_id: StoreId, store_lock: StoreLock) -> Self {
        Self {
            _store_lock: store_lock,
            layout,
            store_id,
            bound_directory: None,
            bound_metadata: None,
            format_version: super::FORMAT_VERSION,
        }
    }

    /// The validated format version of this Store (2, 5 or 6).
    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    /// Whether this Store's CAS may hold packs v1 of small objects (ADR
    /// 0026): format 5 only. Format 6 has packs v2 registered in SQLite.
    pub fn packs_enabled(&self) -> bool {
        self.format_version == super::PACKED_FORMAT_VERSION
    }

    /// Whether this Store has the compact format 6 (ADR 0028).
    pub fn compact(&self) -> bool {
        self.format_version == super::COMPACT_FORMAT_VERSION
    }

    pub(crate) fn with_format_version(mut self, format_version: u32) -> Self {
        self.format_version = format_version;
        self
    }

    pub fn layout(&self) -> &NativeStoreLayout {
        &self.layout
    }

    pub fn store_id(&self) -> &StoreId {
        &self.store_id
    }

    /// Opens the writer connection at the explicit mutation boundary.
    pub(crate) fn begin_write_connection(&self) -> Result<Connection, ScmSchemaError> {
        self.verify_filesystem_identity()?;
        sqlite::open_writer_existing_no_mutation(
            self.layout.sqlite_path(),
            Duration::from_secs(5),
            &self.store_id,
        )
    }

    pub fn with_filesystem_identity(mut self, expected: &str) -> io::Result<Self> {
        self.bound_directory = Some(bind_directory(self.layout.root(), expected)?);
        self.bound_metadata = Some(crate::secure_fs::PinnedDirectory::open(
            self.layout.metadata_dir(),
        )?);
        Ok(self)
    }

    pub fn with_metadata_identity(mut self, expected: &str) -> io::Result<Self> {
        self.bound_metadata = Some(bind_directory(self.layout.metadata_dir(), expected)?);
        self.verify_filesystem_identity()?;
        Ok(self)
    }

    pub fn verify_filesystem_identity(&self) -> io::Result<()> {
        if let Some(directory) = &self.bound_directory {
            directory.verify_path_identity()?;
        }
        if let Some(directory) = &self.bound_metadata {
            directory.verify_path_identity()?;
        }
        Ok(())
    }
}

fn bind_directory(root: &Path, expected: &str) -> io::Result<crate::secure_fs::PinnedDirectory> {
    let directory = crate::secure_fs::PinnedDirectory::open(root)?;
    if directory.identity_token() != expected {
        return Err(io::Error::other("Store filesystem identity changed"));
    }
    Ok(directory)
}

fn ensure_regular_file_durable(path: &Path, parent: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            drop(open_read_no_follow(path)?);
            Ok(())
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native Store boundary is not a regular file: {}",
                path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let file = create_new_file_no_follow(path)?;
            file.sync_all()?;
            sync_directory(parent)
        }
        Err(error) => Err(error),
    }
}

fn absolute_lexical(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "native store root must not be empty",
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "native store path escapes filesystem root: {}",
                            path.display()
                        ),
                    ));
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}
