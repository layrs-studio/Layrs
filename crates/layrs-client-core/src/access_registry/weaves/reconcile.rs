use super::weave_resolution::{labels_to_methods, resolution_method_labels_for_storage};
use super::weave_session::{conflict_dir, read_weave_blob, write_weave_blob};
use super::weave_state::apply_file_choice;
use super::*;
use layrs_lens_sdk::{
    LensCapability, LensError, LensFileResolutionInput, LensManifest, LensReconcileContent,
    LensReconcileInput, LensReconcileResult, LensResult, ResolutionMethod,
};

pub(super) enum PathReconcileOutcome {
    AutoResolved(Option<FileSnapshotEntry>),
    Conflicted(WeaveConflictFile),
}

pub(super) struct PathReconcileRequest<'a> {
    pub(super) weave_id: &'a str,
    pub(super) target_layer_id: &'a str,
    pub(super) source_layer_id: &'a str,
    pub(super) path: &'a str,
    pub(super) base: Option<&'a FileSnapshotEntry>,
    pub(super) ours: Option<&'a FileSnapshotEntry>,
    pub(super) theirs: Option<&'a FileSnapshotEntry>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "compatibility adapter for existing cross-module callers"
)]
pub(super) fn reconcile_path_with_lens(
    handle: &mut LocalSpaceHandle,
    weave_id: &str,
    target_layer_id: &str,
    source_layer_id: &str,
    path: &str,
    base: Option<&FileSnapshotEntry>,
    ours: Option<&FileSnapshotEntry>,
    theirs: Option<&FileSnapshotEntry>,
) -> Result<PathReconcileOutcome, String> {
    reconcile_path_with_lens_request(
        handle,
        PathReconcileRequest {
            weave_id,
            target_layer_id,
            source_layer_id,
            path,
            base,
            ours,
            theirs,
        },
    )
}

pub(super) fn reconcile_path_with_lens_request(
    handle: &mut LocalSpaceHandle,
    request: PathReconcileRequest<'_>,
) -> Result<PathReconcileOutcome, String> {
    let conflict_id = format!("conflict-{}", safe_id_fragment(request.path));
    let base_bytes = read_optional_entry_bytes(&handle.layrs_dir, request.base)?;
    let ours_bytes = read_optional_entry_bytes(&handle.layrs_dir, request.ours)?;
    let theirs_bytes = read_optional_entry_bytes(&handle.layrs_dir, request.theirs)?;
    let (manifest, _) = select_builtin_lens(Some(request.path), None).map_err(|error| {
        format!(
            "Layrs Desktop could not select a Lens for {}: {error}",
            request.path
        )
    })?;
    // Viewer-only image/code lenses do not implement reconciliation. Generic
    // whole-file selection must use the raw resolver and its declared methods.
    let manifest = if manifest.supports_capability(&LensCapability::Reconcile) {
        manifest
    } else {
        built_in_lens_manifest(layrs_lens_raw::RAW_LENS_ID)
            .ok_or("Raw reconciliation Lens is unavailable")?
    };
    let lens_id = manifest.id.as_str();
    let input = LensReconcileInput {
        path: Some(Path::new(request.path)),
        media_type: None,
        base: reconcile_side(request.base, &base_bytes),
        ours: reconcile_side(request.ours, &ours_bytes),
        theirs: reconcile_side(request.theirs, &theirs_bytes),
        ours_label: request.target_layer_id,
        theirs_label: request.source_layer_id,
    };
    let result = reconcile_with_builtin_lens(manifest, input);

    if result.status == layrs_lens_sdk::LensReconcileResultStatus::AutoResolved {
        let content = result.resolved.ok_or_else(|| {
            format!(
                "Lens {lens_id} auto-resolved {} without content.",
                request.path
            )
        })?;
        return Ok(PathReconcileOutcome::AutoResolved(
            write_reconciled_content(handle, request.path, content)?,
        ));
    }

    let dir = conflict_dir(&handle.layrs_dir, request.weave_id, &conflict_id);
    write_weave_blob(&mut handle.store, &dir.join("base"), &base_bytes)?;
    write_weave_blob(&mut handle.store, &dir.join("ours"), &ours_bytes)?;
    write_weave_blob(&mut handle.store, &dir.join("theirs"), &theirs_bytes)?;
    if let Some(conflict_content) = result.conflict.as_ref() {
        if conflict_content.exists {
            write_weave_blob(
                &mut handle.store,
                &dir.join("marked"),
                &conflict_content.bytes,
            )?;
        }
    }
    let blocks = result
        .blocks
        .into_iter()
        .map(|block| WeaveConflictBlockFile {
            block_id: block.block_id,
            status: "open".to_string(),
            base: block.base,
            ours: block.ours,
            theirs: block.theirs,
            methods: resolution_method_labels_for_storage(
                if block.methods.is_empty() {
                    labels_to_methods(&block.supported_resolutions)
                } else {
                    block.methods
                }
                .as_slice(),
            ),
            resolution: None,
            resolved_text: None,
        })
        .collect();
    let segments = result
        .segments
        .into_iter()
        .map(|segment| WeaveConflictSegmentFile {
            kind: match segment.kind {
                layrs_lens_sdk::LensConflictSegmentKind::Text => "text".to_string(),
                layrs_lens_sdk::LensConflictSegmentKind::Block => "block".to_string(),
            },
            text: segment.text,
            block_id: segment.block_id,
        })
        .collect();
    let message = if result.status == layrs_lens_sdk::LensReconcileResultStatus::Unsupported {
        result.summary
    } else {
        format!("{}.", result.summary)
    };
    Ok(PathReconcileOutcome::Conflicted(WeaveConflictFile {
        conflict_id,
        path: request.path.to_string(),
        lens_id: lens_id.to_string(),
        status: "open".to_string(),
        message,
        methods: resolution_method_labels_for_storage(&result.file_methods),
        resolution: None,
        blocks,
        segments,
    }))
}

