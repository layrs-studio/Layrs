//! Strict managed image evidence under a retained recovery checkpoint. This
//! grants no fresh remote rights, source SQL/family authority or destination.
use super::{capture_image, recovery_export};
use crate::projection::provider::{
    records::{activation::activation_snapshot, ProviderBootstrapSource},
    recovery::error::{invalid, Result},
};
use layrs_core::{
    projection::ProjectionSnapshot,
    scm::{ChunkingDescriptor, FileId, FileManifest, RepoPath, TreeEntryTarget},
};
use layrs_vfs::{Cancellation, ExportCheckpoint, SnapshotSource};
use recovery_export::{
    Observed, PlannedFile, ProviderRecoveryExportClass as Class, ProviderRecoveryExportFile,
    ReaderKind,
};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};

pub(in crate::projection::provider) mod stream;

#[path = "managed_image/complete.rs"]
pub(in crate::projection::provider) mod complete;

/// Immutable source manifests are borrowed rather than duplicating the fixed
/// catalog. Only hashed native replacements own new manifest metadata.
pub(super) enum ManagedManifest<'a> {
    Source(&'a FileManifest),
    Native(FileManifest),
}
impl ManagedManifest<'_> {
    pub(super) fn manifest(&self) -> &FileManifest {
        match self {
            Self::Source(value) => value,
            Self::Native(value) => value,
        }
    }
}

pub(super) struct ManagedFile<'a> {
    pub(super) path: RepoPath,
    pub(super) manifest: ManagedManifest<'a>,
    pub(super) executable: bool,
    pub(super) expected: Option<(FileId, bool)>,
}
pub(super) struct ManagedDeletion {
    pub(super) path: RepoPath,
    pub(super) base: FileId,
    pub(super) executable: bool,
}
struct ImageData<'a> {
    files: Vec<ManagedFile<'a>>,
    deletions: Vec<ManagedDeletion>,
    digest: String,
}
impl ImageData<'_> {
    fn matches(&self, other: &Self) -> bool {
        self.digest == other.digest
            && self.files.len() == other.files.len()
            && self.files.iter().zip(&other.files).all(|(a, b)| {
                a.path == b.path
                    && a.manifest.manifest() == b.manifest.manifest()
                    && a.executable == b.executable
                    && a.expected == b.expected
            })
            && self.deletions.len() == other.deletions.len()
            && self
                .deletions
                .iter()
                .zip(&other.deletions)
                .all(|(a, b)| a.path == b.path && a.base == b.base && a.executable == b.executable)
    }
}

/// Holding this object retains the exact ExportCheckpoint. `digest` alone is
/// DATA; `revalidate_and_match_digest` rehashes because identity/size checks
/// alone do not prove retained ordinary-file bytes remain unchanged.
pub(super) struct VerifiedManagedImage<'a, 'c> {
    checkpoint: &'a mut ExportCheckpoint<'c>,
    bytes: &'a dyn SnapshotSource,
    source: &'a ProviderBootstrapSource,
    activated: &'a ProjectionSnapshot,
    data: ImageData<'a>,
}
impl VerifiedManagedImage<'_, '_> {
    pub(super) fn require_source(
        &self,
        source: &ProviderBootstrapSource,
        activated: &ProjectionSnapshot,
    ) -> Result<()> {
        if self.source.commitment() != source.commitment() || self.activated != activated {
            return Err(invalid("managed image fixed source differs"));
        }
        Ok(())
    }

    /// Bind this retained image to the independently authenticated SQL/native
    /// owner. Namespace validation performs no callback or content reads.
    pub(super) fn require_owner(
        &self,
        root: &layrs_vfs::ProviderRootGuard,
        source: &ProviderBootstrapSource,
        activated: &ProjectionSnapshot,
    ) -> Result<()> {
        let retained = self
            .checkpoint
            .root_guard()
            .ok_or_else(|| invalid("managed image lacks retained provider root"))?;
        if retained.root() != root.root()
            || retained.registration().root_id() != root.registration().root_id()
            || retained.control_directory().identity() != root.control_directory().identity()
            || self.source.commitment() != source.commitment()
            || self.activated != activated
        {
            return Err(invalid("managed image owner or source differs"));
        }
        self.checkpoint.revalidate_namespace()?;
        Ok(())
    }

