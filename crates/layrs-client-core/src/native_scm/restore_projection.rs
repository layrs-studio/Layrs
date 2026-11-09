//! Restore metadata refers to verified CAS files; no whole-file reconstruction.
use super::Result;
use super::*;
use layrs_core::scm::*;
use layrs_store::scm::{CanonicalObject, ScmReader};
use layrs_store_local::scm_v1::checkout::{CheckoutProjectionContent, CheckoutProjectionEntry};
use std::{collections::BTreeMap, io::Write};

pub(super) fn limits() -> SnapshotLimits {
    SnapshotLimits {
        max_entries: 100_000,
        max_depth: 128,
        max_file_bytes: usize::MAX,
        max_total_bytes: usize::MAX,
    }
}
pub(super) fn snapshot(entries: &[CheckoutProjectionEntry]) -> Result<PreparedSnapshot> {
    let mut builder = SnapshotBuilder::new(limits());
    for entry in entries {
        let path = RepoPath::parse(&entry.path)?;
        match &entry.content {
            CheckoutProjectionContent::File {
                file_id,
                byte_length,
                executable,
            } => builder.add_file_reference(path, file_id.clone(), *byte_length, *executable)?,
            CheckoutProjectionContent::Symlink { target } => builder.add_symlink(path, target)?,
        }
    }
    builder.finish()
}

