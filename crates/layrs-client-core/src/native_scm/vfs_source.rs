//! Verified complete-Store input for a filesystem provider. No network access,
//! and no Store lock survives a call. Mount lifecycle and acceptance are separate.
use super::{NativeClientError, NativeWorkspace, Result};
use layrs_core::scm::{
    FileId, FileManifest, OperationId, RepoPath, SpaceId, StepId, StoreId, Tree, TreeEntryTarget,
    TreeId,
};
use layrs_store::scm::{ScmReader, StoreObjectId};
use layrs_store_local::scm_v1::checkout::{CheckoutProjectionContent, CheckoutReader};
use layrs_store_local::scm_v1::{self, NativeScmReader};
use layrs_vfs::{Cancellation, Catalog, CatalogEntry, SnapshotSource};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// A locator, not authority: reopening revalidates every identity and the full
/// admitted closure. It contains no network capability or credential.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedVfsSnapshotBinding {
    pub store_root: PathBuf,
    pub store_id: StoreId,
    pub store_identity: String,
    pub store_metadata_identity: String,
    pub space_id: SpaceId,
    pub operation_id: OperationId,
    pub step_id: StepId,
}

/// The exact Step must be reachable from a retained native Operation. Native GC
/// roots retained Operations; this source never treats an admission as a pin.
/// The original Operation remains necessary after the source Worktree advances.
pub struct VerifiedVfsSnapshot {
    store_root: PathBuf,
    store_id: StoreId,
    store_identity: String,
    store_metadata_identity: String,
    space_id: SpaceId,
    operation_id: OperationId,
    step_id: StepId,
    catalog: Catalog,
    /// Admitted manifest of each file, recorded from metadata. The manifest
    /// object itself is read from the CAS (and proven against its id) the
    /// first time the file is hydrated: a file never read costs nothing.
    manifest_ids: BTreeMap<FileId, (layrs_core::scm::FileManifestId, u64)>,
    manifests: std::sync::Mutex<BTreeMap<FileId, std::sync::Arc<FileManifest>>>,
    chunk_reads: AtomicU64,
    chunk_bytes: AtomicU64,
    /// CAS view used by hydration, revalidated against the Store every
    /// `CAS_REVALIDATE`. Holds no Store lock between reads.
    cas: std::sync::Mutex<Option<(scm_v1::NativeCas, Instant)>>,
    /// The Store's shared lock, held around each hydrated range.
    read_lock: scm_v1::SharedReadLocks,
    /// The last few chunks read (and verified) by hydration. Ranges are
    /// smaller than chunks, so a sequential read reuses each chunk instead of
    /// re-reading and re-hashing it per range. Chunks are immutable.
    recent: std::sync::Mutex<std::collections::VecDeque<std::sync::Arc<layrs_core::scm::Chunk>>>,
}

/// Bounds of `recent`: entries and their total bytes.
const RECENT_CHUNKS: usize = 8;
const RECENT_BYTES: u64 = 64 * 1024 * 1024;

/// How long hydration reuses one CAS view before re-binding it to the Store.
const CAS_REVALIDATE: Duration = Duration::from_secs(2);

impl VerifiedVfsSnapshot {
    /// Verify the full local closure before returning a provider capability.
    /// Provider namespace/type limits fail before any mount registration.
    pub fn prepare(
        workspace: &NativeWorkspace,
        step: &StepId,
        cancel: &Cancellation,
    ) -> Result<Self> {
        cancelled(cancel)?;
        let state = workspace.state()?;
        let store = workspace.open_store()?;
        let binding = VerifiedVfsSnapshotBinding {
            store_root: store.layout().root().to_path_buf(),
            store_id: store.store_id().clone(),
            store_identity: workspace.store_identity().to_owned(),
            store_metadata_identity: workspace.metadata_identity().to_owned(),
            space_id: state.space.id().clone(),
            operation_id: state.operation.id().clone(),
            step_id: step.clone(),
        };
        drop(store);
        Self::reopen(&binding, cancel)
    }