    pub(super) fn files(&self) -> &[ManagedFile<'_>] {
        &self.data.files
    }
    pub(super) fn deletions(&self) -> &[ManagedDeletion] {
        &self.data.deletions
    }
    /// Raw 64-hex digest for `ImportVersion.effective_image_digest`.
    pub(super) fn digest(&self) -> &str {
        &self.data.digest
    }
    /// Copy only a file in this verified current image. The output may contain
    /// a prefix on failure and must remain unpublished until this returns Ok.
    /// Historical capture content needs a separate protected family reader.
    pub(super) fn copy_exact_file(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        cancel.check()?;
        let file = exact_file(&self.data.files, path, manifest)?;
        self.checkpoint.revalidate_namespace()?;
        match &file.manifest {
            ManagedManifest::Source(_) => {
                let mut offset = 0u64;
                stream::copy_verified(manifest, output, cancel, |part| {
                    self.bytes
                        .read_exact_at(manifest.file_id(), offset, part, cancel)?;
                    offset = offset
                        .checked_add(part.len() as u64)
                        .ok_or_else(|| invalid("managed image source offset overflow"))?;
                    Ok(())
                })?;
            }
            ManagedManifest::Native(_) => {
                let mut consumer_error = None;
                let native = self.checkpoint.with_reader(path, |reader| {
                    let result = stream::copy_verified(manifest, output, cancel, |part| {
                        reader.read_exact(part).map_err(Into::into)
                    });
                    if let Err(error) = result {
                        consumer_error = Some(error);
                        return Err(layrs_vfs::Error::Io(io::Error::other(
                            "managed image copy consumer failed",
                        )));
                    }
                    Ok(())
                });
                finish_native_copy(native, consumer_error)?;
            }
        }
        self.checkpoint.revalidate_namespace()?;
        cancel.check()?;
        Ok(())
    }

    /// Copy one chunk of an exact current-image representation. The sink may
    /// contain a prefix on failure and must remain unpublished. This proves
    /// the selected ChunkId only; the importer must verify the full FileId
    /// after assembling all chunks. Historical-only manifests are rejected.
    pub(super) fn copy_exact_chunk(
        &mut self,
        path: &RepoPath,
        manifest: &FileManifest,
        ordinal: usize,
        output: &mut dyn Write,
        cancel: &Cancellation,
    ) -> Result<()> {
        cancel.check()?;
        let file = exact_file(&self.data.files, path, manifest)?;
        let chunk = stream::exact_chunk(manifest, ordinal)?;
        self.checkpoint.revalidate_namespace()?;
        match &file.manifest {
            ManagedManifest::Source(_) => {
                stream::copy_verified_chunk(manifest, ordinal, output, cancel, |offset, part| {
                    self.bytes
                        .read_exact_at(manifest.file_id(), offset, part, cancel)?;
                    Ok(())
                })?;
            }
            ManagedManifest::Native(_) => {
                let mut consumer_error = None;
                let native = self.checkpoint.with_range_reader(
                    path,
                    chunk.offset(),
                    chunk.byte_length(),
                    |reader| {
                        let result = stream::copy_verified_chunk(
                            manifest,
                            ordinal,
                            output,
                            cancel,
                            |_, part| reader.read_exact(part).map_err(Into::into),
                        );
                        if let Err(error) = result {
                            consumer_error = Some(error);
                            return Err(layrs_vfs::Error::Io(io::Error::other(
                                "managed image chunk copy consumer failed",
                            )));
                        }
                        Ok(())
                    },
                );
                finish_native_copy(native, consumer_error)?;
            }
        }
        self.checkpoint.revalidate_namespace()?;
        cancel.check()?;
        Ok(())
    }
    pub(super) fn revalidate_and_match_digest(
        &mut self,
        expected: &str,
        cancel: &Cancellation,
    ) -> Result<()> {
        if expected != self.data.digest {
            return Err(invalid("managed image expected digest differs"));
        }
        let refreshed = build(
            self.checkpoint,
            self.bytes,
            self.source,
            self.activated,
            cancel,
        )?;
        if !self.data.matches(&refreshed) {
            return Err(invalid("managed image bytes or namespace changed"));
        }
        Ok(())
    }
}

fn finish_native_copy(
    native: layrs_vfs::Result<()>,
    consumer_error: Option<crate::projection::provider::recovery::error::ProviderBootstrapCause>,
) -> Result<()> {
    // The checkpoint can identify an actual reader/proof failure even when the
    // consumer propagated it. Preserve its path/backing provenance. Only unwrap
    // our consumer sentinel for checksum, cancellation and output failures.
    match native {
        Err(error @ layrs_vfs::Error::ExportConsumer { .. }) => {
            Err(consumer_error.unwrap_or_else(|| error.into()))
        }
        Err(error) => Err(error.into()),
        Ok(()) => consumer_error.map_or(Ok(()), Err),
    }
}

fn exact_file<'f, 'm>(
    files: &'f [ManagedFile<'m>],
    path: &RepoPath,
    manifest: &FileManifest,
) -> Result<&'f ManagedFile<'m>> {
    let file = files
        .iter()
        .find(|file| &file.path == path)
        .ok_or_else(|| invalid("managed image file path absent"))?;
    if file.manifest.manifest() != manifest {
        return Err(invalid("managed image file representation differs"));
    }
    Ok(file)
}

