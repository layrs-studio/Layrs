use super::{
    BackendObjectBinding, ByteRange, CasByteStream, CasHead, CasLimits,
    FailClosedTypedObjectVerifier, PutOutcome, ScrubOutcome, ServerCas, TypedObjectVerifier,
    V3StoreError, V3StoreResult,
};
use crate::v3::cas::{spool_encoded, validate_descriptor, verify_file};
use async_trait::async_trait;
use bytes::Bytes;
use layrs_sync::v3::{ObjectDescriptor, ObjectKind};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

static S3_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Minimal streaming boundary implemented by an AWS S3-compatible adapter.
/// `complete_multipart_if_absent` MUST use a conditional create (or an
/// equivalent strongly consistent lock) and must never replace an object.
#[async_trait]
pub trait S3CompatibleClient: Send + Sync {
    async fn head_object(&self, key: &str) -> V3StoreResult<Option<S3ObjectMetadata>>;
    /// Conditional single PUT for an empty object. Implementations must never
    /// replace an existing key.
    async fn put_empty_if_absent(&self, key: &str) -> V3StoreResult<PutOutcome>;
    async fn begin_multipart(&self, key: &str) -> V3StoreResult<MultipartUpload>;
    async fn upload_part(
        &self,
        upload: &MultipartUpload,
        part_number: u32,
        bytes: Bytes,
    ) -> V3StoreResult<CompletedPart>;
    async fn complete_multipart_if_absent(
        &self,
        upload: MultipartUpload,
        parts: Vec<CompletedPart>,
    ) -> V3StoreResult<PutOutcome>;
    async fn abort_multipart(&self, upload: MultipartUpload) -> V3StoreResult<()>;
    async fn get_range(&self, key: &str, range: ByteRange) -> V3StoreResult<CasByteStream>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3ObjectMetadata {
    pub encoded_byte_len: u64,
    pub backend_etag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartUpload {
    pub key: String,
    pub upload_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedPart {
    pub part_number: u32,
    pub etag: String,
}

#[derive(Clone)]
pub struct S3ServerCas {
    client: Arc<dyn S3CompatibleClient>,
    prefix: String,
    staging: PathBuf,
    limits: CasLimits,
    part_bytes: usize,
    typed_verifier: Arc<dyn TypedObjectVerifier>,
}

impl S3ServerCas {
    pub async fn open(
        client: Arc<dyn S3CompatibleClient>,
        prefix: impl Into<String>,
        staging: impl Into<PathBuf>,
        limits: CasLimits,
    ) -> V3StoreResult<Self> {
        Self::open_with_typed_verifier(
            client,
            prefix,
            staging,
            limits,
            Arc::new(FailClosedTypedObjectVerifier),
        )
        .await
    }

    pub async fn open_with_typed_verifier(
        client: Arc<dyn S3CompatibleClient>,
        prefix: impl Into<String>,
        staging: impl Into<PathBuf>,
        limits: CasLimits,
        typed_verifier: Arc<dyn TypedObjectVerifier>,
    ) -> V3StoreResult<Self> {
        let staging = staging.into();
        tokio::fs::create_dir_all(&staging).await?;
        Ok(Self {
            client,
            prefix: prefix.into().trim_matches('/').to_owned(),
            staging,
            limits,
            part_bytes: 8 * 1024 * 1024,
            typed_verifier,
        })
    }

    pub fn with_part_bytes(mut self, part_bytes: usize) -> V3StoreResult<Self> {
        if part_bytes == 0 {
            return Err(V3StoreError::InvalidDescriptor(
                "multipart size is zero".into(),
            ));
        }
        self.part_bytes = part_bytes;
        Ok(self)
    }

    fn key(&self, descriptor: &ObjectDescriptor) -> String {
        let kind = match descriptor.kind {
            ObjectKind::Step => "step",
            ObjectKind::RepositoryView => "repository_view",
            ObjectKind::Chunk => "chunk",
            ObjectKind::FileManifest => "file_manifest",
            ObjectKind::Tree => "tree",
            ObjectKind::Conflict => "conflict",
            ObjectKind::Symlink => "symlink",
            ObjectKind::Tombstone => "tombstone",
            ObjectKind::Evidence => "evidence",
            ObjectKind::Product => "product",
            ObjectKind::PolicySet => "policy_set",
            ObjectKind::AuthorizationSnapshot => "authorization_snapshot",
        };
        format!(
            "{}/v3/{kind}/{}",
            self.prefix,
            &descriptor.object_id.as_str()[7..]
        )
    }

    fn temp_path(&self) -> PathBuf {
        self.staging.join(format!(
            "s3.{}.{}.upload",
            std::process::id(),
            S3_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    async fn download_verified(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<PathBuf> {
        validate_descriptor(descriptor, self.limits)?;
        let Some(metadata) = self.client.head_object(&self.key(descriptor)).await? else {
            return Err(V3StoreError::NotFound);
        };
        let len = metadata.encoded_byte_len;
        if len != descriptor.encoded_byte_len {
            return Err(V3StoreError::LengthMismatch {
                field: "encoded",
                expected: descriptor.encoded_byte_len,
                actual: len,
            });
        }
        let temp = self.temp_path();
        if len == 0 {
            let file = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)
                .await?;
            file.sync_all().await?;
        } else {
            let range = ByteRange::full(len)?;
            let stream = self.client.get_range(&self.key(descriptor), range).await?;
            if let Err(error) =
                spool_encoded(stream, &temp, len, self.limits.max_encoded_bytes).await
            {
                let _ = tokio::fs::remove_file(&temp).await;
                return Err(error);
            }
        }
        if let Err(error) = verify_file(
            temp.clone(),
            descriptor.clone(),
            self.limits,
            self.typed_verifier.clone(),
        )
        .await
        {
            let _ = tokio::fs::remove_file(&temp).await;
            return Err(error);
        }
        Ok(temp)
    }

    pub(crate) async fn backend_binding(
        &self,
        descriptor: &ObjectDescriptor,
    ) -> V3StoreResult<BackendObjectBinding> {
        let storage_key = self.key(descriptor);
        let Some(metadata) = self.client.head_object(&storage_key).await? else {
            return Err(V3StoreError::NotFound);
        };
        if metadata.encoded_byte_len != descriptor.encoded_byte_len {
            return Err(V3StoreError::LengthMismatch {
                field: "encoded",
                expected: descriptor.encoded_byte_len,
                actual: metadata.encoded_byte_len,
            });
        }
        Ok(BackendObjectBinding {
            storage_key,
            backend_etag: metadata.backend_etag,
        })
    }
}

#[async_trait]
impl ServerCas for S3ServerCas {
    async fn put_if_absent(
        &self,
        descriptor: &ObjectDescriptor,
        body: CasByteStream,
    ) -> V3StoreResult<PutOutcome> {
        validate_descriptor(descriptor, self.limits)?;
        // Verify the caller's complete body before a deduplicated upload can
        // grant Space availability. A guessed digest is not possession.
        let temp = self.temp_path();
        let cleanup = S3TempCleanup(temp.clone());
        spool_encoded(
            body,
            &temp,
            descriptor.encoded_byte_len,
            self.limits.max_encoded_bytes,
        )
        .await?;
        verify_file(
            temp.clone(),
            descriptor.clone(),
            self.limits,
            self.typed_verifier.clone(),
        )
        .await?;

        if let Some(metadata) = self.client.head_object(&self.key(descriptor)).await? {
            if metadata.encoded_byte_len != descriptor.encoded_byte_len {
                return Err(V3StoreError::CorruptObject("remote length differs".into()));
            }
            let existing = self.download_verified(descriptor).await?;
            let _ = tokio::fs::remove_file(existing).await;
            drop(cleanup);
            return Ok(PutOutcome::AlreadyPresent);
        }

        if descriptor.encoded_byte_len == 0 {
            let outcome = self
                .client
                .put_empty_if_absent(&self.key(descriptor))
                .await?;
            drop(cleanup);
            if outcome == PutOutcome::AlreadyPresent {
                let winner = self.download_verified(descriptor).await?;
                let _ = tokio::fs::remove_file(winner).await;
            }
            return Ok(outcome);
        }

        let upload = self.client.begin_multipart(&self.key(descriptor)).await?;
        let result = async {
            let mut source = tokio::fs::File::open(&temp).await?;
            let mut parts = Vec::new();
            let mut part_number = 1_u32;
            loop {
                let mut bytes = vec![0_u8; self.part_bytes];
                let read = source.read(&mut bytes).await?;
                if read == 0 {
                    break;
                }
                bytes.truncate(read);
                parts.push(
                    self.client
                        .upload_part(&upload, part_number, Bytes::from(bytes))
                        .await?,
                );
                part_number = part_number
                    .checked_add(1)
                    .ok_or(V3StoreError::LimitExceeded {
                        resource: "multipart count",
                        limit: u32::MAX as u64,
                    })?;
            }
            self.client
                .complete_multipart_if_absent(upload.clone(), parts)
                .await
        }
        .await;
        if result.is_err() {
            let _ = self.client.abort_multipart(upload).await;
        }
        drop(cleanup);
        let outcome = result?;
        if outcome == PutOutcome::AlreadyPresent {
            let winner = self.download_verified(descriptor).await?;
            let _ = tokio::fs::remove_file(winner).await;
        }
        Ok(outcome)
    }

    async fn head(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<Option<CasHead>> {
        let Some(metadata) = self.client.head_object(&self.key(descriptor)).await? else {
            return Ok(None);
        };
        let len = metadata.encoded_byte_len;
        if len != descriptor.encoded_byte_len {
            return Err(V3StoreError::CorruptObject("remote length differs".into()));
        }
        let temp = self.download_verified(descriptor).await?;
        let _ = tokio::fs::remove_file(temp).await;
        Ok(Some(CasHead {
            descriptor: descriptor.clone(),
        }))
    }

    async fn verified_stream(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<CasByteStream> {
        let temp = self.download_verified(descriptor).await?;
        temporary_stream(temp, 0, descriptor.encoded_byte_len).await
    }

    async fn verified_range(
        &self,
        descriptor: &ObjectDescriptor,
        range: ByteRange,
    ) -> V3StoreResult<CasByteStream> {
        if range.end_exclusive > descriptor.encoded_byte_len {
            return Err(V3StoreError::InvalidRange);
        }
        let temp = self.download_verified(descriptor).await?;
        temporary_stream(temp, range.start, range.len()).await
    }

    async fn scrub(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<ScrubOutcome> {
        match self.download_verified(descriptor).await {
            Ok(temp) => {
                let _ = tokio::fs::remove_file(temp).await;
                Ok(ScrubOutcome::Healthy)
            }
            Err(V3StoreError::NotFound) => Err(V3StoreError::NotFound),
            Err(error) => Ok(ScrubOutcome::Corrupt(error.to_string())),
        }
    }
}

struct S3TempCleanup(PathBuf);
impl Drop for S3TempCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn temporary_stream(path: PathBuf, start: u64, length: u64) -> V3StoreResult<CasByteStream> {
    if length == 0 {
        let _ = tokio::fs::remove_file(path).await;
        return Ok(Box::pin(futures_util::stream::empty()));
    }
    let mut file = tokio::fs::File::open(&path).await?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let state = (file, length, S3TempCleanup(path));
    Ok(Box::pin(futures_util::stream::try_unfold(
        state,
        |(mut file, remaining, cleanup)| async move {
            if remaining == 0 {
                return Ok(None);
            }
            let size = usize::try_from(remaining.min(64 * 1024)).unwrap_or(64 * 1024);
            let mut bytes = vec![0_u8; size];
            let read = file.read(&mut bytes).await?;
            if read == 0 {
                return Err(V3StoreError::CorruptObject("unexpected EOF".into()));
            }
            bytes.truncate(read);
            Ok(Some((
                Bytes::from(bytes),
                (file, remaining - read as u64, cleanup),
            )))
        },
    )))
}