    /// Unsupported development snapshots fail closed without modifying their
    /// checkout or journal. Named empty directories and Tombstones cannot be
    /// represented by the provider's file-derived catalog. Such old mounts also
    /// cannot use normal VFS reopen/export; this is not a flattening or migration.
    pub fn reopen(binding: &VerifiedVfsSnapshotBinding, cancel: &Cancellation) -> Result<Self> {
        cancelled(cancel)?;
        if !binding.store_root.is_absolute()
            || binding.store_identity.len() > 256
            || binding.store_metadata_identity.len() > 256
        {
            return Err(NativeClientError::InvalidState("invalid VFS Store binding"));
        }
        let store = open_for_range(
            &binding.store_root,
            &binding.store_identity,
            &binding.store_metadata_identity,
            cancel,
        )?;
        if store.store_id() != &binding.store_id {
            return Err(NativeClientError::InvalidState(
                "VFS Store identity changed",
            ));
        }
        let reader = NativeScmReader::new(&store)?;
        if reader
            .get_operation(&binding.operation_id)?
            .as_ref()
            .map(|op| op.space_id())
            != Some(&binding.space_id)
            || !reader.operation_reaches_step(&binding.operation_id, &binding.step_id)?
        {
            return Err(NativeClientError::InvalidState(
                "VFS Step is not rooted by the source Operation",
            ));
        }
        if !reader.object_admitted_to_space(
            &StoreObjectId::Step(binding.step_id.clone()),
            &binding.space_id,
        )? {
            return Err(NativeClientError::Missing("Step admitted to this Space"));
        }
        let step = reader
            .get_step(&binding.step_id)?
            .ok_or(NativeClientError::Missing("VFS source Step"))?;
        validate_vfs_tree_namespace(step.root_tree_id(), cancel, &mut |id| {
            reader
                .get_tree(id)?
                .ok_or(NativeClientError::Missing("VFS source Tree"))
        })?;
        let projection = super::projection::read_admitted_step_projection(
            &reader,
            &binding.space_id,
            &binding.step_id,
            Some(store.layout().root()),
        )?;
        let mut entries = Vec::with_capacity(projection.len());
        let mut manifest_ids = BTreeMap::new();
        for entry in projection {
            cancelled(cancel)?;
            let CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable: false,
            } = entry.content
            else {
                return Err(NativeClientError::InvalidState(
                    "VFS prototype requires regular non-executable files",
                ));
            };
            if !manifest_ids.contains_key(&file_id) {
                // Metadata only: the File is recorded present with a manifest
                // validated at admission (chunk ids, offsets, length, FileId).
                // That manifest is read and checked against this file at its
                // first hydration, and every hydrated range is read through
                // the CAS, which proves each chunk against its id. Reading
                // every manifest or file here made Room creation O(project).
                let ids = reader.file_manifest_ids_metadata(&file_id)?;
                let id = ids
                    .first()
                    .ok_or(NativeClientError::Missing("complete VFS file"))?;
                manifest_ids.insert(file_id.clone(), (id.clone(), byte_length));
            }
            entries.push(CatalogEntry {
                path: RepoPath::parse(entry.path)?,
                file_id,
                size: byte_length,
            });
        }
        let catalog = Catalog::new(1, entries)
            .map_err(|e| NativeClientError::Filesystem(io::Error::other(e)))?;
        cancelled(cancel)?;
        Ok(Self {
            store_root: store.layout().root().to_path_buf(),
            store_id: store.store_id().clone(),
            store_identity: binding.store_identity.clone(),
            store_metadata_identity: binding.store_metadata_identity.clone(),
            space_id: binding.space_id.clone(),
            operation_id: binding.operation_id.clone(),
            step_id: binding.step_id.clone(),
            catalog,
            manifest_ids,
            manifests: std::sync::Mutex::new(BTreeMap::new()),
            chunk_reads: AtomicU64::new(0),
            chunk_bytes: AtomicU64::new(0),
            cas: std::sync::Mutex::new(None),
            read_lock: scm_v1::SharedReadLocks::open(store.layout().root()).map_err(|error| {
                NativeClientError::Filesystem(io::Error::other(error.to_string()))
            })?,
            recent: std::sync::Mutex::new(std::collections::VecDeque::new()),
        })
    }

    /// The admitted manifest of `id`, read from the CAS (proven against its
    /// object id) on first use and checked against this file's identity and
    /// catalog length.
    /// Chunk ids of a source File's admitted manifest: content the Space
    /// already has, which a capture of an edited copy need not spool again.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn source_chunk_ids(
        &self,
        id: &FileId,
    ) -> Result<std::collections::BTreeSet<layrs_core::scm::ChunkId>> {
        Ok(self
            .manifest(id, &Cancellation::default())?
            .chunks()
            .iter()
            .map(|chunk| chunk.chunk_id().clone())
            .collect())
    }

    fn manifest(&self, id: &FileId, cancel: &Cancellation) -> Result<std::sync::Arc<FileManifest>> {
        if let Some(manifest) = self.cached_manifest(id)? {
            return Ok(manifest);
        }
        // Read under the Store's shared lock, as every range is.
        let _lock = shared_read_lock(&self.read_lock, cancel)?;
        self.manifest_locked(id, cancel)
    }

    fn cached_manifest(&self, id: &FileId) -> Result<Option<std::sync::Arc<FileManifest>>> {
        Ok(self
            .manifests
            .lock()
            .map_err(|_| NativeClientError::InvalidState("VFS manifest cache poisoned"))?
            .get(id)
            .cloned())
    }

    /// `manifest` while the caller holds the Store's shared lock.
    fn manifest_locked(
        &self,
        id: &FileId,
        cancel: &Cancellation,
    ) -> Result<std::sync::Arc<FileManifest>> {
        if let Some(manifest) = self.cached_manifest(id)? {
            return Ok(manifest);
        }
        let (manifest_id, length) = self
            .manifest_ids
            .get(id)
            .ok_or(NativeClientError::Missing("VFS manifest"))?;
        let manifest = match self
            .read_object(&StoreObjectId::FileManifest(manifest_id.clone()), cancel)
            .map_err(|error| NativeClientError::InvalidSnapshot(format!("VFS manifest: {error}")))?
        {
            layrs_store::scm::CanonicalObject::FileManifest(manifest) => manifest,
            _ => return Err(NativeClientError::InvalidState("VFS manifest kind differs")),
        };
        if manifest.file_id() != id || manifest.byte_length() != *length {
            return Err(NativeClientError::InvalidState(
                "VFS manifest identity differs",
            ));
        }
        let manifest = std::sync::Arc::new(manifest);
        self.manifests
            .lock()
            .map_err(|_| NativeClientError::InvalidState("VFS manifest cache poisoned"))?
            .insert(id.clone(), manifest.clone());
        Ok(manifest)
    }

    /// `id` through the cached CAS view; if that fails, through a view
    /// reloaded now (once). A consolidation in another process may have
    /// retired a pack the cached view's catalog still names; a reloaded
    /// catalog no longer does, while real damage fails again (council s6 r1,
    /// as git rescans its packs).
    fn read_object(
        &self,
        id: &StoreObjectId,
        cancel: &Cancellation,
    ) -> Result<layrs_store::scm::CanonicalObject> {
        match self.chunk_source(cancel)?.read(id) {
            Ok(object) => Ok(object),
            Err(_) => {
                self.forget_chunk_source();
                self.chunk_source(cancel)?
                    .read(id)
                    .map_err(|error| NativeClientError::InvalidSnapshot(error.to_string()))
            }
        }
    }

    /// Drops the cached CAS view: the next read reloads the catalog.
    fn forget_chunk_source(&self) {
        if let Ok(mut cached) = self.cas.lock() {
            *cached = None;
        }
    }

    /// The CAS view for hydration. Opening the Store (lock, SQLite, identity
    /// checks) for every range made each first read of a virtual file cost
    /// ~20 ms. The Step's admission and its retained Operation were proven
    /// when this snapshot was opened and are never withdrawn; every chunk read
    /// below is still proven against its content id by the CAS itself. The
    /// view is re-bound to the original Store incarnation every
    /// `CAS_REVALIDATE`, and its own pins recheck the Store path continuously.
    fn chunk_source(&self, cancel: &Cancellation) -> Result<scm_v1::NativeCas> {
        let mut cached = self
            .cas
            .lock()
            .map_err(|_| NativeClientError::InvalidState("VFS CAS cache poisoned"))?;
        if let Some((cas, bound_at)) = cached.as_ref() {
            if bound_at.elapsed() < CAS_REVALIDATE {
                return Ok(cas.clone());
            }
        }
        let store = open_for_range(
            &self.store_root,
            &self.store_identity,
            &self.store_metadata_identity,
            cancel,
        )?;
        let guard = CheckoutReader::open(&self.store_root, Some(&self.store_identity))?;
        if store.store_id() != &self.store_id {
            return Err(NativeClientError::InvalidState(
                "VFS Store identity changed",
            ));
        }
        let cas = scm_v1::NativeCas::open(&store)
            .map_err(|error| NativeClientError::InvalidSnapshot(format!("VFS CAS: {error}")))?;
        CheckoutReader::open(&self.store_root, Some(&guard.identity()))?;
        *cached = Some((cas.clone(), Instant::now()));
        Ok(cas)
    }

    pub fn binding(&self) -> VerifiedVfsSnapshotBinding {
        VerifiedVfsSnapshotBinding {
            store_root: self.store_root.clone(),
            store_id: self.store_id.clone(),
            store_identity: self.store_identity.clone(),
            store_metadata_identity: self.store_metadata_identity.clone(),
            space_id: self.space_id.clone(),
            operation_id: self.operation_id.clone(),
            step_id: self.step_id.clone(),
        }
    }

    pub fn step_id(&self) -> &StepId {
        &self.step_id
    }
    pub fn retained_operation_id(&self) -> &OperationId {
        &self.operation_id
    }
    /// Actual callback CAS reads. Initial full closure verification is excluded.
    pub fn read_counts(&self) -> (u64, u64) {
        (
            self.chunk_reads.load(Ordering::Relaxed),
            self.chunk_bytes.load(Ordering::Relaxed),
        )
    }
}