/// The source is local-only. An online caller must independently prove fresh
/// Step/root/policy/epoch and rights before treating this image as managed.
pub(super) fn plan<'a, 'c>(
    checkpoint: &'a mut ExportCheckpoint<'c>,
    bytes: &'a dyn SnapshotSource,
    source: &'a ProviderBootstrapSource,
    activated: &'a ProjectionSnapshot,
    cancel: &Cancellation,
) -> Result<VerifiedManagedImage<'a, 'c>> {
    let data = build(checkpoint, bytes, source, activated, cancel)?;
    Ok(VerifiedManagedImage {
        checkpoint,
        bytes,
        source,
        activated,
        data,
    })
}

fn build<'a>(
    checkpoint: &mut ExportCheckpoint<'_>,
    bytes: &dyn SnapshotSource,
    source: &'a ProviderBootstrapSource,
    activated: &ProjectionSnapshot,
    cancel: &Cancellation,
) -> Result<ImageData<'a>> {
    cancel.check()?;
    checkpoint.revalidate_namespace()?;
    let count =
        bound_observation_paths(checkpoint.entries().iter().map(|entry| &entry.path), cancel)?;
    let mut observations = Vec::with_capacity(count);
    for entry in checkpoint.entries() {
        cancel.check()?;
        observations.push(Observed {
            path: entry.path.clone(),
            evidence: entry.evidence.clone(),
            original_path: checkpoint.original_path(&entry.path)?,
        });
    }
    let image = interpret(
        &observations,
        source,
        activated,
        cancel,
        |planned, cancel| {
            let descriptor = source
                .descriptors()
                .binary_search_by(|d| d.path.cmp(&planned.path))
                .ok()
                .and_then(|index| source.descriptors().get(index))
                .ok_or_else(|| invalid("immutable managed descriptor absent"))?;
            verify_source_manifest_stream(&descriptor.manifest, cancel, |offset, output, cancel| {
                bytes.read_exact_at(&descriptor.file_id, offset, output, cancel)
            })
        },
        |planned, cancel| {
            Ok(checkpoint.with_reader(&planned.path, |reader| {
                capture_image::hash_reader(reader, planned.size, cancel)
            })?)
        },
    )?;
    checkpoint.revalidate_namespace()?;
    cancel.check()?;
    Ok(image)
}

/// Authenticate both the full FileId and every chunk ID of the exact source
/// representation. One 64 KiB scratch buffer; no whole chunk/file allocation.
fn verify_source_manifest_stream(
    manifest: &FileManifest,
    cancel: &Cancellation,
    mut read: impl FnMut(u64, &mut [u8], &Cancellation) -> layrs_vfs::Result<()>,
) -> Result<FileId> {
    let mut file_hash = blake3::Hasher::new();
    file_hash.update(b"layrs-file-v1\0");
    let mut offset = 0u64;
    let mut scratch = [0u8; 64 * 1024];
    for chunk in manifest.chunks() {
        cancel.check()?;
        if chunk.offset() != offset {
            return Err(invalid("managed source manifest chunk offset differs"));
        }
        let mut chunk_hash = blake3::Hasher::new();
        chunk_hash.update(b"layrs-chunk-v1\0");
        let mut remaining = chunk.byte_length();
        while remaining > 0 {
            cancel.check()?;
            let count = remaining.min(scratch.len() as u64) as usize;
            read(offset, &mut scratch[..count], cancel)?;
            chunk_hash.update(&scratch[..count]);
            file_hash.update(&scratch[..count]);
            offset = offset
                .checked_add(count as u64)
                .ok_or_else(|| invalid("managed source file offset overflow"))?;
            remaining -= count as u64;
            cancel.check()?;
        }
        if format!("blake3:{}", chunk_hash.finalize()) != chunk.chunk_id().as_str() {
            return Err(invalid("managed source manifest chunk bytes differ"));
        }
    }
    cancel.check()?;
    if offset != manifest.byte_length()
        || format!("blake3:{}", file_hash.finalize()) != manifest.file_id().as_str()
    {
        return Err(invalid("managed source full file bytes differ"));
    }
    Ok(manifest.file_id().clone())
}

/// Borrowed pass before any path clone, native original-path query, or file
/// hash. Recovery export repeats its own structural/alias checks afterwards.
fn bound_observation_paths<'a>(
    paths: impl IntoIterator<Item = &'a RepoPath>,
    cancel: &Cancellation,
) -> Result<usize> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    for path in paths {
        cancel.check()?;
        count = count
            .checked_add(1)
            .filter(|n| *n <= recovery_export::PATH_LIMIT)
            .ok_or_else(|| invalid("managed image path count exceeds bound"))?;
        bytes = bytes
            .checked_add(path.as_str().len())
            .filter(|n| *n <= recovery_export::PATH_BYTES_LIMIT)
            .ok_or_else(|| invalid("managed image path metadata exceeds bound"))?;
    }
    Ok(count)
}

