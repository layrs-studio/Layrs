use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use layrs_core::scm::{ObjectId, RepositoryView, SpaceId, StoreId};
use layrs_store_server::v3::*;
use layrs_sync::v3::{Compression, ObjectDescriptor, ObjectKind};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Mutex;

static TEMP: AtomicU64 = AtomicU64::new(1);
#[path = "support/cas_upload_possession.rs"]
mod cas_upload_possession;

#[tokio::test]
async fn existing_s3_object_requires_full_verified_upload_body() {
    let staging = TestDir::new();
    let client = Arc::new(MockS3::default());
    let cas = S3ServerCas::open(client, "tenant", &staging.0, CasLimits::default())
        .await
        .unwrap();
    let bytes = b"possession-object";
    cas_upload_possession::check(&cas, &descriptor(bytes), bytes).await;
}
struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "layrs-s3-cas-{}-{}",
            std::process::id(),
            TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct MockState {
    objects: BTreeMap<String, Vec<u8>>,
    uploads: BTreeMap<String, (String, BTreeMap<u32, Vec<u8>>)>,
}
#[derive(Default)]
struct MockS3 {
    state: Mutex<MockState>,
    sequence: AtomicU64,
}

#[async_trait]
impl S3CompatibleClient for MockS3 {
    async fn head_object(&self, key: &str) -> V3StoreResult<Option<S3ObjectMetadata>> {
        Ok(self
            .state
            .lock()
            .await
            .objects
            .get(key)
            .map(|bytes| S3ObjectMetadata {
                encoded_byte_len: bytes.len() as u64,
                backend_etag: Some(format!("mock-etag-{}", blake3::hash(bytes).to_hex())),
            }))
    }
    async fn put_empty_if_absent(&self, key: &str) -> V3StoreResult<PutOutcome> {
        let mut state = self.state.lock().await;
        if state.objects.contains_key(key) {
            Ok(PutOutcome::AlreadyPresent)
        } else {
            state.objects.insert(key.to_owned(), Vec::new());
            Ok(PutOutcome::Installed)
        }
    }
    async fn begin_multipart(&self, key: &str) -> V3StoreResult<MultipartUpload> {
        let id = format!("upload-{}", self.sequence.fetch_add(1, Ordering::Relaxed));
        self.state
            .lock()
            .await
            .uploads
            .insert(id.clone(), (key.into(), BTreeMap::new()));
        Ok(MultipartUpload {
            key: key.into(),
            upload_id: id,
        })
    }
    async fn upload_part(
        &self,
        upload: &MultipartUpload,
        part_number: u32,
        bytes: Bytes,
    ) -> V3StoreResult<CompletedPart> {
        self.state
            .lock()
            .await
            .uploads
            .get_mut(&upload.upload_id)
            .unwrap()
            .1
            .insert(part_number, bytes.to_vec());
        Ok(CompletedPart {
            part_number,
            etag: format!("etag-{part_number}"),
        })
    }
    async fn complete_multipart_if_absent(
        &self,
        upload: MultipartUpload,
        parts: Vec<CompletedPart>,
    ) -> V3StoreResult<PutOutcome> {
        if parts.is_empty() {
            return Err(V3StoreError::Backend(
                "mock S3 rejects zero-part multipart completion".into(),
            ));
        }
        let mut state = self.state.lock().await;
        let (key, uploaded) = state.uploads.remove(&upload.upload_id).unwrap();
        if state.objects.contains_key(&key) {
            return Ok(PutOutcome::AlreadyPresent);
        }
        let mut bytes = Vec::new();
        for part in parts {
            bytes.extend_from_slice(uploaded.get(&part.part_number).unwrap());
        }
        state.objects.insert(key, bytes);
        Ok(PutOutcome::Installed)
    }
    async fn abort_multipart(&self, upload: MultipartUpload) -> V3StoreResult<()> {
        self.state.lock().await.uploads.remove(&upload.upload_id);
        Ok(())
    }
    async fn get_range(&self, key: &str, range: ByteRange) -> V3StoreResult<CasByteStream> {
        let state = self.state.lock().await;
        let bytes = state.objects.get(key).ok_or(V3StoreError::NotFound)?;
        let bytes =
            Bytes::copy_from_slice(&bytes[range.start as usize..range.end_exclusive as usize]);
        Ok(Box::pin(stream::once(async move { Ok(bytes) })))
    }
}

fn descriptor(bytes: &[u8]) -> ObjectDescriptor {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"layrs-chunk-v1");
    hasher.update(&[0]);
    hasher.update(bytes);
    ObjectDescriptor {
        kind: ObjectKind::Chunk,
        object_id: ObjectId::parse(format!("blake3:{}", hasher.finalize().to_hex())).unwrap(),
        canonical_byte_len: bytes.len() as u64,
        encoded_byte_len: bytes.len() as u64,
        compression: Compression::Identity,
    }
}
fn body(bytes: &[u8]) -> CasByteStream {
    Box::pin(stream::iter(
        bytes
            .chunks(2)
            .map(|v| Ok(Bytes::copy_from_slice(v)))
            .collect::<Vec<_>>(),
    ))
}
async fn collect(mut stream: CasByteStream) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(v) = stream.next().await {
        out.extend_from_slice(&v.unwrap());
    }
    out
}

