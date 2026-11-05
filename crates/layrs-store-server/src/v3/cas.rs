use super::{V3StoreError, V3StoreResult, conflict::verify_conflict_object};
use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;
use layrs_core::scm::{FileManifest, RepositoryView, Step, Symlink, Tombstone, Tree};
use layrs_sync::v3::{AuthorizationSnapshot, PolicySet, ProductEnvelope, ValidateV3};
use layrs_sync::v3::{Compression, ObjectDescriptor, ObjectKind};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

pub type CasByteStream = Pin<Box<dyn Stream<Item = V3StoreResult<Bytes>> + Send + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ByteRange {
    pub start: u64,
    pub end_exclusive: u64,
}

impl ByteRange {
    pub fn new(start: u64, end_exclusive: u64) -> V3StoreResult<Self> {
        if start > end_exclusive {
            return Err(V3StoreError::InvalidRange);
        }
        Ok(Self {
            start,
            end_exclusive,
        })
    }
    pub fn full(length: u64) -> V3StoreResult<Self> {
        Self::new(0, length)
    }
    pub const fn len(self) -> u64 {
        self.end_exclusive - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end_exclusive
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CasLimits {
    pub max_encoded_bytes: u64,
    pub max_canonical_bytes: u64,
}

impl Default for CasLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 64 * 1024 * 1024,
            max_canonical_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    Installed,
    AlreadyPresent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CasHead {
    pub descriptor: ObjectDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrubOutcome {
    Healthy,
    Corrupt(String),
}

#[async_trait]
pub trait ServerCas: Send + Sync {
    async fn put_if_absent(
        &self,
        descriptor: &ObjectDescriptor,
        body: CasByteStream,
    ) -> V3StoreResult<PutOutcome>;
    async fn head(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<Option<CasHead>>;
    async fn verified_stream(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<CasByteStream>;
    async fn verified_range(
        &self,
        descriptor: &ObjectDescriptor,
        range: ByteRange,
    ) -> V3StoreResult<CasByteStream>;
    async fn scrub(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<ScrubOutcome>;
}

/// Extension point for versioned Product and Evidence encoders. The base V3
/// contract fails closed until each encoder can recompute its typed ObjectId.
pub trait TypedObjectVerifier: Send + Sync {
    fn verify(&self, descriptor: &ObjectDescriptor, canonical: &mut dyn Read) -> V3StoreResult<()>;
}

#[derive(Debug, Default)]
pub struct FailClosedTypedObjectVerifier;

impl TypedObjectVerifier for FailClosedTypedObjectVerifier {
    fn verify(
        &self,
        _descriptor: &ObjectDescriptor,
        _canonical: &mut dyn Read,
    ) -> V3StoreResult<()> {
        Err(V3StoreError::InvalidDescriptor(
            "the V3 contract does not yet define this typed identity encoder".into(),
        ))
    }
}

pub(crate) fn validate_descriptor(
    descriptor: &ObjectDescriptor,
    limits: CasLimits,
) -> V3StoreResult<()> {
    const MAX_EVIDENCE_BYTES: u64 = 64 * 1024 * 1024;
    if descriptor.kind == ObjectKind::Evidence && descriptor.canonical_byte_len > MAX_EVIDENCE_BYTES
    {
        return Err(V3StoreError::LimitExceeded {
            resource: "evidence object",
            limit: MAX_EVIDENCE_BYTES,
        });
    }
    if descriptor.encoded_byte_len > limits.max_encoded_bytes {
        return Err(V3StoreError::LimitExceeded {
            resource: "encoded object",
            limit: limits.max_encoded_bytes,
        });
    }
    if descriptor.canonical_byte_len > limits.max_canonical_bytes {
        return Err(V3StoreError::LimitExceeded {
            resource: "canonical object",
            limit: limits.max_canonical_bytes,
        });
    }
    if descriptor.compression == Compression::Identity
        && descriptor.encoded_byte_len != descriptor.canonical_byte_len
    {
        return Err(V3StoreError::InvalidDescriptor(
            "identity encoding lengths differ".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn spool_encoded(
    body: CasByteStream,
    path: &Path,
    expected: u64,
    max: u64,
) -> V3StoreResult<()> {
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await?;
    let mut body = body;
    let mut written = 0_u64;
    use futures_util::StreamExt;
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        written = written
            .checked_add(chunk.len() as u64)
            .ok_or(V3StoreError::LimitExceeded {
                resource: "encoded object",
                limit: max,
            })?;
        if written > max || written > expected {
            return Err(V3StoreError::LimitExceeded {
                resource: "encoded object",
                limit: max.min(expected),
            });
        }
        output.write_all(&chunk).await?;
    }
    if written != expected {
        return Err(V3StoreError::LengthMismatch {
            field: "encoded",
            expected,
            actual: written,
        });
    }
    output.flush().await?;
    output.sync_all().await?;
    Ok(())
}

pub(crate) async fn verify_file(
    path: PathBuf,
    descriptor: ObjectDescriptor,
    limits: CasLimits,
    typed_verifier: Arc<dyn TypedObjectVerifier>,
) -> V3StoreResult<()> {
    validate_descriptor(&descriptor, limits)?;
    tokio::task::spawn_blocking(move || {
        verify_file_blocking(&path, &descriptor, limits, typed_verifier.as_ref())
    })
    .await
    .map_err(|error| V3StoreError::Backend(error.to_string()))?
}

fn verify_file_blocking(
    path: &Path,
    descriptor: &ObjectDescriptor,
    limits: CasLimits,
    typed_verifier: &dyn TypedObjectVerifier,
) -> V3StoreResult<()> {
    use std::io::Read;
    let file_len = std::fs::metadata(path)?.len();
    if file_len != descriptor.encoded_byte_len {
        return Err(V3StoreError::LengthMismatch {
            field: "encoded",
            expected: descriptor.encoded_byte_len,
            actual: file_len,
        });
    }
    let file = std::fs::File::open(path)?;
    let mut input: Box<dyn Read> = match descriptor.compression {
        Compression::Identity => Box::new(file),
        Compression::Zstd => Box::new(
            zstd::stream::read::Decoder::new(file)
                .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?,
        ),
    };
    let mut buffer = [0_u8; 64 * 1024];
    let mut canonical = Vec::with_capacity(
        usize::try_from(descriptor.canonical_byte_len.min(1024 * 1024)).unwrap_or(0),
    );
    let mut decoded = 0_u64;
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|e| V3StoreError::CorruptObject(e.to_string()))?;
        if count == 0 {
            break;
        }
        decoded = decoded
            .checked_add(count as u64)
            .ok_or(V3StoreError::LimitExceeded {
                resource: "canonical object",
                limit: limits.max_canonical_bytes,
            })?;
        if decoded > limits.max_canonical_bytes || decoded > descriptor.canonical_byte_len {
            return Err(V3StoreError::LimitExceeded {
                resource: "canonical object",
                limit: limits
                    .max_canonical_bytes
                    .min(descriptor.canonical_byte_len),
            });
        }
        canonical.extend_from_slice(&buffer[..count]);
    }
    if decoded != descriptor.canonical_byte_len {
        return Err(V3StoreError::LengthMismatch {
            field: "canonical",
            expected: descriptor.canonical_byte_len,
            actual: decoded,
        });
    }
    verify_canonical_record(descriptor, &canonical, typed_verifier)
}

fn verify_canonical_record(
    descriptor: &ObjectDescriptor,
    canonical: &[u8],
    _typed_verifier: &dyn TypedObjectVerifier,
) -> V3StoreResult<()> {
    match descriptor.kind {
        ObjectKind::Chunk => verify_domain_bytes(b"layrs-chunk-v1", canonical, descriptor),
        ObjectKind::Step => {
            verify_json_record::<Step>(canonical, descriptor, |value| value.id().as_str())
        }
        ObjectKind::RepositoryView => verify_repository_view(canonical, descriptor),
        ObjectKind::FileManifest => {
            verify_json_record::<FileManifest>(canonical, descriptor, |value| value.id().as_str())
        }
        ObjectKind::Tree => {
            verify_json_record::<Tree>(canonical, descriptor, |value| value.id().as_str())
        }
        ObjectKind::Symlink => {
            verify_json_record::<Symlink>(canonical, descriptor, |value| value.id().as_str())
        }
        ObjectKind::Tombstone => {
            verify_json_record::<Tombstone>(canonical, descriptor, |value| value.id().as_str())
        }
        ObjectKind::Conflict => verify_conflict_object(canonical, descriptor),
        ObjectKind::PolicySet => {
            let value: PolicySet = decode_exact_jcs(canonical)?;
            value
                .validate_v3()
                .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
            if value.policy_set_id != descriptor.object_id {
                return Err(V3StoreError::IdentityMismatch);
            }
            Ok(())
        }
        ObjectKind::AuthorizationSnapshot => {
            let value: AuthorizationSnapshot = decode_exact_jcs(canonical)?;
            value
                .validate_v3()
                .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
            if value.authorization_snapshot_id != descriptor.object_id {
                return Err(V3StoreError::IdentityMismatch);
            }
            Ok(())
        }
        ObjectKind::Evidence => verify_domain_bytes(b"layrs-evidence-v1", canonical, descriptor),
        ObjectKind::Product => verify_product(canonical, descriptor),
    }
}

fn verify_domain_bytes(
    domain: &[u8],
    bytes: &[u8],
    descriptor: &ObjectDescriptor,
) -> V3StoreResult<()> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(&[0]);
    hasher.update(bytes);
    (format!("blake3:{}", hasher.finalize().to_hex()) == descriptor.object_id.as_str())
        .then_some(())
        .ok_or(V3StoreError::IdentityMismatch)
}

fn verify_json_record<T>(
    canonical: &[u8],
    descriptor: &ObjectDescriptor,
    id: impl FnOnce(&T) -> &str,
) -> V3StoreResult<()>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let value: T = decode_exact_jcs(canonical)?;
    if id(&value) != descriptor.object_id.as_str() {
        return Err(V3StoreError::IdentityMismatch);
    }
    Ok(())
}

fn verify_repository_view(canonical: &[u8], descriptor: &ObjectDescriptor) -> V3StoreResult<()> {
    let value: RepositoryView = decode_exact_jcs(canonical)?;
    if !value.worktrees().is_empty() {
        return Err(V3StoreError::CorruptObject(
            "server RepositoryView contains local-only Worktree state".into(),
        ));
    }
    if value.id().as_str() != descriptor.object_id.as_str() {
        return Err(V3StoreError::IdentityMismatch);
    }
    Ok(())
}

fn decode_exact_jcs<T>(canonical: &[u8]) -> V3StoreResult<T>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let value: T = serde_json::from_slice(canonical)
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    let encoded = serde_jcs::to_vec(&value)
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    if encoded != canonical {
        return Err(V3StoreError::CorruptObject(
            "record is not exact JCS".into(),
        ));
    }
    Ok(value)
}

fn verify_product(canonical: &[u8], descriptor: &ObjectDescriptor) -> V3StoreResult<()> {
    let value: ProductEnvelope = serde_json::from_slice(canonical)
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    value
        .verify_canonical_wire(canonical)
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    value
        .validate_v3()
        .map_err(|error| V3StoreError::CorruptObject(error.to_string()))?;
    if value.object_id() != &descriptor.object_id {
        return Err(V3StoreError::IdentityMismatch);
    }
    Ok(())
}

pub(crate) async fn file_stream(path: PathBuf, range: ByteRange) -> V3StoreResult<CasByteStream> {
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(range.start)).await?;
    let state = (file, range.len());
    Ok(Box::pin(futures_util::stream::try_unfold(
        state,
        |(mut file, remaining)| async move {
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
            Ok(Some((Bytes::from(bytes), (file, remaining - read as u64))))
        },
    )))
}
