//! Mixed VFS snapshots: reuse only provider evidence bound to a verified source;
//! hash full, modified and new files. Stat/mtime never grants reference reuse.
use super::{
    NativeClientError, PreparedSnapshot, Result, SnapshotBuilder, SnapshotLimits,
    VerifiedVfsSnapshot,
};
use layrs_core::scm::{FileId, RepoPath};
use layrs_vfs::{CaptureEvidence, SnapshotSource};
use serde::Serialize;
use std::io::{self, Write};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VfsFileRecord {
    pub path: RepoPath,
    pub file_id: FileId,
    pub byte_length: u64,
    /// Internal selection only; export verifies every output byte independently.
    #[serde(skip)]
    pub(super) immutable_source: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VfsCaptureCounts {
    pub reused_files: usize,
    pub hashed_files: usize,
    pub hashed_bytes: u64,
    pub deleted_files: usize,
}

pub(super) struct VfsCapturedSnapshot {
    pub snapshot: PreparedSnapshot,
    pub files: Vec<VfsFileRecord>,
    pub counts: VfsCaptureCounts,
}

/// `read_file` must read through the provider checkpoint's retained capability.
/// The caller keeps that checkpoint alive until accept and journal ack finish.
/// Hashed files stream in the chunks of an ordinary capture
/// (`fixed_chunk_bytes`) to a private spool: memory stays bounded whatever
/// their size, which only the streaming entry/depth bounds limit (ADR 0027).
pub(super) fn assemble_snapshot<'a>(
    source: &VerifiedVfsSnapshot,
    inspections: impl IntoIterator<Item = (&'a RepoPath, &'a CaptureEvidence)>,
    read_file: &mut dyn FnMut(&RepoPath, &mut dyn Write) -> Result<()>,
) -> Result<VfsCapturedSnapshot> {
    use layrs_store_local::scm_v1::checkout::{CheckoutProjectionContent, CheckoutProjectionEntry};
    let limits = SnapshotLimits::streaming();
    let mut entries = Vec::new();
    let mut manifests = Vec::new();
    let mut spool = super::snapshot::SnapshotChunkSpool::new()?;
    let reference = |entries: &mut Vec<CheckoutProjectionEntry>, path: &RepoPath, file_id: &FileId, size: u64| {
        entries.push(CheckoutProjectionEntry {
            path: path.as_str().to_owned(),
            content: CheckoutProjectionContent::File {
                file_id: file_id.clone(),
                byte_length: size,
                executable: false,
            },
        });
    };
    let mut files = Vec::new();
    let mut counts = VfsCaptureCounts::default();
    let mut seen = std::collections::BTreeSet::new();
    for (path, evidence) in inspections {
        if !seen.insert(path.clone()) || seen.len() > SnapshotLimits::streaming().max_entries {
            return Err(NativeClientError::InvalidSnapshot(
                "duplicate or excessive VFS checkpoint paths".into(),
            ));
        }
        match evidence {
            CaptureEvidence::Immutable {
                file_id,
                size,
                original_path,
            } => {
                let entry = source
                    .catalog()
                    .entry(path)
                    .ok_or(NativeClientError::InvalidState(
                        "immutable VFS evidence absent from source",
                    ))?;
                if original_path != path || &entry.file_id != file_id || entry.size != *size {
                    return Err(NativeClientError::InvalidState(
                        "immutable VFS evidence differs from verified source",
                    ));
                }
                reference(&mut entries, path, file_id, *size);
                counts.reused_files += 1;
                files.push(VfsFileRecord {
                    path: path.clone(),
                    file_id: file_id.clone(),
                    byte_length: *size,
                    immutable_source: true,
                });
            }
            CaptureEvidence::NeedsHash { size, base } => {
                if *size > limits.max_file_bytes as u64
                    || counts
                        .hashed_bytes
                        .checked_add(*size)
                        .is_none_or(|sum| sum > limits.max_total_bytes as u64)
                {
                    return Err(NativeClientError::InvalidSnapshot(
                        "VFS dirty byte budget exceeded".into(),
                    ));
                }
                // A large edited file keeps most of its source chunks: those
                // are admitted already and are not spooled again (ADR 0027).
                let known = match base {
                    Some(base) if *size >= 64 * 1024 * 1024 => source.source_chunk_ids(base)?,
                    _ => std::collections::BTreeSet::new(),
                };
                let mut output = ChunkingWriter::new(*size, &mut spool)?.skipping(&known);
                read_file(path, &mut output)?;
                let manifest = output.finish()?;
                let file_id = manifest.file_id().clone();
                reference(&mut entries, path, &file_id, *size);
                manifests.push(manifest);
                counts.hashed_files += 1;
                counts.hashed_bytes += *size;
                files.push(VfsFileRecord {
                    path: path.clone(),
                    file_id,
                    byte_length: *size,
                    immutable_source: false,
                });
            }
            CaptureEvidence::Deleted { base } => {
                if source.catalog().entry(path).map(|entry| &entry.file_id) != Some(base) {
                    return Err(NativeClientError::InvalidState(
                        "VFS deletion is not bound to provider base",
                    ));
                }
                counts.deleted_files += 1;
            }
            // Proven by an earlier accepted capture (change index, ADR 0025):
            // a reference to content already admitted. Admission of the new
            // Tree still requires it, so a stale entry fails the capture.
            CaptureEvidence::Vouched { file_id, size } => {
                reference(&mut entries, path, file_id, *size);
                counts.reused_files += 1;
                files.push(VfsFileRecord {
                    path: path.clone(),
                    file_id: file_id.clone(),
                    byte_length: *size,
                    immutable_source: false,
                });
            }
        }
    }
    if source
        .catalog()
        .entries()
        .iter()
        .any(|entry| !seen.contains(&entry.path))
    {
        return Err(NativeClientError::InvalidState(
            "VFS checkpoint omitted a base path",
        ));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    manifests.sort_by(|a, b| a.id().cmp(b.id()));
    manifests.dedup_by(|a, b| a.id() == b.id());
    // Reused files are references to content already admitted (the verified
    // source, or an earlier accepted capture); admission rechecks them.
    let snapshot = PreparedSnapshot::from_streaming_referencing(&entries, manifests, spool, limits)?;
    // The captured Tree's projection, for the next command (derived data).
    layrs_store_local::scm_v1::projection_cache::save(
        &source.binding().store_root,
        snapshot.root_tree_id(),
        snapshot.root_tree_id(),
        &entries,
    );
    Ok(VfsCapturedSnapshot {
        snapshot,
        files,
        counts,
    })
}

/// Splits one file's bytes into the deterministic chunks of an ordinary
/// capture (`fixed_chunk_bytes`), spooling each chunk and building its
/// manifest. The exact announced length is required.
struct ChunkingWriter<'a> {
    builder: layrs_core::scm::FileManifestBuilder,
    spool: &'a mut super::snapshot::SnapshotChunkSpool,
    /// Chunks already admitted to the Space: hashed into the manifest but
    /// not spooled.
    known: Option<&'a std::collections::BTreeSet<layrs_core::scm::ChunkId>>,
    buffer: Vec<u8>,
    chunk_bytes: usize,
    expected: u64,
    written: u64,
}
impl<'a> ChunkingWriter<'a> {
    fn new(expected: u64, spool: &'a mut super::snapshot::SnapshotChunkSpool) -> Result<Self> {
        let chunk_bytes = layrs_core::scm::fixed_chunk_bytes(expected);
        let chunks = usize::try_from(expected.div_ceil(chunk_bytes))
            .map_err(|_| NativeClientError::InvalidState("VFS file chunk count overflow"))?;
        let chunk_bytes = chunk_bytes as usize;
        Ok(Self {
            builder: layrs_core::scm::FileManifestBuilder::new(
                layrs_core::scm::fixed_chunking(expected),
                expected,
                chunks,
            )?,
            spool,
            known: None,
            buffer: Vec::with_capacity(chunk_bytes.min(expected as usize)),
            chunk_bytes,
            expected,
            written: 0,
        })
    }
    fn emit(&mut self) -> io::Result<()> {
        let bytes = std::mem::replace(
            &mut self.buffer,
            Vec::with_capacity(self.chunk_bytes.min((self.expected - self.written) as usize)),
        );
        let chunk = layrs_core::scm::Chunk::new(bytes).map_err(io::Error::other)?;
        self.builder.push_chunk(&chunk).map_err(io::Error::other)?;
        if self.known.is_some_and(|known| known.contains(chunk.id())) {
            return Ok(());
        }
        self.spool.insert(&chunk)
    }
    fn skipping(
        mut self,
        known: &'a std::collections::BTreeSet<layrs_core::scm::ChunkId>,
    ) -> Self {
        self.known = Some(known);
        self
    }
    fn finish(mut self) -> Result<layrs_core::scm::FileManifest> {
        if self.written != self.expected {
            return Err(NativeClientError::InvalidState(
                "VFS file length changed under checkpoint",
            ));
        }
        if !self.buffer.is_empty() {
            self.emit()?;
        }
        Ok(self.builder.finish()?)
    }
}
impl Write for ChunkingWriter<'_> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let accepted = bytes.len();
        if accepted as u64 > self.expected - self.written {
            return Err(io::Error::other(
                "VFS file grew or exceeded its checkpoint bound",
            ));
        }
        self.written += accepted as u64;
        while !bytes.is_empty() {
            let room = self.chunk_bytes - self.buffer.len();
            let take = room.min(bytes.len());
            self.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.buffer.len() == self.chunk_bytes {
                self.emit()?;
            }
        }
        Ok(accepted)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Recovery export keeps only descriptors and incremental hashes in memory.