pub(super) fn captured(snapshot: &PreparedSnapshot) -> Result<Vec<CheckoutProjectionEntry>> {
    let objects: BTreeMap<_, _> = snapshot
        .objects()
        .iter()
        .map(|object| (object.object_id(), object))
        .collect();
    let files: BTreeMap<_, _> = snapshot
        .objects()
        .iter()
        .filter_map(|object| match object {
            CanonicalObject::FileManifest(manifest) => {
                Some((manifest.file_id(), manifest.byte_length()))
            }
            _ => None,
        })
        .collect();
    let mut pending = vec![(String::new(), snapshot.root_tree_id().clone(), 0usize)];
    let mut entries = Vec::new();
    while let Some((prefix, id, depth)) = pending.pop() {
        if depth > 128 || entries.len() + pending.len() >= 100_000 {
            return Err(NativeClientError::InvalidState(
                "snapshot projection exceeds limits",
            ));
        }
        let Some(CanonicalObject::Tree(tree)) =
            objects.get(&layrs_store::scm::StoreObjectId::Tree(id))
        else {
            return Err(NativeClientError::Missing("snapshot Tree"));
        };
        for entry in tree.entries() {
            let path = if prefix.is_empty() {
                entry.name().as_str().to_owned()
            } else {
                format!("{prefix}/{}", entry.name().as_str())
            };
            let content = match entry.target() {
                TreeEntryTarget::Tree { tree_id } => {
                    pending.push((path, tree_id.clone(), depth + 1));
                    continue;
                }
                TreeEntryTarget::File { file_id } => CheckoutProjectionContent::File {
                    file_id: file_id.clone(),
                    byte_length: *files
                        .get(file_id)
                        .ok_or(NativeClientError::Missing("snapshot FileManifest"))?,
                    executable: entry.executable(),
                },
                TreeEntryTarget::Symlink { symlink_id } => {
                    let Some(CanonicalObject::Symlink(link)) = objects.get(
                        &layrs_store::scm::StoreObjectId::Symlink(symlink_id.clone()),
                    ) else {
                        return Err(NativeClientError::Missing("snapshot Symlink"));
                    };
                    CheckoutProjectionContent::Symlink {
                        target: link.target().to_owned(),
                    }
                }
                _ => {
                    return Err(NativeClientError::InvalidState(
                        "captured snapshot contains nonphysical target",
                    ))
                }
            };
            entries.push(CheckoutProjectionEntry { path, content });
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    if layrs_store_local::scm_v1::checkout::projection_tree_id(&entries)?
        != *snapshot.root_tree_id()
    {
        return Err(NativeClientError::InvalidState(
            "snapshot projection identity mismatch",
        ));
    }
    Ok(entries)
}

/// Shared Store capabilities for a whole checkout staging phase, usable from
/// several staging threads and released together by `end` (materialization
/// calls it before any exclusive journal phase). Each concurrent writer
/// checks out its own reading, so its own SQLite connection: the per-file
/// metadata queries run in parallel instead of queueing on one connection.
pub(super) struct SharedStagingStore<F> {
    open: F,
    /// Idle sessions, each holding its catalog's pack files for the whole
    /// staging (a bulk read, ADR 0026): released by `end`.
    idle: std::sync::Mutex<
        Vec<(
            layrs_store_local::scm_v1::NativeStoreReading,
            Option<layrs_store_local::scm_v1::PackFileHold>,
        )>,
    >,
    ended: std::sync::atomic::AtomicBool,
}

impl<F> SharedStagingStore<F>
where
    F: Fn() -> Result<layrs_store_local::scm_v1::NativeStoreOpen> + Sync,
{
    pub(super) fn new(open: F) -> Self {
        Self {
            open,
            idle: std::sync::Mutex::new(Vec::new()),
            ended: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub(super) fn with_reader<T>(
        &self,
        read: impl FnOnce(&layrs_store_local::scm_v1::NativeScmReader<'_>) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let poisoned = || std::io::Error::other("staging Store lock poisoned");
        let ended = || std::io::Error::other("checkout staging already ended");
        if self.ended.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ended());
        }
        let checked_out = self.idle.lock().map_err(|_| poisoned())?.pop();
        let reading = match checked_out {
            Some(reading) => reading,
            None => {
                let reading = layrs_store_local::scm_v1::NativeStoreReading::new(
                    (self.open)().map_err(std::io::Error::other)?,
                )
                .map_err(std::io::Error::other)?;
                let hold = reading.hold_pack_files().map_err(std::io::Error::other)?;
                (reading, hold)
            }
        };
        let result = read(&reading.0.reader());
        // A reading returned after `end` is dropped at once, never kept.
        let mut idle = self.idle.lock().map_err(|_| poisoned())?;
        if !self.ended.load(std::sync::atomic::Ordering::Acquire) {
            idle.push(reading);
        }
        result
    }

    pub(super) fn end(&self) {
        self.ended.store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut idle) = self.idle.lock() {
            idle.clear();
        }
    }
}

pub(super) fn write_file(
    reader: &layrs_store_local::scm_v1::NativeScmReader,
    overlay: &[CanonicalObject],
    entry: &CheckoutProjectionEntry,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    write_file_from(reader, overlay, None, entry, output)
}

fn write_file_from(
    reader: &layrs_store_local::scm_v1::NativeScmReader,
    overlay: &[CanonicalObject],
    spool: Option<&super::snapshot::SnapshotChunkSpool>,
    entry: &CheckoutProjectionEntry,
    output: &mut dyn Write,
) -> std::io::Result<()> {
    fn error(value: impl std::fmt::Display) -> std::io::Error {
        std::io::Error::other(value.to_string())
    }
    let CheckoutProjectionContent::File {
        file_id,
        byte_length,
        ..
    } = &entry.content
    else {
        return Err(error("file source is not a file"));
    };
    let overlaid = overlay.iter().find_map(|object| match object {
        CanonicalObject::FileManifest(manifest) if manifest.file_id() == file_id => {
            Some(manifest.clone())
        }
        _ => None,
    });
    // A capture spools only chunks the Store does not already admit; every
    // other chunk of an overlaid manifest comes from the Store. Either source
    // is proven by `manifest.verifier()` below.
    let spool = spool.filter(|_| overlaid.is_some());
    let manifest = match overlaid {
        Some(manifest) => manifest,
        None => {
            // Manifest from metadata only: every streamed chunk below goes
            // through `manifest.verifier()`, which proves the FileId, so a
            // separate full pre-read and re-hash of the file is redundant.
            let _span = layrs_store_local::perf::span("restore.manifest");
            let ids = reader.file_manifest_ids_metadata(file_id).map_err(error)?;
            let first = ids.first().ok_or_else(|| error("missing admitted file"))?;
            reader
                .get_file_manifest(first)
                .map_err(error)?
                .ok_or_else(|| error("missing file manifest"))?
        }
    };
    if manifest.file_id() != file_id || manifest.byte_length() != *byte_length {
        return Err(error("file source identity mismatch"));
    }
    let mut verifier = manifest.verifier();
    for part in manifest.chunks() {
        let chunk = match overlay.iter().find_map(|object| match object {
            CanonicalObject::ChunkRaw(chunk) if chunk.id() == part.chunk_id() => {
                Some(chunk.clone())
            }
            _ => None,
        }) {
            Some(chunk) => chunk,
            None if spool.is_some_and(|spool| spool.contains(part.chunk_id())) => Chunk::new(
                spool
                    .expect("checked spool")
                    .read(part.chunk_id())
                    .map_err(error)?,
            )
            .map_err(error)?,
            None => {
                let _span = layrs_store_local::perf::span("restore.chunk");
                reader
                    .get_chunk(part.chunk_id())
                    .map_err(error)?
                    .ok_or_else(|| error("missing file chunk"))?
            }
        };
        verifier.push(chunk.bytes()).map_err(error)?;
        output.write_all(chunk.bytes())?;
    }
    verifier.finish().map_err(error)
}

pub(super) fn preview_changes(
    workspace: &NativeWorkspace,
    snapshot: &PreparedSnapshot,
    before: &[CheckoutProjectionEntry],
    after: &[CheckoutProjectionEntry],
) -> Result<Vec<NativeFileChange>> {
    let total = before.iter().chain(after).try_fold(0u64, |total, entry| {
        let length = match &entry.content {
            CheckoutProjectionContent::File { byte_length, .. } => *byte_length,
            CheckoutProjectionContent::Symlink { target } => target.len() as u64,
        };
        total.checked_add(length).filter(|n| *n <= 8 * 1024 * 1024)
    });
    if total.is_none() {
        return Ok(diff::diff_projection_entries(before, after));
    }
    let store = workspace.open_store()?;
    let reader = layrs_store_local::scm_v1::NativeScmReader::new(&store)?;
    let entries=|projection:&[CheckoutProjectionEntry]|->Result<Vec<layrs_store_local::scm_v1::checkout::CheckoutEntry>> {
        use layrs_store_local::scm_v1::checkout::{CheckoutContent,CheckoutEntry};
        projection.iter().map(|entry|{
            let content=match &entry.content {
                CheckoutProjectionContent::File {executable,..}=>{
                    let mut bytes=Vec::new();
                    write_file_from(&reader,snapshot.objects(),snapshot.chunks.as_deref(),entry,&mut bytes)?;
                    CheckoutContent::File {bytes,executable:*executable}
                }
                CheckoutProjectionContent::Symlink {target}=>CheckoutContent::Symlink {target:target.clone()},
            };
            Ok(CheckoutEntry {path:entry.path.clone(),content})
        }).collect()
    };
    Ok(diff::diff_entries(&entries(before)?, &entries(after)?))
}
