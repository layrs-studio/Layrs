//! Eager traversal from 4601b169, adapted to the test I/O boundary and shared entry storage.
use super::*;

pub(super) struct LegacyReader<'a, S> {
    pub source: &'a S,
    pub space_id: &'a SpaceId,
    pub bytes: u64,
    pub objects: usize,
}
impl<S: Source> LegacyReader<'_, S> {
    async fn allowed(&self, target: AuthorizationTarget) -> V3StoreResult<()> {
        self.source.allowed(target).await
    }
    async fn read(&mut self, id: &str, kind: ObjectKind) -> V3StoreResult<Vec<u8>> {
        self.objects += 1;
        if self.objects > 100_000 {
            return Err(limit("comparison objects", 100_000));
        }
        let id = ObjectId::parse(id).map_err(|_| V3StoreError::IdentityMismatch)?;
        let descriptor = self.source.descriptor(&id).await?;
        if descriptor.kind != kind {
            return Err(V3StoreError::IdentityMismatch);
        }
        self.bytes = self
            .bytes
            .checked_add(descriptor.canonical_byte_len)
            .ok_or_else(|| limit("comparison bytes", MAX_BYTES))?;
        if self.bytes > MAX_BYTES {
            return Err(limit("comparison bytes", MAX_BYTES));
        }
        self.source.read(&descriptor).await
    }
    pub async fn step(&mut self, id: &StepId) -> V3StoreResult<Vec<Entry>> {
        self.allowed(AuthorizationTarget::Object {
            space_id: self.space_id.clone(),
            object_id: ObjectId::parse(id.as_str()).expect("typed Step ID"),
        })
        .await?;
        self.source.admitted(id).await?;
        let step: Step = parse(&self.read(id.as_str(), ObjectKind::Step).await?)?;
        if step.id() != id {
            return Err(V3StoreError::IdentityMismatch);
        }
        let mut stack = vec![(step.root_tree_id().clone(), String::new(), 0usize)];
        let mut result = Vec::new();
        let mut count = 0usize;
        while let Some((tree, prefix, depth)) = stack.pop() {
            if depth > 128 {
                return Err(limit("comparison depth", 128));
            }
            let tree: Tree = parse(&self.read(tree.as_str(), ObjectKind::Tree).await?)?;
            for entry in tree.entries() {
                count += 1;
                if count > MAX_ENTRIES {
                    return Err(limit("comparison entries", MAX_ENTRIES as u64));
                }
                let path = if prefix.is_empty() {
                    entry.name().as_str().to_owned()
                } else {
                    format!("{prefix}/{}", entry.name().as_str())
                };
                let repo_path = RepoPath::parse(&path)
                    .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
                self.allowed(AuthorizationTarget::Path {
                    space_id: self.space_id.clone(),
                    repo_path,
                })
                .await?;
                let content = match entry.target() {
                    TreeEntryTarget::Tree { tree_id } => {
                        stack.push((tree_id.clone(), path, depth + 1));
                        continue;
                    }
                    TreeEntryTarget::File { file_id } => {
                        Content::File(self.file(file_id).await?.into(), entry.executable())
                    }
                    TreeEntryTarget::Symlink { symlink_id } => {
                        let link: Symlink =
                            parse(&self.read(symlink_id.as_str(), ObjectKind::Symlink).await?)?;
                        Content::Symlink(link.target().to_owned())
                    }
                    TreeEntryTarget::Tombstone { tombstone_id } => {
                        self.read(tombstone_id.as_str(), ObjectKind::Tombstone)
                            .await?;
                        continue;
                    }
                    TreeEntryTarget::Conflict { .. } => {
                        return Err(V3StoreError::InvalidDescriptor(
                            "comparison of unresolved conflicts is not supported".into(),
                        ));
                    }
                };
                result.push(Entry { path, content });
            }
        }
        Ok(result)
    }
    pub(super) async fn file(&mut self, id: &FileId) -> V3StoreResult<Vec<u8>> {
        let selected = self.source.manifest(id).await?;
        let manifest: FileManifest = parse(&self.read(&selected, ObjectKind::FileManifest).await?)?;
        if manifest.file_id() != id {
            return Err(V3StoreError::IdentityMismatch);
        }
        if manifest.byte_length() > MAX_FILE {
            return Err(limit("comparison file", MAX_FILE));
        }
        let mut bytes = Vec::new();
        for chunk in manifest.chunks() {
            let value = self
                .read(chunk.chunk_id().as_str(), ObjectKind::Chunk)
                .await?;
            if value.len() as u64 != chunk.byte_length()
                || bytes.len() as u64 + value.len() as u64 > manifest.byte_length()
            {
                return Err(V3StoreError::IdentityMismatch);
            }
            bytes.extend_from_slice(&value);
        }
        if bytes.len() as u64 != manifest.byte_length() || FileId::from_bytes(&bytes) != *id {
            return Err(V3StoreError::IdentityMismatch);
        }
        Ok(bytes)
    }
}
