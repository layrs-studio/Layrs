use super::{
    BackendObjectBinding, ByteRange, CasByteStream, CasHead, CasLimits,
    FailClosedTypedObjectVerifier, PutOutcome, ScrubOutcome, ServerCas, TypedObjectVerifier,
    V3StoreError, V3StoreResult,
};
use crate::v3::cas::{file_stream, spool_encoded, validate_descriptor, verify_file};
use async_trait::async_trait;
use futures_util::StreamExt;
use layrs_sync::v3::{ObjectDescriptor, ObjectKind};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
mod maintenance;
pub use maintenance::FsCasMaintenance;

#[derive(Clone)]
pub struct FsServerCas {
    root: PathBuf,
    limits: CasLimits,
    typed_verifier: std::sync::Arc<dyn TypedObjectVerifier>,
    // Every clone keeps the shared process-lifetime maintenance fence alive.
    _maintenance: std::sync::Arc<std::fs::File>,
}

impl FsServerCas {
    pub async fn open(root: impl Into<PathBuf>, limits: CasLimits) -> V3StoreResult<Self> {
        Self::open_with_typed_verifier(
            root,
            limits,
            std::sync::Arc::new(FailClosedTypedObjectVerifier),
        )
        .await
    }

    pub async fn open_with_typed_verifier(
        root: impl Into<PathBuf>,
        limits: CasLimits,
        typed_verifier: std::sync::Arc<dyn TypedObjectVerifier>,
    ) -> V3StoreResult<Self> {
        let root = root.into();
        tokio::fs::create_dir_all(&root).await?;
        let maintenance = maintenance::shared(&root)?;
        match std::fs::symlink_metadata(root.join(".layrs-gc-active")) {
            Ok(_) => {
                return Err(V3StoreError::Backend(
                    "unfinished offline GC: resume maintenance before serving".into(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        tokio::fs::create_dir_all(root.join("objects")).await?;
        tokio::fs::create_dir_all(root.join("staging")).await?;
        sync_directory(root.join("objects")).await?;
        sync_directory(root.join("staging")).await?;
        Ok(Self {
            root,
            limits,
            typed_verifier,
            _maintenance: std::sync::Arc::new(maintenance),
        })
    }

    pub fn limits(&self) -> CasLimits {
        self.limits
    }

    fn retain_fence(&self, stream: CasByteStream) -> CasByteStream {
        let guard = self._maintenance.clone();
        Box::pin(stream.map(move |item| {
            let _keep_alive = &guard;
            item
        }))
    }

    fn object_path(&self, descriptor: &ObjectDescriptor) -> PathBuf {
        self.root.join(self.storage_key(descriptor))
    }

    fn storage_key(&self, descriptor: &ObjectDescriptor) -> PathBuf {
        let hex = &descriptor.object_id.as_str()[7..];
        PathBuf::from("objects")
            .join(kind_name(descriptor.kind))
            .join(&hex[..2])
            .join(hex)
    }

    pub(crate) async fn backend_binding(
        &self,
        descriptor: &ObjectDescriptor,
    ) -> V3StoreResult<BackendObjectBinding> {
        let path = self.object_path(descriptor);
        if !tokio::fs::try_exists(&path).await? {
            return Err(V3StoreError::NotFound);
        }
        let storage_key = self
            .storage_key(descriptor)
            .to_str()
            .ok_or_else(|| V3StoreError::Backend("filesystem CAS key is not UTF-8".into()))?
            .replace('\\', "/");
        Ok(BackendObjectBinding {
            storage_key,
            backend_etag: None,
        })
    }

    fn temp_path(&self) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.root
            .join("staging")
            .join(format!("{}.{}.upload", std::process::id(), sequence))
    }
}

#[async_trait]
impl ServerCas for FsServerCas {
    async fn put_if_absent(
        &self,
        descriptor: &ObjectDescriptor,
        body: CasByteStream,
    ) -> V3StoreResult<PutOutcome> {
        validate_descriptor(descriptor, self.limits)?;
        let destination = self.object_path(descriptor);
        // A global CAS hit does not prove possession for a new Space. Consume
        // and verify the submitted bytes even when the immutable object exists.
        let temp = self.temp_path();
        let cleanup = TempCleanup(temp.clone());
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
        let parent = destination
            .parent()
            .ok_or_else(|| V3StoreError::Backend("object path has no parent".into()))?;
        tokio::fs::create_dir_all(parent).await?;

        match tokio::fs::hard_link(&temp, &destination).await {
            Ok(()) => {
                sync_directory(parent.to_path_buf()).await?;
                drop(cleanup);
                Ok(PutOutcome::Installed)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                verify_file(
                    destination,
                    descriptor.clone(),
                    self.limits,
                    self.typed_verifier.clone(),
                )
                .await?;
                drop(cleanup);
                Ok(PutOutcome::AlreadyPresent)
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn head(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<Option<CasHead>> {
        validate_descriptor(descriptor, self.limits)?;
        let path = self.object_path(descriptor);
        if !tokio::fs::try_exists(&path).await? {
            return Ok(None);
        }
        verify_file(
            path,
            descriptor.clone(),
            self.limits,
            self.typed_verifier.clone(),
        )
        .await?;
        Ok(Some(CasHead {
            descriptor: descriptor.clone(),
        }))
    }

    async fn verified_stream(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<CasByteStream> {
        let path = self.object_path(descriptor);
        if !tokio::fs::try_exists(&path).await? {
            return Err(V3StoreError::NotFound);
        }
        verify_file(
            path.clone(),
            descriptor.clone(),
            self.limits,
            self.typed_verifier.clone(),
        )
        .await?;
        let range = if descriptor.encoded_byte_len == 0 {
            return Ok(Box::pin(futures_util::stream::empty()));
        } else {
            ByteRange::full(descriptor.encoded_byte_len)?
        };
        Ok(self.retain_fence(file_stream(path, range).await?))
    }

    async fn verified_range(
        &self,
        descriptor: &ObjectDescriptor,
        range: ByteRange,
    ) -> V3StoreResult<CasByteStream> {
        if range.end_exclusive > descriptor.encoded_byte_len {
            return Err(V3StoreError::InvalidRange);
        }
        let path = self.object_path(descriptor);
        if !tokio::fs::try_exists(&path).await? {
            return Err(V3StoreError::NotFound);
        }
        verify_file(
            path.clone(),
            descriptor.clone(),
            self.limits,
            self.typed_verifier.clone(),
        )
        .await?;
        Ok(self.retain_fence(file_stream(path, range).await?))
    }

    async fn scrub(&self, descriptor: &ObjectDescriptor) -> V3StoreResult<ScrubOutcome> {
        let path = self.object_path(descriptor);
        if !tokio::fs::try_exists(&path).await? {
            return Err(V3StoreError::NotFound);
        }
        match verify_file(
            path,
            descriptor.clone(),
            self.limits,
            self.typed_verifier.clone(),
        )
        .await
        {
            Ok(()) => Ok(ScrubOutcome::Healthy),
            Err(error) => Ok(ScrubOutcome::Corrupt(error.to_string())),
        }
    }
}

struct TempCleanup(PathBuf);
impl Drop for TempCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn sync_directory(path: PathBuf) -> V3StoreResult<()> {
    tokio::task::spawn_blocking(move || {
        let directory = open_directory_for_sync(&path)?;
        directory.sync_all()
    })
    .await
    .map_err(|error| V3StoreError::Backend(error.to_string()))??;
    Ok(())
}

#[cfg(windows)]
fn open_directory_for_sync(path: &Path) -> std::io::Result<std::fs::File> {
    // `FlushFileBuffers` rejects directory handles on supported Rust/Windows
    // targets. A durable fence file in the same directory gives the volume a
    // flush point after the no-clobber link is installed.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.join(".layrs-dir-fsync"))
}

#[cfg(not(windows))]
fn open_directory_for_sync(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn kind_name(kind: ObjectKind) -> &'static str {
    match kind {
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
    }
}
