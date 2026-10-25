use super::{Checkpoint, InspectionPurpose, Session, api::check, fs};
use crate::{CaptureEvidence, Error, Inspection, Result};
use layrs_core::scm::{FileId, RepoPath};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    mem::{offset_of, size_of_val},
    sync::Arc,
};
use windows_sys::Win32::Storage::{CloudFilters::*, FileSystem::*};

/// A retained recovery-export view, never a capture capability. Relocated
/// placeholders always require reading and hashing their actual native bytes.
/// Keep writers quiesced and retain this value until the export is verified.
/// There is deliberately no conversion or Deref to Checkpoint.
pub struct ExportCheckpoint<'a> {
    checkpoint: Checkpoint<'a>,
    proofs: BTreeMap<RepoPath, FileProof>,
}

#[derive(Debug, PartialEq, Eq)]
struct FileProof {
    volume: u32,
    index: u64,
    size: u64,
    identity: Option<Vec<u8>>,
    backing: Option<FileId>,
}

impl Session {
    /// Metadata only. Unlike capture, permits identity-preserving relocated
    /// placeholders for explicit full-byte recovery reads, without adopting a
    /// new catalog or altering registration, history, or placeholder identities.
    pub fn export_checkpoint(&self) -> Result<ExportCheckpoint<'_>> {
        let checkpoint = self.checkpoint_for(InspectionPurpose::Export)?;
        checkpoint.validate_export_namespace()?;
        let mut proofs = BTreeMap::new();
        for entry in checkpoint.entries() {
            let Some(file) = &entry._guard else { continue };
            let proof = FileProof::read(self, file).map_err(|error| contextual(entry, error))?;
            let size = match &entry.evidence {
                CaptureEvidence::Immutable { size, .. }
                | CaptureEvidence::NeedsHash { size, .. } => *size,
                CaptureEvidence::Deleted { .. } => return Err(Error::IdentityMismatch),
            };
            if proof.size != size || (proof.identity.is_some() && proof.backing != backing(entry)) {
                return Err(contextual(entry, Error::IdentityMismatch));
            }
            proofs.insert(entry.path.clone(), proof);
        }
        let export = ExportCheckpoint { checkpoint, proofs };
        export.revalidate_namespace()?;
        Ok(export)
    }
}

