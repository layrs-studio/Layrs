//! Complete fixed-source image reconciliation. The real adapter reads only
//! retained NeedsHash checkpoint handles; this plan grants no SQL admission.
use super::*;
use crate::projection::capture::{
    CapturedChange, CapturedFile, CapturedTarget, MAX_CAPTURE_BYTES, MAX_CAPTURE_FILE_BYTES,
};
use crate::projection::provider::records::activation::activation_snapshot;
use layrs_core::{
    projection::{EntryState, ProjectionSnapshot},
    scm::{
        Chunk, ChunkingDescriptor, FileId, FileManifest, FileManifestBuilder, RepoPath,
        TreeEntryTarget,
    },
};
use layrs_vfs::{Cancellation, CaptureEvidence, Checkpoint};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

const IMAGE_DOMAIN: &str = "layrs-projection-provider-capture-image-v1";
const CHUNK_BYTES: usize = 1024 * 1024;
const READ_BYTES: usize = 64 * 1024;
const MAX_PATHS: usize = 100_000;
const MAX_CHANGES: usize = 1024; // Existing CapturedPatch storage limit.

#[derive(Clone)]
pub(super) struct ImageObservation {
    pub path: RepoPath,
    pub evidence: CaptureEvidence,
}

pub(super) struct HashedCaptureFile {
    pub path: RepoPath,
    pub manifest: FileManifest,
}

/// Data for a later owned stage. The caller keeps the original Checkpoint and
/// must revalidate it through acceptance/ack and reread changed bytes from its
/// retained handles before installation. No file bytes are stored here.
pub(super) struct ProviderCaptureImagePlan {
    pub changes: Vec<CapturedChange>,
    pub image_digest: String,
    pub image_path_count: u64,
    pub hashed: Vec<HashedCaptureFile>,
}

/// Real retained-checkpoint adapter. `with_reader` bounds the logical size and
/// checks its own exact counted consumption; the checkpoint guards and later
/// namespace revalidation are the native identity boundary.
pub(super) fn plan(
    checkpoint: &mut Checkpoint<'_>,
    source: &ProviderBootstrapSource,
    activated: &ProjectionSnapshot,
    cancel: &Cancellation,
) -> CauseResult<ProviderCaptureImagePlan> {
    if checkpoint.root_guard().is_none() {
        return Err(invalid(
            "provider capture requires a retained native root proof",
        ));
    }
    checkpoint.revalidate_namespace()?;
    let observations: Vec<_> = checkpoint
        .entries()
        .iter()
        .map(|entry| ImageObservation {
            path: entry.path.clone(),
            evidence: entry.evidence.clone(),
        })
        .collect();
    let result = reconcile(
        &observations,
        source,
        activated,
        cancel,
        |path, size, cancel| {
            checkpoint
                .with_reader(path, |reader| hash_reader(reader, size, cancel))
                .map_err(Into::into)
        },
    )?;
    checkpoint.revalidate_namespace()?;
    Ok(result)
}

