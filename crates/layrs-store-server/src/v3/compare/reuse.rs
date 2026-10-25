//! Reuse only identities verified during this comparison, never catalog claims alone.
//!
//! Equal trees avoid duplicate CAS reads/decoding; their entries still undergo
//! traversal, path authorization, conflict checks and logical budget accounting.
//! The first occurrence of every file is fully reconstructed and identity checked.
//! This is neither persisted admission evidence nor an integrity scrub of CAS.
use super::*;

pub(super) struct VerifiedFile {
    manifest: ObjectDescriptor,
    chunks: Vec<ObjectDescriptor>,
    bytes: Arc<[u8]>,
}

impl<S: Source> Reader<'_, S> {
    pub(super) async fn tree(&mut self, id: &TreeId) -> V3StoreResult<Arc<Tree>> {
        let descriptor = self.descriptor(id.as_str(), ObjectKind::Tree).await?;
        if let Some((previous, tree)) = self.trees.get(id) {
            if previous == &descriptor {
                return Ok(Arc::clone(tree));
            }
        }
        let tree = Arc::new(parse::<Tree>(&self.source.read(&descriptor).await?)?);
        self.trees
            .insert(id.clone(), (descriptor, Arc::clone(&tree)));
        Ok(tree)
    }

    pub(super) async fn file(&mut self, id: &FileId) -> V3StoreResult<Arc<[u8]>> {
        // Selection is live: another verified representation may have become preferred.
        let selected = self.source.manifest(id).await?;
        let descriptor = self.descriptor(&selected, ObjectKind::FileManifest).await?;
        let previous = self
            .files
            .get(id)
            .filter(|file| file.manifest == descriptor)
            .cloned();
        if let Some(file) = previous {
            let mut offset = 0;
            for chunk in &file.chunks {
                let current = self
                    .descriptor(chunk.object_id.as_str(), ObjectKind::Chunk)
                    .await?;
                let end = offset + chunk.canonical_byte_len as usize;
                if &current != chunk {
                    // Changed encoding/descriptor cannot borrow previous verification.
                    // Read it through the normal verified CAS path and check the exact
                    // slice of the already reconstructed FileId, including its length.
                    let value = self.source.read(&current).await?;
                    if value.as_slice() != &file.bytes[offset..end] {
                        return Err(V3StoreError::IdentityMismatch);
                    }
                }
                offset = end;
            }
            return Ok(Arc::clone(&file.bytes));
        }

        let manifest: FileManifest = parse(&self.source.read(&descriptor).await?)?;
        if manifest.file_id() != id {
            return Err(V3StoreError::IdentityMismatch);
        }
        if manifest.byte_length() > MAX_FILE {
            return Err(limit("comparison file", MAX_FILE));
        }
        let mut bytes = Vec::new();
        let mut chunks = Vec::new();
        for chunk in manifest.chunks() {
            let descriptor = self
                .descriptor(chunk.chunk_id().as_str(), ObjectKind::Chunk)
                .await?;
            let value = self.source.read(&descriptor).await?;
            if value.len() as u64 != chunk.byte_length()
                || bytes.len() as u64 + value.len() as u64 > manifest.byte_length()
            {
                return Err(V3StoreError::IdentityMismatch);
            }
            bytes.extend_from_slice(&value);
            chunks.push(descriptor);
        }
        if bytes.len() as u64 != manifest.byte_length() || FileId::from_bytes(&bytes) != *id {
            return Err(V3StoreError::IdentityMismatch);
        }
        let bytes: Arc<[u8]> = bytes.into();
        self.files.insert(
            id.clone(),
            Arc::new(VerifiedFile {
                manifest: descriptor,
                chunks,
                bytes: Arc::clone(&bytes),
            }),
        );
        Ok(bytes)
    }
}
