use layrs_client_core::native_scm::{checkout_kind, execute_projection, ProjectionAction};
use layrs_client_core::native_scm::{
    execute_remote, execute_workspace, NativeRemoteRequest, WorkspaceAction,
};

#[tauri::command]
pub async fn native_checkout_kind(root: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        checkout_kind(root)
            .map(|kind| kind.to_string())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn native_projection(
    root: String,
    request: ProjectionAction,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        execute_projection(root, request).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn native_workspace(
    root: String,
    request: WorkspaceAction,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        execute_workspace(root, request).map_err(|e| format!("{}: {e}", e.code()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn native_remote(
    root: String,
    request: NativeRemoteRequest,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || execute_remote(root, request))
        .await
        .map_err(|e| e.to_string())?
}