/// All logical bounds are checked before reading or creating an output tree:
/// 100,000 entries and the streaming depth bound; sizes are unbounded.
/// `read_file` must consume the retained ExportCheckpoint reader to real EOF;
/// the provider verifies its exact count/identity before returning success.
pub(super) fn assemble_export_snapshot<'a>(
    catalog: &layrs_vfs::Catalog,
    inspections: impl IntoIterator<Item = (&'a RepoPath, &'a CaptureEvidence)>,
    read_file: &mut dyn FnMut(&RepoPath, &mut dyn Write) -> Result<()>,
) -> Result<VfsCapturedSnapshot> {
    let limits = SnapshotLimits::streaming();
    let mut entries = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0u64;
    for (path, evidence) in inspections {
        if !seen.insert(path.clone())
            || seen.len() > limits.max_entries
            || path.as_str().split('/').count() > limits.max_depth
        {
            return Err(NativeClientError::InvalidSnapshot(
                "VFS export path/depth budget exceeded or duplicate checkpoint path".into(),
            ));
        }
        let length = match evidence {
            CaptureEvidence::Immutable {
                file_id,
                size,
                original_path,
            } => {
                let entry = catalog.entry(path).ok_or(NativeClientError::InvalidState(
                    "immutable export evidence absent from verified source",
                ))?;
                if original_path != path || &entry.file_id != file_id || entry.size != *size {
                    return Err(NativeClientError::InvalidState(
                        "immutable export evidence differs from verified source",
                    ));
                }
                *size
            }
            CaptureEvidence::NeedsHash { size, .. } => *size,
            CaptureEvidence::Vouched { .. } => {
                return Err(NativeClientError::InvalidState(
                    "export never admits a file from its listing",
                ))
            }
            CaptureEvidence::Deleted { base } => {
                if catalog.entry(path).map(|entry| &entry.file_id) != Some(base) {
                    return Err(NativeClientError::InvalidState(
                        "export deletion is not bound to provider base",
                    ));
                }
                0
            }
        };
        total = total
            .checked_add(length)
            .ok_or(NativeClientError::InvalidState(
                "VFS export logical size overflow",
            ))?;
        if length > limits.max_file_bytes as u64 || total > limits.max_total_bytes as u64 {
            return Err(NativeClientError::InvalidSnapshot(
                "VFS export size limit exceeded".into(),
            ));
        }
        entries.push((path, evidence));
    }
    if catalog
        .entries()
        .iter()
        .any(|entry| !seen.contains(&entry.path))
    {
        return Err(NativeClientError::InvalidState(
            "VFS export checkpoint omitted a base path",
        ));
    }
    let mut builder = SnapshotBuilder::new(limits);
    let mut files = Vec::new();
    let mut counts = VfsCaptureCounts::default();
    for (path, evidence) in entries {
        let (file_id, byte_length, immutable_source) = match evidence {
            CaptureEvidence::Immutable { file_id, size, .. } => {
                counts.reused_files += 1;
                (file_id.clone(), *size, true)
            }
            CaptureEvidence::NeedsHash { size, .. } => {
                let mut output = ExportHash::new(*size);
                read_file(path, &mut output)?;
                let file_id = output.finish()?;
                counts.hashed_files += 1;
                counts.hashed_bytes += *size;
                (file_id, *size, false)
            }
            CaptureEvidence::Deleted { .. } => {
                counts.deleted_files += 1;
                continue;
            }
            CaptureEvidence::Vouched { .. } => {
                return Err(NativeClientError::InvalidState(
                    "export never admits a file from its listing",
                ))
            }
        };
        // Export never accepts this metadata into CAS. References are backed
        // by the verified source or the exact retained-file hash above; the
        // ordinary writer independently verifies every byte in a second pass.
        builder.add_file_reference(path.clone(), file_id.clone(), byte_length, false)?;
        files.push(VfsFileRecord {
            path: path.clone(),
            file_id,
            byte_length,
            immutable_source,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(VfsCapturedSnapshot {
        snapshot: builder.finish()?,
        files,
        counts,
    })
}

struct ExportHash {
    hash: blake3::Hasher,
    expected: u64,
    written: u64,
    failed: bool,
}
impl ExportHash {
    fn new(expected: u64) -> Self {
        let mut hash = blake3::Hasher::new();
        hash.update(b"layrs-file-v1\0");
        Self {
            hash,
            expected,
            written: 0,
            failed: false,
        }
    }
    fn finish(self) -> Result<FileId> {
        if self.failed || self.written != self.expected {
            return Err(NativeClientError::InvalidState(
                "VFS export file length changed under checkpoint",
            ));
        }
        Ok(FileId::parse(format!(
            "blake3:{}",
            self.hash.finalize().to_hex()
        ))?)
    }
}
impl Write for ExportHash {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed || bytes.len() as u64 > self.expected.saturating_sub(self.written) {
            self.failed = true;
            return Err(io::Error::other(
                "VFS export file exceeds its checkpoint length",
            ));
        }
        self.hash.update(bytes);
        self.written += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod export_tests {
    use super::*;

    #[test]
    fn export_streams_more_than_capture_aggregate_limit_as_metadata_only() {
        let size = 65 * 1024 * 1024 + 3u64;
        let catalog = layrs_vfs::Catalog::new(1, vec![]).unwrap();
        let entries: Vec<_> = (0..5)
            .map(|index| {
                (
                    RepoPath::parse(format!("f{index}")).unwrap(),
                    CaptureEvidence::NeedsHash { base: None, size },
                )
            })
            .collect();
        let block = [b'x'; 8192];
        let mut expected = blake3::Hasher::new();
        expected.update(b"layrs-file-v1\0");
        let mut left = size;
        while left != 0 {
            let count = left.min(block.len() as u64) as usize;
            expected.update(&block[..count]);
            left -= count as u64;
        }
        let expected = FileId::parse(format!("blake3:{}", expected.finalize().to_hex())).unwrap();
        let result = assemble_export_snapshot(
            &catalog,
            entries.iter().map(|(path, evidence)| (path, evidence)),
            &mut |_, output| {
                let mut left = size;
                while left != 0 {
                    let count = left.min(block.len() as u64) as usize;
                    output.write_all(&block[..count])?;
                    left -= count as u64;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(result.counts.hashed_bytes, 5 * size);
        assert!(result.counts.hashed_bytes > SnapshotLimits::default().max_total_bytes as u64);
        assert_eq!(result.files.len(), 5);
        assert!(result
            .files
            .iter()
            .all(|file| file.file_id == expected && file.byte_length == size));
        assert!(result
            .snapshot
            .objects()
            .iter()
            .all(|object| matches!(object, layrs_store::scm::CanonicalObject::Tree(_))));
    }

    #[test]
    fn export_accepts_any_file_size_and_refuses_size_overflow_before_reading() {
        let catalog = layrs_vfs::Catalog::new(1, vec![]).unwrap();
        let entries = |sizes: Vec<u64>| -> Vec<_> {
            sizes
                .into_iter()
                .enumerate()
                .map(|(index, size)| {
                    (
                        RepoPath::parse(format!("f{index}")).unwrap(),
                        CaptureEvidence::NeedsHash { base: None, size },
                    )
                })
                .collect()
        };
        // Files beyond 1 GiB, and a total beyond 4 GiB, reach the reader
        // (ADR 0027): the read is what fails here, not a size preflight.
        let large = entries(vec![5 << 30, 5 << 30]);
        let mut reads = 0;
        let error = assemble_export_snapshot(
            &catalog,
            large.iter().map(|(path, evidence)| (path, evidence)),
            &mut |_, _| {
                reads += 1;
                Err(NativeClientError::InvalidState("reader stopped"))
            },
        )
        .err()
        .unwrap();
        assert_eq!(reads, 1);
        assert!(matches!(error, NativeClientError::InvalidState("reader stopped")));
        // A logical size overflow still refuses before any read.
        let overflow = entries(vec![u64::MAX, 1]);
        let error = assemble_export_snapshot(
            &catalog,
            overflow.iter().map(|(path, evidence)| (path, evidence)),
            &mut |_, _| panic!("overflowing export must refuse before reading"),
        )
        .err()
        .unwrap();
        assert!(matches!(
            error,
            NativeClientError::InvalidState("VFS export logical size overflow")
        ));
    }

    #[test]
    fn export_hash_requires_exact_count_and_preserves_consumer_errors() {
        let mut exact = ExportHash::new(3);
        exact.write_all(b"abc").unwrap();
        assert_eq!(exact.finish().unwrap(), FileId::from_bytes(b"abc"));
        let mut short = ExportHash::new(4);
        short.write_all(b"abc").unwrap();
        assert!(short.finish().is_err());
        let mut long = ExportHash::new(2);
        assert!(long.write_all(b"abc").is_err());
        assert!(long.finish().is_err());
        let catalog = layrs_vfs::Catalog::new(1, vec![]).unwrap();
        let path = RepoPath::parse("file").unwrap();
        let evidence = CaptureEvidence::NeedsHash {
            base: None,
            size: 3,
        };
        let error = assemble_export_snapshot(&catalog, [(&path, &evidence)], &mut |_, _| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "destination refusal").into())
        })
        .err()
        .unwrap();
        assert!(matches!(error, NativeClientError::Filesystem(error)
            if error.kind() == io::ErrorKind::PermissionDenied && error.to_string().contains("destination refusal")));
    }
}