/// Pure evidence interpreter. Hash callback is invoked only after the *entire*
/// namespace/catalog shape has passed preflight; tests provide a synthetic
/// callback while production uses the retained VFS checkpoint reader.
pub(super) fn reconcile(
    observations: &[ImageObservation],
    source: &ProviderBootstrapSource,
    activated: &ProjectionSnapshot,
    cancel: &Cancellation,
    mut hash: impl FnMut(&RepoPath, u64, &Cancellation) -> CauseResult<FileManifest>,
) -> CauseResult<ProviderCaptureImagePlan> {
    check(cancel)?;
    if activated != &activation_snapshot(source)? {
        return Err(invalid(
            "provider capture activation image differs from fixed source",
        ));
    }
    if observations.len() > MAX_PATHS {
        return Err(invalid("provider checkpoint path count exceeds bound"));
    }
    let mut descriptors = BTreeMap::new();
    for descriptor in source.descriptors() {
        descriptors.insert(&descriptor.path, descriptor);
    }
    let mut selected = BTreeMap::new();
    for entry in &activated.entries {
        if source.selection().includes(&entry.path) {
            if !descriptors.contains_key(&entry.path) && entry.state != EntryState::Absent {
                return Err(invalid(
                    "provider selected image lacks fixed catalog/absence",
                ));
            }
            selected.insert(&entry.path, entry);
        }
    }
    if selected.len() > MAX_PATHS {
        return Err(invalid("provider selected image count exceeds bound"));
    }
    let mut observed = BTreeMap::new();
    let mut aliases = BTreeSet::new();
    let mut dirty_bytes = 0u64;
    for observation in observations {
        check(cancel)?;
        let path = observation.path.as_str();
        if path.is_empty() || path.len() > 4096 || !aliases.insert(path.to_ascii_lowercase()) {
            return Err(invalid("provider checkpoint path bound or alias differs"));
        }
        let entry = selected
            .get(&observation.path)
            .ok_or_else(|| invalid("provider checkpoint contains unknown/excluded path"))?;
        let descriptor = descriptors.get(&observation.path);
        match (&observation.evidence, descriptor) {
            (
                CaptureEvidence::Immutable {
                    file_id,
                    size,
                    original_path,
                },
                Some(descriptor),
            ) if file_id == &descriptor.file_id
                && *size == descriptor.bytes
                && original_path == &descriptor.path => {}
            (CaptureEvidence::Deleted { base }, Some(descriptor))
                if base == &descriptor.file_id => {}
            (CaptureEvidence::NeedsHash { base, size }, Some(descriptor))
                if base.as_ref().is_none_or(|id| id == &descriptor.file_id) =>
            {
                // Editors may replace a placeholder with an ordinary file.
                // Its retained handle has no CFAPI base identity; hash every
                // byte and compare against the bound path's original target.
                dirty_bytes = bound_hash_bytes(dirty_bytes, *size)?;
            }
            (CaptureEvidence::NeedsHash { base: None, size }, None)
                if entry.state == EntryState::Absent =>
            {
                dirty_bytes = bound_hash_bytes(dirty_bytes, *size)?;
            }
            _ => {
                return Err(invalid(
                    "provider checkpoint evidence differs from fixed catalog/absence",
                ))
            }
        }
        if observed
            .insert(&observation.path, &observation.evidence)
            .is_some()
        {
            return Err(invalid("provider checkpoint duplicate path"));
        }
    }
    if descriptors.keys().any(|path| !observed.contains_key(path)) {
        return Err(invalid("provider checkpoint omitted fixed catalog entry"));
    }
    // The source includes all selected file descriptors. A selected explicit
    // absence may have no checkpoint entry, which means it remains absent.
    let mut changes = Vec::new();
    let mut hashed = Vec::new();
    let mut image = blake3::Hasher::new();
    image.update(&layrs_core::scm::domain_separated_bytes(IMAGE_DOMAIN, &[]));
    let image_path_count = selected.len() as u64;
    image.update(&image_path_count.to_le_bytes());
    for (path, entry) in selected {
        check(cancel)?;
        let expected = match &entry.target {
            Some(TreeEntryTarget::File { file_id }) => Some(CapturedTarget {
                file_id: file_id.clone(),
                executable: entry.executable,
            }),
            None if entry.state == EntryState::Absent => None,
            _ => {
                return Err(invalid(
                    "provider capture selected target is not a file/absence",
                ))
            }
        };
        let evidence = observed.get(path).copied();
        let (actual, manifest) = match evidence {
            None if expected.is_none() => (None, None),
            Some(CaptureEvidence::Immutable { file_id, .. }) => (
                Some(CapturedFile {
                    file_id: file_id.clone(),
                    executable: entry.executable,
                    bytes: descriptors
                        .get(path)
                        .ok_or_else(|| invalid("immutable provider descriptor absent"))?
                        .bytes,
                }),
                None,
            ),
            Some(CaptureEvidence::Deleted { .. }) => (None, None),
            Some(CaptureEvidence::NeedsHash { size, .. }) => {
                let manifest = hash(path, *size, cancel)?;
                if manifest.byte_length() != *size
                    || manifest.chunking()
                        != &ChunkingDescriptor::deterministic_fixed(CHUNK_BYTES as u64)
                            .map_err(|e| invalid(e.to_string()))?
                {
                    return Err(invalid("provider hash manifest size/chunking differs"));
                }
                (
                    Some(CapturedFile {
                        file_id: manifest.file_id().clone(),
                        executable: entry.executable,
                        bytes: *size,
                    }),
                    Some(manifest),
                )
            }
            _ => return Err(invalid("provider capture selected entry missing evidence")),
        };
        let actual_target = actual.as_ref().map(|file| CapturedTarget {
            file_id: file.file_id.clone(),
            executable: file.executable,
        });
        if actual_target != expected {
            if !entry.writable {
                return Err(invalid("provider protected file changed"));
            }
            if changes.len() >= MAX_CHANGES {
                return Err(invalid(
                    "provider capture diff exceeds existing patch bound",
                ));
            }
            changes.push(CapturedChange {
                path: (*path).clone(),
                expected,
                replacement: actual,
            });
            if let Some(manifest) = manifest {
                hashed.push(HashedCaptureFile {
                    path: (*path).clone(),
                    manifest,
                });
            }
        }
        image_path(&mut image, path, &actual_target);
    }
    check(cancel)?;
    Ok(ProviderCaptureImagePlan {
        changes,
        image_digest: image.finalize().to_hex().to_string(),
        image_path_count,
        hashed,
    })
}