impl ExportCheckpoint<'_> {
    pub fn entries(&self) -> &[Inspection] {
        self.checkpoint.entries()
    }

    /// Recheck exact namespace/native identity and the original backing blob.
    /// Residency and in-sync metadata may change during an explicit export read;
    /// neither is substituted for byte verification.
    pub fn revalidate_namespace(&self) -> Result<()> {
        self.checkpoint.revalidate_namespace()?;
        for entry in self.entries() {
            if let Some(file) = &entry._guard {
                let expected = self
                    .proofs
                    .get(&entry.path)
                    .ok_or(Error::IdentityMismatch)?;
                expected
                    .verify(
                        &FileProof::read(self.checkpoint.session(), file)
                            .map_err(|error| contextual(entry, error))?,
                    )
                    .map_err(|error| contextual(entry, error))?;
            }
        }
        Ok(())
    }

    /// Explicitly hydrates missing ranges, then supplies actual retained-file
    /// bytes only. Success requires consuming the expected size and real EOF.
    /// A native failure retains its exact HRESULT; no source range is guessed.
    /// Consumer errors (including destination writes or incomplete consumption)
    /// are ExportConsumer; actual reader/proof/hydration errors are ExportRead.
    pub fn with_reader<T>(
        &mut self,
        path: &RepoPath,
        consume: impl FnOnce(&mut dyn Read) -> Result<T>,
    ) -> Result<T> {
        let entry = self
            .entries()
            .iter()
            .find(|entry| entry.path == *path)
            .ok_or_else(|| Error::ExportRead {
                path: path.clone(),
                backing: None,
                cause: Box::new(Error::IdentityMismatch),
            })?;
        self.read_entry(entry, consume)
            .map_err(|error| match error {
                Error::ExportConsumer { .. } => error,
                other => contextual(entry, other),
            })
    }

    fn read_entry<T>(
        &self,
        entry: &Inspection,
        consume: impl FnOnce(&mut dyn Read) -> Result<T>,
    ) -> Result<T> {
        let CaptureEvidence::NeedsHash { size, .. } = &entry.evidence else {
            return Err(Error::Unsupported(
                "only NeedsHash export entries may read content",
            ));
        };
        let file = entry._guard.as_ref().ok_or(Error::IdentityMismatch)?;
        let expected = self
            .proofs
            .get(&entry.path)
            .ok_or(Error::IdentityMismatch)?;
        let session = self.checkpoint.session();
        expected.verify(&FileProof::read(session, file)?)?;
        if expected.identity.is_some() && *size != 0 {
            #[cfg(test)]
            super::stage2_tests::hydration_call();
            check("CfHydratePlaceholder(export)", unsafe {
                (session.api.hydrate)(
                    fs::handle(file),
                    0,
                    -1,
                    CF_HYDRATE_FLAG_NONE,
                    std::ptr::null_mut(),
                )
            })?;
        }
        expected.verify(&FileProof::read(session, file)?)?;
        let mut reader = file.try_clone()?;
        reader.seek(SeekFrom::Start(0))?;
        let result = match read_complete(reader, *size, consume) {
            Ok(result) => result,
            Err(ReadFailure::Reader(error)) => return Err(error),
            Err(ReadFailure::Consumer(error)) => {
                return Err(Error::ExportConsumer {
                    path: entry.path.clone(),
                    cause: Box::new(error),
                });
            }
        };
        expected.verify(&FileProof::read(session, file)?)?;
        Ok(result)
    }
}

fn backing(entry: &Inspection) -> Option<FileId> {
    match &entry.evidence {
        CaptureEvidence::Immutable { file_id, .. } => Some(file_id.clone()),
        CaptureEvidence::NeedsHash { base, .. } => base.clone(),
        CaptureEvidence::Deleted { base } => Some(base.clone()),
    }
}

fn contextual(entry: &Inspection, cause: Error) -> Error {
    Error::ExportRead {
        path: entry.path.clone(),
        backing: backing(entry),
        cause: Box::new(cause),
    }
}

impl FileProof {
    fn verify(&self, current: &Self) -> Result<()> {
        if self.volume != current.volume
            || self.index != current.index
            || self.identity != current.identity
            || self.backing != current.backing
        {
            return Err(Error::IdentityMismatch);
        }
        if self.size != current.size {
            return Err(Error::Busy);
        }
        Ok(())
    }