fn write_reconciled_content(
    handle: &LocalSpaceHandle,
    path: &str,
    content: layrs_lens_sdk::LensReconcileContent,
) -> Result<Option<FileSnapshotEntry>, String> {
    if content.exists {
        write_file_object(&handle.layrs_dir, path, &content.bytes, true).map(Some)
    } else {
        Ok(None)
    }
}

fn reconcile_with_builtin_lens(
    manifest: &LensManifest,
    input: LensReconcileInput<'_>,
) -> LensReconcileResult {
    if !manifest.supports_capability(&LensCapability::Reconcile) {
        return LensReconcileResult::unsupported(format!(
            "Lens {} does not support Weave reconciliation",
            manifest.id
        ));
    }

    match manifest.id.as_str() {
        layrs_lens_text::TEXT_LENS_ID => layrs_lens_text::reconcile_text(input),
        layrs_lens_raw::RAW_LENS_ID => layrs_lens_raw::reconcile_raw(input),
        lens_id => LensReconcileResult::unsupported(format!(
            "Lens {lens_id} has no client-side Weave reconciler"
        )),
    }
}

fn reconcile_side<'a>(
    entry: Option<&'a FileSnapshotEntry>,
    bytes: &'a [u8],
) -> layrs_lens_sdk::LensReconcileSide<'a> {
    if let Some(entry) = entry {
        layrs_lens_sdk::LensReconcileSide {
            exists: true,
            bytes,
            content_hash: Some(entry.hash.as_str()),
            size: entry.size,
        }
    } else {
        layrs_lens_sdk::LensReconcileSide::absent()
    }
}

fn read_optional_entry_bytes(
    layrs_dir: &Path,
    entry: Option<&FileSnapshotEntry>,
) -> Result<Vec<u8>, String> {
    entry
        .map(|entry| read_snapshot_object_bytes(layrs_dir, entry))
        .unwrap_or_else(|| Ok(Vec::new()))
}

pub(super) fn apply_conflict_marker_to_state(
    handle: &LocalSpaceHandle,
    state: &mut WorkingStateFile,
    weave_id: &str,
    conflict: &WeaveConflictFile,
    ours: Option<&FileSnapshotEntry>,
) -> Result<(), String> {
    let dir = conflict_dir(&handle.layrs_dir, weave_id, &conflict.conflict_id);
    let marked = dir.join("marked");
    if handle.store.exists(&marked)? {
        let bytes = read_weave_blob(&handle.store, &marked)?;
        let entry = write_file_object(&handle.layrs_dir, &conflict.path, &bytes, true)?;
        apply_file_choice(state, &conflict.path, Some(entry));
    } else {
        apply_file_choice(state, &conflict.path, ours.cloned());
    }
    Ok(())
}

