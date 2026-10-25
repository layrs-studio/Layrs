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
/// These initial bounds apply to dirty bytes in memory, not clean logical files.
pub(super) fn assemble_snapshot<'a>(
    source: &VerifiedVfsSnapshot,
    inspections: impl IntoIterator<Item = (&'a RepoPath, &'a CaptureEvidence)>,
    read_file: &mut dyn FnMut(&RepoPath, &mut dyn Write) -> Result<()>,
) -> Result<VfsCapturedSnapshot> {
    let mut builder = SnapshotBuilder::new(SnapshotLimits::streaming());
    let limits = SnapshotLimits::default();
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
                builder.add_file_reference(path.clone(), file_id.clone(), *size, false)?;
                counts.reused_files += 1;
                files.push(VfsFileRecord {
                    path: path.clone(),
                    file_id: file_id.clone(),
                    byte_length: *size,
                    immutable_source: true,
                });
            }
            CaptureEvidence::NeedsHash { size, .. } => {
                let size = usize::try_from(*size)
                    .map_err(|_| NativeClientError::InvalidState("VFS file size overflow"))?;
                if size > limits.max_file_bytes
                    || counts
                        .hashed_bytes
                        .checked_add(size as u64)
                        .is_none_or(|sum| sum > limits.max_total_bytes as u64)
                {
                    return Err(NativeClientError::InvalidSnapshot(
                        "VFS dirty byte budget exceeded".into(),
                    ));
                }
                let mut output = BoundedFile {
                    bytes: Vec::new(),
                    limit: size,
                };
                read_file(path, &mut output)?;
                if output.bytes.len() != size {
                    return Err(NativeClientError::InvalidState(
                        "VFS file length changed under checkpoint",
                    ));
                }
                let file_id = FileId::from_bytes(&output.bytes);
                builder.add_file(path.clone(), &output.bytes, false)?;
                counts.hashed_files += 1;
                counts.hashed_bytes += size as u64;
                files.push(VfsFileRecord {
                    path: path.clone(),
                    file_id,
                    byte_length: size as u64,
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
    Ok(VfsCapturedSnapshot {
        snapshot: builder.finish()?,
        files,
        counts,
    })
}

/// Recovery export keeps only descriptors and incremental hashes in memory.
/// All logical bounds are checked before reading or creating an output tree:
/// 1 GiB/file, 4 GiB total, 100,000 entries and the streaming depth bound.
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
                "VFS export limit exceeded (1 GiB/file, 4 GiB total)".into(),
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

struct BoundedFile {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for BoundedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > self.limit)
        {
            return Err(io::Error::other(
                "VFS file grew or exceeded its checkpoint bound",
            ));
        }
        self.bytes.extend_from_slice(bytes);
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
    fn export_preflights_file_and_total_limits_before_reading() {
        let limits = SnapshotLimits::streaming();
        let catalog = layrs_vfs::Catalog::new(1, vec![]).unwrap();
        for sizes in [
            vec![limits.max_file_bytes as u64 + 1],
            vec![limits.max_file_bytes as u64; 5],
        ] {
            let entries: Vec<_> = sizes
                .into_iter()
                .enumerate()
                .map(|(index, size)| {
                    (
                        RepoPath::parse(format!("f{index}")).unwrap(),
                        CaptureEvidence::NeedsHash { base: None, size },
                    )
                })
                .collect();
            let error = assemble_export_snapshot(
                &catalog,
                entries.iter().map(|(path, evidence)| (path, evidence)),
                &mut |_, _| panic!("over-budget export must refuse before reading"),
            )
            .err()
            .unwrap();
            assert!(matches!(error, NativeClientError::InvalidSnapshot(message)
                if message.contains("1 GiB/file, 4 GiB total")));
        }
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
