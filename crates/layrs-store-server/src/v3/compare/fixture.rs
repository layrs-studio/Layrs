use super::*;
use async_trait::async_trait;
use layrs_core::scm::{Chunk, ChunkingDescriptor, RepoPathComponent, TreeEntry, TreeId};
use layrs_sync::v3::{Compression, ObjectDescriptor};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

#[derive(Clone, Default)]
pub(super) struct Fixture {
    pub objects: BTreeMap<ObjectId, (ObjectDescriptor, Vec<u8>)>,
    pub manifests: BTreeMap<FileId, String>,
    pub admitted: BTreeSet<StepId>,
    pub denied: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Counts {
    pub reads: BTreeMap<ObjectKind, usize>,
    pub descriptors: Vec<ObjectId>,
    pub authorizations: Vec<AuthorizationTarget>,
}

impl Counts {
    pub fn reads(&self, kind: ObjectKind) -> usize {
        self.reads.get(&kind).copied().unwrap_or_default()
    }
}

pub(super) struct TestSource {
    pub data: Mutex<Fixture>,
    pub counts: Mutex<Counts>,
}

pub(super) fn space() -> SpaceId {
    SpaceId::parse("space:compare-fixture").unwrap()
}

pub(super) fn entry(name: &str, target: TreeEntryTarget, mode: bool) -> TreeEntry {
    TreeEntry::new(RepoPathComponent::parse(name).unwrap(), target, mode)
}

impl Fixture {
    pub fn source(&self) -> TestSource {
        TestSource {
            data: Mutex::new(self.clone()),
            counts: Mutex::new(Counts::default()),
        }
    }

    pub fn object(&mut self, id: &str, kind: ObjectKind, bytes: Vec<u8>) {
        let id = ObjectId::parse(id).unwrap();
        let descriptor = ObjectDescriptor {
            kind,
            object_id: id.clone(),
            canonical_byte_len: bytes.len() as u64,
            encoded_byte_len: bytes.len() as u64,
            compression: Compression::Identity,
        };
        self.objects.insert(id, (descriptor, bytes));
    }

    pub fn file(&mut self, bytes: &[u8]) -> FileId {
        let chunks = if bytes.is_empty() {
            vec![]
        } else {
            vec![Chunk::new(bytes).unwrap()]
        };
        for chunk in &chunks {
            self.object(
                chunk.id().as_str(),
                ObjectKind::Chunk,
                chunk.bytes().to_vec(),
            );
        }
        let manifest = FileManifest::from_chunks(ChunkingDescriptor::single(), &chunks).unwrap();
        self.object(
            manifest.id().as_str(),
            ObjectKind::FileManifest,
            serde_json::to_vec(&manifest).unwrap(),
        );
        self.manifests
            .insert(manifest.file_id().clone(), manifest.id().to_string());
        manifest.file_id().clone()
    }

    pub fn tree(&mut self, entries: Vec<TreeEntry>) -> TreeId {
        let tree = Tree::new(entries).unwrap();
        self.object(
            tree.id().as_str(),
            ObjectKind::Tree,
            serde_json::to_vec(&tree).unwrap(),
        );
        tree.id().clone()
    }

    pub fn step(&mut self, root: TreeId) -> StepId {
        let step = Step::new(vec![], root).unwrap();
        self.object(
            step.id().as_str(),
            ObjectKind::Step,
            serde_json::to_vec(&step).unwrap(),
        );
        self.admitted.insert(step.id().clone());
        step.id().clone()
    }

    pub fn link(&mut self, target: &str) -> TreeEntryTarget {
        let link = Symlink::new(target).unwrap();
        self.object(
            link.id().as_str(),
            ObjectKind::Symlink,
            serde_json::to_vec(&link).unwrap(),
        );
        TreeEntryTarget::symlink(link.id().clone())
    }
}

#[async_trait]
impl Source for TestSource {
    async fn allowed(&self, target: AuthorizationTarget) -> V3StoreResult<()> {
        self.counts
            .lock()
            .unwrap()
            .authorizations
            .push(target.clone());
        if let AuthorizationTarget::Path { repo_path, .. } = target {
            if self
                .data
                .lock()
                .unwrap()
                .denied
                .contains(repo_path.as_str())
            {
                return Err(V3StoreError::InvalidCapability);
            }
        }
        Ok(())
    }

    async fn admitted(&self, id: &StepId) -> V3StoreResult<()> {
        if self.data.lock().unwrap().admitted.contains(id) {
            Ok(())
        } else {
            Err(V3StoreError::NotFound)
        }
    }

    async fn manifest(&self, id: &FileId) -> V3StoreResult<String> {
        self.data
            .lock()
            .unwrap()
            .manifests
            .get(id)
            .cloned()
            .ok_or(V3StoreError::NotFound)
    }

    async fn descriptor(&self, id: &ObjectId) -> V3StoreResult<ObjectDescriptor> {
        self.counts.lock().unwrap().descriptors.push(id.clone());
        self.data
            .lock()
            .unwrap()
            .objects
            .get(id)
            .map(|v| v.0.clone())
            .ok_or(V3StoreError::NotFound)
    }

    async fn read(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<Vec<u8>> {
        *self
            .counts
            .lock()
            .unwrap()
            .reads
            .entry(descriptor.kind)
            .or_default() += 1;
        if descriptor.canonical_byte_len > MAX_FILE || descriptor.encoded_byte_len > MAX_FILE {
            return Err(limit("service object", MAX_FILE));
        }
        let data = self.data.lock().unwrap();
        let (current, bytes) = data
            .objects
            .get(&descriptor.object_id)
            .ok_or(V3StoreError::NotFound)?;
        if current != descriptor || bytes.len() as u64 != descriptor.canonical_byte_len {
            return Err(V3StoreError::CorruptObject(
                "canonical length mismatch".into(),
            ));
        }
        Ok(bytes.clone())
    }
}