    fn read(session: &Session, file: &File) -> Result<Self> {
        let attributes = fs::attributes(file)?;
        let native = fs::identity(file)?;
        if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 || native.nNumberOfLinks != 1 {
            return Err(Error::Unsupported(
                "export requires a regular single-link file",
            ));
        }
        if native.dwVolumeSerialNumber != session.registration.volume {
            return Err(Error::ForeignRoot);
        }
        let mut proof = Self {
            volume: native.dwVolumeSerialNumber,
            index: fs::index(&native),
            size: fs::size(&native),
            identity: None,
            backing: None,
        };
        let state =
            unsafe { (session.api.state)(attributes.FileAttributes, attributes.ReparseTag) };
        if state == CF_PLACEHOLDER_STATE_INVALID {
            return Err(Error::Unsupported("invalid export placeholder state"));
        }
        if state & CF_PLACEHOLDER_STATE_PLACEHOLDER == 0 {
            if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(Error::Unsupported("non-CFAPI export reparse point"));
            }
            return Ok(proof);
        }
        let mut storage = [0u64; 1024];
        let mut returned = 0;
        check("CfGetPlaceholderInfo(export)", unsafe {
            (session.api.placeholder_info)(
                fs::handle(file),
                CF_PLACEHOLDER_INFO_STANDARD,
                storage.as_mut_ptr().cast(),
                size_of_val(&storage) as u32,
                &mut returned,
            )
        })?;
        let offset = offset_of!(CF_PLACEHOLDER_STANDARD_INFO, FileIdentity);
        if returned as usize > size_of_val(&storage) || (returned as usize) < offset {
            return Err(Error::IdentityMismatch);
        }
        let info = unsafe { &*storage.as_ptr().cast::<CF_PLACEHOLDER_STANDARD_INFO>() };
        let length = info.FileIdentityLength as usize;
        if length > 4096
            || offset + length > returned as usize
            || info.OnDiskDataSize < 0
            || info.ModifiedDataSize < 0
        {
            return Err(Error::IdentityMismatch);
        }
        if info.SyncRootFileId as u64 != session.registration.file_index
            || info.FileId as u64 != proof.index
        {
            return Err(Error::ForeignRoot);
        }
        let identity = unsafe {
            std::slice::from_raw_parts(storage.as_ptr().cast::<u8>().add(offset), length)
        };
        let original = session.catalog.resolve_identity(identity)?;
        if state & CF_PLACEHOLDER_STATE_PARTIAL != 0 && proof.size != original.size {
            return Err(Error::Unsupported(
                "partial placeholder size differs from immutable backing",
            ));
        }
        proof.identity = Some(identity.to_vec());
        proof.backing = Some(original.file_id.clone());
        Ok(proof)
    }
}

#[derive(Debug)]
enum ReadFailure {
    Reader(Error),
    Consumer(Error),
}

fn read_complete<R: Read, T>(
    reader: R,
    size: u64,
    consume: impl FnOnce(&mut dyn Read) -> Result<T>,
) -> std::result::Result<T, ReadFailure> {
    let mut bounded = ExactReader {
        inner: reader,
        remaining: size,
        failure: None,
    };
    let consumed = consume(&mut bounded);
    // Identify a failure at the actual reader boundary, even when io::copy
    // propagates it through the consumer. Never classify from message text.
    if let Some(error) = bounded.failure.take() {
        return Err(ReadFailure::Reader(Error::Io(shared_read_error(error))));
    }
    let result = consumed.map_err(ReadFailure::Consumer)?;
    if bounded.remaining != 0 {
        return Err(ReadFailure::Consumer(Error::InvalidRange));
    }
    // Take(size) would synthesize EOF even if an unexpected tail existed.
    let mut tail = [0u8; 1];
    if bounded
        .inner
        .read(&mut tail)
        .map_err(|error| ReadFailure::Reader(error.into()))?
        != 0
    {
        return Err(ReadFailure::Reader(Error::InvalidRange));
    }
    Ok(result)
}

struct ExactReader<R> {
    inner: R,
    remaining: u64,
    failure: Option<Arc<std::io::Error>>,
}