/// A directory is representable only if some live descendant will produce its
/// path. Every named child Tree is inspected separately, so an empty branch is
/// refused even when its parent also contains files or shares a Tree identity.
/// The implicit root is always present and needs no catalog entry of its own.
/// Tombstones are refused at every depth: reconstructing the catalog's files
/// would otherwise silently remove that canonical metadata during capture.
pub(super) fn validate_vfs_tree_placement(named: bool, tree: &Tree) -> Result<()> {
    if tree
        .entries()
        .iter()
        .any(|entry| matches!(entry.target(), TreeEntryTarget::Tombstone { .. }))
    {
        return Err(NativeClientError::InvalidState(
            "VFS snapshot unsupported: Tombstones cannot be represented; existing checkout bytes and journal are retained",
        ));
    }
    if named && tree.entries().is_empty() {
        return Err(NativeClientError::InvalidState(
            "VFS snapshot unsupported: named empty directories cannot be represented; existing checkout bytes and journal are retained",
        ));
    }
    if tree
        .entries()
        .iter()
        .any(|entry| entry.executable() && matches!(entry.target(), TreeEntryTarget::Tree { .. }))
    {
        return Err(NativeClientError::InvalidState(
            "VFS snapshot unsupported: executable directory entries cannot be represented; existing checkout bytes and journal are retained",
        ));
    }
    Ok(())
}

