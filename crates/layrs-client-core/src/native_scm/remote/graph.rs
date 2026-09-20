//! Bounded transfer graph; all cached data lives within this one verified closure.
use super::*;
use layrs_store_local::scm_v1::checkout::{
    projection_tree_id, CheckoutProjectionContent, CheckoutProjectionEntry,
};

#[derive(Default)]
pub(super) struct GraphObjects {
    pub(super) objects: BTreeMap<StoreObjectId, CanonicalObject>,
    pub(super) steps: BTreeMap<StepId, Step>,
    manifests: BTreeMap<FileId, FileManifestId>,
    chunks: chunk_spool::ChunkSpool,
    pub(super) metadata_bytes: usize,
}
impl GraphObjects {
    pub(super) fn has_conflicts(&self, step: &StepId) -> RemoteResult<bool> {
        let step = self
            .steps
            .get(step)
            .ok_or(NativeRemoteError::Invalid("missing Step"))?;
        let mut pending = vec![step.root_tree_id().clone()];
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(CanonicalObject::Tree(tree)) = self.objects.get(&StoreObjectId::Tree(id))
            else {
                return Err(NativeRemoteError::Invalid("missing Tree"));
            };
            for entry in tree.entries() {
                match entry.target() {
                    TreeEntryTarget::Conflict { .. } => return Ok(true),
                    TreeEntryTarget::Tree { tree_id } => pending.push(tree_id.clone()),
                    _ => (),
                }
            }
        }
        Ok(false)
    }
    pub(super) fn file_bytes(&self, id: &FileId) -> RemoteResult<Vec<u8>> {
        let manifest = self.manifest(id)?;
        if manifest.byte_length() > MAX_OBJECT {
            return Err(NativeRemoteError::Invalid("file exceeds limit"));
        }
        Ok(manifest
            .reconstruct(|id| self.chunks.get(id).ok().map(|chunk| chunk.bytes().to_vec()))?)
    }
    pub(super) fn manifest(&self, id: &FileId) -> RemoteResult<&FileManifest> {
        let manifest_id = self
            .manifests
            .get(id)
            .ok_or(NativeRemoteError::Invalid("missing File manifest"))?;
        let Some(CanonicalObject::FileManifest(manifest)) = self
            .objects
            .get(&StoreObjectId::FileManifest(manifest_id.clone()))
        else {
            return Err(NativeRemoteError::Invalid("missing File manifest"));
        };
        Ok(manifest)
    }
    pub(super) fn write_file(
        &self,
        id: &FileId,
        output: &mut (impl Write + ?Sized),
    ) -> RemoteResult<u64> {
        let manifest = self.manifest(id)?;
        if manifest.byte_length() > MAX_FILE_STREAM || manifest.chunks().len() > 16_384 {
            return Err(NativeRemoteError::Invalid("file exceeds limit"));
        }
        let mut verifier = manifest.verifier();
        for reference in manifest.chunks() {
            let chunk = self.chunks.get(reference.chunk_id())?;
            verifier.push(chunk.bytes())?;
            output.write_all(chunk.bytes())?;
        }
        verifier.finish()?;
        Ok(manifest.byte_length())
    }
    pub(super) fn verify_file(&self, manifest: &FileManifest) -> RemoteResult<()> {
        if manifest.byte_length() > MAX_FILE_STREAM || manifest.chunks().len() > 16_384 {
            return Err(NativeRemoteError::Invalid("file exceeds limit"));
        }
        let mut verifier = manifest.verifier();
        for reference in manifest.chunks() {
            let chunk = self.chunks.get(reference.chunk_id())?;
            verifier.push(chunk.bytes())?;
        }
        Ok(verifier.finish()?)
    }
    pub(super) fn verify_all_files(
        &self,
        mut check: impl FnMut() -> RemoteResult<()>,
    ) -> RemoteResult<()> {
        let mut work = 0u64;
        for object in self.objects.values() {
            check()?;
            if let CanonicalObject::FileManifest(manifest) = object {
                work = work
                    .checked_add(manifest.byte_length())
                    .filter(|n| *n <= MAX_STREAM_BYTES)
                    .ok_or(NativeRemoteError::Invalid(
                        "file verification budget exhausted",
                    ))?;
                self.verify_file(manifest)?;
            }
        }
        Ok(())
    }
    pub(super) fn entries(&self, step: &StepId) -> RemoteResult<Vec<CheckoutEntry>> {
        let projection = self.projection(step)?;
        let total = projection
            .iter()
            .map(|entry| match &entry.content {
                CheckoutProjectionContent::File { byte_length, .. } => *byte_length,
                CheckoutProjectionContent::Symlink { target } => target.len() as u64,
            })
            .sum::<u64>();
        if total > MAX_TOTAL as u64 {
            return Err(NativeRemoteError::Invalid(
                "in-memory checkout exceeds limits",
            ));
        }
        projection
            .into_iter()
            .map(|entry| {
                Ok(CheckoutEntry {
                    path: entry.path,
                    content: match entry.content {
                        CheckoutProjectionContent::File {
                            file_id,
                            executable,
                            ..
                        } => CheckoutContent::File {
                            bytes: self.file_bytes(&file_id)?,
                            executable,
                        },
                        CheckoutProjectionContent::Symlink { target } => {
                            CheckoutContent::Symlink { target }
                        }
                    },
                })
            })
            .collect()
    }
    pub(super) fn projection(&self, step: &StepId) -> RemoteResult<Vec<CheckoutProjectionEntry>> {
        let step = self
            .steps
            .get(step)
            .ok_or(NativeRemoteError::Invalid("missing target Step"))?;
        let mut entries = Vec::new();
        let mut total = 0u64;
        let mut pending = vec![(String::new(), step.root_tree_id().clone(), 0usize)];
        while let Some((prefix, id, depth)) = pending.pop() {
            if depth > 128 || entries.len() + pending.len() > MAX_OBJECTS {
                return Err(NativeRemoteError::Invalid("checkout exceeds limits"));
            }
            let Some(CanonicalObject::Tree(tree)) = self.objects.get(&StoreObjectId::Tree(id))
            else {
                return Err(NativeRemoteError::Invalid("missing Tree"));
            };
            for entry in tree.entries() {
                let path = if prefix.is_empty() {
                    entry.name().as_str().into()
                } else {
                    format!("{prefix}/{}", entry.name().as_str())
                };
                let content = match entry.target() {
                    TreeEntryTarget::Tree { tree_id } => {
                        pending.push((path, tree_id.clone(), depth + 1));
                        continue;
                    }
                    TreeEntryTarget::File { file_id } => CheckoutProjectionContent::File {
                        file_id: file_id.clone(),
                        byte_length: self.manifest(file_id)?.byte_length(),
                        executable: entry.executable(),
                    },
                    TreeEntryTarget::Symlink { symlink_id } => {
                        let Some(CanonicalObject::Symlink(link)) = self
                            .objects
                            .get(&StoreObjectId::Symlink(symlink_id.clone()))
                        else {
                            return Err(NativeRemoteError::Invalid("missing symlink"));
                        };
                        CheckoutProjectionContent::Symlink {
                            target: link.target().into(),
                        }
                    }
                    TreeEntryTarget::Tombstone { .. } => continue,
                    TreeEntryTarget::Conflict { .. } => {
                        return Err(NativeRemoteError::Invalid(
                            "unresolved conflict cannot be materialized",
                        ));
                    }
                };
                total = total
                    .checked_add(match &content {
                        CheckoutProjectionContent::File { byte_length, .. } => *byte_length,
                        CheckoutProjectionContent::Symlink { target } => target.len() as u64,
                    })
                    .ok_or(NativeRemoteError::Invalid("checkout exceeds limits"))?;
                if total > 4 * 1024 * 1024 * 1024 {
                    return Err(NativeRemoteError::Invalid("checkout exceeds limits"));
                }
                entries.push(CheckoutProjectionEntry { path, content });
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        projection_tree_id(&entries)?;
        Ok(entries)
    }
    pub(super) fn insert(&mut self, object: CanonicalObject) -> RemoteResult<()> {
        if self.contains(&object.object_id()) {
            return Ok(());
        }
        if self.objects.len() + self.chunks.len() >= MAX_OBJECTS {
            return Err(NativeRemoteError::Invalid(
                "native transfer closure exceeds bounded limits",
            ));
        }
        if let CanonicalObject::ChunkRaw(chunk) = object {
            if chunk.bytes().len() as u64 > MAX_OBJECT {
                return Err(NativeRemoteError::Invalid("chunk exceeds limit"));
            }
            self.chunks.insert(&chunk)?;
            return Ok(());
        }
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(encode(&object)?.1.len())
            .filter(|bytes| *bytes <= MAX_TOTAL)
            .ok_or(NativeRemoteError::Invalid(
                "closure metadata exceeds limits",
            ))?;
        if let CanonicalObject::Step(step) = &object {
            self.steps.insert(step.id().clone(), step.clone());
        }
        if let CanonicalObject::FileManifest(manifest) = &object {
            self.manifests
                .entry(manifest.file_id().clone())
                .and_modify(|id| {
                    if manifest.id() < id {
                        *id = manifest.id().clone();
                    }
                })
                .or_insert_with(|| manifest.id().clone());
        }
        self.objects.insert(object.object_id(), object);
        Ok(())
    }
    pub(super) fn has_manifest(&self, id: &FileId) -> bool {
        self.manifests.contains_key(id)
    }
    fn contains(&self, id: &StoreObjectId) -> bool {
        match id {
            StoreObjectId::ChunkRaw(id) => self.chunks.contains(id),
            _ => self.objects.contains_key(id),
        }
    }
    pub(super) fn chunk_objects(&self) -> impl Iterator<Item = RemoteResult<CanonicalObject>> + '_ {
        self.chunks.iter()
    }
    pub(super) fn chunk_descriptors(&self) -> Vec<(ChunkId, u64)> {
        self.chunks.descriptors()
    }
    pub(super) fn read_chunk(&self, id: &ChunkId) -> layrs_store::scm::Result<Vec<u8>> {
        self.chunks
            .get(id)
            .map(|chunk| chunk.bytes().to_vec())
            .map_err(|error| layrs_store::scm::StoreError::Corruption {
                message: error.to_string(),
            })
    }
    pub(super) fn merge(&mut self, other: Self) -> RemoteResult<()> {
        for chunk in other.chunk_objects() {
            self.insert(chunk?)?;
        }
        for object in other.objects.into_values() {
            self.insert(object)?;
        }
        Ok(())
    }
    pub(super) fn dag(&self) -> RemoteResult<StepDag> {
        StepDag::from_steps(self.steps.values()).map_err(graph_error)
    }
    pub(super) fn local(workspace: &NativeWorkspace, state: &WorkspaceState) -> RemoteResult<Self> {
        let store = scm_v1::open(workspace.store_root())?;
        let reader = NativeScmReader::new(&store)?;
        let mut result = Self::default();
        let mut verified_files = BTreeSet::new();
        let mut verification_bytes = 0u64;
        let mut pending: Vec<_> = state
            .view
            .layer_heads()
            .iter()
            .flat_map(|h| h.step_ids())
            .cloned()
            .map(StoreObjectId::Step)
            .collect();
        pending.extend(
            reader
                .products_in_space(state.space.id())?
                .into_iter()
                .map(StoreObjectId::Product),
        );
        for id in state.view.marker_ids() {
            let marker = reader
                .get_marker(id)?
                .ok_or(NativeRemoteError::Invalid("missing local Marker"))?;
            pending.push(StoreObjectId::Step(marker.target_step_id().clone()));
        }
        for id in state.view.release_ids() {
            let release = reader
                .get_release(id)?
                .ok_or(NativeRemoteError::Invalid("missing local Release"))?;
            pending.push(StoreObjectId::Step(release.target_step_id().clone()));
        }
        while let Some(id) = pending.pop() {
            if result.contains(&id) {
                continue;
            }
            let object = match id {
                StoreObjectId::Product(id) => {
                    let product = reader
                        .get_product(&id)?
                        .ok_or(NativeRemoteError::Invalid("missing local Product"))?;
                    pending.extend(product.direct_dependencies());
                    CanonicalObject::Product(product)
                }
                StoreObjectId::Step(id) => {
                    let step = reader
                        .get_step(&id)?
                        .ok_or(NativeRemoteError::Invalid("missing local Step"))?;
                    pending.extend(
                        step.parent_step_ids()
                            .iter()
                            .cloned()
                            .map(StoreObjectId::Step),
                    );
                    pending.push(StoreObjectId::Tree(step.root_tree_id().clone()));
                    CanonicalObject::Step(step)
                }
                StoreObjectId::Tree(id) => {
                    let tree = reader
                        .get_tree(&id)?
                        .ok_or(NativeRemoteError::Invalid("missing local Tree"))?;
                    for e in tree.entries() {
                        match e.target() {
                            TreeEntryTarget::Tree { tree_id } => {
                                pending.push(StoreObjectId::Tree(tree_id.clone()))
                            }
                            TreeEntryTarget::File { file_id } => {
                                if !verified_files.insert(file_id.clone()) {
                                    continue;
                                }
                                pending.push(StoreObjectId::FileManifest(Self::local_manifest(
                                    &reader,
                                    file_id,
                                    &mut verification_bytes,
                                )?));
                            }
                            TreeEntryTarget::Symlink { symlink_id } => {
                                pending.push(StoreObjectId::Symlink(symlink_id.clone()))
                            }
                            TreeEntryTarget::Tombstone { tombstone_id } => {
                                pending.push(StoreObjectId::Tombstone(tombstone_id.clone()))
                            }
                            TreeEntryTarget::Conflict { conflict_id } => {
                                pending.push(StoreObjectId::Conflict(conflict_id.clone()))
                            }
                        }
                    }
                    CanonicalObject::Tree(tree)
                }
                StoreObjectId::FileManifest(id) => {
                    let m = reader
                        .get_file_manifest(&id)?
                        .ok_or(NativeRemoteError::Invalid("missing local manifest"))?;
                    pending.extend(
                        m.chunks()
                            .iter()
                            .map(|c| StoreObjectId::ChunkRaw(c.chunk_id().clone())),
                    );
                    CanonicalObject::FileManifest(m)
                }
                StoreObjectId::ChunkRaw(id) => CanonicalObject::ChunkRaw(
                    reader
                        .get_chunk(&id)?
                        .ok_or(NativeRemoteError::Invalid("missing local chunk"))?,
                ),
                StoreObjectId::Symlink(id) => CanonicalObject::Symlink(
                    reader
                        .get_symlink(&id)?
                        .ok_or(NativeRemoteError::Invalid("missing local symlink"))?,
                ),
                StoreObjectId::Conflict(id) => {
                    let conflict = reader
                        .get_conflict(&id)?
                        .ok_or(NativeRemoteError::Invalid("missing local Conflict"))?;
                    let (origins, targets) = conflict_references(&conflict)?;
                    pending.extend(origins.into_iter().map(StoreObjectId::Step));
                    for target in targets {
                        match target {
                            TreeEntryTarget::Tree { tree_id } => {
                                pending.push(StoreObjectId::Tree(tree_id))
                            }
                            TreeEntryTarget::Symlink { symlink_id } => {
                                pending.push(StoreObjectId::Symlink(symlink_id))
                            }
                            TreeEntryTarget::File { file_id } => {
                                if !verified_files.insert(file_id.clone()) {
                                    continue;
                                }
                                pending.push(StoreObjectId::FileManifest(Self::local_manifest(
                                    &reader,
                                    &file_id,
                                    &mut verification_bytes,
                                )?));
                            }
                            _ => {
                                return Err(NativeRemoteError::Invalid(
                                    "unsupported Conflict term",
                                ));
                            }
                        }
                    }
                    CanonicalObject::Conflict(conflict)
                }
                StoreObjectId::Tombstone(id) => CanonicalObject::Tombstone(
                    reader
                        .get_tombstone(&id)?
                        .ok_or(NativeRemoteError::Invalid("missing local tombstone"))?,
                ),
                _ => return Err(NativeRemoteError::Invalid("unsupported local object")),
            };
            result.insert(object)?;
        }
        result.dag()?;
        Ok(result)
    }
    fn local_manifest(
        reader: &NativeScmReader<'_>,
        id: &FileId,
        work: &mut u64,
    ) -> RemoteResult<FileManifestId> {
        let remaining = MAX_STREAM_BYTES
            .checked_sub(*work)
            .ok_or(NativeRemoteError::Invalid(
                "file verification budget exhausted",
            ))?;
        let FileAvailability::Present(file) = reader.file_availability_with_budget(
            id,
            32_768,
            usize::try_from(remaining).unwrap_or(usize::MAX),
            128,
            16_384,
        )?
        else {
            return Err(NativeRemoteError::Invalid("missing local File"));
        };
        for manifest_id in file.verified_manifest_ids() {
            let manifest = reader
                .get_file_manifest(manifest_id)?
                .ok_or(NativeRemoteError::Invalid("missing local File manifest"))?;
            if manifest.byte_length() > MAX_FILE_STREAM {
                return Err(NativeRemoteError::Invalid("file exceeds limit"));
            }
            let metadata = serde_jcs::to_vec(&manifest)?.len() as u64;
            *work = work
                .checked_add(manifest.byte_length())
                .and_then(|n| n.checked_add(metadata))
                .filter(|n| *n <= MAX_STREAM_BYTES)
                .ok_or(NativeRemoteError::Invalid(
                    "file verification budget exhausted",
                ))?;
        }
        Ok(file.verified_manifest_ids()[0].clone())
    }
}