#[tokio::test]
async fn s3_adapter_streams_multipart_and_conditionally_installs() {
    let staging = TestDir::new();
    let client = Arc::new(MockS3::default());
    let cas = S3ServerCas::open(client, "tenant", &staging.0, CasLimits::default())
        .await
        .unwrap()
        .with_part_bytes(3)
        .unwrap();
    let bytes = b"multipart-object";
    let descriptor = descriptor(bytes);
    assert_eq!(
        cas.put_if_absent(&descriptor, body(bytes)).await.unwrap(),
        PutOutcome::Installed
    );
    assert_eq!(
        cas.put_if_absent(&descriptor, body(bytes)).await.unwrap(),
        PutOutcome::AlreadyPresent
    );
    assert_eq!(
        collect(
            cas.verified_range(&descriptor, ByteRange::new(5, 9).unwrap())
                .await
                .unwrap()
        )
        .await,
        b"part"
    );
    assert_eq!(cas.scrub(&descriptor).await.unwrap(), ScrubOutcome::Healthy);
}

#[tokio::test]
async fn s3_empty_evidence_uses_conditional_single_put_and_is_concurrent_safe() {
    let staging = TestDir::new();
    let client = Arc::new(MockS3::default());
    let cas = S3ServerCas::open(client, "tenant", &staging.0, CasLimits::default())
        .await
        .unwrap();
    let descriptor = ObjectDescriptor {
        kind: ObjectKind::Evidence,
        object_id: ObjectId::parse(
            "blake3:5c2342d0bb714f79a97ad9b13151ed909d20a1741df627afa2916eb4b4ed8ae6",
        )
        .unwrap(),
        canonical_byte_len: 0,
        encoded_byte_len: 0,
        compression: Compression::Identity,
    };
    let (left, right) = tokio::join!(
        cas.put_if_absent(&descriptor, body(b"")),
        cas.put_if_absent(&descriptor, body(b""))
    );
    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == PutOutcome::Installed)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == PutOutcome::AlreadyPresent)
            .count(),
        1
    );
    assert_eq!(
        collect(cas.verified_stream(&descriptor).await.unwrap()).await,
        b""
    );
}

#[tokio::test]
async fn cataloged_s3_uses_backend_key_and_etag_for_generated_and_empty_objects() {
    let staging = TestDir::new();
    let client = Arc::new(MockS3::default());
    let inner = S3ServerCas::open(client.clone(), "tenant", &staging.0, CasLimits::default())
        .await
        .unwrap()
        .with_part_bytes(3)
        .unwrap();
    let cas = CatalogedS3Cas::new(StoreId::parse("store:s3-main").unwrap(), inner);

    let view = RepositoryView::new(
        SpaceId::parse("space:s3-cataloged").unwrap(),
        vec![],
        vec![],
        vec![],
        vec![],
    )
    .unwrap();
    let canonical = Bytes::from(serde_jcs::to_vec(&view).unwrap());
    let (left, right) = tokio::join!(
        cas.prepare_generated(ObjectKind::RepositoryView, canonical.clone()),
        cas.prepare_generated(ObjectKind::RepositoryView, canonical)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left, right);
    assert_eq!(left.store_id, "store:s3-main");
    assert!(left.storage_key.starts_with("tenant/v3/repository_view/"));
    assert!(
        left.backend_etag
            .as_deref()
            .unwrap()
            .starts_with("mock-etag-")
    );
    assert_eq!(cas.verify_existing(&left.descriptor).await.unwrap(), left);

    let empty = ObjectDescriptor {
        kind: ObjectKind::Evidence,
        object_id: ObjectId::parse(
            "blake3:5c2342d0bb714f79a97ad9b13151ed909d20a1741df627afa2916eb4b4ed8ae6",
        )
        .unwrap(),
        canonical_byte_len: 0,
        encoded_byte_len: 0,
        compression: Compression::Identity,
    };
    assert_eq!(
        cas.put_if_absent(&empty, body(b"")).await.unwrap(),
        PutOutcome::Installed
    );
    let empty_binding = cas.verify_existing(&empty).await.unwrap();
    assert_eq!(
        empty_binding.storage_key,
        format!("tenant/v3/evidence/{}", &empty.object_id.as_str()[7..])
    );
    assert!(empty_binding.backend_etag.is_some());

    client.state.lock().await.objects.insert(
        left.storage_key.clone(),
        vec![b'x'; left.descriptor.encoded_byte_len as usize],
    );
    assert!(matches!(
        cas.verify_existing(&left.descriptor).await,
        Err(V3StoreError::CorruptObject(_)) | Err(V3StoreError::IdentityMismatch)
    ));
}