#[derive(Debug)]
struct SharedReadError(Arc<std::io::Error>);
impl std::fmt::Display for SharedReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.0.as_ref(), f)
    }
}
impl std::error::Error for SharedReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
fn shared_read_error(error: Arc<std::io::Error>) -> std::io::Error {
    std::io::Error::new(error.kind(), SharedReadError(error))
}
impl<R> ExactReader<R> {
    fn record(&mut self, error: std::io::Error) -> std::io::Error {
        let error = Arc::new(error);
        self.failure = Some(error.clone());
        shared_read_error(error)
    }
}
impl<R: Read> Read for ExactReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let length = bytes
            .len()
            .min(crate::READ_BUFFER_SIZE)
            .min(self.remaining.min(usize::MAX as u64) as usize);
        if length == 0 {
            return Ok(0);
        }
        match self.inner.read(&mut bytes[..length]) {
            Ok(0) => Err(self.record(std::io::ErrorKind::UnexpectedEof.into())),
            Ok(count) => {
                // A consumer may successfully retry a transient read error.
                self.failure = None;
                self.remaining -= count as u64;
                Ok(count)
            }
            Err(error) => Err(self.record(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn export_distinguishes_consumer_disk_full_from_reader_failure() {
        use windows_sys::Win32::Foundation::{ERROR_DISK_FULL, ERROR_READ_FAULT};
        let consumer = read_complete(Cursor::new(b"abc"), 3, |reader| -> Result<()> {
            reader.read_exact(&mut [0u8; 1])?;
            Err(std::io::Error::from_raw_os_error(ERROR_DISK_FULL as i32).into())
        });
        assert!(
            matches!(consumer, Err(ReadFailure::Consumer(Error::Io(error))) if error.raw_os_error() == Some(ERROR_DISK_FULL as i32))
        );
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from_raw_os_error(ERROR_READ_FAULT as i32))
            }
        }
        let error = read_complete(Broken, 3, |reader| {
            std::io::copy(reader, &mut std::io::sink())?;
            Ok(())
        })
        .unwrap_err();
        let ReadFailure::Reader(error) = error else {
            panic!("reader failure mislabeled as consumer");
        };
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        let mut found = false;
        while let Some(error) = current {
            found |= error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.raw_os_error() == Some(ERROR_READ_FAULT as i32));
            current = error.source();
        }
        assert!(found, "original native I/O error absent from source chain");
    }

    #[test]
    fn export_allows_successful_retry_of_interrupted_read() {
        struct InterruptedOnce {
            interrupted: bool,
            reader: Cursor<&'static [u8]>,
        }
        impl Read for InterruptedOnce {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                self.reader.read(output)
            }
        }
        let reader = InterruptedOnce {
            interrupted: false,
            reader: Cursor::new(b"abc"),
        };
        let bytes = read_complete(reader, 3, |reader| {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .unwrap();
        assert_eq!(bytes, b"abc");
    }

    #[test]
    fn export_requires_consumption_and_physical_eof() {
        assert!(matches!(
            read_complete(Cursor::new(b"abc"), 3, |_| Ok(())),
            Err(ReadFailure::Consumer(Error::InvalidRange))
        ));
        assert!(
            matches!(read_complete(Cursor::new(b"ab"), 3, |r| { r.read_to_end(&mut Vec::new())?; Ok(()) }), Err(ReadFailure::Reader(Error::Io(e))) if e.kind() == std::io::ErrorKind::UnexpectedEof)
        );
        assert!(matches!(
            read_complete(Cursor::new(b"abcd"), 3, |r| {
                r.read_to_end(&mut Vec::new())?;
                Ok(())
            }),
            Err(ReadFailure::Reader(Error::InvalidRange))
        ));
        let bytes = read_complete(Cursor::new(b"abc"), 3, |r| {
            let mut bytes = Vec::new();
            r.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .unwrap();
        assert_eq!(bytes, b"abc");
        read_complete(Cursor::new(b""), 0, |_| Ok(())).unwrap();
    }

    #[test]
    fn export_proof_binds_native_backing_and_size_separately() {
        let expected = FileProof {
            volume: 1,
            index: 2,
            size: 3,
            identity: Some(vec![4]),
            backing: Some(FileId::from_bytes(b"base")),
        };
        for current in [
            FileProof {
                volume: 9,
                index: 2,
                size: 3,
                identity: Some(vec![4]),
                backing: expected.backing.clone(),
            },
            FileProof {
                volume: 1,
                index: 9,
                size: 3,
                identity: Some(vec![4]),
                backing: expected.backing.clone(),
            },
            FileProof {
                volume: 1,
                index: 2,
                size: 3,
                identity: Some(vec![9]),
                backing: expected.backing.clone(),
            },
        ] {
            assert!(matches!(
                expected.verify(&current),
                Err(Error::IdentityMismatch)
            ));
        }
        let changed_size = FileProof {
            volume: 1,
            index: 2,
            size: 4,
            identity: Some(vec![4]),
            backing: expected.backing.clone(),
        };
        assert!(matches!(expected.verify(&changed_size), Err(Error::Busy)));
    }
}