/// Pure bounded interpreter with injected full-file hash callbacks. Native
/// production uses the retained checkpoint and never stores complete bytes.
fn interpret<'a>(
    observations: &[Observed],
    source: &'a ProviderBootstrapSource,
    activated: &ProjectionSnapshot,
    cancel: &Cancellation,
    mut hash_source: impl FnMut(&PlannedFile, &Cancellation) -> Result<FileId>,
    mut hash_native: impl FnMut(&PlannedFile, &Cancellation) -> Result<FileManifest>,
) -> Result<ImageData<'a>> {
    cancel.check()?;
    if activated != &activation_snapshot(source)? {
        return Err(invalid(
            "managed image activation differs from fixed source",
        ));
    }
    // Entire namespace, aliases, path count and aggregate byte limits are
    // settled before either hash callback can read a file.
    let planned = recovery_export::plan(observations, source, activated)?;
    if planned
        .files
        .iter()
        .any(|file| file.class != Class::ManagedCandidate || file.executable.is_none())
        || planned
            .deletions
            .iter()
            .any(|deletion| deletion.class != Class::ManagedCandidate)
    {
        return Err(invalid("managed image contains recovery-only paths"));
    }
    let descriptors: BTreeMap<_, _> = source.descriptors().iter().map(|d| (&d.path, d)).collect();
    let entries: BTreeMap<_, _> = activated.entries.iter().map(|e| (&e.path, e)).collect();
    let mut files = Vec::with_capacity(planned.files.len());
    let mut report_files = Vec::with_capacity(planned.files.len());
    for file in &planned.files {
        cancel.check()?;
        let (actual, manifest) = match file.reader {
            ReaderKind::ImmutableSource => {
                let descriptor = *descriptors
                    .get(&file.path)
                    .ok_or_else(|| invalid("immutable managed descriptor absent"))?;
                let actual = hash_source(file, cancel)?;
                if actual != descriptor.file_id || file.size != descriptor.bytes {
                    return Err(invalid("immutable managed source bytes differ"));
                }
                (actual, ManagedManifest::Source(&descriptor.manifest))
            }
            ReaderKind::Native => {
                let hashed = hash_native(file, cancel)?;
                if hashed.byte_length() != file.size
                    || hashed.chunking()
                        != &ChunkingDescriptor::deterministic_fixed(1024 * 1024)
                            .map_err(|e| invalid(e.to_string()))?
                {
                    return Err(invalid("managed native manifest size or chunking differs"));
                }
                (hashed.file_id().clone(), ManagedManifest::Native(hashed))
            }
        };
        cancel.check()?;
        if recovery_export::verified_class(file, &actual) != Class::ManagedCandidate {
            return Err(invalid("managed image changed a protected file"));
        }
        let executable = file
            .executable
            .ok_or_else(|| invalid("managed file mode unknown"))?;
        report_files.push(ProviderRecoveryExportFile {
            path: file.path.clone(),
            file_id: actual,
            length: file.size,
            executable: Some(executable),
            class: Class::ManagedCandidate,
        });
        files.push(ManagedFile {
            path: file.path.clone(),
            manifest,
            executable,
            expected: file.expected.clone(),
        });
    }
    let mut deletions = Vec::with_capacity(planned.deletions.len());
    for deletion in &planned.deletions {
        cancel.check()?;
        let entry = entries
            .get(&deletion.path)
            .ok_or_else(|| invalid("managed deletion path absent"))?;
        if !entry.writable
            || !source.selection().includes(&deletion.path)
            || !matches!(&entry.target, Some(TreeEntryTarget::File { file_id }) if file_id == &deletion.base)
            || deletion.expected_executable != Some(entry.executable)
        {
            return Err(invalid(
                "managed deletion lacks exact writable base and mode",
            ));
        }
        deletions.push(ManagedDeletion {
            path: deletion.path.clone(),
            base: deletion.base.clone(),
            executable: entry.executable,
        });
    }
    cancel.check()?;
    let token = recovery_export::image_digest(&report_files, &planned.deletions, &[])?;
    let digest = token
        .strip_prefix("blake3:")
        .filter(|raw| raw.len() == 64)
        .ok_or_else(|| invalid("managed image digest shape differs"))?
        .to_owned();
    Ok(ImageData {
        files,
        deletions,
        digest,
    })
}

#[cfg(test)]
#[path = "managed_image/tests.rs"]
mod tests;
