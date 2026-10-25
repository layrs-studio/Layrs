//! Restore metadata refers to verified CAS files; no whole-file reconstruction.
use super::Result;
use super::*;
use layrs_core::scm::*;
use layrs_store::scm::{CanonicalObject, FileAvailability, ScmReader};
use layrs_store_local::scm_v1::checkout::{CheckoutProjectionContent, CheckoutProjectionEntry};
use std::{collections::BTreeMap, io::Write};

pub(super) fn limits() -> SnapshotLimits {
    SnapshotLimits {
        max_entries: 100_000,
        max_depth: 128,
        max_file_bytes: 1024 * 1024 * 1024,
        max_total_bytes: 4usize * 1024 * 1024 * 1024,
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
    let use_spool = overlaid.is_some() && spool.is_some();
    let manifest = match overlaid {
        Some(manifest) => manifest,
        None => {
            let FileAvailability::Present(file) = reader
                .file_availability_with_budget(
                    file_id,
                    32_768,
                    4usize * 1024 * 1024 * 1024,
                    128,
                    16_384,
                )
                .map_err(error)?
            else {
                return Err(error("missing admitted file"));
            };
            reader
                .get_file_manifest(&file.verified_manifest_ids()[0])
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
            None if use_spool => Chunk::new(
                spool
                    .expect("checked spool")
                    .read(part.chunk_id())
                    .map_err(error)?,
            )
            .map_err(error)?,
            None => reader
                .get_chunk(part.chunk_id())
                .map_err(error)?
                .ok_or_else(|| error("missing file chunk"))?,
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
