use super::*;
use super::weave_resolution::{
    conflict_block_method_labels, conflict_file_method_labels,
};

pub(super) fn weave_dir(layrs_dir: &Path, weave_id: &str) -> PathBuf {
    layrs_dir.join("weaves").join(safe_id_fragment(weave_id))
}

fn active_weave_marker_path(layrs_dir: &Path) -> PathBuf {
    layrs_dir.join("weaves").join("active.json")
}

fn session_path(layrs_dir: &Path, weave_id: &str) -> PathBuf {
    weave_dir(layrs_dir, weave_id).join("session.json")
}

pub(super) fn proposed_state_path(layrs_dir: &Path, weave_id: &str) -> PathBuf {
    weave_dir(layrs_dir, weave_id).join("proposed-state.json")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncWeavePlanGuard {
    schema: String,
    layer_id: String,
    planned_steps: Vec<String>,
    received_tree_id: Option<String>,
    proposed_tree_id: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct WeaveBlobFile {
    schema: String,
    bytes_hex: String,
}

fn sync_weave_plan_guard_path(layrs_dir: &Path, weave_id: &str) -> PathBuf {
    weave_dir(layrs_dir, weave_id).join("sync-plan-guard.json")
}

pub(super) fn write_sync_weave_plan_guard(
    store: &mut LocalStoreSession,
    layrs_dir: &Path,
    session: &WeaveSessionFile,
    received_state: &WorkingStateFile,
    proposed_state: &WorkingStateFile,
) -> Result<(), String> {
    write_json(
        store,
        &sync_weave_plan_guard_path(layrs_dir, &session.weave_id),
        &SyncWeavePlanGuard {
            schema: "layrs.sync_weave_plan_guard.v1".to_string(),
            layer_id: session.target_layer_id.clone(),
            planned_steps: session.planned_steps.clone(),
            received_tree_id: received_state.root_tree_id.clone(),
            proposed_tree_id: proposed_state.root_tree_id.clone(),
        },
    )
}

pub(super) fn ensure_sync_weave_plan_is_current(
    handle: &mut LocalSpaceHandle,
    session: &WeaveSessionFile,
    proposed_state: &WorkingStateFile,
) -> Result<(), String> {
    let guard: SyncWeavePlanGuard = read_json(
        &handle.store,
        &sync_weave_plan_guard_path(&handle.layrs_dir, &session.weave_id),
    )
    .map_err(|error| format!("Sync Weave plan guard is unavailable: {error}"))?;
    if guard.layer_id != session.target_layer_id
        || guard.planned_steps != session.planned_steps
        || guard.proposed_tree_id != proposed_state.root_tree_id
    {
        return Err(
            "Sync Weave plan changed since preparation. Abort and run Sync again.".to_string(),
        );
    }
    let pending_ids = read_pending_publish_files(
        &handle.store,
        &handle.layrs_dir,
        &session.target_layer_id,
    )?
        .into_iter()
        .map(|pending| pending.step_id)
        .collect::<Vec<_>>();
    if pending_ids != guard.planned_steps {
        return Err(
            "Sync Weave pending Step set changed since preparation. Abort and run Sync again; no pending Step was removed."
                .to_string(),
        );
    }
    let current_index = read_layer_index(
        &handle.store,
        &handle.layrs_dir,
        &session.target_layer_id,
    )?;
    if current_index.root_tree_id != guard.received_tree_id {
        return Err(
            "Sync Weave target changed since preparation. Abort and run Sync again.".to_string(),
        );
    }
    let current_working = capture_working_state(
        &mut handle.store,
        &handle.root,
        &session.target_layer_id,
    )?;
    if current_working.root_tree_id != guard.proposed_tree_id {
        return Err(
            "Sync Weave working tree changed since preparation. No files or pending Steps were modified."
                .to_string(),
        );
    }
    Ok(())
}

pub(super) fn ensure_local_weave_target_is_current(
    handle: &mut LocalSpaceHandle,
    session: &WeaveSessionFile,
    proposed_state: &WorkingStateFile,
) -> Result<(), String> {
    let target_state = read_layer_state(
        &handle.store,
        &handle.layrs_dir,
        &session.target_layer_id,
    )
    .or_else(|_| {
        read_layer_index(
            &handle.store,
            &handle.layrs_dir,
            &session.target_layer_id,
        )
    })?;
    if target_state.root_tree_id != session.pre_weave_target_tree_id {
        return Err(
            "Weave target changed since preparation. Abort and prepare the Weave again."
                .to_string(),
        );
    }
    if handle.active.layer_id == session.target_layer_id {
        let current_working = capture_working_state(
            &mut handle.store,
            &handle.root,
            &session.target_layer_id,
        )?;
        if current_working.root_tree_id != proposed_state.root_tree_id {
            return Err(
                "Weave working tree changed since preparation. No files or pending Steps were modified."
                    .to_string(),
            );
        }
    }
    Ok(())
}

pub(super) fn conflict_dir(layrs_dir: &Path, weave_id: &str, conflict_id: &str) -> PathBuf {
    weave_dir(layrs_dir, weave_id)
        .join("conflicts")
        .join(safe_id_fragment(conflict_id))
}

pub(super) fn write_weave_blob(
    store: &mut LocalStoreSession,
    path: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    let mut bytes_hex = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut bytes_hex, "{byte:02x}")
            .map_err(|error| format!("Layrs could not encode Weave bytes: {error}"))?;
    }
    write_json(
        store,
        path,
        &WeaveBlobFile {
            schema: "layrs.weave_blob.v1".to_string(),
            bytes_hex,
        },
    )
}

