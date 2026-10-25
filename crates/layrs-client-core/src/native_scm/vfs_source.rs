//! Verified complete-Store input for a filesystem provider. No network access,
//! and no Store lock survives a call. Mount lifecycle and acceptance are separate.
use super::{NativeClientError, NativeWorkspace, Result};
use layrs_core::scm::{
    FileId, FileManifest, OperationId, RepoPath, SpaceId, StepId, StoreId, Tree, TreeEntryTarget,
    TreeId,
};
use layrs_store::scm::{FileAvailability, ScmReader, StoreObjectId};
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
    manifests: BTreeMap<FileId, FileManifest>,
    chunk_reads: AtomicU64,
    chunk_bytes: AtomicU64,
}

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
        )?;
        let mut entries = Vec::with_capacity(projection.len());
        let mut manifests = BTreeMap::new();
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
            if !manifests.contains_key(&file_id) {
                let FileAvailability::Present(verified) = reader.file_availability_with_budget(
                    &file_id,
                    32_768,
                    4usize * 1024 * 1024 * 1024,
                    128,
                    16_384,
                )?
                else {
                    return Err(NativeClientError::Missing("complete VFS file"));
                };
                let id = verified
                    .verified_manifest_ids()
                    .first()
                    .ok_or(NativeClientError::Missing("verified VFS manifest"))?;
                let manifest = reader
                    .get_file_manifest(id)?
                    .ok_or(NativeClientError::Missing("VFS manifest"))?;
                if manifest.file_id() != &file_id || manifest.byte_length() != byte_length {
                    return Err(NativeClientError::InvalidState(
                        "VFS manifest identity differs",
                    ));
                }
                manifests.insert(file_id.clone(), manifest);
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
            manifests,
            chunk_reads: AtomicU64::new(0),
            chunk_bytes: AtomicU64::new(0),
        })
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
        let manifest = self
            .manifests
            .get(id)
            .ok_or_else(|| layrs_vfs::Error::MissingBytes {
                file_id: id.clone(),
                offset,
                length: output.len(),
            })?;
        let end = offset
            .checked_add(output.len() as u64)
            .filter(|end| *end <= manifest.byte_length())
            .ok_or(layrs_vfs::Error::InvalidRange)?;
        let result = (|| -> Result<()> {
            let store = open_for_range(
                &self.store_root,
                &self.store_identity,
                &self.store_metadata_identity,
                cancel,
            )?;
            // Check the original incarnation under the short Store capability.
            // A directory copy with identical logical IDs cannot replace it.
            let guard = CheckoutReader::open(&self.store_root, Some(&self.store_identity))?;
            if store.store_id() != &self.store_id {
                return Err(NativeClientError::InvalidState(
                    "VFS Store identity changed",
                ));
            }
            let reader = NativeScmReader::new(&store)?;
            if !reader.object_admitted_to_space(
                &StoreObjectId::Step(self.step_id.clone()),
                &self.space_id,
            )? || reader
                .get_operation(&self.operation_id)?
                .as_ref()
                .map(|op| op.space_id())
                != Some(&self.space_id)
            {
                return Err(NativeClientError::Missing("retained VFS source"));
            }
            let mut copied = 0usize;
            for part in manifest.chunks() {
                cancelled(cancel)?;
                let part_end = part
                    .offset()
                    .checked_add(part.byte_length())
                    .ok_or(NativeClientError::InvalidState("VFS chunk range overflow"))?;
                let start = offset.max(part.offset());
                let stop = end.min(part_end);
                if start >= stop {
                    continue;
                }
                let chunk = reader
                    .get_chunk(part.chunk_id())?
                    .ok_or(NativeClientError::Missing("VFS chunk"))?;
                if chunk.byte_length() != part.byte_length() {
                    return Err(NativeClientError::InvalidState("VFS chunk length differs"));
                }
                self.chunk_reads.fetch_add(1, Ordering::Relaxed);
                self.chunk_bytes
                    .fetch_add(chunk.byte_length(), Ordering::Relaxed);
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
            CheckoutReader::open(&self.store_root, Some(&guard.identity()))?;
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