// This metadata-only preflight intentionally leaves ordinary projection rules
// unchanged. Count each placement, not unique Tree IDs; aliases expand paths.
fn validate_vfs_tree_namespace(
    root: &TreeId,
    cancel: &Cancellation,
    read_tree: &mut impl FnMut(&TreeId) -> Result<Tree>,
) -> Result<()> {
    let mut pending = vec![(root.clone(), 0usize)];
    let mut user_entries = 0usize;
    let mut metadata_entries = 0usize;
    while let Some((id, depth)) = pending.pop() {
        cancelled(cancel)?;
        if depth > 64 {
            return Err(NativeClientError::InvalidState("VFS namespace depth limit"));
        }
        let tree = read_tree(&id)?;
        cancelled(cancel)?;
        // Bound metadata traversal work before classifying unsupported entries
        // (the Store's admission edge-visit budget).
        metadata_entries = metadata_entries
            .checked_add(tree.entries().len())
            .filter(|count| *count <= 262_144)
            .ok_or(NativeClientError::InvalidState(
                "VFS namespace metadata limit",
            ))?;
        validate_vfs_tree_placement(depth != 0, &tree)?;
        for entry in tree.entries() {
            cancelled(cancel)?;
            user_entries += 1;
            if user_entries > 100_000 {
                return Err(NativeClientError::InvalidState("VFS namespace entry limit"));
            }
            if let TreeEntryTarget::Tree { tree_id } = entry.target() {
                pending.push((tree_id.clone(), depth + 1));
            }
        }
    }
    cancelled(cancel)
}