fn bound_hash_bytes(total: u64, size: u64) -> CauseResult<u64> {
    if size > MAX_CAPTURE_FILE_BYTES {
        return Err(invalid("provider hash file exceeds 1GiB"));
    }
    total
        .checked_add(size)
        .filter(|bytes| *bytes <= MAX_CAPTURE_BYTES)
        .ok_or_else(|| invalid("provider hashed bytes exceed 4GiB"))
}

fn image_path(image: &mut blake3::Hasher, path: &RepoPath, actual: &Option<CapturedTarget>) {
    let bytes = path.as_str().as_bytes();
    image.update(&(bytes.len() as u32).to_le_bytes());
    image.update(bytes);
    match actual {
        Some(file) => {
            image.update(&[1]);
            image.update(file.file_id.as_str().as_bytes());
            image.update(&[u8::from(file.executable)]);
        }
        None => {
            image.update(&[0]);
        }
    }
}

fn check(cancel: &Cancellation) -> CauseResult<()> {
    if cancel.is_cancelled() {
        Err(layrs_vfs::Error::Cancelled.into())
    } else {
        Ok(())
    }
}

pub(super) fn hash_reader(
    reader: &mut dyn Read,
    size: u64,
    cancel: &Cancellation,
) -> layrs_vfs::Result<FileManifest> {
    if size > MAX_CAPTURE_FILE_BYTES {
        return Err(layrs_vfs::Error::Unsupported(
            "provider hash file exceeds 1GiB",
        ));
    }
    let chunking = ChunkingDescriptor::deterministic_fixed(CHUNK_BYTES as u64)
        .map_err(|e| layrs_vfs::Error::InvalidCatalog(e.to_string()))?;
    let mut builder = FileManifestBuilder::new(chunking, MAX_CAPTURE_FILE_BYTES, 1024)
        .map_err(|e| layrs_vfs::Error::InvalidCatalog(e.to_string()))?;
    let mut left = size;
    while left > 0 {
        let length = left.min(CHUNK_BYTES as u64) as usize;
        let mut chunk = vec![0; length];
        for part in chunk.chunks_mut(READ_BYTES) {
            if cancel.is_cancelled() {
                return Err(layrs_vfs::Error::Cancelled);
            }
            reader.read_exact(part)?;
        }
        let chunk =
            Chunk::new(chunk).map_err(|e| layrs_vfs::Error::InvalidCatalog(e.to_string()))?;
        builder
            .push_chunk(&chunk)
            .map_err(|e| layrs_vfs::Error::InvalidCatalog(e.to_string()))?;
        left -= length as u64;
    }
    if cancel.is_cancelled() {
        return Err(layrs_vfs::Error::Cancelled);
    }
    let manifest = builder
        .finish()
        .map_err(|e| layrs_vfs::Error::InvalidCatalog(e.to_string()))?;
    if manifest.byte_length() != size {
        return Err(layrs_vfs::Error::IdentityMismatch);
    }
    Ok(manifest)
}

#[cfg(test)]
#[path = "capture_image/tests.rs"]
mod tests;
