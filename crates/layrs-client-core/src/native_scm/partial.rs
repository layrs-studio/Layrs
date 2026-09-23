//! Explicit format3 routing. Full Workspace operations never receive a partial Store.
use layrs_store_local::projection::ProjectionStore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io::Read, path::Path};

#[derive(Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProjectionAction {
    Status,
    Fsck,
    History {
        before: Option<u64>,
        limit: u16,
    },
    Capture {
        expected_generation: u64,
    },
    PreviewNamespace {
        expected_generation: u64,
        mutation: NamespaceMutation,
    },
    ResumeInstallation,
    ResumeNamespace,
    ResumeRefresh,
    ResumeRestore,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NamespaceMutation {
    pub kind: NamespaceMutationKind,
    pub source: Option<String>,
    pub path: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NamespaceMutationKind {
    Create,
    Rename,
    Move,
}

pub fn checkout_kind(root: impl AsRef<Path>) -> Result<&'static str, String> {
    let path = root.as_ref().join(".layrs/FORMAT");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("absent"),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 65536 {
        return Err("invalid_store_format: metadata is not a bounded regular file".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(65537).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() > 65536 {
        return Err("invalid_store_format: marker exceeds limit".into());
    }
    let marker: Value =
        serde_json::from_slice(&bytes).map_err(|_| "invalid_store_format: invalid JSON")?;
    if marker["magic"] != "LAYRS_NATIVE_STORE" {
        return Err("invalid_store_format: wrong magic".into());
    }
    match marker["formatVersion"].as_u64() {
        Some(2) => Ok("complete"),
        Some(3) => Ok("projection"),
        _ => Err("unsupported_store_format: explicit migration required".into()),
    }
}

pub fn execute_projection(
    root: impl AsRef<Path>,
    action: ProjectionAction,
) -> Result<Value, String> {
    let resumed = match action {
        ProjectionAction::ResumeInstallation => Some(
            ProjectionStore::resume_installation(&root)
                .map(|state| json!({"projection":state}))
                .map_err(|error| error.to_string()),
        ),
        ProjectionAction::ResumeNamespace => Some(
            ProjectionStore::resume_namespace(&root)
                .map(|patch| json!({"capture":patch}))
                .map_err(|error| error.to_string()),
        ),
        ProjectionAction::ResumeRefresh => Some(
            ProjectionStore::resume_refresh(&root)
                .map(|state| json!({"projection":state}))
                .map_err(|error| error.to_string()),
        ),
        ProjectionAction::ResumeRestore => Some(
            ProjectionStore::resume_restore(&root)
                .map(|receipt| json!({"restoration":receipt}))
                .map_err(|error| error.to_string()),
        ),
        _ => None,
    };
    if let Some(result) = resumed {
        return result;
    }
    let store =
        ProjectionStore::open(root).map_err(|error| format!("projection_store_error: {error}"))?;
    let current = store
        .state()
        .map_err(|error| format!("projection_store_error: {error}"))?;
    match action {
        ProjectionAction::Status => {
            let pending = store.pending_publication().map_err(|e| e.to_string())?;
            let pending_info = pending.as_ref().map(|record| {
                let document: Value=serde_json::from_slice(&record.request_bytes).map_err(|e|e.to_string())?;
                let (layer, intent) = if document.get("request").is_some() {
                    let submission:layrs_sync::v3::PathPatchSubmission=serde_json::from_value(document).map_err(|e|e.to_string())?;
                    (submission.request.layer_id, "advance")
                } else {
                    let request:layrs_sync::v3::ScopedPathPatchRequest=serde_json::from_value(document).map_err(|e|e.to_string())?;
                    request.validate().map_err(|e|e.to_string())?;
                    (request.layer_id, if request.intent==layrs_sync::v3::ScopedPathPatchIntent::Candidate {"candidate"} else {"advance"})
                };
                Ok::<_,String>(json!({"sequence":record.sequence,"layerId":layer,"intent":intent,"requestDigest":record.request_digest,"receiptRecorded":record.receipt_bytes.is_some()}))
            }).transpose()?;
            let latest = match &pending {
                Some(record) => Some(
                    store
                        .captured_patch(record.sequence)
                        .map_err(|e| e.to_string())?,
                ),
                None => store.latest_captured_patch().map_err(|e| e.to_string())?,
            };
            Ok(json!({
                "identity": store.identity(), "projection": current, "needsRepair": false,
                "latestCapture": latest, "pendingPublication":pending_info,
                "capabilities": {"capture":pending.is_none(),"publish":true,"namespace":pending.is_none(),"receive":pending.is_none(),"restore":pending.is_none()}
            }))
        }
        ProjectionAction::Fsck => store
            .fsck()
            .map(|report| json!(report))
            .map_err(|e| e.to_string()),
        ProjectionAction::History { before, limit } => store
            .capture_history(before, limit)
            .map(|page| json!(page))
            .map_err(|e| e.to_string()),
        ProjectionAction::Capture {
            expected_generation,
        } => store
            .capture_patch(expected_generation)
            .map(|patch| json!(patch))
            .map_err(|error| format!("projection_capture_refused: {error}")),
        ProjectionAction::PreviewNamespace {
            expected_generation,
            mutation,
        } => {
            if current.generation != expected_generation {
                return Err("projection_stale: refresh the project before previewing".into());
            }
            let fences = current
                .snapshot
                .reserved_paths()
                .map_err(|error| error.to_string())?;
            match mutation.kind {
                NamespaceMutationKind::Create => {
                    if mutation.source.is_some() {
                        return Err("invalid_projection_mutation: creation has no source".into());
                    }
                    fences
                        .check_path(&mutation.path)
                        .map_err(|error| format!("reserved_projection_path: {error}"))?;
                }
                NamespaceMutationKind::Rename | NamespaceMutationKind::Move => {
                    let source = mutation
                        .source
                        .as_deref()
                        .ok_or("invalid_projection_mutation: source required")?;
                    fences
                        .check_move(source, &mutation.path)
                        .map_err(|error| format!("reserved_projection_path: {error}"))?;
                }
            }
            // A clean reservation check is necessary but not a write grant for
            // a previously unknown destination. The online mutation flow must
            // classify it under the current authority before returning a plan.
            Err("projection_path_authorization_required: refresh destination permissions before creating or moving a file".into())
        }
        ProjectionAction::ResumeInstallation
        | ProjectionAction::ResumeNamespace
        | ProjectionAction::ResumeRefresh
        | ProjectionAction::ResumeRestore => unreachable!(),
    }
}