impl SnapshotSource for VerifiedVfsSnapshot {
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    fn read_exact_at(
        &self,
        id: &FileId,
        offset: u64,
        output: &mut [u8],
        cancel: &Cancellation,
    ) -> layrs_vfs::Result<()> {
        cancel.check()?;
        let _range = layrs_store_local::perf::span("vfs.read_range");
        // Each range waits for an exclusive Store writer like a shared open
        // would, but holds only the Store's shared file lock: one hold for
        // the manifest (on a first read) and the chunks.
        let lock_span = layrs_store_local::perf::span("vfs.read_lock");
        let lock = shared_read_lock(&self.read_lock, cancel)
            .map_err(|error| layrs_vfs::Error::Io(io::Error::other(error.to_string())))?;
        drop(lock_span);
        let manifest_span = layrs_store_local::perf::span("vfs.read_manifest");
        let manifest = self
            .manifest_locked(id, cancel)
            .map_err(|error| match error {
                NativeClientError::Missing(_) => layrs_vfs::Error::MissingBytes {
                    file_id: id.clone(),
                    offset,
                    length: output.len(),
                },
                error => layrs_vfs::Error::Io(io::Error::other(error.to_string())),
            })?;
        drop(manifest_span);
        let end = offset
            .checked_add(output.len() as u64)
            .filter(|end| *end <= manifest.byte_length())
            .ok_or(layrs_vfs::Error::InvalidRange)?;
        let result = (|| -> Result<()> {
            let _lock = &lock;
            {
                let _span = layrs_store_local::perf::span("vfs.read_cas_bind");
                self.chunk_source(cancel)?;
            }
            let _chunks = layrs_store_local::perf::span("vfs.read_chunks");
            let mut copied = 0usize;
            // Chunks are ordered and contiguous: start at the one holding
            // `offset` instead of scanning a large file's whole list.
            let first = manifest
                .chunks()
                .partition_point(|part| part.offset() + part.byte_length() <= offset);
            for part in &manifest.chunks()[first..] {
                cancelled(cancel)?;
                if part.offset() >= end {
                    break;
                }
                let part_end = part
                    .offset()
                    .checked_add(part.byte_length())
                    .ok_or(NativeClientError::InvalidState("VFS chunk range overflow"))?;
                let start = offset.max(part.offset());
                let stop = end.min(part_end);
                if start >= stop {
                    continue;
                }
                let cached =
                    self.recent.lock().ok().and_then(|recent| {
                        recent.iter().find(|c| c.id() == part.chunk_id()).cloned()
                    });
                let chunk = match cached {
                    Some(chunk) => chunk,
                    None => {
                        let chunk = match self
                            .read_object(&StoreObjectId::ChunkRaw(part.chunk_id().clone()), cancel)
                            .map_err(|error| {
                                NativeClientError::InvalidSnapshot(format!("VFS chunk: {error}"))
                            })? {
                            layrs_store::scm::CanonicalObject::ChunkRaw(chunk) => {
                                std::sync::Arc::new(chunk)
                            }
                            _ => {
                                return Err(NativeClientError::InvalidState(
                                    "VFS chunk kind differs",
                                ));
                            }
                        };
                        self.chunk_reads.fetch_add(1, Ordering::Relaxed);
                        self.chunk_bytes
                            .fetch_add(chunk.byte_length(), Ordering::Relaxed);
                        if let Ok(mut recent) = self.recent.lock() {
                            recent.push_back(std::sync::Arc::clone(&chunk));
                            while recent.len() > RECENT_CHUNKS
                                || recent.iter().map(|c| c.byte_length()).sum::<u64>()
                                    > RECENT_BYTES
                            {
                                recent.pop_front();
                            }
                        }
                        chunk
                    }
                };
                if chunk.byte_length() != part.byte_length() {
                    return Err(NativeClientError::InvalidState("VFS chunk length differs"));
                }
                let source = usize::try_from(start - part.offset())
                    .map_err(|_| NativeClientError::InvalidState("VFS offset overflow"))?;
                let destination = usize::try_from(start - offset)
                    .map_err(|_| NativeClientError::InvalidState("VFS offset overflow"))?;
                let length = usize::try_from(stop - start)
                    .map_err(|_| NativeClientError::InvalidState("VFS length overflow"))?;
                output[destination..destination + length]
                    .copy_from_slice(&chunk.bytes()[source..source + length]);
                copied += length;
            }
            if copied != output.len() {
                return Err(NativeClientError::Missing("complete VFS range"));
            }
            cancelled(cancel)?;
            Ok(())
        })();
        if cancel.is_cancelled() {
            return Err(layrs_vfs::Error::Cancelled);
        }
        result.map_err(|error| layrs_vfs::Error::Io(io::Error::other(error)))
    }
}