pub(super) fn read_weave_blob(
    store: &LocalStoreSession,
    path: &Path,
) -> Result<Vec<u8>, String> {
    let blob: WeaveBlobFile = read_json(store, path)?;
    if blob.schema != "layrs.weave_blob.v1" || blob.bytes_hex.len() % 2 != 0 {
        return Err(format!("Layrs Weave blob {} is invalid.", path.display()));
    }
    blob.bytes_hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair)
                .map_err(|error| format!("Layrs Weave blob hex is invalid: {error}"))?;
            u8::from_str_radix(text, 16)
                .map_err(|error| format!("Layrs Weave blob hex is invalid: {error}"))
        })
        .collect()
}

pub(super) fn active_weave_id(
    store: &LocalStoreSession,
    layrs_dir: &Path,
) -> Result<Option<String>, String> {
    let path = active_weave_marker_path(layrs_dir);
    if !store.exists(&path)? {
        return Ok(None);
    }
    let value = read_json::<Value>(store, &path)?;
    Ok(value
        .get("weaveId")
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

pub(super) fn write_active_weave_marker(
    store: &mut LocalStoreSession,
    layrs_dir: &Path,
    weave_id: Option<&str>,
) -> Result<(), String> {
    let path = active_weave_marker_path(layrs_dir);
    if let Some(weave_id) = weave_id {
        write_json(
            store,
            &path,
            &serde_json::json!({
                "schema": "layrs.active_weave.v1",
                "weaveId": weave_id,
                "updatedAtUnix": unix_now()
            }),
        )
    } else if store.exists(&path)? {
        store.remove(&path).map(|_| ())
    } else {
        Ok(())
    }
}

pub(super) fn read_weave_session(
    store: &LocalStoreSession,
    layrs_dir: &Path,
    weave_id: &str,
) -> Result<WeaveSessionFile, String> {
    read_json(store, &session_path(layrs_dir, weave_id))
}

pub(super) fn write_weave_session(
    store: &mut LocalStoreSession,
    layrs_dir: &Path,
    session: &WeaveSessionFile,
) -> Result<(), String> {
    write_json(store, &session_path(layrs_dir, &session.weave_id), session)
}

pub(super) fn summarize_weave_session(session: &WeaveSessionFile) -> WeaveSessionSummary {
    WeaveSessionSummary {
        weave_id: session.weave_id.clone(),
        source_layer_id: session.source_layer_id.clone(),
        target_layer_id: session.target_layer_id.clone(),
        status: session.status.clone(),
        pre_weave_target_tree_id: session.pre_weave_target_tree_id.clone(),
        pre_weave_target_step_id: session.pre_weave_target_step_id.clone(),
        planned_steps: session.planned_steps.clone(),
        applied_steps: session.applied_steps.clone(),
        conflicts: session
            .conflicts
            .iter()
            .map(|conflict| WeaveConflictSummary {
                conflict_id: conflict.conflict_id.clone(),
                path: conflict.path.clone(),
                lens_id: conflict.lens_id.clone(),
                status: conflict.status.clone(),
                message: conflict.message.clone(),
                methods: conflict_file_method_labels(conflict),
                resolution: conflict.resolution.clone(),
                blocks: conflict
                    .blocks
                    .iter()
                    .map(|block| WeaveConflictBlockSummary {
                        block_id: block.block_id.clone(),
                        status: block.status.clone(),
                        base: block.base.clone(),
                        existing: block.ours.clone(),
                        incoming: block.theirs.clone(),
                        ours: block.ours.clone(),
                        theirs: block.theirs.clone(),
                        methods: conflict_block_method_labels(conflict, block),
                        resolution: block.resolution.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}