pub(super) fn resolve_file_conflict_with_lens(
    handle: &LocalSpaceHandle,
    weave_id: &str,
    conflict: &WeaveConflictFile,
    method: ResolutionMethod,
) -> Result<Vec<u8>, String> {
    let dir = conflict_dir(&handle.layrs_dir, weave_id, &conflict.conflict_id);
    let base_bytes = read_conflict_side(&handle.store, &dir, "base", conflict)?;
    let existing_bytes = read_conflict_side(&handle.store, &dir, "ours", conflict)?;
    let incoming_bytes = read_conflict_side(&handle.store, &dir, "theirs", conflict)?;
    let input = LensFileResolutionInput {
        method,
        base: side_from_bytes(&base_bytes),
        existing: side_from_bytes(&existing_bytes),
        incoming: side_from_bytes(&incoming_bytes),
    };
    let manifest = built_in_lens_manifest(&conflict.lens_id).ok_or_else(|| {
        format!(
            "unknown_lens: Lens {} is not registered for {}.",
            conflict.lens_id, conflict.path
        )
    })?;
    let content = resolve_file_conflict_with_builtin_lens(manifest, input)
        .map_err(|error| error.to_string())?;
    if content.exists {
        Ok(content.bytes)
    } else {
        Ok(Vec::new())
    }
}

fn resolve_file_conflict_with_builtin_lens(
    manifest: &LensManifest,
    input: LensFileResolutionInput<'_>,
) -> LensResult<LensReconcileContent> {
    if !manifest.supports_capability(&LensCapability::Reconcile)
        || manifest.resolution_methods.file.is_empty()
    {
        return Err(LensError::new(
            "lens_reconcile_unsupported",
            format!(
                "Lens {} does not support file-level Weave resolution",
                manifest.id
            ),
        ));
    }

    match manifest.id.as_str() {
        layrs_lens_raw::RAW_LENS_ID => layrs_lens_raw::resolve_raw_conflict(input),
        lens_id => Err(LensError::new(
            "lens_reconcile_unsupported",
            format!("Lens {lens_id} has no file-level Weave resolver"),
        )),
    }
}

fn read_conflict_side(
    store: &LocalStoreSession,
    dir: &Path,
    name: &str,
    conflict: &WeaveConflictFile,
) -> Result<Vec<u8>, String> {
    read_weave_blob(store, &dir.join(name)).map_err(|error| {
        format!(
            "Layrs Desktop could not read resolution bytes for {}: {error}",
            conflict.path
        )
    })
}

fn side_from_bytes(bytes: &[u8]) -> layrs_lens_sdk::LensReconcileSide<'_> {
    layrs_lens_sdk::LensReconcileSide {
        exists: true,
        bytes,
        content_hash: None,
        size: bytes.len() as u64,
    }
}

#[cfg(test)]
mod canonical_lens_reconcile_tests {
    use super::*;
    use layrs_lens_sdk::{LensReconcileResultStatus, LensReconcileSide};

    #[test]
    fn code_and_image_reconcile_fail_closed_without_raw_fallback() {
        for manifest in [layrs_lens_code::manifest(), layrs_lens_image::manifest()] {
            let result = reconcile_with_builtin_lens(&manifest, reconcile_input());

            assert_eq!(result.status, LensReconcileResultStatus::Unsupported);
            assert!(result.resolved.is_none());
            assert!(result.conflict.is_none());
            assert!(result.file_methods.is_empty());

            let error = resolve_file_conflict_with_builtin_lens(&manifest, file_input())
                .expect_err("unsupported Lens must not route to Raw");
            assert_eq!(error.code, "lens_reconcile_unsupported");
        }
    }

    #[test]
    fn text_and_raw_use_their_canonical_reconcilers() {
        let text = reconcile_with_builtin_lens(&layrs_lens_text::manifest(), reconcile_input());
        assert_eq!(text.status, LensReconcileResultStatus::AutoResolved);
        assert_eq!(text.resolved.expect("text result").bytes, b"incoming\n");

        let raw = reconcile_with_builtin_lens(&layrs_lens_raw::manifest(), reconcile_input());
        assert_eq!(raw.status, LensReconcileResultStatus::AutoResolved);
        assert_eq!(raw.resolved.expect("raw result").bytes, b"incoming\n");

        let raw_file =
            resolve_file_conflict_with_builtin_lens(&layrs_lens_raw::manifest(), file_input())
                .expect("Raw file resolution");
        assert_eq!(raw_file.bytes, b"existing\n");
    }

    fn reconcile_input() -> LensReconcileInput<'static> {
        LensReconcileInput {
            path: Some(Path::new("notes/readme.txt")),
            media_type: Some("text/plain"),
            base: LensReconcileSide::present(b"base\n", None),
            ours: LensReconcileSide::present(b"base\n", None),
            theirs: LensReconcileSide::present(b"incoming\n", None),
            ours_label: "target",
            theirs_label: "source",
        }
    }

    fn file_input() -> LensFileResolutionInput<'static> {
        LensFileResolutionInput {
            method: ResolutionMethod::Existing,
            base: LensReconcileSide::present(b"base\n", None),
            existing: LensReconcileSide::present(b"existing\n", None),
            incoming: LensReconcileSide::present(b"incoming\n", None),
        }
    }
}