/// A repository writer in another Worktree may briefly own the exclusive lock.
/// Wait only for the two typed contention errors, and never hold a Store lock
/// while waiting. Callers must not request hydration inside a write transaction.
/// The deadline bounds shutdown even if a writer never releases its lock.
fn open_for_range(
    root: &Path,
    identity: &str,
    metadata_identity: &str,
    cancel: &Cancellation,
) -> Result<scm_v1::NativeStoreOpen> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cancelled(cancel)?;
        match scm_v1::open(root) {
            Ok(store) => {
                cancelled(cancel)?;
                return Ok(store
                    .with_filesystem_identity(identity)?
                    .with_metadata_identity(metadata_identity)?);
            }
            Err(scm_v1::NativeStoreError::Lock(
                layrs_store_local::StoreError::LockBusy(_)
                | layrs_store_local::StoreError::ReentrantLock(_),
            )) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// The Store's shared lock, waiting (bounded, cancellable) for a writer.
fn shared_read_lock<'a>(
    locks: &'a scm_v1::SharedReadLocks,
    cancel: &Cancellation,
) -> Result<scm_v1::SharedReadLock<'a>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cancelled(cancel)?;
        match locks.hold() {
            Ok(lock) => return Ok(lock),
            Err(
                layrs_store_local::StoreError::LockBusy(_)
                | layrs_store_local::StoreError::ReentrantLock(_),
            ) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            Err(error) => {
                return Err(NativeClientError::Filesystem(io::Error::other(
                    error.to_string(),
                )))
            }
        }
    }
}

fn cancelled(cancel: &Cancellation) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(NativeClientError::Filesystem(io::Error::new(
            io::ErrorKind::Interrupted,
            "VFS source cancelled",
        )));
    }
    Ok(())
}

#[cfg(test)]
mod namespace_tests {
    use super::*;
    use layrs_core::scm::{RepoPathComponent, Tombstone, TreeEntry};

