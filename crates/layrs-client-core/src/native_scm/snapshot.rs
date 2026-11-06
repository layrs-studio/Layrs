use super::{NativeClientError, Result};
use layrs_core::scm::{
    Chunk, ChunkingDescriptor, FileManifest, RepoPath, RepoPathComponent, Symlink, Tree, TreeEntry,
    TreeEntryTarget, TreeId,
};
use layrs_store::scm::CanonicalObject;
use std::collections::BTreeMap;
mod spool;
pub(crate) use spool::SnapshotChunkSpool;

const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct SnapshotLimits {
    pub max_entries: usize,
    pub max_depth: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
}
impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_depth: 128,
            max_file_bytes: 64 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
        }
    }
}
impl SnapshotLimits {
    /// Logical limits for spool-backed paths: content is never held whole,
    /// so file and capture sizes are unbounded (ADR 0027); only the disk
    /// bounds them. In-memory callers deliberately retain the Default limits.
    pub fn streaming() -> Self {
        Self {
            max_file_bytes: usize::MAX,
            max_total_bytes: usize::MAX,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreparedSnapshot {
    root_tree_id: TreeId,
    objects: Vec<CanonicalObject>,
    pub(super) chunks: Option<std::sync::Arc<SnapshotChunkSpool>>,
}
impl PreparedSnapshot {
    pub(super) fn from_streaming(
        entries: &[layrs_store_local::scm_v1::checkout::CheckoutProjectionEntry],
        manifests: Vec<FileManifest>,
        chunks: SnapshotChunkSpool,
        limits: SnapshotLimits,
    ) -> Result<Self> {
        Self::from_streaming_inner(entries, manifests, chunks, limits, false)
    }

    /// As `from_streaming`, where a file without a streamed manifest is a
    /// reference to content already admitted to the Space (a change-index
    /// entry, ADR 0025). Admission of the resulting Tree still requires that
    /// content to be durably admitted, so a stale reference fails the capture
    /// instead of producing a dangling Step.
    pub(super) fn from_streaming_referencing(
        entries: &[layrs_store_local::scm_v1::checkout::CheckoutProjectionEntry],
        manifests: Vec<FileManifest>,
        chunks: SnapshotChunkSpool,
        limits: SnapshotLimits,
    ) -> Result<Self> {
        Self::from_streaming_inner(entries, manifests, chunks, limits, true)
    }

    fn from_streaming_inner(
        entries: &[layrs_store_local::scm_v1::checkout::CheckoutProjectionEntry],
        manifests: Vec<FileManifest>,
        mut chunks: SnapshotChunkSpool,
        limits: SnapshotLimits,
        references: bool,
    ) -> Result<Self> {
        use layrs_store_local::scm_v1::checkout::CheckoutProjectionContent;
        let mut builder = SnapshotBuilder::new(limits);
        let manifests: BTreeMap<_, _> = manifests
            .into_iter()
            .map(|m| (m.file_id().clone(), m))
            .collect();
        let mut retained = std::collections::BTreeSet::new();
        for entry in entries {
            let path = RepoPath::parse(&entry.path)?;
            match &entry.content {
                CheckoutProjectionContent::File {
                    file_id,
                    byte_length,
                    executable,
                } => {
                    let Some(manifest) = manifests.get(file_id) else {
                        if !references {
                            return Err(NativeClientError::Missing("streamed FileManifest"));
                        }
                        builder.add_file_reference(
                            path,
                            file_id.clone(),
                            *byte_length,
                            *executable,
                        )?;
                        continue;
                    };
                    if manifest.byte_length() != *byte_length {
                        return Err(NativeClientError::InvalidState(
                            "streamed file length changed",
                        ));
                    }
                    let length = usize::try_from(*byte_length).map_err(|_| {
                        NativeClientError::InvalidState("streamed file length overflow")
                    })?;
                    let parts = builder.validate(&path, length)?;
                    insert(
                        &mut builder.entries,
                        &parts,
                        Node::Leaf(TreeEntryTarget::file(file_id.clone()), *executable),
                    )?;
                    retained.extend(manifest.chunks().iter().map(|c| c.chunk_id().clone()));
                    builder.object(CanonicalObject::FileManifest(manifest.clone()));
                    builder.count += 1;
                    builder.bytes += length;
                }
                CheckoutProjectionContent::Symlink { target } => {
                    builder.add_symlink(path, target)?
                }
            }
        }
        chunks.retain(&retained);
        let mut snapshot = builder.finish()?;
        snapshot.chunks = Some(std::sync::Arc::new(chunks));
        Ok(snapshot)
    }
    /// A capture that found exactly its base projection: no new object, and
    /// the root is that projection's Tree (what building it would give).
    pub(super) fn unchanged(root_tree_id: TreeId) -> Self {
        Self {
            root_tree_id,
            objects: Vec::new(),
            chunks: None,
        }
    }

    pub fn root_tree_id(&self) -> &TreeId {
        &self.root_tree_id
    }
    pub fn objects(&self) -> &[CanonicalObject] {
        &self.objects
    }
}

enum Node {
    Directory(BTreeMap<String, Node>),
    Leaf(TreeEntryTarget, bool),
}

/// Bounded, deterministic snapshot construction. Input is raw captured bytes;
/// adding an entry never reads or changes files and never follows a symlink.
pub struct SnapshotBuilder {
    entries: BTreeMap<String, Node>,
    objects: BTreeMap<String, CanonicalObject>,
    limits: SnapshotLimits,
    count: usize,
    bytes: usize,
}
impl SnapshotBuilder {
    pub fn new(limits: SnapshotLimits) -> Self {
        Self {
            entries: BTreeMap::new(),
            objects: BTreeMap::new(),
            limits,
            count: 0,
            bytes: 0,
        }
    }

    fn validate<'a>(&self, path: &'a RepoPath, bytes: usize) -> Result<Vec<&'a str>> {
        let components: Vec<_> = path.as_str().split('/').collect();
        if self.count >= self.limits.max_entries
            || components.len() > self.limits.max_depth.min(128)
            || bytes > self.limits.max_file_bytes
            || self
                .bytes
                .checked_add(bytes)
                .is_none_or(|n| n > self.limits.max_total_bytes)
        {
            return Err(NativeClientError::InvalidSnapshot(
                "snapshot resource limit exceeded".into(),
            ));
        }
        if components
            .iter()
            .any(|c| c.eq_ignore_ascii_case(".layrs") || c.eq_ignore_ascii_case(".git"))
        {
            return Err(NativeClientError::InvalidSnapshot(
                "reserved metadata path".into(),
            ));
        }
        Ok(components)
    }

    pub fn add_file(&mut self, path: RepoPath, bytes: &[u8], executable: bool) -> Result<()> {
        let parts = self.validate(&path, bytes.len())?;
        let chunks = bytes
            .chunks(CHUNK_BYTES)
            .map(|b| Chunk::new(b.to_vec()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let manifest = FileManifest::from_chunks(
            ChunkingDescriptor::deterministic_fixed(CHUNK_BYTES as u64)?,
            &chunks,
        )?;
        insert(
            &mut self.entries,
            &parts,
            Node::Leaf(
                TreeEntryTarget::file(manifest.file_id().clone()),
                executable,
            ),
        )?;
        for chunk in chunks {
            self.object(CanonicalObject::ChunkRaw(chunk));
        }
        self.object(CanonicalObject::FileManifest(manifest));
        self.count += 1;
        self.bytes += bytes.len();
        Ok(())
    }

    /// Reuse an already verified file without duplicating its immutable bytes.
    /// The caller must validate its admitted CAS closure before applying the Step.
    pub(super) fn add_file_reference(
        &mut self,
        path: RepoPath,
        file_id: layrs_core::scm::FileId,
        byte_length: u64,
        executable: bool,
    ) -> Result<()> {
        let length = usize::try_from(byte_length)
            .map_err(|_| NativeClientError::InvalidState("file length overflow"))?;
        let parts = self.validate(&path, length)?;
        insert(
            &mut self.entries,
            &parts,
            Node::Leaf(TreeEntryTarget::file(file_id), executable),
        )?;
        self.count += 1;
        self.bytes += length;
        Ok(())
    }

    pub fn add_symlink(&mut self, path: RepoPath, target: &str) -> Result<()> {
        let parts = self.validate(&path, target.len())?;
        let link = Symlink::new(target)?;
        insert(
            &mut self.entries,
            &parts,
            Node::Leaf(TreeEntryTarget::symlink(link.id().clone()), false),
        )?;
        self.object(CanonicalObject::Symlink(link));
        self.count += 1;
        self.bytes += target.len();
        Ok(())
    }

    fn object(&mut self, object: CanonicalObject) {
        self.objects
            .entry(object.object_id().as_str().to_owned())
            .or_insert(object);
    }

    pub fn finish(mut self) -> Result<PreparedSnapshot> {
        let root_tree_id = build_tree(self.entries, &mut self.objects)?;
        Ok(PreparedSnapshot {
            root_tree_id,
            objects: self.objects.into_values().collect(),
            chunks: None,
        })
    }
}

fn insert(entries: &mut BTreeMap<String, Node>, parts: &[&str], leaf: Node) -> Result<()> {
    let key = parts[0];
    if parts.len() == 1 {
        if entries.contains_key(key) {
            return Err(NativeClientError::InvalidSnapshot(
                "duplicate path or file/directory collision".into(),
            ));
        }
        entries.insert(key.to_owned(), leaf);
        return Ok(());
    }
    match entries
        .entry(key.to_owned())
        .or_insert_with(|| Node::Directory(BTreeMap::new()))
    {
        Node::Directory(children) => insert(children, &parts[1..], leaf),
        Node::Leaf(..) => Err(NativeClientError::InvalidSnapshot(
            "file/directory collision".into(),
        )),
    }
}

fn build_tree(
    entries: BTreeMap<String, Node>,
    objects: &mut BTreeMap<String, CanonicalObject>,
) -> Result<TreeId> {
    let mut children = Vec::with_capacity(entries.len());
    for (name, node) in entries {
        let (target, executable) = match node {
            Node::Directory(children) => {
                (TreeEntryTarget::tree(build_tree(children, objects)?), false)
            }
            Node::Leaf(target, executable) => (target, executable),
        };
        children.push(TreeEntry::new(
            RepoPathComponent::parse(name)?,
            target,
            executable,
        ));
    }
    let tree = Tree::new(children)?;
    let id = tree.id().clone();
    let object = CanonicalObject::Tree(tree);
    objects
        .entry(object.object_id().as_str().to_owned())
        .or_insert(object);
    Ok(id)
}