    fn tree(entries: Vec<(&str, TreeEntryTarget)>) -> Tree {
        Tree::new(
            entries
                .into_iter()
                .map(|(name, target)| {
                    TreeEntry::new(RepoPathComponent::parse(name).unwrap(), target, false)
                })
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn named_empty_predicate_preserves_root_and_live_child_cases() {
        let empty = tree(vec![]);
        let tombstone = Tombstone::new(None).unwrap();
        let deleted = tree(vec![(
            "deleted",
            TreeEntryTarget::tombstone(tombstone.id().clone()),
        )]);
        validate_vfs_tree_placement(false, &empty).unwrap();
        assert!(validate_vfs_tree_placement(true, &empty).is_err());
        assert!(validate_vfs_tree_placement(false, &deleted).is_err());
        assert!(validate_vfs_tree_placement(true, &deleted).is_err());
        let mixed = tree(vec![
            (
                "deleted",
                TreeEntryTarget::tombstone(tombstone.id().clone()),
            ),
            ("file", TreeEntryTarget::file(FileId::from_bytes(&[]))),
        ]);
        assert!(validate_vfs_tree_placement(false, &mixed).is_err());
        assert!(validate_vfs_tree_placement(true, &mixed).is_err());
        let executable_directory = Tree::new(vec![TreeEntry::new(
            RepoPathComponent::parse("directory").unwrap(),
            TreeEntryTarget::tree(empty.id().clone()),
            true,
        )])
        .unwrap();
        assert!(validate_vfs_tree_placement(false, &executable_directory).is_err());
        assert!(validate_vfs_tree_placement(true, &executable_directory).is_err());
        let branch = tree(vec![("nested", TreeEntryTarget::tree(empty.id().clone()))]);
        // The local predicate does not mistake a child placement for a leaf.
        // The traversal must visit that child and refuse it independently.
        validate_vfs_tree_placement(true, &branch).unwrap();
        let graphs = BTreeMap::from([
            (empty.id().clone(), empty),
            (branch.id().clone(), branch.clone()),
        ]);
        assert!(
            validate_vfs_tree_namespace(branch.id(), &Cancellation::default(), &mut |id| Ok(
                graphs[id].clone()
            ))
            .is_err()
        );
    }

    #[test]
    fn namespace_walk_visits_every_nonempty_alias_placement() {
        let branch = tree(vec![(
            "file",
            TreeEntryTarget::file(FileId::from_bytes(&[])),
        )]);
        let root = tree(vec![
            ("a", TreeEntryTarget::tree(branch.id().clone())),
            ("b", TreeEntryTarget::tree(branch.id().clone())),
        ]);
        let graphs = BTreeMap::from([
            (branch.id().clone(), branch.clone()),
            (root.id().clone(), root.clone()),
        ]);
        let mut reads = Vec::new();
        validate_vfs_tree_namespace(root.id(), &Cancellation::default(), &mut |id| {
            reads.push(id.clone());
            Ok(graphs[id].clone())
        })
        .unwrap();
        assert_eq!(
            reads,
            [root.id().clone(), branch.id().clone(), branch.id().clone()]
        );
    }

    #[test]
    fn namespace_walk_bounds_depth_and_expanded_aliases() {
        for branching in [false, true] {
            let leaf = tree(vec![(
                "file",
                TreeEntryTarget::file(FileId::from_bytes(&[])),
            )]);
            let mut root = leaf.id().clone();
            let mut graphs = BTreeMap::from([(root.clone(), leaf)]);
            for _ in 0..if branching { 17 } else { 65 } {
                let mut entries = vec![("a", TreeEntryTarget::tree(root.clone()))];
                if branching {
                    entries.push(("b", TreeEntryTarget::tree(root)));
                }
                let parent = tree(entries);
                root = parent.id().clone();
                graphs.insert(root.clone(), parent);
            }
            let error = validate_vfs_tree_namespace(&root, &Cancellation::default(), &mut |id| {
                Ok(graphs[id].clone())
            })
            .unwrap_err();
            assert!(error.to_string().contains(if branching {
                "entry limit"
            } else {
                "depth limit"
            }));
        }
    }

    #[test]
    fn namespace_walk_honors_cancellation_before_and_after_metadata_read() {
        let root = tree(vec![]);
        for before in [true, false] {
            let cancel = Cancellation::default();
            if before {
                cancel.cancel();
            }
            let mut reads = 0;
            let error = validate_vfs_tree_namespace(root.id(), &cancel, &mut |_| {
                reads += 1;
                cancel.cancel();
                Ok(root.clone())
            })
            .unwrap_err();
            assert_eq!(reads, usize::from(!before));
            assert!(matches!(error, NativeClientError::Filesystem(error)
                if error.kind() == io::ErrorKind::Interrupted));
        }
    }
}
